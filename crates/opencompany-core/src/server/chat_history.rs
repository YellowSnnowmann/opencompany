//! Shared desk-history read logic (issue #65).
//!
//! Both the GraphQL `Chat.history` resolver
//! ([`crate::server::graphql::company`]) and the REST `GET .../chat/history`
//! route ([`crate::server::operator`]) need to answer the same question — "what
//! messages belong to this desk, as seen by this viewer?" — and they must never
//! be allowed to disagree about it. This module is the one place that answers
//! it; both surfaces call through it instead of each keeping their own copy of
//! the filter + projection logic.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::company::runtime::CompanyRuntime;
use crate::error::OpenCompanyError;
use crate::ports::CompanyStore;
use crate::ports::general_channel::{
    GENERAL_CHANNEL_ID, GENERAL_CHANNEL_NAME, decode_general_chat_id,
};
use crate::ports::types::{
    Actor, ActorKind, Attachment, ChatOutput, ChatOutputKind, CompanyEvent, CompanyId,
    CompanyRecord, EventSeq, Mention, MentionTarget, StoredEvent, TurnStep, UtteranceKind,
};
use crate::server::readable::{DisplayNames, project_history};

/// The largest message page either history surface may materialize. Keeping
/// the limit beside the shared reader prevents a new caller from turning its
/// `Vec` reservation back into an allocation controlled by the request.
pub const CHAT_HISTORY_PAGE_LIMIT: usize = 200;

/// Resolves an incoming `chat_id` to the `(desk_id, desk_name)` pair [`owns`]
/// filters on. `None` and every legacy General spelling resolve to #general;
/// anything else is resolved against the company's desks by [`desk_aliases`].
pub async fn resolve_seed_desk(
    store: &Arc<dyn CompanyStore>,
    company: &CompanyId,
    chat_id: Option<&str>,
) -> (String, String) {
    let desk = decode_general_chat_id(chat_id.unwrap_or_default().to_string());
    if desk == GENERAL_CHANNEL_ID {
        return (desk.clone(), desk);
    }
    match store.load(company).await {
        Ok(Some(record)) => desk_aliases(&record, Some(&desk)),
        Ok(None) | Err(_) => (desk.clone(), desk),
    }
}

/// The other spelling the same operator DM is journaled under, if `key` names
/// one.
///
/// A DM has two correct addresses and the host uses both. The console posts an
/// ordinary teammate's DM under the **bare** teammate id (`dmThreadId`,
/// `views/room/channels.ts`), while a DM hive keys its episode -- and therefore
/// every row the episode journals (`hive::conducted`'s `chat:`) -- under
/// `dm:<id>`. A reader that matched only the address it was asked for saw half
/// its own conversation: the operator's message under one key and the episode's
/// replies under the other, so an episode's transcript vanished on reload while
/// the company sat blocked on an approval it had raised there.
///
/// `agent_channels` already folds both spellings for an agent's own session and
/// says why ("the console and the route disagree and both are correct"); this
/// is that rule, for a reader that starts from either address.
///
/// # Why this is only reached for a non-desk key
///
/// Because a declared desk resolves first. A blueprint may name both a desk and
/// a teammate `main` -- manifest validation does not forbid the collision
/// (issue #1743) -- and the desk must keep the bare key, so this is asked only
/// once `resolve_desk_id` has declined it.
#[must_use]
pub fn dm_sibling(record: &CompanyRecord, key: &str) -> Option<String> {
    // **A declared desk owns its key outright, and grows no second one.**
    //
    // Asserted here rather than assumed of the callers. `desk_aliases` does
    // reach this only after `resolve_desk_id` declines, but `operator`'s own
    // `resolve_desk` matches `manifest.group_chats` alone -- so an **overlay**
    // desk, created from the console and absent from the manifest, arrives
    // looking unmatched. Where one shares an id with a teammate, that desk's
    // transcript would have taken the teammate's DM rows (tinysweeper on
    // #2484). `resolve_desk_id` is the rule that knows about overlays, so it
    // is the one asked.
    if record.resolve_desk_id(key).is_some() {
        return None;
    }
    let prefix = crate::runtime::assignee::DM_PREFIX;
    // **The exact id first, and only then the prefix.**
    //
    // A teammate may itself be named `dm:<something>` -- nothing forbids it,
    // and this module already carries the mirror case of a teammate named for
    // a General spelling. Stripping first would answer `dm:ceo` with `ceo`'s
    // sibling while a teammate literally called `dm:ceo` sat in the roster,
    // handing one teammate's DM the other's rows.
    if let Some(agent) = record.resolve_roster_agent_id(key) {
        return Some(format!("{prefix}{agent}"));
    }
    key.strip_prefix(prefix)
        .and_then(|bare| record.resolve_roster_agent_id(bare))
}

/// [`resolve_seed_desk`] for a caller that already holds the record.
///
/// The cycle's briefings do: they are handed a `&CompanyRecord` and were paying
/// for a `load` per message to answer a question the record in their hand
/// already answers. Same resolution, no store round-trip — and one body, so the
/// two cannot drift into disagreeing about what a desk id means.
pub fn desk_aliases(record: &CompanyRecord, chat_id: Option<&str>) -> (String, String) {
    let desk = decode_general_chat_id(chat_id.unwrap_or_default().to_string());
    if desk == GENERAL_CHANNEL_ID {
        return (desk.clone(), desk);
    }
    let desk = desk.as_str();
    // **Through `resolve_desk_id`, not a second lookup of its own** (codex +
    // coderabbit on #1972). That function already answers "which desk is this
    // key", and it answers two things a one-pass `id == key || name == key`
    // find gets wrong: an **overlay desk** — one created from the console, which
    // lives in `overlay_desks` and not in the manifest at all — is a routable
    // desk, and an **exact id beats a display-name alias**, because desk
    // creation enforces unique ids but not unique names, so `{id: "ops", name:
    // "sales"}` can sit ahead of `{id: "sales", …}` and answer for it. Getting
    // that wrong here does not merely miss lines, it *merges* two desks: `owns`
    // would then be handed one desk's id and another's name.
    let Some(id) = record.resolve_desk_id(desk) else {
        // Not a desk this company declares — an ad-hoc thread id or a DM.
        //
        // An ad-hoc thread owns everything journaled under that exact string,
        // which is what the verbatim pair says. A DM owns **both** spellings it
        // is journaled under, and `owns` already matches either slot — so the
        // sibling rides in the name slot, which that function only ever
        // compares and never renders. See [`dm_sibling`].
        let sibling = dm_sibling(record, desk).unwrap_or_else(|| desk.to_string());
        return (desk.to_string(), sibling);
    };
    let name = record
        .manifest
        .group_chats
        .iter()
        .find(|chat| chat.id == id)
        .map(|chat| chat.name.clone())
        .or_else(|| {
            record
                .overlay_desks
                .iter()
                .find(|overlay| overlay.id == id)
                .map(|overlay| overlay.name.clone())
        })
        .unwrap_or_else(|| id.clone());
    (id, name)
}

/// Does a chat id stamped on a room's own bookkeeping name the conversation
/// being read? Matches either slot the resolver produced, as [`owns`] does.
fn bookkeeping_names(stored: &str, desk_id: &str, desk_name: &str) -> bool {
    stored == desk_id || stored == desk_name
}

/// Does a conversation id stamped onto a record name `desk`? A record with no
/// stamped conversation names none.
pub fn stamped_conversation_is(origin: Option<&str>, desk: &str) -> bool {
    origin == Some(desk)
}

/// One channel this agent can read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Channel {
    /// The desk id the journal stores rows under.
    pub id: String,
    /// The desk's display name — `chat_history::owns` matches either.
    pub name: String,
    /// How the cue names this channel to the agent.
    pub label: String,
}

