//! Split from `built_in::tests::chat_seed_regression` (issue #1840) because
//! that inline module exceeded the 750-line file limit. See
//! `built_in_chat_seed_thread_binding_tests` for the rest.
//!
//! End-to-end proof that a chat reply is assembled WITH this desk's recent
//! journaled history in front of the model (issue #1840), driven through the
//! real `HarnessPool::run` path with only the model captured.
//!
//! Each test is RED on the pre-fix code: the old switch branch re-seeded via
//! OpenHuman's `seed_resume_from_thread_transcript`, which reads a file
//! OpenCompany never writes for a `chat_id`, so the model saw `history_len =
//! 0` and none of these markers reached it.

use super::*;

use std::sync::Mutex as StdMutex;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use tinyinference::model::{ChatModel, ModelRequest, ModelResponse};

use super::built_in_test_fixtures::*;
use crate::ports::events::EventStreamItem;
use crate::ports::types::{CompanyEvent, EventSeq, StoredEvent};

/// An appendable in-memory journal. `read_from` returns ascending order,
/// so the trait's default `read_before` yields the newest-first paging the
/// seed projector walks.
///
/// `reads` counts every `read_from` call (the default `read_before`'s
/// only path into a backend) — a stand-in for the filesystem backend's
/// whole-file JSONL scan (`store::fs::read_before`'s docs), so a test
/// can assert the seed projector only walks the journal when a chat
/// switch actually needs it, not on every chat turn (codex review
/// finding).
#[derive(Default)]
struct InMemoryLog {
    events: StdMutex<Vec<StoredEvent>>,
    reads: std::sync::atomic::AtomicUsize,
}

