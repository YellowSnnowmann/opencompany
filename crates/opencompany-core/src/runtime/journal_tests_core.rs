pub(super) use std::path::Path;

pub(super) use super::*;
pub(super) use crate::ports::now_millis;
pub(super) use crate::ports::types::EffectGroup;

pub(super) fn effect() -> Effect {
    Effect {
        kind: "filing.submit".into(),
        group: EffectGroup::Sign,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    }
}

/// A private directory for one test's journal file.
///
/// The name comes from the OS, not from [`crate::ports::generate_id`] —
/// minted ids are unique only within a process, so two test processes
/// sharing `/tmp` could otherwise land on the same journal path and mix
/// their records into one file. Since #386 that no longer produces an
/// unparseable line, but it still produces a journal holding another
/// test's history, which fails these assertions just as thoroughly.
/// Dropping the returned handle removes the directory, including after a
/// failed assert.
pub(super) fn tmp_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-journal-")
        .tempdir()
        .expect("tempdir")
}

/// An executed effect as journaled (issue #351): irreversible, against
/// `t-1`, unless a test says otherwise.
pub(super) fn executed(at_millis: u64) -> ExecutedEffect {
    ExecutedEffect {
        kind: "filing.submit".into(),
        amount_usd: None,
        task_id: Some("t-1".into()),
        at_millis,
        irreversible: true,
    }
}

pub(super) fn grant(id: &str, at_millis: u64) -> GrantedCall {
    GrantedCall {
        approval_id: ApprovalId::new(id),
        agent: "finance".into(),
        tool: "composio_execute".into(),
        args: crate::policy::test_support::composio_send_args(),
        at_millis,
        origin_thread: None,
        origin_parent: None,
        origin_task: None,
    }
}

pub(super) fn standing(id: &str, tool: &str, expires_at_millis: u64) -> StandingGrant {
    StandingGrant {
        id: GrantId::new(id),
        agent: "ops".into(),
        workflow: None,
        tool: tool.into(),
        verdict: crate::ports::types::Verdict::Approve,
        granted_by: Actor {
            kind: crate::ports::types::ActorKind::User,
            id: "user-42".into(),
        },
        approval_id: ApprovalId::new(format!("appr-{id}")),
        at_millis: 1_000,
        expires_at_millis,
        origin_thread: None,
        origin_parent: None,
        origin_task: None,
        scope: None,
    }
}

/// Every non-empty line of the journal at `path`, parsed. Panics with the
/// offending line's number and text when one does not parse, because a
/// torn line is exactly what these tests exist to catch and
/// `unwrap`-on-`Err` hides which line it was.
pub(super) async fn parse_every_line(path: &Path) -> Vec<JournalRecord> {
    let raw = tokio::fs::read_to_string(path).await.expect("journal file");
    raw.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, line)| {
            serde_json::from_str::<JournalRecord>(line)
                .unwrap_or_else(|e| panic!("line {} did not parse: {e}\n  {line}", i + 1))
        })
        .collect()
}