/// Every channel this agent can read: each desk it sits on, its own DM, and
/// the company's General line.
///
/// Enumerated the way [`CompanyRecord::agent_desk_tools`] enumerates desks —
/// manifest desks first, then operator-created overlay desks, deduplicated —
/// so a teammate seated through the console is in its own session exactly as a
/// manifest member is.
pub fn agent_channels(record: &CompanyRecord, agent_id: &str) -> Vec<Channel> {
    let mut seen = std::collections::HashSet::new();
    let mut channels = Vec::new();

    let manifest = record
        .manifest
        .group_chats
        .iter()
        .map(|chat| chat.id.clone());
    let overlay = record.overlay_desks.iter().map(|desk| desk.id.clone());
    for desk_id in manifest.chain(overlay) {
        if !seen.insert(desk_id.clone()) {
            continue;
        }
        if !record
            .effective_desk_members(&desk_id)
            .iter()
            .any(|member| member == agent_id)
        {
            continue;
        }
        let name = desk_display_name(record, &desk_id);
        channels.push(Channel {
            label: format!("#{}", name),
            id: desk_id,
            name,
        });
    }

    // This agent's own direct line — under **both** spellings it is journaled
    // under, because the console and the route disagree and both are correct.
    //
    // `dmThreadId` (`views/room/channels.ts`) posts a DM under the teammate's
    // **bare id**; `dm:<id>` is the console's channel key and is *also* a
    // documented key on the chat route (`assignee::dm_key`), which a teammate
    // whose id is a General spelling is always addressed by. A session that
    // listed only one of them would miss every DM keyed the other way — which
    // is the whole of the operator's own line to this agent.
    //
    // Keyed on the id and never the name: renaming somebody must not move their
    // DM or orphan its history (issue #364).
    for (label, id) in [
        ("dm", agent_id.to_string()),
        (
            "dm",
            format!("{}{agent_id}", crate::runtime::assignee::DM_PREFIX),
        ),
    ] {
        if seen.insert(id.clone()) {
            channels.push(Channel {
                label: label.to_string(),
                name: id.clone(),
                id,
            });
        }
    }

    if seen.insert(GENERAL_CHANNEL_ID.to_string()) {
        channels.push(Channel {
            label: "#general".to_string(),
            name: GENERAL_CHANNEL_NAME.to_string(),
            id: GENERAL_CHANNEL_ID.to_string(),
        });
    }

    channels
}

