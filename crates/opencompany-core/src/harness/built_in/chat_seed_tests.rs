use super::*;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};

use crate::ports::events::EventStreamItem;
use crate::ports::types::{EventSeq, StoredEvent};

/// A log that replays a fixed history in ascending sequence order. The
/// trait's default `read_before` (forward-read + reverse + truncate) then
/// gives us newest-first paging for free — exactly what production backends
/// override but what a fixture does not need to.
struct FixedLog(Vec<StoredEvent>);

#[async_trait]
impl EventLog for FixedLog {
    async fn append(&self, _id: &CompanyId, _event: CompanyEvent) -> crate::Result<EventSeq> {
        unreachable!("the seed projector only reads")
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        seq: EventSeq,
        limit: usize,
    ) -> crate::Result<Vec<StoredEvent>> {
        Ok(self
            .0
            .iter()
            .filter(|e| e.seq.value() >= seq.value())
            .take(limit)
            .cloned()
            .collect())
    }
    fn subscribe(&self, _id: &CompanyId) -> BoxStream<'static, EventStreamItem> {
        Box::pin(stream::empty())
    }
}

/// A [`FixedLog`] that counts how many PAGES the projector pulled, so a scan
/// bound is observable rather than merely asserted. Pages, not events: the
/// trait's default `read_before` reads forward and reverses, so an event
/// count says more about the fixture than about the walk.
#[derive(Default)]
struct CountingLog {
    events: Vec<StoredEvent>,
    scanned: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl EventLog for CountingLog {
    async fn append(&self, _id: &CompanyId, _event: CompanyEvent) -> crate::Result<EventSeq> {
        unreachable!("the seed projector only reads")
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        seq: EventSeq,
        limit: usize,
    ) -> crate::Result<Vec<StoredEvent>> {
        self.scanned
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self
            .events
            .iter()
            .filter(|e| e.seq.value() >= seq.value())
            .take(limit)
            .cloned()
            .collect())
    }
    fn subscribe(&self, _id: &CompanyId) -> BoxStream<'static, EventStreamItem> {
        Box::pin(stream::empty())
    }
}

/// A log whose reads always fail, to prove the projector degrades to an empty
/// seed rather than propagating.
struct BrokenLog;

#[async_trait]
impl EventLog for BrokenLog {
    async fn append(&self, _id: &CompanyId, _event: CompanyEvent) -> crate::Result<EventSeq> {
        unreachable!()
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        _seq: EventSeq,
        _limit: usize,
    ) -> crate::Result<Vec<StoredEvent>> {
        Err(OpenCompanyError::InvalidRequest("boom".to_string()))
    }
    fn subscribe(&self, _id: &CompanyId) -> BoxStream<'static, EventStreamItem> {
        Box::pin(stream::empty())
    }
}

use crate::error::OpenCompanyError;

fn operator(seq: u64, chat: Option<&str>, text: &str) -> StoredEvent {
    StoredEvent {
        seq: EventSeq::new(seq),
        company: CompanyId::new("acme"),
        event: CompanyEvent::OperatorMessage {
            text: text.to_string(),
            by: None,
            chat: chat.map(str::to_string),
            parent: None,
            deliverable: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
        },
        at_millis: seq,
    }
}

/// An operator message sent by a signed-in human (#2075 review).
fn operator_by(seq: u64, chat: Option<&str>, user_id: &str, text: &str) -> StoredEvent {
    let mut stored = operator(seq, chat, text);
    if let CompanyEvent::OperatorMessage { by, .. } = &mut stored.event {
        *by = Some(crate::ports::types::Actor {
            kind: crate::ports::types::ActorKind::User,
            id: user_id.to_string(),
        });
    }
    stored
}

fn operator_with_attachment(
    seq: u64,
    chat: Option<&str>,
    text: &str,
    attachment: crate::ports::types::Attachment,
) -> StoredEvent {
    StoredEvent {
        seq: EventSeq::new(seq),
        company: CompanyId::new("acme"),
        event: CompanyEvent::OperatorMessage {
            text: text.to_string(),
            by: None,
            chat: chat.map(str::to_string),
            parent: None,
            deliverable: None,
            mentions: Vec::new(),
            attachments: vec![attachment],
        },
        at_millis: seq,
    }
}

