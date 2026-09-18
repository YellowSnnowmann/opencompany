use std::sync::Mutex as StdMutex;

use super::*;
use crate::ports::events::EventLog;
use crate::ports::runs::{RunOutcome, RunStatus, reap_orphaned_runs};
use crate::ports::types::EventSeq;
use crate::ports::types::StoredEvent;
use crate::store::FsOps;

/// An [`EventLog`] that keeps what it was handed.
#[derive(Default)]
struct MemLog {
    events: StdMutex<Vec<CompanyEvent>>,
}

impl MemLog {
    /// The `(from, to)` pairs journalled so far, in order.
    fn moves(&self) -> Vec<(Option<String>, String)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                CompanyEvent::RunStatusChanged { from, to, .. } => Some((from.clone(), to.clone())),
                _ => None,
            })
            .collect()
    }
}

#[async_trait]
impl EventLog for MemLog {
    async fn append(&self, _id: &CompanyId, event: CompanyEvent) -> Result<EventSeq> {
        let mut guard = self.events.lock().unwrap();
        guard.push(event);
        Ok(EventSeq::new(guard.len() as u64))
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        _seq: EventSeq,
        _limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        Ok(Vec::new())
    }
    fn subscribe(
        &self,
        _id: &CompanyId,
    ) -> futures::stream::BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(futures::stream::empty())
    }
}

/// A wrapped store over a real filesystem backend, plus its log.
fn store(dir: &std::path::Path) -> (EventingRunStore, Arc<MemLog>) {
    let log = Arc::new(MemLog::default());
    let inner: Arc<dyn RunStore> = Arc::new(FsOps::new(dir));
    (
        EventingRunStore::new(inner, log.clone() as Arc<dyn EventLog>),
        log,
    )
}

/// The happy path, end to end: mint, start, settle — three frames, each
/// naming both ends of its move.
#[tokio::test]
async fn every_transition_of_an_attempt_is_journalled() {
    let dir = tempfile::tempdir().unwrap();
    let (runs, log) = store(dir.path());
    let company = CompanyId::new("acme");

    runs.create_run(&company, NewRun::for_task("run-1", "card-1", "ceo"))
        .await
        .expect("mint");
    runs.begin_run(&company, "run-1", EventSeq::new(1))
        .await
        .expect("begin");
    runs.finish_run(&company, "run-1", RunOutcome::new(RunStatus::Succeeded))
        .await
        .expect("finish");

    assert_eq!(
        log.moves(),
        vec![
            (None, "pending".to_string()),
            (Some("pending".to_string()), "running".to_string()),
            (Some("running".to_string()), "succeeded".to_string()),
        ],
        "a mint and both transitions each journal one frame, naming both ends"
    );
}

/// **The case the issue's own proposed seam drops.**
///
/// `reap_orphaned_runs` settles crash-killed runs by calling `finish_run`
/// directly on the store — it never goes through `cycle.rs`. Emitting from
/// the cycle's call sites would leave exactly these runs, the ones the
/// reaper exists to make visible, moving in silence.
#[tokio::test]
async fn the_boot_reaper_journals_the_runs_it_settles() {
    let dir = tempfile::tempdir().unwrap();
    let (runs, log) = store(dir.path());
    let company = CompanyId::new("acme");

    runs.create_run(&company, NewRun::for_task("run-1", "card-1", "ceo"))
        .await
        .expect("mint");
    runs.begin_run(&company, "run-1", EventSeq::new(1))
        .await
        .expect("begin");

    let reaped = reap_orphaned_runs(&runs, &company).await.expect("reap");
    assert_eq!(reaped.len(), 1, "the active row is reclaimed: {reaped:?}");

    assert_eq!(
        log.moves().last().cloned(),
        Some((Some("running".to_string()), "failed".to_string())),
        "a run the host died under says so, rather than moving in silence"
    );
}

/// A write that revises a settled row's cost or step count is not a
/// transition and must not manufacture a frame — otherwise a consumer
/// applying frames sees a move that never happened.
#[tokio::test]
async fn a_write_that_does_not_move_the_status_journals_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (runs, log) = store(dir.path());
    let company = CompanyId::new("acme");

    let run = runs
        .create_run(&company, NewRun::for_task("run-1", "card-1", "ceo"))
        .await
        .expect("mint");
    let before = log.moves().len();

    let mut revised = run.clone();
    revised.step_count = 7;
    runs.put_run(&company, &revised).await.expect("revise");

    assert_eq!(
        log.moves().len(),
        before,
        "the status did not move, so nothing claims it did"
    );
}

/// The frame is appended only after the row is durable, so a consumer
/// reacting to it can never read state that has not landed.
///
/// Asserted by reading the store from inside the log's own `append`: at the
/// moment the frame exists, the row must already carry the status it names.
#[tokio::test]
async fn the_row_is_durable_before_its_frame_is_appended() {
    /// Reads the run back the instant a frame is appended.
    struct ReadsBack {
        runs: StdMutex<Option<Arc<dyn RunStore>>>,
        seen: StdMutex<Vec<(String, Option<RunStatus>)>>,
    }

    #[async_trait]
    impl EventLog for ReadsBack {
        async fn append(&self, id: &CompanyId, event: CompanyEvent) -> Result<EventSeq> {
            if let CompanyEvent::RunStatusChanged { run_id, to, .. } = &event {
                let inner = self.runs.lock().unwrap().clone();
                let landed = match inner {
                    Some(runs) => runs.get_run(id, run_id).await.ok().flatten(),
                    None => None,
                };
                self.seen
                    .lock()
                    .unwrap()
                    .push((to.clone(), landed.map(|run| run.status)));
            }
            Ok(EventSeq::new(1))
        }
        async fn read_from(
            &self,
            _id: &CompanyId,
            _seq: EventSeq,
            _limit: usize,
        ) -> Result<Vec<StoredEvent>> {
            Ok(Vec::new())
        }
        fn subscribe(
            &self,
            _id: &CompanyId,
        ) -> futures::stream::BoxStream<'static, crate::ports::events::EventStreamItem> {
            Box::pin(futures::stream::empty())
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let inner: Arc<dyn RunStore> = Arc::new(FsOps::new(dir.path()));
    let log = Arc::new(ReadsBack {
        runs: StdMutex::new(Some(inner.clone())),
        seen: StdMutex::new(Vec::new()),
    });
    let runs = EventingRunStore::new(inner, log.clone() as Arc<dyn EventLog>);
    let company = CompanyId::new("acme");

    runs.create_run(&company, NewRun::for_task("run-1", "card-1", "ceo"))
        .await
        .expect("mint");
    runs.begin_run(&company, "run-1", EventSeq::new(1))
        .await
        .expect("begin");

    let seen = log.seen.lock().unwrap().clone();
    assert!(!seen.is_empty(), "frames were appended");
    for (claimed, landed) in seen {
        assert_eq!(
            landed.map(|s| s.as_str().to_string()),
            Some(claimed.clone()),
            "a frame claiming `{claimed}` was appended before the row said so"
        );
    }
}