/// The desk's display name, falling back to its id.
pub(crate) fn desk_display_name(record: &CompanyRecord, desk_id: &str) -> String {
    record
        .manifest
        .group_chats
        .iter()
        .find(|chat| chat.id == desk_id)
        .map(|chat| chat.name.clone())
        .or_else(|| {
            record
                .overlay_desks
                .iter()
                .find(|desk| desk.id == desk_id)
                .map(|desk| desk.name.clone())
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| desk_id.to_string())
}

/// Whether a stored event belongs to the desk identified by `desk_id` /
/// `desk_name`. A dispatch terminal with no origin belongs to no conversation.
pub fn owns(desk_id: &str, desk_name: &str, event: &CompanyEvent) -> bool {
    let stored = match event {
        CompanyEvent::AgentReply { chat_id, .. } => chat_id.as_str(),
        CompanyEvent::OperatorMessage { chat, .. } => chat.as_deref().unwrap_or(GENERAL_CHANNEL_ID),
        CompanyEvent::DeskTaskCompleted { origin_chat_id, .. } => match origin_chat_id.as_deref() {
            Some(origin) => origin,
            None => return false,
        },
        _ => return false,
    };
    stored == desk_id || stored == desk_name
}

/// The channel line a settled dispatch leaves behind (issue #377) —
/// `finished → In review`.
///
/// Deliberately **structural and short**: where the card landed, and nothing
/// else. The run's prose already reaches the same channel as the orchestrator's
/// relay bubble (#151), so repeating it here would put one run's words into one
/// conversation twice. What was missing was never the words — it was the fact
/// that the card *settled*, and *where*, which a reader watching only the prose
/// could not tell apart from "still working".
///
/// "finished" means *the run stopped*, not *it succeeded* — the same reading
/// [`CompanyEvent::DeskTaskCompleted`] itself takes. A cancelled or failed
/// dispatch lands in To-do and says so; a paused one says Paused. That is the
/// whole point: the misleading case this exists for is precisely the run that
/// stopped without finishing the work.
///
/// An unrecognised column id passes through **verbatim**, the same posture
/// `harness::lifecycle::relay_text` takes — a newer host naming a column this
/// build has not heard of should read a little raw, never render blank.
///
/// The label is [`crate::ledger::board`]'s, not a fourth copy of it. This
/// function used to carry its own `match` from id to label — one of three on
/// the host and a fourth in the console — and each was a place a renamed column
/// could half-land.
///
/// Pinned by tests on both sides of the wire: the console has its own
/// `dispatchMarkerText` (`frontend/src/lib/chat.ts`), because the live SSE
/// frame carries the raw column id rather than prose and a marker renders
/// synchronously from it, with no ledger read to await. That copy is the one
/// remaining exception, and it is the safe one: two spellings of a sentence can
/// only *reword* a marker across a reload — never double it, since the dedupe
/// is on identity, and never lose a card.
pub fn dispatch_marker_text(column: &str) -> String {
    format!("finished → {}", crate::ports::tasks::column_label(column))
}

#[cfg(test)]
impl MessageView {
    /// A bare row, for tests that exercise the folds rather than the projection.
    ///
    /// Test-only on purpose: the real constructor is `From<StoredEvent>` and has
    /// to stay the only way a view is built from a journal, or a projection
    /// concern could be skipped by a caller that assembled one by hand.
    pub(crate) fn for_test(id: &str, author: &str, text: &str, audience: Vec<String>) -> Self {
        Self {
            id: id.to_owned(),
            channel: author.to_owned(),
            admin_only: false,
            cue_author: author.to_owned(),
            author: author.to_owned(),
            cue_text: text.to_owned(),
            text: text.to_owned(),
            at_millis: 0.0,
            mine: false,
            by_person: false,
            aside_audience: audience,
            hive: None,
            steps: Vec::new(),
            task_id: None,
            parent_id: None,
            reactions: Vec::new(),
            mentions: Vec::new(),
            attachments: Vec::new(),
            outputs: Vec::new(),
            resolution_user_facing: false,
            resolution_code: None,
            resolution_pair_agent_id: None,
            resolution_provider_slug: None,
        }
    }
}

/// Who is reading a desk history. `mine` is relative to this.
///
/// There is no `From<StoredEvent> for MessageView`, and there cannot be:
/// `mine` depends on who is asking. With one operator it was safe to hardcode
/// `true`; with several users it would mark everyone's messages as everyone
/// else's.
#[derive(Clone, Debug, PartialEq)]
pub enum Viewer {
    /// An operator or platform credential. Legacy unattributed messages are
    /// theirs, because that is who sent them before users existed.
    Operator,
    /// A human collaborator, by user id.
    User(String),
}

/// The label a message with no nameable human author carries in an agent's cue.
///
/// Safe to sit in the same namespace as roster ids and user ids: a manifest
/// refuses the reserved ids (`company/manifest.rs`), and a minted user id is
/// not this word. Nothing a message *body* can say matters either, because
/// bodies are nested under their own speaker's label by
/// `chat_seed::prefix_every_line`.
pub const CUE_OPERATOR_LABEL: &str = "operator";

/// The signed-in person behind a message, when there is one.
///
/// `Some(id)` for a [`ActorKind::User`] actor and nothing else. An agent-sent
/// message (a crossing referral arrives as one) and a machine credential both
/// answer `None` — neither names a person.
pub fn cue_author_id(by: &Option<Actor>) -> Option<String> {
    match by {
        // CodeRabbit: an agent-authored crossing arrives as an
        // `OperatorMessage` too (the `ActorKind::Agent` arm a few lines below
        // this function's own callers, in `MessageView::project`) — this
        // matched only `User` and fell through to the `operator` fallback for
        // that arm, so the raw view and the cue line both attributed the
        // teammate's own line to "operator". Both actor kinds name a real
        // sender; only a machine credential (`None`, or neither kind) has
        // nobody to name.
        Some(actor) if matches!(actor.kind, ActorKind::User | ActorKind::Agent) => {
            Some(actor.id.clone())
        }
        _ => None,
    }
}

/// **What an agent is told to call the sender** — a stable id, not a screen name.
///
/// This is the single source of truth for that string. `chat_seed::operator_label`
/// delegates to it, [`MessageView::cue_author`] is projected from it, and the
/// per-agent session route ships it to the console, so the byline an agent was
/// handed, the one the seed writes and the one an operator reads in the raw
/// view cannot drift apart.
///
/// # Why an id and not the display name the console shows
///
/// Two reasons, and the second is the one that matters.
///
/// Resolving a name costs a store read per distinct author, on a projection
/// that runs inside the per-company cycle lock and whose whole design note is
/// that it must not do avoidable I/O.
///
/// And a display name is **neither unique nor unforgeable**. The label becomes
/// a per-line attribution prefix ([`prefix_every_line`](crate::harness::built_in::chat_seed),
/// issues #1956 / #2075), so a person who set their display name to a
/// teammate's id would have their own lines prefixed as if that teammate had
/// said them. An id cannot be chosen, so it cannot be chosen to impersonate.
///
/// The console is under the opposite constraint — it shows a person to other
/// people, and an id is not a name there — which is why `author_labels` walks
/// the display ladder and lands on `"someone"`. The two answers are different
/// on purpose; what must never happen is a surface claiming to show one and
/// showing the other.
pub fn cue_author(by: &Option<Actor>) -> String {
    cue_author_id(by).unwrap_or_else(|| CUE_OPERATOR_LABEL.to_string())
}

/// One message in a desk history, independent of transport. Mirrors
/// `frontend/src/lib/chat.ts`. The GraphQL `Message` type and the REST
/// `chat/history` JSON shape both project from this.
#[derive(Clone, Debug)]
pub struct MessageView {
    /// The message id (its EventLog sequence position).
    pub id: String,
    /// The channel the message came in on.
    pub channel: String,
    /// The author label.
    pub author: String,
    /// **What an agent is told to call this row's author** — see
    /// [`cue_author`].
    ///
    /// Deliberately not [`Self::author`]: that one is the display name a
    /// *person* reads, and the two resolve differently on purpose. Projected
    /// here so the per-agent session route can ship the agent's own byline to
    /// the console without the console guessing at it — a raw view that showed
    /// the display name while claiming to show the cue would be asserting the
    /// agent saw something it did not.
    ///
    /// Mirrors `agent_session::body_of` arm for arm; the two are pinned
    /// together by `cue_author_matches_the_envelope_the_agent_is_handed`.
    pub cue_author: String,
    /// The message text.
    pub text: String,
    /// **What the agent was actually handed for this row** — the text before
    /// [`readable_moves`](crate::server::readable::readable_moves) projected it
    /// for a person.
    ///
    /// Same reasoning as [`Self::cue_author`], applied to the other half of
    /// the cue line: `render_cues` in `agent_session.rs` prepends
    /// `[channel · author] text` using the **pre-rewrite** body (`body_of`
    /// reads the stored event directly, never a projected `MessageView`), so
    /// a surface that claims to show what the model saw — the raw-turns view
    /// — must not feed it [`Self::text`], which has already had `!support
    /// #topic ^3` turned into prose. Equal to [`Self::text`] on every row
    /// the display projection does not touch.
    pub cue_text: String,
    /// When it was journaled, epoch millis.
    pub at_millis: f64,
    /// Whether it is the operator's own message.
    pub mine: bool,
    /// Whether a **person** wrote this line, as opposed to the runtime.
    ///
    /// Not derivable downstream, which is why it is projected here (issue
    /// #1734). [`Self::mine`] answers "did *you* write it" and is per-viewer, so
    /// a colleague's message reads `mine: false` and reaches the console on the
    /// company side of the transcript — indistinguishable there from an agent
    /// reply. [`Self::channel`] cannot separate them either: the offline echo
    /// brain names its own outbound channel `operator`, exactly as the
    /// `OperatorMessage` arm does, so a journaled echo reply and a human's
    /// message carry the same label.
    ///
    /// The host is the only layer that still knows the difference — it is
    /// reading the event variant. Anything downstream is guessing, and the guess
    /// this exists to stop is chat marking a colleague's own words as the echo
    /// brain's, which fabricates an attribution rather than merely missing one.
    ///
    /// `true` for [`CompanyEvent::OperatorMessage`] and nothing else. A
    /// dispatch marker and an agent reply are both `false`: neither was typed by
    /// a person.
    pub by_person: bool,
    /// The addressees of this row when the host narrowed it — a seat's `dm`
    /// inside a desk — else empty. Projected as `audience`; an operator reads
    /// the row regardless, because audience is a coordination device between
    /// agents and never access control.
    pub aside_audience: Vec<String>,
    /// Where this reply sits in the company hive's transcript (OC-2): its
    /// hive sequence, episode and thread. `None` for a row that did not come
    /// through the hive.
    pub hive: Option<crate::ports::types::HiveRef>,
    /// Whether this row may reach only administrators (issue #1781 review,
    /// Codex P1).
    ///
    /// `true` for exactly one shape today: an `owner`-destination workflow
    /// report that fell back to the operator channel because the company has
    /// no mailbox, or no active admin has an address. The ordinary email
    /// branch of that same destination reaches active admins only
    /// (`workflows::delivery::owner_recipients`); this field is what lets the
    /// channel fallback honour the same restriction rather than silently
    /// widening the audience to every signed-in company user. The caller
    /// (`server::operator::chat_history_response`) drops any row with this set
    /// before returning to a non-admin viewer — see
    /// [`OWNER_FALLBACK_REPORT_AUTHOR`](crate::runtime::OWNER_FALLBACK_REPORT_AUTHOR)
    /// for how the underlying event is marked.
    pub admin_only: bool,
    /// The scrubbed processing steps behind a company reply, so a rehydrated
    /// transcript renders the same tool-call timeline the live turn showed.
    /// Empty for operator messages and tool-less replies.
    pub steps: Vec<TurnStep>,
    /// The board card this reply is about (issue #246) — the card the turn
    /// opened, or the dispatched card the turn ran for (#185).
    ///
    /// Projected here so a rehydrated transcript renders the same "card opened"
    /// chip the live turn showed. Both surfaces read it from this one field, so
    /// REST and GraphQL cannot disagree about which messages carry a card
    /// (issue #65's whole point). `None` on operator messages and on every
    /// reply journaled before the field existed.
    ///
    /// Also `None` once the card itself is gone, whoever deleted it — see
    /// [`drop_dead_cards`]. The journal still records that the turn opened a
    /// card, because it did; this field answers the narrower question the
    /// renderer actually asks, which is whether there is still a card to link
    /// to (issue #984).
    pub task_id: Option<String>,
    /// Addressable workspace objects produced by this reply's turn.
    ///
    /// Empty for messages predating output tracking and after every recorded
    /// target has been removed. The journal remains append-only; history
    /// projects only targets that are still clickable.
    pub outputs: Vec<ChatOutput>,
    /// The message this one replies to (issue #364), by that message's own id —
    /// what makes a thread survive a reload rather than living in one browser.
    ///
    /// `None` on a message posted straight into the channel, which is every
    /// message journaled before threads were persisted.
    pub parent_id: Option<String>,
    /// Who reacted to this message with what (issue #364), one row per person
    /// per emoji, oldest reaction first.
    ///
    /// Rows rather than a tally, because a tally cannot answer the two
    /// questions the console actually asks of a reaction — *who* reacted, and
    /// *have I* — and the second is what makes the chip a toggle rather than an
    /// ever-increasing counter. Grouping rows into a count is the renderer's
    /// job; deriving names from a count is impossible.
    pub reactions: Vec<ReactionView>,
    /// Who this message names, in reading order.
    ///
    /// Spans plus a **label**, never a target id: this is the surface a member
    /// reads other members' messages through, and handing every reader the raw
    /// user id of everyone ever mentioned would widen who-sees-what for no gain
    /// the renderer can use. Same discipline as [`ReactionView::by_label`], and
    /// the same reason.
    ///
    /// Empty for a message that mentions nobody, which is every message
    /// journaled before mentions existed.
    pub mentions: Vec<MentionView>,
    /// Files attached to this message (issue #1682), each a durable reference
    /// into the company workspace with the store-computed name / mime / size.
    ///
    /// Projected straight from the stored [`Attachment`] rows — the name and
    /// mime are already the store's, resolved server-side at send time, so this
    /// surface adds no viewer-scoping the way [`MentionView`] does: an
    /// attachment names a file the operator themself put in this company's own
    /// workspace, reachable by the same person through the blob route.
    ///
    /// Empty on an [`AgentReply`](CompanyEvent::AgentReply), a system pill, and
    /// every operator message journaled before this field existed — the shared
    /// [`MessageView`], so REST and GraphQL carry the same rows (issue #65).
    pub attachments: Vec<Attachment>,
    /// Whether this row is a classified resolution failure (keys rework
    /// #2306, round-2 review KR-L2-03) — a pinned provider gone or switched
    /// off, a broken company default, a provider with no key, or no model
    /// chosen at all. `false` for every ordinary reply and every other
    /// failure class (a tool timeout, an empty response, a rate limit),
    /// which keep only [`Self::text`]'s generic retry wording, exactly as
    /// before this field existed.
    ///
    /// Derived in [`Self::project`] from the stored `AgentReply`'s own
    /// `text` — `server::operator::spawn_chat_turn` writes the bare X9
    /// sentence there, unwrapped, for exactly this class of failure — rather
    /// than a new field on [`CompanyEvent::AgentReply`] itself, so the ~30
    /// other call sites that construct that variant need no change.
    pub resolution_user_facing: bool,
    /// One of the codes `docs/key-reworks/in-use-guards.md` §5 names, when
    /// [`Self::resolution_user_facing`] is `true`.
    pub resolution_code: Option<String>,
    /// The agent this failure's *pair* names, when the classifier could
    /// recover one (`company::inference::copy::classify`'s hidden marker —
    /// today, only the pin pre-check attaches one).
    pub resolution_pair_agent_id: Option<String>,
    /// The provider slug the failure names, when the classifier could
    /// recover one — only `pair_provider_removed`'s sentence names a raw
    /// slug rather than a display label (there is no row left to read one
    /// from); every other code leaves this `None`.
    pub resolution_provider_slug: Option<String>,
}

/// One mention inside one message, as a reader sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionView {
    /// The literal span the author typed, so the renderer highlights what is
    /// actually in the text rather than what the target is called now.
    pub text: String,
    /// Byte offset of [`Self::text`] in the message body.
    pub offset: usize,
    /// Who was named, as a display label — a teammate's id, a person's label, a
    /// desk's name, or `everyone`. Never a raw user id.
    pub label: String,
    /// Whether this mention is the viewer's own — what the console renders as a
    /// highlighted chip and counts as "this message is for me". Relative to the
    /// [`Viewer`], on the same terms [`MessageView::mine`] is.
    ///
    /// True for a direct mention of the viewer **and** for `@everyone`, because
    /// a broadcast is addressed to them too.
    pub mine: bool,
    /// Whether this mention renders but does not ping — a duplicate, a mention
    /// past the cap, or a target that has since left the company.
    pub quiet: bool,
}

