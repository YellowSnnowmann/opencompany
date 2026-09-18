//! Transient per-turn progress bus — the live tool-call timeline that rides the
//! company SSE feed *while a turn is still running*.
//!
//! This mirrors OpenHuman's web-chat event bus
//! (`vendor/openhuman/crates/openhuman-core/src/web_chat/event_bus.rs`): a process-wide
//! `tokio::sync::broadcast` fabric, one sender per company, that the harness
//! publishes each mapped [`AgentProgress`](openhuman) event onto the instant it
//! happens, and that the operator SSE route ([`company_events`]) fans back out
//! to every watching console.
//!
//! It is deliberately **separate from the durable company event log**
//! ([`CompanyEvent`](crate::ports::types::CompanyEvent) / [`EventLog`]):
//!
//! * Tool-call progress is high-volume and ephemeral — one turn can emit dozens
//!   of start/complete pairs. Journaling it would bloat the audit log and the
//!   GraphQL `Chat.history` for no lasting value.
//! * The durable, operator-facing timeline is still folded into
//!   [`TurnStep`](crate::ports::types::TurnStep)s at turn end
//!   ([`fold_steps`](crate::harness::steps::fold_steps)) and rides the final
//!   chat reply. This bus only makes those same steps *appear live* first.
//!
//! ## Security
//!
//! The bus carries only the **already-scrubbed** projection produced by
//! [`crate::harness::steps`] — a label, a redacted argument line, a result
//! *summary*, a typed failure, an elapsed time, a status. It never carries raw
//! tool output, exactly like `fold_steps`; arguments reach it only through
//! issue #372's host-side redactor, and a remote body reaches it only as a
//! shape (issue #411). The mapping that enforces all of that lives next to
//! `fold_steps` (same helpers, same rules); this module is a dumb transport and
//! models no openhuman types, so it stays compiled in the default
//! (non-`openhuman`) build where it simply has no publishers and every
//! subscription is an empty stream.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use futures::stream::BoxStream;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::ports::types::{CompanyId, TurnStepFailure};

/// Per-company broadcast senders. Created lazily on first publish/subscribe for
/// a company and kept for the process lifetime (companies are few and long
/// lived, matching the durable event log's own `senders` map in `store::fs`).
static REGISTRY: LazyLock<Mutex<HashMap<CompanyId, broadcast::Sender<LiveFrame>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Ring capacity per company. A console that lags past this many un-drained
/// live events drops the gap (`Lagged`) and keeps going — the authoritative
/// timeline still arrives folded on the final reply, so a dropped live frame is
/// cosmetic, never lost state.
const CAPACITY: usize = 256;

/// Anything this bus carries.
///
/// `untagged` because every variant already serializes its own `type` key, so
/// the wire form of a turn frame is **byte-identical** to what it was before
/// presence existed — no envelope, no nesting, no second discriminant. The
/// console keeps switching on `type` exactly as it does for `agent_reply`.
///
/// Presence and typing belong here rather than on the durable event log for
/// the reason stated in this module's header: they are high-volume, worthless
/// a second later, and journaling them would bloat the audit log and every
/// chat history read for no lasting value. They are also, unlike a turn frame,
/// facts about *people* — which is why the projection that puts them on the
/// wire lives beside the route that authenticates one.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum LiveFrame {
    /// Tool-call progress while a turn runs.
    Turn(Box<TurnStreamEvent>),
    /// Somebody arrived, went idle, or left.
    Presence(PresenceFrame),
    /// Somebody is typing in a channel.
    Typing(TypingFrame),
    /// A coarse "near your credit limit" warning (issue #1846).
    BudgetProximity(BudgetProximityFrame),
}

impl LiveFrame {
    /// The turn frame this carries, or `None` for a presence/typing frame.
    ///
    /// The bus is mostly turn frames and every pre-existing reader wants one,
    /// so this keeps those call sites reading as they did rather than matching
    /// on a union they do not care about.
    pub fn as_turn(&self) -> Option<&TurnStreamEvent> {
        match self {
            Self::Turn(event) => Some(event),
            _ => None,
        }
    }
}

impl From<TurnStreamEvent> for LiveFrame {
    fn from(event: TurnStreamEvent) -> Self {
        // Boxed because `TurnStreamEvent` is much the largest variant, and an
        // un-boxed union would make every presence frame on the broadcast ring
        // pay for it. The ring holds `CAPACITY` of these per company.
        Self::Turn(Box::new(event))
    }
}

