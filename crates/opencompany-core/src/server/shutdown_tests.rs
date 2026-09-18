/// The three shutdown budgets must still fit inside a pod's default grace.
///
/// Kubernetes' default `terminationGracePeriodSeconds` is 30, and every one
/// of these constants is sized against it — but the arithmetic lived only in
/// prose, so issue #1739's flush was added on the end and took the worst
/// case to 32s without anything complaining. A `SIGKILL` mid-drain is a
/// half-finished turn, which is worse than anything the extra window buys.
///
/// This fails on the next constant raised in isolation. Raise the pod's
/// grace period deliberately, together, if the budget really has to grow.
#[test]
fn the_shutdown_budgets_fit_inside_a_pods_default_grace() {
    // Every drain an operator can configure, not just the default: a flat
    // flush on top of a configurable drain is exactly how 28s became 32s.
    for secs in 0..=40u64 {
        let drain = Duration::from_secs(secs);
        let total = drain + super::CONNECTION_GRACE + super::flush_budget(drain);
        if drain + super::CONNECTION_GRACE > super::POD_DEFAULT_GRACE {
            // Already past the budget on its own — the flush must not make
            // it worse, and cannot make it better.
            assert_eq!(
                super::flush_budget(drain),
                Duration::ZERO,
                "a drain of {secs}s already fills the budget; the flush must take nothing"
            );
            continue;
        }
        assert!(
            total <= super::POD_DEFAULT_GRACE,
            "drain {secs}s + connections {}s + flush {}s = {}s, past the {}s default",
            super::CONNECTION_GRACE.as_secs(),
            super::flush_budget(drain).as_secs(),
            total.as_secs(),
            super::POD_DEFAULT_GRACE.as_secs(),
        );
    }
    // And the default drain still gets the full ceiling — a budget derived
    // down to nothing would be a silent retirement of the flush.
    assert_eq!(
        super::flush_budget(super::DEFAULT_GRACE),
        super::FLUSH_BUDGET,
        "the default drain must still afford the whole flush budget"
    );
}
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::{DEFAULT_GRACE, drain, parse_grace};
use crate::company::CompanyManifest;
use crate::ports::types::{
    Actor, ActorKind, CompanyEvent, CompanyId, CompressedTrace, CycleRequest, CycleResult,
    TokenUsage,
};
use crate::ports::{Brain, CycleHost};
use crate::runtime::RuntimeBuilder;
use crate::{AppConfig, AppState, Result};

fn home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-shutdown-")
        .tempdir()
        .expect("tempdir")
}

fn manifest(name: &str) -> CompanyManifest {
    toml::from_str(&format!(
        "[company]\nname = \"{name}\"\n[policy]\nmode = \"full\"\n"
    ))
    .expect("parse manifest")
}

fn operator_message(text: &str) -> CompanyEvent {
    CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        text: text.into(),
        by: Some(Actor {
            kind: ActorKind::Operator,
            id: "owner".into(),
        }),
        chat: None,
        parent: None,
        deliverable: None,
        attachments: Vec::new(),
    }
}