/// One person's reaction to one message, as a reader sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReactionView {
    /// The emoji.
    pub emoji: String,
    /// Who reacted, as a display label — never a raw user id, for the same
    /// reason [`author_labels`] never hands out an email.
    pub by_label: String,
    /// Whether this row is the viewer's own. Relative to the [`Viewer`], on the
    /// same terms [`MessageView::mine`] is.
    pub mine: bool,
}

/// How a reaction's author is keyed while folding.
///
/// A signed-in person keys on their user id; anything else — a platform
/// credential, an event journaled before attribution existed — keys on the one
/// shared "operator" identity, which is the same collapse
/// [`MessageView::project`] makes for authorship. Two different machine
/// credentials therefore share a reaction row, which is correct: they are the
/// same principal as far as this company's history is concerned.
fn reaction_actor_key(by: &Option<Actor>) -> String {
    match by {
        Some(actor) if actor.kind == ActorKind::User => format!("user:{}", actor.id),
        _ => "operator".to_string(),
    }
}

/// Folds every [`CompanyEvent::ReactionToggled`] in a log into per-message
/// reaction rows, keyed by the reacted-to message's id.
///
/// Last event per `(message, actor, emoji)` wins — that is what makes the
/// route's explicit `on` flag idempotent — and a row that ends up `off` is
/// dropped entirely rather than kept as a zero. Order is first-set order, so a
/// message's chips do not reshuffle between reads.
struct ReactionFold {
    // (message, actor, emoji) → (position among first-seen keys, currently on).
    state: HashMap<(u64, String, String), (usize, bool)>,
    seen: usize,
}

impl ReactionFold {
    fn observe(&mut self, event: &StoredEvent, wanted: Option<&HashSet<u64>>) {
        let CompanyEvent::ReactionToggled {
            message_seq,
            emoji,
            on,
            by,
        } = &event.event
        else {
            return;
        };
        if wanted.is_some_and(|ids| !ids.contains(&message_seq.value())) {
            return;
        }
        let key = (message_seq.value(), reaction_actor_key(by), emoji.clone());
        match self.state.get_mut(&key) {
            Some(slot) => slot.1 = *on,
            None => {
                self.state.insert(key, (self.seen, *on));
                self.seen += 1;
            }
        }
    }

    fn finish(
        self,
        viewer: &Viewer,
        authors: &HashMap<String, String>,
    ) -> HashMap<String, Vec<ReactionView>> {
        let mut rows: Vec<(usize, u64, String, String)> = self
            .state
            .into_iter()
            .filter(|(_, (_, on))| *on)
            .map(|((message, actor, emoji), (order, _))| (order, message, actor, emoji))
            .collect();
        rows.sort_unstable();

        let mut out: HashMap<String, Vec<ReactionView>> = HashMap::new();
        for (_, message, actor, emoji) in rows {
            let (by_label, mine) = match actor.strip_prefix("user:") {
                Some(user_id) => (
                    authors
                        .get(user_id)
                        .cloned()
                        .unwrap_or_else(|| "someone".to_string()),
                    *viewer == Viewer::User(user_id.to_string()),
                ),
                None => ("operator".to_string(), matches!(viewer, Viewer::Operator)),
            };
            out.entry(message.to_string())
                .or_default()
                .push(ReactionView {
                    emoji,
                    by_label,
                    mine,
                });
        }
        out
    }
}

#[cfg(test)]
fn fold_reactions(
    stored: &[StoredEvent],
    viewer: &Viewer,
    authors: &HashMap<String, String>,
) -> HashMap<String, Vec<ReactionView>> {
    let mut fold = ReactionFold {
        state: HashMap::new(),
        seen: 0,
    };
    for event in stored {
        fold.observe(event, None);
    }
    fold.finish(viewer, authors)
}