impl From<PresenceFrame> for LiveFrame {
    fn from(frame: PresenceFrame) -> Self {
        Self::Presence(frame)
    }
}

impl From<TypingFrame> for LiveFrame {
    fn from(frame: TypingFrame) -> Self {
        Self::Typing(frame)
    }
}

impl From<BudgetProximityFrame> for LiveFrame {
    fn from(frame: BudgetProximityFrame) -> Self {
        Self::BudgetProximity(frame)
    }
}

/// A coarse, non-blocking "near your credit limit" warning (issue #1846).
///
/// Rides the same ephemeral, journal-less bus as [`TurnStreamEvent`] and for
/// the same reason: this is a soft heads-up, not an authoritative fact worth
/// persisting — a console that missed one because it was offline sees the
/// next one on the next dispatch, and there is nothing to "catch up" on. It
/// is published **beside the existing pre-flight cap reads** in
/// [`crate::harness::HarnessPool`]'s dispatch gate, reusing the meter read
/// that already happens there — never a second query, and never a per-task
/// cost estimate (that needs a net-new `TaskPlan` cost primitive and is
/// explicitly out of scope here).
///
/// Fail-open by construction: this is emitted only on the branch where the
/// meter read already SUCCEEDED and a coarse threshold was crossed. An
/// unreadable meter publishes nothing, exactly like the pre-flight refusals
/// beside it fall through to running the turn rather than bricking dispatch.
#[derive(Clone, Debug, Serialize)]
pub struct BudgetProximityFrame {
    /// Always `"budget_proximity"`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// The teammate whose dispatch triggered the read, when the warning is
    /// scoped to one agent's own cap. `None` for the company-wide ceiling.
    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// An operator-facing sentence — no raw figures the config threshold
    /// might not want surfaced verbatim, just "you're near your limit".
    pub message: String,
    #[serde(rename = "atMillis")]
    pub at_millis: u64,
}

/// One person's presence changed.
///
/// Published on a change only — an arrival, a status flip, a departure — never
/// on a routine heartbeat renewal, or a company of ten would put ten frames a
/// minute on every open console for no visible difference.
///
/// Carries a user id and no label: every signed-in member already holds the
/// directory that names them (`GET {scope}/chat/mentionables`), so a label here
/// would be a second copy to keep in step.
#[derive(Clone, Debug, Serialize)]
pub struct PresenceFrame {
    /// Always `"presence"`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(rename = "userId")]
    pub user_id: String,
    /// `online` / `away` / `offline`.
    pub status: &'static str,
    #[serde(rename = "atMillis")]
    pub at_millis: u64,
}

/// Somebody is typing.
///
/// Stored nowhere at all, not even in the presence registry: a typing
/// indicator is eight seconds of leased truth, and the console expires it on
/// its own. There is deliberately no "stopped typing" frame — the absence of a
/// renewal is the stop signal, which means a console that closes mid-word
/// clears itself with no teardown to get wrong.
#[derive(Clone, Debug, Serialize)]
pub struct TypingFrame {
    /// Always `"typing"`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(rename = "userId")]
    pub user_id: String,
    /// The channel being typed in.
    #[serde(rename = "chatId")]
    pub chat_id: String,
    /// The thread inside it, when it is one.
    #[serde(rename = "parentId", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(rename = "atMillis")]
    pub at_millis: u64,
}

