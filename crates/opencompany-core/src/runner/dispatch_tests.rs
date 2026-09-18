use super::*;
use crate::harness::acp_run_turn::AcpUpdate;
use crate::runner::registry::{HarnessOffer, RunnerCapabilities, RunnerStatus};
use std::sync::Arc;

#[derive(Default)]
struct Recorder {
    opened: Mutex<Vec<(String, String)>>,
    prompted: Mutex<Vec<(String, String, String)>>,
    cancelled: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl RunnerLink for Arc<Recorder> {
    async fn open_session(&self, runner_id: &str, scope: &str) -> Result<String> {
        self.opened
            .lock()
            .unwrap()
            .push((runner_id.to_string(), scope.to_string()));
        // Deliberately the same id from every runner, so a test that
        // conflated namespaces would collide.
        Ok("sess-1".to_string())
    }
    async fn prompt(&self, runner: &str, session: &str, message: &str) -> Result<AcpTurn> {
        self.prompted.lock().unwrap().push((
            runner.to_string(),
            session.to_string(),
            message.to_string(),
        ));
        Ok(AcpTurn {
            updates: vec![AcpUpdate::MessageChunk(format!("ran on {runner}"))],
            stop_reason: "end_turn".to_string(),
        })
    }
    async fn cancel(&self, runner: &str, session: &str) -> Result<()> {
        self.cancelled
            .lock()
            .unwrap()
            .push((runner.to_string(), session.to_string()));
        Ok(())
    }
}

fn runner(id: &str, scope: &str, ready: bool, seen: u64) -> RunnerStatus {
    RunnerStatus {
        runner_id: id.to_string(),
        owner: "owner".to_string(),
        scopes: vec![scope.to_string()],
        capabilities: RunnerCapabilities {
            harnesses: vec![HarnessOffer {
                id: "claude".to_string(),
                ready,
            }],
            max_parallel: 1,
        },
        last_seen_millis: seen,
        connection: format!("conn-{id}"),
    }
}

fn dispatch(registry: Arc<RunnerRegistry>) -> (RunnerDispatch<Arc<Recorder>>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let dispatch = RunnerDispatch::new(registry, Arc::clone(&recorder)).with_clock(|| 0);
    (dispatch, recorder)
}

#[tokio::test]
async fn a_turn_lands_on_the_runner_holding_the_scope() {
    let registry = Arc::new(RunnerRegistry::new());
    registry.admit(runner("ada", "acme::ceo", true, 0));
    let (dispatch, recorder) = dispatch(registry);

    let turn = dispatch
        .prompt(&CompanyId::new("acme"), "acme::ceo", "do it", None)
        .await
        .unwrap();

    assert_eq!(turn.updates.len(), 1);
    assert_eq!(recorder.prompted.lock().unwrap()[0].0, "ada");
}

#[tokio::test]
async fn a_session_is_opened_once_and_reused() {
    // Opening per turn would give the harness no memory of the previous
    // question — every turn a fresh conversation.
    let registry = Arc::new(RunnerRegistry::new());
    registry.admit(runner("ada", "acme::ceo", true, 0));
    let (dispatch, recorder) = dispatch(registry);

    for _ in 0..3 {
        dispatch
            .prompt(&CompanyId::new("acme"), "acme::ceo", "again", None)
            .await
            .unwrap();
    }
    assert_eq!(recorder.opened.lock().unwrap().len(), 1);
    assert_eq!(recorder.prompted.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn two_runners_minting_the_same_session_id_do_not_collide() {
    // THE reason ids are rewritten rather than forwarded. Both runners here
    // answer `sess-1`; a single namespace would cross two companies' turns.
    let registry = Arc::new(RunnerRegistry::new());
    registry.admit(runner("ada", "acme::ceo", true, 0));
    registry.admit(runner("bob", "globex::cto", true, 0));
    let (dispatch, recorder) = dispatch(registry);

    dispatch
        .prompt(&CompanyId::new("acme"), "acme::ceo", "x", None)
        .await
        .unwrap();
    dispatch
        .prompt(&CompanyId::new("globex"), "globex::cto", "y", None)
        .await
        .unwrap();

    let prompted = recorder.prompted.lock().unwrap();
    assert_eq!(prompted[0].0, "ada");
    assert_eq!(prompted[1].0, "bob");
    // Two entries, one per (runner, scope) — not one shared by both.
    assert_eq!(dispatch.sessions.len(), 2);
}

#[tokio::test]
async fn no_attached_runner_says_exactly_that() {
    let registry = Arc::new(RunnerRegistry::new());
    let (dispatch, _) = dispatch(registry);

    let error = dispatch
        .prompt(&CompanyId::new("acme"), "acme::ceo", "x", None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("no runner is attached"),
        "{error}"
    );
}

#[tokio::test]
async fn a_signed_out_runner_is_reported_differently_from_a_missing_one() {
    // Different answers: start the desktop, versus sign the harness in. One
    // message for both would send someone looking in the wrong place.
    let registry = Arc::new(RunnerRegistry::new());
    registry.admit(runner("ada", "acme::ceo", false, 0));
    let (dispatch, _) = dispatch(registry);

    let error = dispatch
        .prompt(&CompanyId::new("acme"), "acme::ceo", "x", None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("no signed-in harness"),
        "{error}"
    );
}

#[tokio::test]
async fn a_runner_that_stopped_reporting_is_reported_as_such() {
    let registry = Arc::new(RunnerRegistry::new());
    registry.admit(runner("ada", "acme::ceo", true, 0));
    let recorder = Arc::new(Recorder::default());
    // A clock well past the presence TTL.
    let dispatch = RunnerDispatch::new(registry, recorder)
        .with_clock(|| crate::runner::registry::PRESENCE_TTL_MILLIS * 10);

    let error = dispatch
        .prompt(&CompanyId::new("acme"), "acme::ceo", "x", None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("stopped reporting"), "{error}");
}

#[tokio::test]
async fn a_replacement_runner_takes_the_next_turn() {
    // A runner can go away between turns. Re-choosing per turn is what
    // makes the next one land on whatever replaced it.
    let registry = Arc::new(RunnerRegistry::new());
    registry.admit(runner("ada", "acme::ceo", true, 0));
    let (dispatch, recorder) = dispatch(Arc::clone(&registry));

    dispatch
        .prompt(&CompanyId::new("acme"), "acme::ceo", "first", None)
        .await
        .unwrap();
    // Ada's laptop closes; Bob's takes the scope.
    registry.admit(runner("bob", "acme::ceo", true, 0));
    dispatch
        .prompt(&CompanyId::new("acme"), "acme::ceo", "second", None)
        .await
        .unwrap();

    let prompted = recorder.prompted.lock().unwrap();
    assert_eq!(prompted[0].0, "ada");
    assert_eq!(prompted[1].0, "bob");
    // Bob got his own session rather than inheriting Ada's id.
    assert_eq!(recorder.opened.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn cancelling_before_a_session_exists_is_a_no_op() {
    // A cancel racing the first turn is ordinary; failing it would surface
    // an alarming message for nothing.
    let registry = Arc::new(RunnerRegistry::new());
    registry.admit(runner("ada", "acme::ceo", true, 0));
    let (dispatch, recorder) = dispatch(registry);

    dispatch
        .cancel(&CompanyId::new("acme"), "acme::ceo")
        .await
        .expect("a cancel with nothing open must not fail");
    assert!(recorder.cancelled.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_cancel_reaches_the_runner_holding_the_session() {
    let registry = Arc::new(RunnerRegistry::new());
    registry.admit(runner("ada", "acme::ceo", true, 0));
    let (dispatch, recorder) = dispatch(registry);

    dispatch
        .prompt(&CompanyId::new("acme"), "acme::ceo", "x", None)
        .await
        .unwrap();
    dispatch
        .cancel(&CompanyId::new("acme"), "acme::ceo")
        .await
        .unwrap();

    assert_eq!(
        recorder.cancelled.lock().unwrap()[0],
        ("ada".to_string(), "sess-1".to_string())
    );
}

#[test]
fn detaching_a_runner_forgets_its_sessions() {
    // A reconnecting runner must not be handed ids from its previous life:
    // its new process has never heard of them, and every turn would fail
    // looking like a protocol bug rather than a stale mapping.
    let map = SessionMap::new();
    map.insert("ada", "acme::ceo", "sess-1".to_string());
    map.insert("bob", "globex::cto", "sess-1".to_string());

    map.forget_runner("ada");
    assert!(map.get("ada", "acme::ceo").is_none());
    assert_eq!(map.get("bob", "globex::cto").as_deref(), Some("sess-1"));
}