impl MessageView {
    /// Projects a stored event for one viewer.
    ///
    /// `authors` maps user id → display label, resolved once per history
    /// rather than per message.
    pub fn project(
        stored: StoredEvent,
        viewer: &Viewer,
        authors: &HashMap<String, String>,
    ) -> Self {
        let id = stored.seq.value().to_string();
        let at_millis = stored.at_millis as f64;
        match stored.event {
            CompanyEvent::AgentReply {
                agent_id,
                text,
                steps,
                task_id,
                outputs,
                parent,
                mentions,
                audience,
                hive,
                ..
            } => {
                // Keys rework #2306, round-2 review KR-L2-03: re-classifies
                // the stored `text` against the same X9 sentences
                // `server::operator::spawn_chat_turn` writes verbatim (no
                // "Nothing was left half-done" wrapper) for exactly this
                // class of failure. `None` for every ordinary reply and
                // every other failure class, which keep their existing text
                // unchanged.
                let resolution = crate::company::inference::copy::classify(&text);
                MessageView {
                    id,
                    channel: agent_id.clone(),
                    admin_only: agent_id == crate::runtime::OWNER_FALLBACK_REPORT_AUTHOR,
                    // `body_of`'s `AgentReply` arm names the agent, so this does.
                    cue_author: agent_id.clone(),
                    author: agent_id,
                    // `body_of`'s `AgentReply` arm hands the agent this body
                    // untouched; `text` is projected for a person at the end of
                    // `history_for_desk`.
                    cue_text: text.clone(),
                    text,
                    at_millis,
                    mine: false,
                    // The runtime wrote this, whichever brain produced it.
                    by_person: false,
                    aside_audience: audience,
                    hive,
                    steps,
                    task_id,
                    outputs,
                    parent_id: parent.map(|seq| seq.value().to_string()),
                    reactions: Vec::new(),
                    mentions: project_mentions(&mentions, authors, viewer),
                    // A reply is the company's own voice and carries no operator
                    // upload (issue #1682).
                    attachments: Vec::new(),
                    resolution_user_facing: resolution.is_some(),
                    resolution_code: resolution.as_ref().map(|r| r.code.to_string()),
                    resolution_pair_agent_id: resolution
                        .as_ref()
                        .and_then(|r| r.pair_agent_id.clone()),
                    resolution_provider_slug: resolution.and_then(|r| r.provider_slug),
                }
            }
            CompanyEvent::OperatorMessage {
                text,
                by,
                parent,
                mentions,
                attachments,
                ..
            } => {
                // `voice` is what the console draws the byline from: `senderOf`
                // reads `channel`, not `author`, and treats the values in its
                // COMPANY_VOICE set ("operator", "console", …) as "no distinct
                // speaker — use the room's own name".
                //
                // That is right for a message a person sent. It is wrong for a
                // referral, which arrives authored by a TEAMMATE: falling into
                // that set made design's channel name the speaker, so an
                // engineer asking design read as design talking to itself. An
                // agent-authored line names the agent, exactly as an
                // `AgentReply` already does one arm below.
                let (author, mine, by_person, voice) = match &by {
                    // Sent by a signed-in human.
                    Some(actor) if actor.kind == ActorKind::User => {
                        let label = authors
                            .get(&actor.id)
                            .cloned()
                            .unwrap_or_else(|| "someone".to_string());
                        (
                            label,
                            *viewer == Viewer::User(actor.id.clone()),
                            true,
                            "operator".to_string(),
                        )
                    }
                    // A TEAMMATE sent it — a crossing referral arrives on the
                    // target's desk as a message authored by the agent that
                    // asked (tinyhivemind P15).
                    //
                    // Without this arm it fell to the machine-credential
                    // fallback below and was projected as `operator`, with
                    // `mine: true` for the operator's own view — so a question
                    // engineering asked read as one the person at the console
                    // had asked. That is the same shape as the `agent: None`
                    // → `agent_id: "operator"` defect (#885): a fallback that
                    // means "no person to name" rendered as a specific, wrong
                    // person. `mine` is emphatically false: nobody typed it.
                    Some(actor) if actor.kind == ActorKind::Agent => {
                        (actor.id.clone(), false, false, actor.id.clone())
                    }
                    // Sent with a machine credential, or journaled before
                    // attribution existed. Either way there is no person to
                    // name, and it belongs to whoever holds that credential.
                    _ => (
                        "operator".to_string(),
                        matches!(viewer, Viewer::Operator),
                        true,
                        "operator".to_string(),
                    ),
                };
                MessageView {
                    aside_audience: Vec::new(),
                    hive: None,
                    id,
                    channel: voice,
                    admin_only: false,
                    // The agent's byline for this row, resolved by the one
                    // function that decides it. NOT `author` above: that is the
                    // display name a person reads, and it lands on "someone"
                    // where this lands on the user id.
                    cue_author: cue_author(&by),
                    author,
                    // `body_of`'s `OperatorMessage` arm applies no rewrite
                    // either, so the cue and the rendered text agree here.
                    cue_text: text.clone(),
                    text,
                    at_millis,
                    mine,
                    // Decided with the author above, not assumed: this arm used
                    // to be "the one arm where a person typed it", which stopped
                    // being true when a referral began arriving here authored by
                    // a teammate. `byPerson` gates real behaviour downstream —
                    // the console's inline-reply promotion reads it — so a
                    // teammate's line claiming to be a person's is not cosmetic.
                    by_person,
                    steps: Vec::new(),
                    task_id: None,
                    outputs: Vec::new(),
                    parent_id: parent.map(|seq| seq.value().to_string()),
                    reactions: Vec::new(),
                    mentions: project_mentions(&mentions, authors, viewer),
                    // Issue #1682: the operator's attached files, carried
                    // through so a reload renders the same chips the live send
                    // showed.
                    attachments,
                    // An operator message is never a resolution failure — that
                    // notice is authored by the runtime (`AgentReply`, above).
                    resolution_user_facing: false,
                    resolution_code: None,
                    resolution_pair_agent_id: None,
                    resolution_provider_slug: None,
                }
            }
            // The dispatch terminal (issue #377), as the channel marker a
            // reader needs to see the card settle.
            //
            // A **dedicated arm**, not a lean on the defensive fallback below:
            // that one renders `format!("{other:?}")`, so without this the
            // marker would reach a person as a line of Rust `Debug` output —
            // and it would do so only on reload, which is the half of this
            // feature nobody watches while developing it.
            //
            // Authored as `system` on both keys, which is what makes the
            // console render it as a centred pill rather than a company bubble
            // (`MessageRow`), and `mine: false` because nobody said it.
            // `task_id` carries the card so the pill can link to it — the same
            // field, and therefore the same renderer, an `AgentReply`'s "card
            // opened" chip uses. No new `MessageView` field: this type is
            // shared with the GraphQL `Message` projection, and the reuse is
            // what keeps #377 additive on both wire surfaces at once.
            //
            // No `steps`: a marker is not a turn, so there is no timeline on it.
            //
            // `parent_id` **is** carried, since issue #1890 B. A marker was
            // never threaded because a card recorded no thread to thread it
            // into — not because a marker cannot be threaded — so a card raised
            // inside a thread settled flat in the channel and the thread that
            // asked for the work never showed it finishing. The card carries
            // its root now, the terminal captures it, and this is where it
            // reaches the reader. `None` is still the overwhelmingly common
            // case: it is every card raised straight into a channel.
            CompanyEvent::DeskTaskCompleted {
                task_id,
                column,
                origin_parent,
                ..
            } => MessageView {
                id,
                channel: crate::ports::SYSTEM_AUTHOR.to_string(),
                admin_only: false,
                // `body_of` hands an agent no structural marker, so nothing
                // ever reads this — named rather than left to drift.
                cue_author: crate::ports::SYSTEM_AUTHOR.to_string(),
                author: crate::ports::SYSTEM_AUTHOR.to_string(),
                // `body_of` never delivers this marker to an agent (see the
                // comment on `cue_author` above); equal to `text` for the same
                // reason that one is named rather than left to drift.
                cue_text: dispatch_marker_text(&column),
                text: dispatch_marker_text(&column),
                at_millis,
                mine: false,
                by_person: false,
                aside_audience: Vec::new(),
                hive: None,
                steps: Vec::new(),
                task_id: Some(task_id),
                outputs: Vec::new(),
                // Rendered the same way an `OperatorMessage`'s parent is, a few
                // arms up — the console keys a thread off this string and does
                // not care which event minted it.
                parent_id: origin_parent.map(|seq| seq.value().to_string()),
                reactions: Vec::new(),
                mentions: Vec::new(),
                attachments: Vec::new(),
                // A dispatch marker is never a resolution failure.
                resolution_user_facing: false,
                resolution_code: None,
                resolution_pair_agent_id: None,
                resolution_provider_slug: None,
            },
            // `owns` never admits other variants into a history.
            other => MessageView {
                id,
                channel: crate::ports::SYSTEM_AUTHOR.to_string(),
                admin_only: false,
                // `body_of` hands an agent no structural marker, so nothing
                // ever reads this — named rather than left to drift.
                cue_author: crate::ports::SYSTEM_AUTHOR.to_string(),
                author: crate::ports::SYSTEM_AUTHOR.to_string(),
                // `body_of` never delivers this fallback marker to an agent
                // either — same reasoning as the arm above.
                cue_text: format!("{other:?}"),
                text: format!("{other:?}"),
                at_millis,
                mine: false,
                by_person: false,
                aside_audience: Vec::new(),
                hive: None,
                steps: Vec::new(),
                task_id: None,
                outputs: Vec::new(),
                parent_id: None,
                reactions: Vec::new(),
                mentions: Vec::new(),
                attachments: Vec::new(),
                // The defensive fallback is never a resolution failure.
                resolution_user_facing: false,
                resolution_code: None,
                resolution_pair_agent_id: None,
                resolution_provider_slug: None,
            },
        }
    }
}