/// Entry constructors for the [`strip_current_message`] cases, which test
/// the pre-flatten shape now that the strip reads the speaker rather than
/// a role string.
fn op_entry(text: &str) -> SeedEntry {
    SeedEntry {
        role: "user",
        speaker: Speaker::Operator(crate::server::chat_history::CUE_OPERATOR_LABEL.to_string()),
        text: text.to_string(),
        parent: None,
    }
}

fn viewer_entry(text: &str) -> SeedEntry {
    SeedEntry {
        role: "agent",
        speaker: Speaker::Viewer,
        text: text.to_string(),
        parent: None,
    }
}

fn peer_entry(label: &str, text: &str) -> SeedEntry {
    SeedEntry {
        role: "agent",
        speaker: Speaker::Other(label.to_string()),
        text: text.to_string(),
        parent: None,
    }
}

fn flattened(entries: Vec<SeedEntry>) -> Vec<(String, String)> {
    entries.into_iter().map(SeedEntry::flatten).collect()
}

/// The agent every seed below is built **for**, and the author `reply`
/// journals under — so an unqualified fixture reply is the viewer's own
/// prior turn, and the pre-#1956 assertions still read as written.
const VIEWER: &str = "ceo";

fn reply(seq: u64, chat_id: &str, text: &str) -> StoredEvent {
    reply_by(seq, chat_id, VIEWER, text)
}

/// A reply journaled by a named author — a teammate, or one of the reserved
/// non-teammate authors `chat_history::is_known_author` enumerates.
fn reply_by(seq: u64, chat_id: &str, agent_id: &str, text: &str) -> StoredEvent {
    StoredEvent {
        seq: EventSeq::new(seq),
        company: CompanyId::new("acme"),
        event: CompanyEvent::AgentReply {
            audience: Vec::new(),
            chat_id: chat_id.to_string(),
            agent_id: agent_id.to_string(),
            text: text.to_string(),
            steps: Vec::new(),
            task_id: None,
            outputs: Vec::new(),
            parent: None,
            mentions: Vec::new(),
            mention_depth: 0,
        },
        at_millis: seq,
    }
}

/// A **private aside**: a reply journaled with a non-empty `audience`.
///
/// `audience` is the set the line was addressed to. An empty vector is the
/// ordinary desk-wide reply every other fixture here writes; a non-empty one
/// is the aside seam (`[group_chat.hive.aside]`), which
/// `runtime::hivemind`'s adapter maps to
/// `tinyhivemind_hive::aside::Audience::Aside { members }` and the episode
/// adapter in `hivemind::log` maps identically.
fn aside_by(seq: u64, chat_id: &str, agent_id: &str, audience: &[&str], text: &str) -> StoredEvent {
    StoredEvent {
        seq: EventSeq::new(seq),
        company: CompanyId::new("acme"),
        event: CompanyEvent::AgentReply {
            audience: audience.iter().map(|id| (*id).to_string()).collect(),
            chat_id: chat_id.to_string(),
            agent_id: agent_id.to_string(),
            text: text.to_string(),
            steps: Vec::new(),
            task_id: None,
            outputs: Vec::new(),
            parent: None,
            mentions: Vec::new(),
            mention_depth: 0,
        },
        at_millis: seq,
    }
}

fn desk_completed(seq: u64, origin_chat_id: Option<&str>) -> StoredEvent {
    threaded_desk_completed(seq, origin_chat_id, None)
}

/// A settle whose card recorded the thread it was raised in (#1890 B).
fn threaded_desk_completed(
    seq: u64,
    origin_chat_id: Option<&str>,
    origin_parent: Option<u64>,
) -> StoredEvent {
    StoredEvent {
        seq: EventSeq::new(seq),
        company: CompanyId::new("acme"),
        event: CompanyEvent::DeskTaskCompleted {
            task_id: "t-1".to_string(),
            desk: "eng".to_string(),
            output: "shipped".to_string(),
            column: "done".to_string(),
            artifact_ids: Vec::new(),
            origin_chat_id: origin_chat_id.map(str::to_string),
            origin_parent: origin_parent.map(EventSeq::new),
        },
        at_millis: seq,
    }
}