/// A brain that parks inside its cycle until released, so a test can deliver
/// a shutdown while a turn is provably in flight — the situation the whole
/// module exists for.
struct BlockingBrain {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    finished: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl Brain for BlockingBrain {
    async fn run_cycle(&self, req: CycleRequest, _host: &dyn CycleHost) -> Result<CycleResult> {
        self.entered.notify_waiters();
        self.release.notified().await;
        self.finished
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(CycleResult {
            channel_responses: Vec::new(),
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "blocking")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

/// A company whose next turn parks until `release` is notified.
struct StalledCompany {
    state: AppState,
    id: CompanyId,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    finished: Arc<std::sync::atomic::AtomicBool>,
}

async fn stalled_company(home: &std::path::Path) -> StalledCompany {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let manifest = manifest("Acme");
    let runtime = RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(CompanyId::new("acme"))
        .with_brain(Arc::new(BlockingBrain {
            entered: entered.clone(),
            release: release.clone(),
            finished: finished.clone(),
        }))
        .build()
        .await
        .expect("build a runtime");
    let id = CompanyId::new("acme");
    let state = AppState::new(AppConfig::default()).with_home(home.to_path_buf());
    state.registry().insert(id.clone(), Arc::new(runtime));
    StalledCompany {
        state,
        id,
        entered,
        release,
        finished,
    }
}

/// Starts a turn the way production does — on a detached task holding its own
/// `Arc<CompanyRuntime>`, with nothing awaiting it — and returns once the
/// brain is provably inside the cycle.
async fn start_detached_turn(c: &StalledCompany) -> tokio::task::JoinHandle<()> {
    let runtime = c
        .state
        .registry()
        .get(&c.id)
        .expect("the company is registered");
    let entered = c.entered.notified();
    tokio::pin!(entered);
    // Arm the waiter before the spawn so the notification cannot be missed.
    let handle = tokio::spawn(async move {
        let _ = runtime.run_cycle(vec![operator_message("do it")]).await;
    });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("the turn started");
    handle
}

#[test]
fn an_unset_grace_uses_the_default() {
    assert_eq!(parse_grace(None), DEFAULT_GRACE);
}

#[test]
fn a_grace_of_zero_is_honoured_rather_than_treated_as_unset() {
    // The escape hatch back to pre-#986 behaviour. Folding `0` into the
    // default would make it impossible to ask for no wait at all.
    assert_eq!(parse_grace(Some("0")), Duration::ZERO);
    assert_eq!(parse_grace(Some(" 90 ")), Duration::from_secs(90));
}

#[test]
fn a_malformed_grace_falls_back_instead_of_failing() {
    // Read while the process is already on its way out: refusing to shut
    // down over a typo would be worse than the wrong bound.
    assert_eq!(parse_grace(Some("thirty")), DEFAULT_GRACE);
    assert_eq!(parse_grace(Some("")), DEFAULT_GRACE);
    assert_eq!(parse_grace(Some("-1")), DEFAULT_GRACE);
}

#[tokio::test]
async fn a_host_with_no_companies_drains_immediately() {
    let dir = home();
    let state = AppState::new(AppConfig::default()).with_home(dir.path().to_path_buf());
    assert!(drain(&state, Duration::from_secs(30)).await);
}

#[tokio::test]
async fn a_drain_stops_every_company_accepting_new_work() {
    let dir = home();
    let state = AppState::new(AppConfig::default()).with_home(dir.path().to_path_buf());
    for name in ["Acme", "Globex"] {
        let id = CompanyId::new(name.to_lowercase());
        let runtime = RuntimeBuilder::new(dir.path().join(name), manifest(name))
            .with_id(id.clone())
            .build()
            .await
            .expect("build a runtime");
        state.registry().insert(id, Arc::new(runtime));
    }

    assert!(drain(&state, Duration::from_secs(30)).await);
    for id in state.registry().list() {
        assert!(
            state.registry().get(&id).expect("registered").is_quiesced(),
            "`{id}` was left accepting cycles after the drain"
        );
    }
}

/// **The keystone.** A turn that is not attached to any request must still
/// be waited on.
///
/// This is the whole of issue #986's root cause: issue #383 deliberately
/// moved the agent turn off the request future so a client hanging up could
/// not cancel it, which means at `SIGTERM` the connection set can be empty
/// while several turns are running. A connection drain — all
/// `with_graceful_shutdown` gives on its own — would look at that empty set,
/// report "nothing in flight" and exit straight through the live turn.
///
/// So the assertion is ordering, not elapsed time: the drain must still be
/// pending while the detached turn is parked, and must only complete after
/// the turn does.
#[tokio::test]
async fn a_drain_waits_for_a_turn_that_no_request_is_holding() {
    let dir = home();
    let c = stalled_company(dir.path()).await;
    let turn = start_detached_turn(&c).await;

    let mut draining = Box::pin(drain(&c.state, Duration::from_secs(30)));
    tokio::select! {
        _ = &mut draining => panic!(
            "the drain returned through a live turn: shutdown would have killed it"
        ),
        () = tokio::time::sleep(Duration::from_millis(200)) => {}
    }
    assert!(
        !c.finished.load(std::sync::atomic::Ordering::SeqCst),
        "the turn is still parked, so the drain had nothing to have waited for"
    );

    c.release.notify_waiters();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), draining)
            .await
            .expect("the drain returned once the turn finished"),
        "a turn that finished inside the bound must report a complete drain"
    );
    assert!(
        c.finished.load(std::sync::atomic::Ordering::SeqCst),
        "the drain returned before the turn completed"
    );
    turn.await.expect("the turn task did not panic");
}

/// **The provisioning race.** A company registered *while the drain is
/// running* must not be able to start a turn.
///
/// The host keeps serving through the drain on purpose, so
/// `POST /api/v1/companies` stays reachable for the whole window — up to 25
/// seconds. A company registered after the drain took its snapshot is not in
/// that snapshot, so nothing waits for it; if it could still accept a cycle
/// it would start a turn seconds before the process exits and lose it.
///
/// Closed at the registry rather than in the provisioning handler: boot, the
/// provision route and a rebuild swap all register through `insert`, and a
/// re-scan after the drain would only move the race to whatever lands after
/// the last scan.
#[tokio::test]
async fn a_company_registered_during_the_drain_cannot_start_a_turn() {
    let dir = home();
    let c = stalled_company(dir.path()).await;
    let turn = start_detached_turn(&c).await;

    // The drain is now pending on the parked turn — the window a provision
    // request would land in.
    let mut draining = Box::pin(drain(&c.state, Duration::from_secs(30)));
    tokio::select! {
        _ = &mut draining => panic!("the drain returned through a live turn"),
        () = tokio::time::sleep(Duration::from_millis(100)) => {}
    }

    let late = CompanyId::new("globex");
    let runtime = RuntimeBuilder::new(dir.path().join("globex"), manifest("Globex"))
        .with_id(late.clone())
        .build()
        .await
        .expect("build a runtime");
    c.state.registry().insert(late.clone(), Arc::new(runtime));

    let registered = c.state.registry().get(&late).expect("registered");
    assert!(
        registered.is_quiesced(),
        "a company registered mid-drain can accept cycles nothing will wait for"
    );
    assert!(
        registered
            .run_cycle(vec![operator_message("late work")])
            .await
            .is_err(),
        "the late company must refuse the cycle, not merely be flagged"
    );

    c.release.notify_waiters();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), draining)
            .await
            .expect("the drain finished")
    );
    turn.await.expect("the turn task did not panic");
}