/// Turns stored mentions into what a particular reader should see.
///
/// Two things happen here and nowhere else:
///
/// * **Ids become labels.** A [`MentionTarget::User`] carries a user id, which
///   no member-facing surface hands out; it is resolved through the same
///   `authors` map the byline above the message uses, so a chip and the author
///   line can never disagree about what somebody is called. A target that
///   resolves to nothing falls back to the literal text the author typed, minus
///   its `@` — which is exactly what a reader would have seen anyway.
/// * **`mine` is decided.** Per viewer, and `true` for `@everyone` as well as
///   for a direct mention, because a broadcast is addressed to this reader too.
pub(crate) fn project_mentions(
    mentions: &[Mention],
    authors: &HashMap<String, String>,
    viewer: &Viewer,
) -> Vec<MentionView> {
    mentions
        .iter()
        .map(|mention| {
            let fallback = || mention.text.trim_start_matches('@').to_string();
            let (label, mine) = match &mention.target {
                MentionTarget::Agent { id } => (id.clone(), false),
                MentionTarget::Desk { id } => (id.clone(), false),
                MentionTarget::User { id } => (
                    authors.get(id).cloned().unwrap_or_else(fallback),
                    *viewer == Viewer::User(id.clone()),
                ),
                // Addressed to the room, so it is addressed to whoever is
                // reading — including the operator credential, which is a
                // reader even though it is not a person.
                MentionTarget::Everyone => ("everyone".to_string(), true),
            };
            MentionView {
                text: mention.text.clone(),
                offset: mention.offset,
                label,
                mine,
                quiet: mention.quiet,
            }
        })
        .collect()
}

/// The blast radius of issue #885, for one company.
///
/// Reported rather than repaired. See [`channel_attributed_replies`] for why a
/// repair is not available.
///
/// # The figure is not comparable across the #966 cutover
///
/// Host-authored notices — the approval-overflow line, the `"Acknowledged."`
/// fallback, the failed-continuation report — used to journal under the
/// operator channel, so every one already on disk is counted here as damage.
/// Since #966 they journal under [`SYSTEM_AUTHOR`](crate::ports::SYSTEM_AUTHOR)
/// and are not counted, because they are correct rows and inflating this number
/// with them would make the one figure that has to be trustworthy the least
/// trustworthy one.
///
/// The consequence is a step in the series that nothing on the wire labels: a
/// company's `affected` can fall without a single row being repaired, purely
/// because it stopped minting new false positives. Read a decline across that
/// boundary as "the bleeding stopped", never as "history got better" — no row
/// counted here has ever become attributable, and none can.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AttributionAudit {
    /// Every `AgentReply` inspected.
    pub replies: usize,
    /// Those whose stored `agent_id` names no roster teammate.
    pub affected: usize,
    /// The distinct bad `agent_id` values, with a count each — so an operator
    /// can see at a glance whether they are all `operator` (the #885 shape) or
    /// whether something else is also writing a non-agent into the field.
    pub by_agent_id: std::collections::BTreeMap<String, usize>,
}

impl AttributionAudit {
    /// Folds one page of journal events in.
    ///
    /// Split out from the paging so the rule itself is testable without a
    /// `CompanyRuntime` — the classification is the part that can be silently
    /// wrong, and a store fixture would only obscure it.
    pub fn fold(&mut self, page: &[StoredEvent], is_roster_agent: impl Fn(&str) -> bool) {
        for stored in page {
            let CompanyEvent::AgentReply { agent_id, .. } = &stored.event else {
                continue;
            };
            self.replies += 1;
            if !is_roster_agent(agent_id) {
                self.affected += 1;
                *self.by_agent_id.entry(agent_id.clone()).or_insert(0) += 1;
            }
        }
    }
}

/// Counts desk replies whose author was overwritten with a destination (#885).
///
/// # The rule
///
/// [`CompanyEvent::AgentReply`]'s `agent_id` is documented as *"the agent that
/// produced the reply"*, so a value naming no roster teammate is by definition
/// not an author. That is the whole test, and it is deliberately not `== "operator"`:
/// the same defect on any other channel — a Telegram chat id, a desk slug —
/// produces a different wrong string and has to be counted too.
///
/// # Why this only counts, and never repairs
///
/// **The true author is not recoverable from what is on disk.** `agent_id` was
/// the only field that carried it and it was overwritten in place. Nothing else
/// on the event, and nothing beside it, records who spoke:
///
/// * `steps` — [`TurnStep`] has no agent field;
/// * `task_id` — `None` on exactly these rows (it is set on the dispatch path,
///   which is the one writer that was already correct);
/// * `parent` — names the question, never the answerer;
/// * `chat_id` — the desk, which yields *today's* desk lead. Desk membership is
///   mutable (manifest members unioned with operator-added overlay members), so
///   that is a re-derivation against current state, not a recovery — and it is
///   silently wrong for any desk whose lead has changed since.
/// * the metering store — bucketed per calendar **day** with per-agent
///   aggregates, so it cannot name the author of one message.
///
/// So a backfill would synthesise an author rather than restore one, and a
/// confident wrong name in a transcript is worse than an admitted gap. These
/// rows are ambiguous, permanently, and this reports how many there are.
///
/// # One deliberate false positive
///
/// `CompanyRuntime::announce_continuation_failure` journals a **system** notice
/// as `agent_id: "operator"` on purpose — it is the runtime telling the operator
/// a continuation failed, not an agent speaking. It is indistinguishable from a
/// #885 row on disk, so it is counted here. The count is therefore an upper
/// bound; in practice that notice is rare enough not to move it.
/// Whether a stored `agent_id` names an author we can actually resolve.
///
/// The roster, **plus three ids that are truthful authors without being teammates**.
///
/// [`SYSTEM_AUTHOR`](crate::ports::SYSTEM_AUTHOR) (issue #966) is the runtime
/// speaking for itself — an approval-overflow notice, the `"Acknowledged."`
/// fallback, a failed-continuation report. Those rows are *correct*, so counting
/// them as damage would inflate the one figure in #965 that has to be
/// trustworthy, and would caption a legitimate system message as unattributable.
///
/// `CONFINED_AGENT_ID`
/// is deliberately not a roster id — it names no teammate, carries no manifest
/// grants and cannot be addressed — but a copilot turn genuinely authored its
/// reply, so the id is a truthful author rather than a destination that leaked
/// into the field. Counting it would swap one wrong answer for a permanent
/// false positive, and would make the audit's number drift upward on a company
/// doing nothing wrong.
///
/// [`WORKFLOW_REPLY_AUTHOR`](crate::runtime::WORKFLOW_REPLY_AUTHOR) is the same
/// case as `SYSTEM_AUTHOR`: a delivered workflow report is journaled under it
/// on purpose, not a destination that leaked into the author field, so it must
/// not inflate the count either — and unlike `SYSTEM_AUTHOR`, no roster entry
/// can *ever* shadow it, on this company or any other: the id is hyphenated,
/// so neither a minted slug nor a manifest-declared one can equal it (see the
/// constant's doc).
///
/// [`OWNER_FALLBACK_REPORT_AUTHOR`](crate::runtime::OWNER_FALLBACK_REPORT_AUTHOR)
/// is the same case again, one level narrower: it is `WORKFLOW_REPLY_AUTHOR`'s
/// own admin-only sibling, journaled when an `owner` report has no mailbox to
/// reach (issue #1781 review, Codex P2) — a legitimate report, deliberately
/// unmintable for the same reason, and it must not inflate the count either.
///
/// This is the single predicate the audit and any presentation of its result
/// must share; two copies would let the count and the rendering disagree about
/// which rows are unknown.
pub fn is_known_author(agent_id: &str, record: &CompanyRecord) -> bool {
    agent_id == crate::ports::CONFINED_AGENT_ID
        || agent_id == crate::ports::SYSTEM_AUTHOR
        || agent_id == crate::runtime::WORKFLOW_REPLY_AUTHOR
        || agent_id == crate::runtime::OWNER_FALLBACK_REPORT_AUTHOR
        || record.resolve_roster_agent_id(agent_id).is_some()
}