impl InMemoryLog {
    fn reads(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn operator(&self, chat: &str, text: &str) {
        self.push(CompanyEvent::OperatorMessage {
            text: text.to_string(),
            by: None,
            chat: Some(chat.to_string()),
            parent: None,
            deliverable: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
        });
    }
    /// An operator message posted inside the thread rooted at `parent`.
    fn operator_in(&self, chat: &str, text: &str, parent: u64) {
        self.push(CompanyEvent::OperatorMessage {
            text: text.to_string(),
            by: None,
            chat: Some(chat.to_string()),
            parent: Some(EventSeq::new(parent)),
            deliverable: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
        });
    }
    fn reply(&self, chat_id: &str, text: &str) {
        self.push(CompanyEvent::AgentReply {
            audience: Vec::new(),
            chat_id: chat_id.to_string(),
            agent_id: "ceo".to_string(),
            text: text.to_string(),
            steps: Vec::new(),
            task_id: None,
            outputs: Vec::new(),
            parent: None,
            mentions: Vec::new(),
            mention_depth: 0,
        });
    }
    fn push(&self, event: CompanyEvent) {
        let mut log = self.events.lock().unwrap();
        let seq = EventSeq::new(log.len() as u64);
        log.push(StoredEvent {
            seq,
            company: CompanyId::new("acme"),
            event,
            at_millis: seq.value(),
        });
    }
}

#[async_trait]
impl EventLog for InMemoryLog {
    async fn append(&self, _id: &CompanyId, event: CompanyEvent) -> crate::Result<EventSeq> {
        let mut log = self.events.lock().unwrap();
        let seq = EventSeq::new(log.len() as u64);
        log.push(StoredEvent {
            seq,
            company: CompanyId::new("acme"),
            event,
            at_millis: seq.value(),
        });
        Ok(seq)
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        seq: EventSeq,
        limit: usize,
    ) -> crate::Result<Vec<StoredEvent>> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self
            .events
            .lock()
            .unwrap()
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

/// A model that records the full text of every request it is handed, so a
/// test can assert which prior turns reached the model's context.
#[derive(Default)]
struct RecordingProvider {
    seen: Arc<StdMutex<Vec<String>>>,
}

#[async_trait]
impl ChatModel<()> for RecordingProvider {
    async fn invoke(
        &self,
        _state: &(),
        request: ModelRequest,
    ) -> tinyinference::Result<ModelResponse> {
        let joined = request
            .messages
            .iter()
            .map(|m| m.text())
            .collect::<Vec<_>>()
            .join("\n");
        self.seen.lock().unwrap().push(joined);
        // A fixed non-empty reply: empty would trip the empty-response
        // retry wrapper into a second invoke.
        Ok(ModelResponse::assistant("ok"))
    }
}

impl HarnessModel for RecordingProvider {
    fn telemetry_provider_id(&self) -> String {
        "recording".to_string()
    }
}

/// A fixture whose journal and model are observable: the returned `log` is
/// pre-populated by the test, and `seen` collects every model request.
fn recording_fixture() -> (Fixture, Arc<InMemoryLog>, Arc<StdMutex<Vec<String>>>) {
    let mut fx = fixture();
    let log = Arc::new(InMemoryLog::default());
    let provider = Arc::new(RecordingProvider::default());
    let seen = provider.seen.clone();
    fx.deps.events = Some(log.clone());
    fx.deps.provider = provider;
    (fx, log, seen)
}

/// A — fresh process, first chat turn on `general` (bound = None): the
/// prior journaled exchange is seeded into the model, and the current
/// message (already journaled) is not duplicated.
#[tokio::test]
async fn first_chat_turn_seeds_prior_journaled_exchange() {
    let (fx, log, seen) = recording_fixture();
    let rec = record();
    log.operator("general", "PRIOR_USER_MARKER");
    log.reply("general", "PRIOR_AGENT_MARKER");
    // The current operator message is journaled BEFORE the turn runs, just
    // as the server does — so the projector sees it as the newest event.
    log.operator("general", "CURRENT_MARKER");

    let pool = HarnessPool::new();
    pool.ensure(&rec, &fx.deps).await.expect("ensure");
    pool.run(
        &rec.id,
        "ceo",
        "CURRENT_MARKER",
        &fx.deps,
        crate::runtime::delegation::ChatTarget::channel(Some("general")),
    )
    .await
    .expect("chat turn runs");

    let all = seen.lock().unwrap().join("\n===\n");
    assert!(
        all.contains("PRIOR_USER_MARKER") && all.contains("PRIOR_AGENT_MARKER"),
        "the prior journaled exchange must reach the model: {all:?}"
    );
    assert_eq!(
        all.matches("CURRENT_MARKER").count(),
        1,
        "the current message is stripped from the seed, so it appears once \
         (as this turn's user message), not duplicated: {all:?}"
    );
}

/// B — an unthreaded (background) turn between two chat turns resets
/// `bound_chat` to None, making the next chat turn a switch. It must STILL
/// re-seed the desk's history — the exact "every background turn blinds the
/// next chat reply" failure the fix removes.
#[tokio::test]
async fn chat_turn_after_a_background_turn_still_seeds_history() {
    let (fx, log, seen) = recording_fixture();
    let rec = record();
    log.operator("general", "HISTORY_USER_MARKER");
    log.reply("general", "HISTORY_AGENT_MARKER");

    let pool = HarnessPool::new();
    pool.ensure(&rec, &fx.deps).await.expect("ensure");

    // First chat turn binds to general.
    pool.run(
        &rec.id,
        "ceo",
        "first",
        &fx.deps,
        crate::runtime::delegation::ChatTarget::channel(Some("general")),
    )
    .await
    .expect("first chat turn");
    // A background/unthreaded turn: resets bound_chat to None.
    pool.run_background(&rec.id, "ceo", "background", &fx.deps, None)
        .await
        .expect("background turn");

    let before = seen.lock().unwrap().len();
    // Second chat turn on general — a switch again, because the background
    // turn invalidated the binding.
    pool.run(
        &rec.id,
        "ceo",
        "second",
        &fx.deps,
        crate::runtime::delegation::ChatTarget::channel(Some("general")),
    )
    .await
    .expect("second chat turn");

    let after: Vec<String> = seen.lock().unwrap()[before..].to_vec();
    let last = after
        .last()
        .expect("the second chat turn made a model call");
    assert!(
        last.contains("HISTORY_USER_MARKER") && last.contains("HISTORY_AGENT_MARKER"),
        "a chat turn after a background turn must still see the desk's \
         recent history: {last:?}"
    );
}

/// C — isolation: history on desk A must never leak into a turn on desk B.
#[tokio::test]
async fn a_switch_seeds_only_the_incoming_desks_history() {
    let (fx, log, seen) = recording_fixture();
    let rec = record();
    log.operator("alpha", "ALPHA_USER_MARKER");
    log.reply("alpha", "ALPHA_AGENT_MARKER");
    log.operator("beta", "BETA_USER_MARKER");
    log.reply("beta", "BETA_AGENT_MARKER");

    let pool = HarnessPool::new();
    pool.ensure(&rec, &fx.deps).await.expect("ensure");
    pool.run(
        &rec.id,
        "ceo",
        "hello beta",
        &fx.deps,
        crate::runtime::delegation::ChatTarget::channel(Some("beta")),
    )
    .await
    .expect("beta chat turn");

    let all = seen.lock().unwrap().join("\n===\n");
    assert!(
        all.contains("BETA_USER_MARKER") && all.contains("BETA_AGENT_MARKER"),
        "beta's own history must be seeded: {all:?}"
    );
    assert!(
        !all.contains("ALPHA_USER_MARKER") && !all.contains("ALPHA_AGENT_MARKER"),
        "alpha's history must NEVER leak into a beta turn: {all:?}"
    );
}

/// D — DM parity: a `dm:<id>` thread seeds exactly like a named desk.
#[tokio::test]
async fn a_dm_thread_seeds_its_own_history() {
    let (fx, log, seen) = recording_fixture();
    let rec = record();
    log.operator("dm:teammate", "DM_USER_MARKER");
    log.reply("dm:teammate", "DM_AGENT_MARKER");

    let pool = HarnessPool::new();
    pool.ensure(&rec, &fx.deps).await.expect("ensure");
    pool.run(
        &rec.id,
        "ceo",
        "hey there",
        &fx.deps,
        crate::runtime::delegation::ChatTarget::channel(Some("dm:teammate")),
    )
    .await
    .expect("dm chat turn");

    let all = seen.lock().unwrap().join("\n===\n");
    assert!(
        all.contains("DM_USER_MARKER") && all.contains("DM_AGENT_MARKER"),
        "a DM thread's own history must be seeded (parity with named desks): {all:?}"
    );
}

/// E — a second chat turn on the SAME desk, back to back, is not a
/// switch: `bound_chat` already points at it, so `run_with_steer`'s
/// switch check must skip both the re-seed AND the journal read that
/// builds it. RED on the pre-fix code, which built the (costly on the
/// filesystem backend — `chat_seed::build_chat_seed`'s docs) seed in
/// the caller for every chat turn, switch or not, and simply discarded
/// it on a non-switch turn; GREEN once the projection only runs inside
/// the confirmed-switch branch (codex review finding).
#[tokio::test]
async fn a_non_switch_chat_turn_does_not_re_read_the_journal() {
    let (fx, log, _seen) = recording_fixture();
    let rec = record();
    log.operator("general", "PRIOR_USER_MARKER");
    log.reply("general", "PRIOR_AGENT_MARKER");
    // The current operator message for turn 1, journaled before it runs.
    log.operator("general", "first");

    let pool = HarnessPool::new();
    pool.ensure(&rec, &fx.deps).await.expect("ensure");

    // Turn 1 on "general": a switch (bound_chat starts None) — must
    // read the journal to build the seed.
    pool.run(
        &rec.id,
        "ceo",
        "first",
        &fx.deps,
        crate::runtime::delegation::ChatTarget::channel(Some("general")),
    )
    .await
    .expect("first chat turn");
    let reads_after_first = log.reads();
    assert!(
        reads_after_first > 0,
        "the first (switching) turn must read the journal to build its seed"
    );

    // The current operator message for turn 2, journaled before it runs
    // — same desk as turn 1, so `bound_chat` already matches it.
    log.operator("general", "second");

    // Turn 2 on "general" — NOT a switch. Must not touch the journal
    // again to build a seed nothing downstream will use.
    pool.run(
        &rec.id,
        "ceo",
        "second",
        &fx.deps,
        crate::runtime::delegation::ChatTarget::channel(Some("general")),
    )
    .await
    .expect("second chat turn");
    // The property this pins CHANGED when the session became
    // continuous, and the change is the feature rather than a
    // regression against it.
    //
    // It used to assert **zero** further reads: a same-desk turn was
    // not a switch, so no seed was built, so the journal was not
    // touched. An agent now asks what it missed on every chat turn —
    // that question is the whole of "one session", and its answer
    // cannot be cached, because another teammate may have said
    // something on another desk a moment ago.
    //
    // What must still hold is the bound. The delta walks **backwards
    // from the tail and stops at the watermark**, so a quiet company
    // costs one page and a busy one costs no more than
    // `SESSION_SCAN_LIMIT`. That is what the original test was
    // protecting — the fs backend's whole-journal scan — and it is
    // what this asserts now.
    let delta_reads = log.reads() - reads_after_first;
    assert!(
        delta_reads > 0,
        "a chat turn must ask what it missed; that is the session"
    );
    assert!(
        delta_reads <= reads_after_first,
        "the delta must cost no more than the seed it replaced: \
         {delta_reads} reads against {reads_after_first}"
    );
}

/// Two threads of ONE channel are two conversations (#1890). Moving
/// between them was not a switch while the binding was keyed on the
/// chat id alone, so the clear-and-re-seed never ran and the second
/// thread answered with the first one's turns still in `history`.
#[tokio::test]
async fn a_thread_switch_within_one_channel_re_seeds() {
    let (fx, log, _seen) = recording_fixture();
    let rec = record();
    log.operator("general", "root A"); // seq 0
    log.operator("general", "root B"); // seq 1
    log.operator_in("general", "first", 0); // seq 2 — turn 1, thread A

    let pool = HarnessPool::new();
    pool.ensure(&rec, &fx.deps).await.expect("ensure");

    pool.run(
        &rec.id,
        "ceo",
        "first",
        &fx.deps,
        crate::runtime::delegation::ChatTarget::in_thread(Some("general"), Some(EventSeq::new(0))),
    )
    .await
    .expect("first chat turn");
    let reads_after_first = log.reads();
    assert!(reads_after_first > 0, "the first turn binds and seeds");

    log.operator_in("general", "second", 1); // seq 3 — turn 2, thread B

    pool.run(
        &rec.id,
        "ceo",
        "second",
        &fx.deps,
        crate::runtime::delegation::ChatTarget::in_thread(Some("general"), Some(EventSeq::new(1))),
    )
    .await
    .expect("second chat turn");
    assert!(
        log.reads() > reads_after_first,
        "a different thread of the same channel is a switch: it must \
         clear the previous thread's history and re-seed from its own"
    );
}