/// The flag is one-way and scoped to shutdown: an ordinary registration is
/// untouched, or every company would boot refusing work.
#[tokio::test]
async fn registering_a_company_before_shutdown_leaves_it_accepting_work() {
    let dir = home();
    let state = AppState::new(AppConfig::default()).with_home(dir.path().to_path_buf());
    assert!(!state.registry().is_shutting_down());
    let id = CompanyId::new("acme");
    let runtime = RuntimeBuilder::new(dir.path().to_path_buf(), manifest("Acme"))
        .with_id(id.clone())
        .build()
        .await
        .expect("build a runtime");
    state.registry().insert(id.clone(), Arc::new(runtime));
    assert!(!state.registry().get(&id).expect("registered").is_quiesced());
}

/// The same proof one level up, over the real server.
///
/// Lives here rather than beside `serve_on_until` because what it asserts is
/// this module's contract — that the wiring in `routes` actually reaches the
/// drain — and the stalled-company fixture it needs is right above.
///
/// Before issue #986 the host was a bare `axum::serve(listener, router)`
/// with no shutdown path at all: `SIGTERM` was unhandled, so this sequence
/// had no analogue — the process simply stopped existing, mid-turn.
#[tokio::test]
async fn the_host_does_not_exit_until_the_in_flight_turn_has_settled() {
    let dir = home();
    let c = stalled_company(dir.path()).await;
    let turn = start_detached_turn(&c).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let (signal, wait_for_signal) = tokio::sync::oneshot::channel::<()>();
    let mut host = tokio::spawn(crate::server::serve_on_until(
        listener,
        c.state.clone(),
        async move {
            let _ = wait_for_signal.await;
        },
    ));

    signal.send(()).expect("deliver the termination signal");
    tokio::select! {
        _ = &mut host => panic!("the host exited through a live turn"),
        () = tokio::time::sleep(Duration::from_millis(300)) => {}
    }
    assert!(
        !c.finished.load(std::sync::atomic::Ordering::SeqCst),
        "the turn is still parked, so the host had nothing to have waited for"
    );

    c.release.notify_waiters();
    tokio::time::timeout(Duration::from_secs(10), &mut host)
        .await
        .expect("the host exited once the turn settled")
        .expect("the host task did not panic")
        .expect("a graceful shutdown is not an error");
    assert!(
        c.finished.load(std::sync::atomic::Ordering::SeqCst),
        "the host exited before the turn completed"
    );
    turn.await.expect("the turn task did not panic");
}