/// `is_admin` gates the same admin-only rows [`history_for_desk`] and
/// [`history_total_for_desk`] already exclude for a non-admin viewer (issue
/// #1781 review, Codex P2): an owner-fallback report is invisible on the
/// transcript and over SSE, but the raw `replies` count previously included
/// it regardless of caller, so a Member watching the count tick up around an
/// owner-fallback delivery could infer a hidden admin-only message exists.
/// Excluded here — before `fold` — so a non-admin's count can never expose
/// that inference.
pub async fn channel_attributed_replies(
    runtime: &CompanyRuntime,
    record: &CompanyRecord,
    is_admin: bool,
) -> Result<AttributionAudit, OpenCompanyError> {
    const PAGE: usize = 512;
    let mut audit = AttributionAudit::default();
    let mut cursor = EventSeq::new(0);
    loop {
        let page = runtime
            .events()
            .read_from(runtime.id(), cursor, PAGE)
            .await?;
        if page.is_empty() {
            break;
        }
        let last = page[page.len() - 1].seq;
        if is_admin {
            audit.fold(&page, |agent_id| is_known_author(agent_id, record));
        } else {
            let visible: Vec<StoredEvent> = page
                .into_iter()
                .filter(|stored| !is_admin_only_event(&stored.event))
                .collect();
            audit.fold(&visible, |agent_id| is_known_author(agent_id, record));
        }
        cursor = EventSeq::new(last.value() + 1);
    }
    Ok(audit)
}

/// Loads roster display labels for a company: user id → label.
///
/// Prefers a display name, and falls back to one derived from the email's
/// local part rather than the whole address: a desk history is read by every
/// member, and it should not hand each of them everyone else's email. The
/// ladder is [`UserRecord::display_label`] — the same one the profile pane and
/// the mention picker use, so the same person reads the same way everywhere.
pub async fn author_labels(
    runtime: &CompanyRuntime,
) -> Result<HashMap<String, String>, OpenCompanyError> {
    let users = runtime.users().list_users(runtime.id()).await?;
    Ok(users
        .into_iter()
        .map(|user| {
            let label = user
                .display_label()
                .unwrap_or_else(|| "someone".to_string());
            (user.id, label)
        })
        .collect())
}

/// One desk's message history for one viewer, most-recent last.
///
/// `before_seq` is an opaque EventLog cursor (a sequence position); only
/// messages before it are considered. `first` caps how many of the remaining,
/// most-recent messages come back.
///
/// `is_admin` gates [`MessageView::admin_only`] rows (issue #1781 review,
/// Codex P1): a non-admin viewer never sees one, and the exclusion happens
/// **inside** the paging loop, before a row counts toward `first` — filtering
/// the returned `Vec` afterward would silently short a non-admin's page by
/// however many admin-only rows it held, which is a pagination bug, not
/// merely a display one.
///
/// Shared by the GraphQL `Chat.history` resolver and the REST
/// `GET .../chat/history` route so the two can never disagree about what a
/// desk's history contains (issue #65).
pub async fn history_for_desk(
    runtime: &CompanyRuntime,
    desk_id: &str,
    desk_name: &str,
    viewer: &Viewer,
    before_seq: Option<u64>,
    first: usize,
    is_admin: bool,
) -> Result<Vec<MessageView>, OpenCompanyError> {
    // A page is events rather than messages: a busy company can put unrelated
    // events between two chat turns. Walking backward keeps the newest `first`
    // transcript entries without ever materialising that unrelated journal.
    const EVENT_PAGE: usize = 512;

    // A zero-sized GraphQL page is a valid request, and the REST limit can be
    // clamped to zero. It must not touch the journal merely to construct an
    // empty response.
    let first = first.min(CHAT_HISTORY_PAGE_LIMIT);
    if first == 0 {
        return Ok(Vec::new());
    }

    // One roster read per history, not one per message.
    let authors = author_labels(runtime).await?;
    let names = DisplayNames::load(runtime).await;
    let mut cursor = before_seq.map(EventSeq::new);
    let mut messages = Vec::with_capacity(first);
    while messages.len() < first {
        let page = runtime
            .events()
            .read_before(runtime.id(), cursor, EVENT_PAGE)
            .await?;
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|event| event.seq);
        for event in page {
            if owns(desk_id, desk_name, &event.event) {
                let message = MessageView::project(event, viewer, &authors);
                // Excluded before it counts toward `first` — see this fn's
                // doc. A non-admin viewer's page fills with the next visible
                // row instead of coming back short.
                if message.admin_only && !is_admin {
                    continue;
                }
                // The retired trace-grammar hive's closing reports and failure
                // notices (`hive-report` / `hive-failure`) and the referral
                // relay's author (`hive-referral`) were never teammates. Old
                // journals still hold their rows; they stay out of the page,
                // excluded here so a page fills with the next visible row
                // instead of coming back short.
                if is_retired_hive_author(&message.channel) {
                    continue;
                }
                messages.push(message);
                if messages.len() == first {
                    break;
                }
            }
        }
    }

    // `read_before` supplies each page newest-first, as does `messages` above.
    // Restore chronological order for the renderer before attaching reactions.
    messages.reverse();

    // Reactions necessarily follow their message. Once the displayed window is
    // known, fold only toggles that could affect one of its messages, streaming
    // forward from the window's oldest id through the current tail. The cursor
    // limits *messages*, not the reaction snapshot: a later toggle still
    // changes the state displayed on an older message.
    let wanted: HashSet<u64> = messages
        .iter()
        .filter_map(|message| message.id.parse::<u64>().ok())
        .collect();
    if let Some(oldest) = wanted.iter().min().copied() {
        let mut next = oldest.saturating_add(1);
        let mut fold = ReactionFold {
            state: HashMap::new(),
            seen: 0,
        };
        loop {
            let page = runtime
                .events()
                .read_from(runtime.id(), EventSeq::new(next), EVENT_PAGE)
                .await?;
            if page.is_empty() {
                break;
            }
            for event in &page {
                fold.observe(event, Some(&wanted));
            }
            next = page
                .last()
                .map(|event| event.seq.value().saturating_add(1))
                .unwrap_or(next);
            if page.len() < EVENT_PAGE {
                break;
            }
        }
        let mut reactions = fold.finish(viewer, &authors);
        for message in &mut messages {
            message.reactions = reactions.remove(&message.id).unwrap_or_default();
        }
    }

    drop_dead_cards(runtime, &mut messages).await?;
    drop_dead_outputs(runtime, &mut messages).await?;
    project_history(&mut messages, &names);
    Ok(messages)
}

/// Whether `author` is one of the retired hive's reserved system authors —
/// rows an old journal holds that no teammate wrote.
fn is_retired_hive_author(author: &str) -> bool {
    matches!(author, "hive-report" | "hive-failure" | "hive-referral")
}