/// `current_message` empty means "no boundary to bound against" — the
/// tests exercising desk ownership, folding, and window truncation below
/// pass `""` on purpose, so they see the unbounded-tail fallback
/// [`build_chat_seed`]'s doc describes and are unaffected by the
/// self-boundary search.
async fn seed_of(
    log: FixedLog,
    desk_id: &str,
    desk_name: &str,
    window: usize,
    current_message: &str,
) -> Vec<(String, String)> {
    let events: Arc<dyn EventLog> = Arc::new(log);
    build_chat_seed(
        &events,
        &CompanyId::new("acme"),
        desk_id,
        desk_name,
        // The fixtures' own author (see `reply`), so every case written
        // before #1956 keeps asserting the unlabelled `"agent"` turns it
        // always did — those are the viewer's own replies.
        VIEWER,
        // The channel-level conversation. Every fixture below journals
        // `parent: None`, which is what an unthreaded company writes — so
        // these cases assert the pre-#1890 behaviour is byte-identical.
        None,
        window,
        // The text fallback, which is the boundary every case below is
        // written against.
        SelfBoundary::Text(current_message),
    )
    .await
}

// ── Identity boundary ────────────────────────────────────────────────

/// A channel-level seed whose boundary is the journal position `seq`,
/// rather than any message's text.
async fn seed_anchored(log: FixedLog, seq: u64) -> Vec<(String, String)> {
    let events: Arc<dyn EventLog> = Arc::new(log);
    build_chat_seed(
        &events,
        &CompanyId::new("acme"),
        "general",
        "general",
        VIEWER,
        None,
        CHAT_SEED_WINDOW,
        SelfBoundary::Seq(EventSeq::new(seq)),
    )
    .await
}

// ── Thread scoping (#1890) ───────────────────────────────────────────

/// An operator message posted inside the thread rooted at `parent`.
fn operator_in(seq: u64, chat: Option<&str>, text: &str, parent: u64) -> StoredEvent {
    let mut stored = operator(seq, chat, text);
    if let CompanyEvent::OperatorMessage { parent: p, .. } = &mut stored.event {
        *p = Some(EventSeq::new(parent));
    }
    stored
}

/// An agent reply journaled under the thread rooted at `parent` — the
/// message's OWN parent, never the message itself, which is what stops a
/// thread nesting inside a thread.
fn reply_in(seq: u64, chat_id: &str, text: &str, parent: u64) -> StoredEvent {
    reply_by_in(seq, chat_id, VIEWER, text, parent)
}

/// [`reply_in`] by a named author (#1956).
fn reply_by_in(seq: u64, chat_id: &str, agent_id: &str, text: &str, parent: u64) -> StoredEvent {
    let mut stored = reply_by(seq, chat_id, agent_id, text);
    if let CompanyEvent::AgentReply { parent: p, .. } = &mut stored.event {
        *p = Some(EventSeq::new(parent));
    }
    stored
}

async fn seed_of_thread(
    log: FixedLog,
    desk: &str,
    thread_root: Option<u64>,
    current_message: &str,
) -> Vec<(String, String)> {
    let events: Arc<dyn EventLog> = Arc::new(log);
    build_chat_seed(
        &events,
        &CompanyId::new("acme"),
        desk,
        desk,
        VIEWER,
        thread_root.map(EventSeq::new),
        CHAT_SEED_WINDOW,
        SelfBoundary::Text(current_message),
    )
    .await
}

// ── Attribution (#1956) ──────────────────────────────────────────────

/// A seed built for a named viewer. The desk and boundary are fixed —
/// these cases are about *who spoke*, and every other axis has its own
/// section above.
async fn seed_for(log: FixedLog, viewer: &str, thread_root: Option<u64>) -> Vec<(String, String)> {
    let events: Arc<dyn EventLog> = Arc::new(log);
    build_chat_seed(
        &events,
        &CompanyId::new("acme"),
        "growth",
        "growth",
        viewer,
        thread_root.map(EventSeq::new),
        CHAT_SEED_WINDOW,
        SelfBoundary::Text(""),
    )
    .await
}

// ── Peer/boundary collision (#2075 review) ───────────────────────────

// ── Byline forgery (#2075 review) ────────────────────────────────────

#[path = "chat_seed_tests_part1.rs"]
mod tests_part1;
#[path = "chat_seed_tests_part2.rs"]
mod tests_part2;
#[path = "chat_seed_tests_part3.rs"]
mod tests_part3;