/// The ceiling. An open connection must not hold the pod past its grace
/// period, because the reward for that is a `SIGKILL` — the exact abrupt
/// stop this module exists to remove, arriving a few seconds later.
///
/// The connection here is a real one: a chat `POST` runs its cycle *inside*
/// the request future, so with the brain parked the request is genuinely
/// in flight and hyper is genuinely waiting on it. The console's event
/// stream is the case that actually motivates this — it never ends on its
/// own — but a parked request reproduces the same hold with none of the
/// streaming scaffolding.
#[tokio::test]
async fn an_open_connection_does_not_hold_the_host_past_the_bound() {
    use tokio::io::AsyncWriteExt;

    let dir = home();
    let c = stalled_company(dir.path()).await;
    crate::server::test_support::seed_fixed_admin(&c.state, "acme").await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let addr = listener.local_addr().expect("a bound address");
    let (signal, wait_for_signal) = tokio::sync::oneshot::channel::<()>();
    let host = tokio::spawn(crate::server::routes::serve_on_until_with_grace(
        listener,
        c.state.clone(),
        async move {
            let _ = wait_for_signal.await;
        },
        Duration::from_millis(200),
    ));

    // A chat POST whose cycle parks in the brain: the request future is
    // still pending, so the connection stays open with nothing on it to
    // finish.
    let body = serde_json::json!({ "text": "do it" }).to_string();
    let request = format!(
        "POST /api/v1/companies/acme/chat HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Cookie: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        crate::server::test_support::fixed_cookie("acme"),
        body.len(),
    );
    let mut socket = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect to the host");
    socket
        .write_all(request.as_bytes())
        .await
        .expect("write the request");
    socket.flush().await.expect("flush the request");
    tokio::time::timeout(Duration::from_secs(5), c.entered.notified())
        .await
        .expect("the chat turn started");

    // The turn is never released, so nothing here finishes on its own.
    signal.send(()).expect("deliver the termination signal");
    tokio::time::timeout(super::CONNECTION_GRACE + Duration::from_secs(5), host)
        .await
        .expect("an open connection held the host past its own bound")
        .expect("the host task did not panic")
        .expect("giving up on a connection is not an error");

    c.release.notify_waiters();
    drop(socket);
}

/// An **idle** host must not sit out the whole drain bound just because a
/// connection is open.
///
/// This is the case the ceiling's clock placement decides, and the one the
/// other ceiling test cannot see: with nothing in flight the drain returns
/// at once, so timing the connection window from the *signal* would leave
/// the host waiting out all of `grace` for a stream that is never going to
/// end, while timing it from the drain lets it go in two seconds. Staging
/// tenants are refreshed several times an afternoon and are idle most of the
/// time, so this is the common path, not the exotic one.
///
/// The connection is a real event stream — the thing that actually never
/// ends — and, unlike a parked chat `POST`, it holds no `serial` lock, so
/// the host is genuinely idle while it is open.
#[tokio::test]
async fn an_idle_host_does_not_wait_out_the_drain_bound_for_an_open_stream() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let dir = home();
    let c = stalled_company(dir.path()).await;
    crate::server::test_support::seed_fixed_admin(&c.state, "acme").await;
    // Deliberately no turn: `drain` has nothing to wait for and returns
    // immediately, which is what puts the two clock placements far apart.

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let addr = listener.local_addr().expect("a bound address");
    let (signal, wait_for_signal) = tokio::sync::oneshot::channel::<()>();
    // A bound far larger than the connection grace, so the two placements
    // are trivially distinguishable: 30s versus about 2s.
    let grace = Duration::from_secs(30);
    let host = tokio::spawn(crate::server::routes::serve_on_until_with_grace(
        listener,
        c.state.clone(),
        async move {
            let _ = wait_for_signal.await;
        },
        grace,
    ));

    let request = format!(
        "GET /api/v1/company/events HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Cookie: {}\r\n\
         Accept: text/event-stream\r\n\r\n",
        crate::server::test_support::fixed_cookie("acme"),
    );
    let mut socket = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect to the host");
    socket
        .write_all(request.as_bytes())
        .await
        .expect("write the request");
    socket.flush().await.expect("flush the request");

    // Read the response head so the stream is provably established — an
    // unestablished connection would be idle, which hyper closes at once and
    // which would prove nothing.
    let mut head = [0u8; 32];
    let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut head))
        .await
        .expect("the event stream answered")
        .expect("read the response head");
    assert!(read > 0, "the event stream sent nothing");

    signal.send(()).expect("deliver the termination signal");
    tokio::time::timeout(super::CONNECTION_GRACE + Duration::from_secs(5), host)
        .await
        .expect("an idle host waited out the whole drain bound for an open stream")
        .expect("the host task did not panic")
        .expect("giving up on a connection is not an error");

    drop(socket);
}

/// The honest half of the bound: a turn longer than the grace period is
/// still cut off, and the drain says so rather than holding the process past
/// the pod's grace period into a `SIGKILL`.
#[tokio::test]
async fn a_turn_that_outlasts_the_bound_does_not_hold_the_process() {
    let dir = home();
    let c = stalled_company(dir.path()).await;
    let turn = start_detached_turn(&c).await;

    assert!(
        !drain(&c.state, Duration::from_millis(200)).await,
        "an expired bound must report an incomplete drain, so the boot reaper's \
         failed-run record stays the backstop"
    );
    // And the company stopped accepting new cycles regardless, so nothing
    // starts a fresh turn in the window before the process exits.
    assert!(
        c.state
            .registry()
            .get(&c.id)
            .expect("registered")
            .is_quiesced()
    );

    c.release.notify_waiters();
    let _ = tokio::time::timeout(Duration::from_secs(5), turn).await;
}