/// One live progress frame for the console's in-flight tool timeline.
///
/// Serializes onto the same SSE stream as the durable
/// [`project_event`](crate::server::operator) projections, so it carries a
/// `type` discriminant the frontend switches on alongside `agent_reply`,
/// `task_steered`, etc. Every string field here is the scrubbed projection from
/// [`crate::harness::steps`] — never raw arguments/output.
#[derive(Clone, Debug, Serialize)]
pub struct TurnStreamEvent {
    /// The wire discriminant: `"tool_call"` (a call just started, `status`
    /// `running`) or `"tool_result"` (it finished, `status` `ok`/`error`).
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// Monotonic per-turn sequence so the client can order/dedup frames that a
    /// reconnecting console might otherwise interleave.
    pub seq: u64,
    /// The responding agent/desk this turn belongs to, so the console can label
    /// which member is working.
    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The chat/desk thread this turn belongs to — the same id the durable
    /// `agent_reply` frame carries (`chat_id`). The console keys the live tool
    /// timeline on this so concurrent turns on different threads never
    /// cross-attribute their frames. `agentId` alone is insufficient: a thread
    /// is a desk, which doesn't map 1:1 to the responding member.
    #[serde(rename = "chatId", skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    /// Stable id pairing a `tool_result` back to its `tool_call` row so the
    /// console flips that row `running → ok/error` in place (mirrors
    /// OpenHuman's `tool_call_id` keying).
    #[serde(rename = "toolCallId", skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// The step label (server-computed `display_label`, else humanized tool
    /// name). Never derived from arguments or output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// **What the call was doing** — its arguments, put through issue #372's
    /// host-side redactor and bounded (issue #411). Never raw remote text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// **What came back** — a success's shape summary or an intrinsic tool's
    /// own message, or a failure's plain-language cause (issue #411). Never a
    /// remote body's content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The typed reason a call did not succeed (issue #411), so a live row
    /// renders the same known state the final folded step does. `None` on a
    /// success and on a parked call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<TurnStepFailure>,
    /// The result was cut before the agent could read all of it (issue #410).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// `"running"` on a start; `"ok"` / `"error"` / `"awaiting_approval"` on a
    /// completion. Sourced from
    /// [`TurnStepStatus::wire_word`](crate::ports::types::TurnStepStatus::wire_word)
    /// so the live word and the persisted one cannot drift.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
    /// Wall-clock the completed call took, on `tool_result` only.
    #[serde(rename = "elapsedMs", skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// The journal sequence of the operator message this turn is answering,
    /// when it is answering one at all.
    ///
    /// `chatId` says which *conversation* a frame belongs to; this says which
    /// *query*. The distinction only starts to matter when one conversation has
    /// two turns in flight, which is exactly when the console needs it. Keyed by
    /// thread alone, a second send shares one live timeline with the first, so
    /// two questions' rows pile into one list with nothing to tell them apart —
    /// and because the console holds a single row-list per thread, arming the
    /// second turn discards the first's rows outright. A turn blocked on a
    /// teammate emits nothing further, so its timeline never comes back.
    ///
    /// With this, a console can group live rows per query and render each under
    /// the message that asked it, the same way a settled turn's folded steps
    /// already render under their reply.
    ///
    /// Carried from `ChatTarget::message_seq`, so it is `None` on every turn
    /// that answers no journaled operator message — a relay, a delegate's
    /// instruction, a dispatched card, a workflow node. A consumer falls back to
    /// keying by `chatId` alone, which is what every consumer did before this
    /// field existed.
    #[serde(rename = "messageSeq", skip_serializing_if = "Option::is_none")]
    pub message_seq: Option<u64>,
    /// The workflow **run** this frame belongs to, when the turn is a workflow
    /// agent node rather than a chat turn (issue #1702). The console's run-trace
    /// sheet keys the live tool timeline on this so a node's in-flight frames
    /// append to the right run. Absent on a chat turn, which routes by `chatId`
    /// instead — the two are mutually exclusive routing dimensions, never both.
    #[serde(rename = "workflowRunId", skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
    /// The workflow **node** inside that run this frame belongs to (issue
    /// #1702), so the sheet groups a run's live frames under the same node the
    /// durable trace attributes them to. Absent on a chat turn.
    #[serde(rename = "nodeId", skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

impl Default for TurnStreamEvent {
    /// An empty `tool_call` frame, so the mapping in
    /// [`crate::harness::steps`] can fill only the fields a given event kind
    /// actually carries via `..Default::default()`. `kind` is a placeholder
    /// every caller overrides.
    fn default() -> Self {
        Self {
            kind: "tool_call",
            seq: 0,
            agent_id: None,
            chat_id: None,
            tool_call_id: None,
            label: None,
            detail: None,
            result: None,
            failure: None,
            truncated: false,
            status: None,
            elapsed_ms: None,
            message_seq: None,
            workflow_run_id: None,
            node_id: None,
        }
    }
}

impl TurnStreamEvent {
    /// Stamp the responding agent/desk onto a frame just before publish.
    pub fn with_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    /// Stamp the chat/desk thread onto a frame just before publish, so the
    /// console routes it to the same thread the durable reply lands on.
    pub fn with_chat(mut self, chat_id: impl Into<String>) -> Self {
        self.chat_id = Some(chat_id.into());
        self
    }