/// One value of every [`JournalRecord`] variant (issue #392).
///
/// Hand-built, so it carries its own completeness check below — the tag
/// count. The *classification* needs no such guard: `durability`'s match is
/// wildcard-free, so a new variant cannot compile until somebody decides
/// which failure it must survive.
pub(super) fn every_record_kind() -> Vec<JournalRecord> {
    vec![
        JournalRecord::EffectExecuted {
            key: "k".into(),
            effect: Some(executed(1)),
        },
        JournalRecord::ApprovalParked {
            id: ApprovalId::new("a"),
            effect: effect(),
            at_millis: 1,
            task: Some(TaskLink::Unlinked),
            thread: None,
            parent: None,
            cycle: None,
        },
        JournalRecord::ApprovalResolved {
            id: ApprovalId::new("a"),
        },
        JournalRecord::ApprovalExpired {
            id: ApprovalId::new("a"),
            at_millis: 2,
            reason: ExpiryReason::Ttl,
        },
        JournalRecord::ApprovalAmended {
            id: ApprovalId::new("a"),
            amended_effect: effect(),
            at_millis: 3,
        },
        JournalRecord::ApprovalGranted {
            grant: grant("a", 4),
        },
        JournalRecord::GrantDispatched {
            id: ApprovalId::new("a"),
            at_millis: 4,
        },
        JournalRecord::ApprovalContinuationQueued {
            continuation: ApprovalContinuation {
                call: grant("continuation", 4),
                verdict: crate::ports::types::Verdict::Approve,
                by: revoker(),
            },
        },
        JournalRecord::ApprovalContinuationDispatched {
            id: ApprovalId::new("continuation"),
            at_millis: 4,
        },
        JournalRecord::ApprovalContinuationConsumed {
            id: ApprovalId::new("continuation"),
        },
        JournalRecord::ApprovalContinuationExpired {
            id: ApprovalId::new("continuation"),
            at_millis: 5,
        },
        JournalRecord::GrantConsumed {
            id: ApprovalId::new("a"),
            effect: None,
        },
        JournalRecord::GrantExpired {
            id: ApprovalId::new("a"),
            at_millis: 5,
        },
        JournalRecord::StandingGrantMinted {
            grant: standing("s", "composio_execute", 9),
        },
        JournalRecord::StandingGrantRevoked {
            id: GrantId::new("s"),
            by: revoker(),
            at_millis: 6,
        },
        JournalRecord::StandingGrantExpired {
            id: GrantId::new("s"),
            at_millis: 7,
        },
        JournalRecord::CycleStarted {
            cycle_id: "c".into(),
            at_millis: 8,
            trigger: "test".into(),
        },
        JournalRecord::CycleFinished {
            cycle_id: "c".into(),
            at_millis: 9,
            error: None,
        },
        JournalRecord::BlockedNodeStashed {
            turn: "t".into(),
            workflow_id: "w".into(),
            input: serde_json::json!({}),
            started_by: StartedBy::Operator,
            thread_id: Some("lineage".into()),
            workflow_fingerprint: Some("fingerprint".into()),
            at_millis: 10,
        },
        JournalRecord::BlockedNodeReleased { turn: "t".into() },
        JournalRecord::BlockedNodeApproved { turn: "t".into() },
        JournalRecord::BlockedNodeDispatched { turn: "t".into() },
    ]
}

/// The operator who takes a standing grant back.
pub(super) fn revoker() -> Actor {
    Actor {
        kind: crate::ports::types::ActorKind::User,
        id: "user-42".into(),
    }
}

/// A record's `record` tag — the same name the serialized line carries, so a
/// failure names the variant rather than an index.
pub(super) fn record_tag(record: &JournalRecord) -> String {
    serde_json::to_value(record).unwrap()["record"]
        .as_str()
        .expect("every record is tagged")
        .to_string()
}

/// One `ApprovalParked` line, built from the given effect kind.
pub(super) fn parked_with_kind(kind: &str) -> JournalRecord {
    let mut effect = effect();
    effect.kind = kind.to_string();
    JournalRecord::ApprovalParked {
        id: ApprovalId::new("a"),
        effect,
        at_millis: 1,
        task: Some(TaskLink::Unlinked),
        thread: None,
        parent: None,
        cycle: None,
    }
}

/// A [`JournalStore`] whose `append_journal` fails exactly once — the
/// park-time write's transient failure — then passes every later append
/// straight through to an in-memory backend, including a retry of the
/// very same record. Mirrors `FailNJournalStore` in
/// `blocked_node_continuation_test`, scoped down to this module's own
/// unit tests via [`RuntimeJournal::with_store`].
pub(super) struct FailOnceJournalStore {
    inner: crate::ports::journal::MemoryJournalStore,
    failed: std::sync::atomic::AtomicBool,
}

impl FailOnceJournalStore {
    pub(super) fn new() -> Self {
        Self {
            inner: crate::ports::journal::MemoryJournalStore::default(),
            failed: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl JournalStore for FailOnceJournalStore {
    async fn append_journal(
        &self,
        id: &CompanyId,
        line: &str,
        durability: Durability,
    ) -> crate::Result<()> {
        if !self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Err(crate::error::OpenCompanyError::Store(
                "FailOnceJournalStore: forced failure on the first append".to_string(),
            ));
        }
        self.inner.append_journal(id, line, durability).await
    }

    async fn read_journal(&self, id: &CompanyId) -> crate::Result<Vec<String>> {
        self.inner.read_journal(id).await
    }

    async fn journal_imported(&self, id: &CompanyId) -> crate::Result<bool> {
        self.inner.journal_imported(id).await
    }

    async fn complete_import(&self, id: &CompanyId, lines: Vec<String>) -> crate::Result<()> {
        self.inner.complete_import(id, lines).await
    }
}