/// Blanks `task_id` on any row naming a card the board no longer has
/// (issue #984).
///
/// # Why this is a projection concern and not a write
///
/// The obvious fix — clear `task_id` on the journaled rows when the card is
/// deleted — is not available, and it is worth saying why so nobody reaches for
/// it later. `task_id` is not a column on a mutable chat row: it is a field of
/// the [`CompanyEvent::AgentReply`] that *happened*, and the journal is
/// append-only. Rewriting it would be editing history to record that a turn
/// never opened a card, when it did.
///
/// So the id stays in the journal and the **projection** stops reporting it once
/// the card is gone. That is also strictly more correct than a write would have
/// been:
///
/// - It covers a card deleted by **any** path, not just the chat chip — the
///   board's own `TaskEditDialog` delete leaves exactly the same stale chip, and
///   always did.
/// - It covers cards deleted **before** this change, which no write-time fix
///   could reach.
/// - It cannot drift: there is one board, read at render time, rather than a
///   denormalised copy that a missed call site leaves stale.
///
/// Without this a dismissal survives only until the next full reload:
/// `transcripts` is React state and is never serialised, but the console
/// rehydrates from this projection (`lib/chat.ts`'s `fromHistory`) and merges by
/// message id, so an empty transcript takes every row back — chip included. The
/// chip would return pointing at a `404`, which reads as the delete having
/// failed.
///
/// One board read per history, and only when the window actually carries a
/// card — the same shape as the single roster read above, not a read per
/// message.
async fn drop_dead_cards(
    runtime: &CompanyRuntime,
    messages: &mut [MessageView],
) -> Result<(), OpenCompanyError> {
    if !messages.iter().any(|message| message.task_id.is_some()) {
        return Ok(());
    }

    let live: HashSet<String> = runtime
        .tasks()
        .list(runtime.id())
        .await?
        .into_iter()
        .map(|task| task.id)
        .collect();

    for message in messages {
        if message
            .task_id
            .as_deref()
            .is_some_and(|id| !live.contains(id))
        {
            message.task_id = None;
        }
    }
    Ok(())
}

/// Removes reply output links whose target no longer exists.
///
/// Like [`drop_dead_cards`], this is a projection rule rather than a journal
/// rewrite: the turn really did produce the object, but history must not
/// rehydrate a button that now leads nowhere.
async fn drop_dead_outputs(
    runtime: &CompanyRuntime,
    messages: &mut [MessageView],
) -> Result<(), OpenCompanyError> {
    if !messages.iter().any(|message| !message.outputs.is_empty()) {
        return Ok(());
    }

    let needs_workspace = messages.iter().any(|message| {
        message
            .outputs
            .iter()
            .any(|output| output.kind == ChatOutputKind::WorkspaceNode)
    });
    let live_workspace: HashSet<String> = if needs_workspace {
        runtime
            .workspace()
            .tree(runtime.id())
            .await?
            .into_iter()
            .map(|node| node.id)
            .collect()
    } else {
        HashSet::new()
    };
    let needs_artifacts = messages.iter().any(|message| {
        message
            .outputs
            .iter()
            .any(|output| output.kind == ChatOutputKind::Artifact)
    });
    let live_artifacts: HashSet<(String, String, u32)> = if needs_artifacts {
        runtime
            .artifacts()
            .list(runtime.id(), None)
            .await?
            .into_iter()
            .flat_map(|artifact| {
                artifact.versions.into_iter().map(move |version| {
                    (
                        artifact.id.clone(),
                        artifact.task_id.clone(),
                        version.version,
                    )
                })
            })
            .collect()
    } else {
        HashSet::new()
    };

    for message in messages {
        let mut kept = Vec::with_capacity(message.outputs.len());
        for output in message.outputs.drain(..) {
            let live = match output.kind {
                ChatOutputKind::WorkspaceNode => live_workspace.contains(&output.target_id),
                ChatOutputKind::Artifact => {
                    let Some(task_id) = output.task_id.as_deref() else {
                        continue;
                    };
                    let Some(version) = output.version else {
                        continue;
                    };
                    live_artifacts.contains(&(
                        output.target_id.clone(),
                        task_id.to_string(),
                        version,
                    ))
                }
            };
            if live {
                kept.push(output);
            }
        }
        message.outputs = kept;
    }
    Ok(())
}

/// Counts a desk's messages before a cursor without materialising them.
///
/// GraphQL's [`Page`](crate::server::graphql::pagination::Page) exposes an
/// unpaginated `total`, while the REST transcript endpoint deliberately does
/// not. Keep that potentially full journal walk out of [`history_for_desk`],
/// so bounded transcript readers stop as soon as their requested window is
/// complete.
///
/// `is_admin` excludes an owner-fallback report the same way
/// [`history_for_desk`]'s `is_admin` param excludes it from `items` (issue
/// #1781 review, Codex P2): without this, a non-admin querying a GraphQL desk
/// that holds one — notably a grandfathered real desk at the literal
/// `operator` id — got a `total` counting a row `items` had already hidden,
/// which both breaks `Page.total`'s item-count contract and reveals that a
/// hidden admin report exists.
pub async fn history_total_for_desk(
    runtime: &CompanyRuntime,
    desk_id: &str,
    desk_name: &str,
    before_seq: Option<u64>,
    is_admin: bool,
) -> Result<i32, OpenCompanyError> {
    const EVENT_PAGE: usize = 512;

    let mut next = EventSeq::new(0);
    let mut total = 0i32;
    loop {
        let page = runtime
            .events()
            .read_from(runtime.id(), next, EVENT_PAGE)
            .await?;
        if page.is_empty() {
            break;
        }
        for event in &page {
            if before_seq.is_some_and(|before| event.seq.value() >= before) {
                return Ok(total);
            }
            if !owns(desk_id, desk_name, &event.event) {
                continue;
            }
            // Same admin-only exclusion `MessageView::project` applies to
            // `history_for_desk`'s rows (see `is_admin_only_event`'s doc).
            if !is_admin && is_admin_only_event(&event.event) {
                continue;
            }
            total = total.saturating_add(1);
        }
        let Some(last) = page.last() else {
            break;
        };
        next = EventSeq::new(last.seq.value().saturating_add(1));
        if page.len() < EVENT_PAGE {
            break;
        }
    }
    Ok(total)
}

/// Whether `event` is the owner-fallback report — admin-only on both
/// [`history_for_desk`] (via [`MessageView::project`]'s `admin_only` field,
/// which applies the identical `agent_id == OWNER_FALLBACK_REPORT_AUTHOR`
/// check inline) and [`history_total_for_desk`]'s count (issue #1781 review,
/// Codex P2), so the two projections of the same journal cannot disagree
/// about which rows a non-admin is shown.
fn is_admin_only_event(event: &CompanyEvent) -> bool {
    matches!(
        event,
        CompanyEvent::AgentReply { agent_id, .. }
            if agent_id == crate::runtime::OWNER_FALLBACK_REPORT_AUTHOR
    )
}

#[cfg(test)]
#[path = "chat_history_attribution_audit_tests.rs"]
mod attribution_audit;
#[cfg(test)]
#[path = "chat_history_mentions_tests.rs"]
mod tests_mentions;
#[cfg(test)]
#[path = "chat_history_reactions_tests.rs"]
mod tests_reactions;
#[cfg(test)]
#[path = "chat_history_terminal_tests.rs"]
mod tests_terminal;

#[cfg(test)]
#[path = "chat_history_dead_card_test.rs"]
mod dead_card_test;

/// A completion the driver refused, reconciled against the row already on the
/// desk.
#[cfg(test)]
#[path = "chat_history_refused_completion_test.rs"]
mod refused_completion_test;

/// Where a referred line says it came from, and who it says is speaking.
#[cfg(test)]
#[path = "chat_history_agent_conversation_test.rs"]
mod agent_conversation_test;
#[cfg(test)]
#[path = "chat_history_referral_origin_crossing_test.rs"]
mod referral_origin_crossing_test;
#[cfg(test)]
#[path = "chat_history_referral_origin_episode_test.rs"]
mod referral_origin_episode_test;
#[cfg(test)]
#[path = "chat_history_referral_origin_relay_test.rs"]
mod referral_origin_relay_test;

#[cfg(test)]
#[path = "chat_history_referral_origin_test_support.rs"]
mod referral_origin_test_support;

/// How a chat selector becomes the `(desk id, desk name)` pair [`owns`] filters
/// on — the one answer to "which desk is this", shared by the seed, the cycle's
/// briefings and `read_thread`.
#[cfg(test)]
#[path = "chat_history_desk_resolution_test.rs"]
mod desk_resolution_test;