    /// Stamp the workflow run + node onto a frame just before publish, so the
    /// console's run-trace sheet routes it under the right run and node (issue
    /// #1702). Used instead of [`with_chat`](Self::with_chat) on a workflow
    /// agent node, which carries no chat thread — the two routing dimensions
    /// are mutually exclusive.
    pub fn with_workflow(mut self, run_id: impl Into<String>, node_id: impl Into<String>) -> Self {
        self.workflow_run_id = Some(run_id.into());
        self.node_id = Some(node_id.into());
        self
    }

    /// Stamps the operator message this turn answers, so the console can group
    /// the frame under that query rather than under the thread as a whole.
    ///
    /// Takes the `Option` rather than the value: every publish site reads it off
    /// a `ChatTarget` that may legitimately carry none, and answering no
    /// journaled message is the normal case for half of them.
    pub fn with_message_seq(mut self, seq: Option<u64>) -> Self {
        self.message_seq = seq;
        self
    }
}

/// Routing context a turn carries so its live frames reach the right console
/// thread: which company to publish on, which responding agent/desk the frames
/// belong to, and which chat thread they render onto. Built by the harness brain
/// (which knows all three) and handed to the turn runner; `None` disables live
/// streaming (e.g. background/system turns and the default non-`openhuman`
/// build).
#[derive(Clone, Debug)]
pub struct TurnStreamCtx {
    /// The company whose SSE feed the frames publish onto.
    pub company: CompanyId,
    /// The responding agent/desk, stamped onto each frame's `agentId`.
    pub agent_id: String,
    /// Where this turn's live frames route on the console: a chat thread, or a
    /// workflow run+node. The two are mutually exclusive, so an enum keeps a
    /// frame from ever carrying both (or neither) routing key.
    pub route: LiveRoute,
    /// The operator message this turn answers, stamped onto every frame as
    /// `messageSeq` so two turns sharing a thread stay tellable apart. `None`
    /// for a turn answering no journaled message; see
    /// [`TurnStreamEvent::message_seq`].
    pub message_seq: Option<u64>,
}

/// Which console surface a turn's live frames route to.
///
/// A chat turn keys its in-flight tool timeline on `chatId`; a workflow agent
/// node has no chat thread and keys on the workflow run + node instead (issue
/// #1702). Modelled as an enum rather than two `Option`s so a frame cannot be
/// built with both keys set or neither — the same "make the illegal state
/// unrepresentable" posture the run-trace sink takes.
#[derive(Clone, Debug)]
pub enum LiveRoute {
    /// The chat/desk thread this turn answers, stamped onto each frame's
    /// `chatId` so the console routes the live timeline to the same thread the
    /// durable `agent_reply` lands on. Matches the id journaled as
    /// `AgentReply.chat_id` for this turn.
    Chat { chat_id: String },
    /// The workflow run + node this turn belongs to, stamped onto each frame's
    /// `workflowRunId`/`nodeId` so the console's run-trace sheet appends the
    /// node's in-flight frames to the right run while it is still executing.
    Workflow { run_id: String, node_id: String },
}

/// The sender for a company, created on first use. Mirrors `store::fs`'s
/// `sender_for`: a subscribers-may-be-zero broadcast whose `send` error (no
/// listeners) is ignored — a turn streams whether or not a console is watching.
fn sender_for(company: &CompanyId) -> broadcast::Sender<LiveFrame> {
    let mut reg = REGISTRY.lock().expect("turn-stream registry poisoned");
    reg.entry(company.clone())
        .or_insert_with(|| broadcast::channel(CAPACITY).0)
        .clone()
}

/// Publish one live frame for `company`. A no-op (send error ignored) when no
/// console is subscribed.
///
/// Takes `impl Into<LiveFrame>` so every existing harness call site — which
/// passes a bare [`TurnStreamEvent`] — is unchanged by the bus being widened.
pub fn publish(company: &CompanyId, event: impl Into<LiveFrame>) {
    let _ = sender_for(company).send(event.into());
}

/// Subscribe to a company's live turn frames as a stream, for merging into the
/// operator SSE feed. On a cold company (no turn yet) this still yields a live
/// receiver — it simply stays quiet until the first `publish`. A `Lagged` gap is
/// skipped (see [`CAPACITY`]); `Closed` never happens because `REGISTRY` holds a
/// sender for the process lifetime.
pub fn subscribe(company: &CompanyId) -> BoxStream<'static, LiveFrame> {
    let rx = sender_for(company).subscribe();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(event) => return Some((event, rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Box::pin(stream)
}

#[cfg(test)]
#[path = "turn_stream_tests.rs"]
mod tests;
