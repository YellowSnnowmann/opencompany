pub(super) use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) use async_trait::async_trait;
pub(super) use serde_json::Value;
pub(super) use tokio::sync::Semaphore;

pub(super) use super::*;
pub(super) use crate::company::CompanyManifest;
pub(super) use crate::ports::types::{CompanyEvent, CompanyRecord, EventSeq, OverlayWorkflow};
pub(super) use crate::ports::{DeliveryReason, WorkflowRun, WorkflowRunner};
pub(super) use crate::runtime::{FakeClock, RuntimeBuilder};

// --- tracing capture -----------------------------------------------------
//
// The scheduler's only channel for a failed scheduled delivery IS a log
// line (see the module docs), so proving it is "observable rather than
// silent" means reading what it actually emitted. The run happens on a
// spawned task, and a thread-local subscriber does not reach one, so the
// capture is installed process-wide exactly once. Every test in this binary
// therefore shares one buffer — assertions must key on something unique to
// their own company id, never on "the buffer contains one line".

/// The shared capture buffer. `None` until `captured_logs()` installs it.
pub(super) static CAPTURE: std::sync::OnceLock<Arc<Mutex<Vec<u8>>>> = std::sync::OnceLock::new();

/// A writer that accumulates one event and appends it to the shared buffer
/// in a single locked write on drop, so events from concurrent tests
/// interleave whole-line rather than mid-line.
pub(super) struct CaptureWriter {
    buf: Vec<u8>,
    sink: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for CaptureWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            self.sink.lock().unwrap().extend_from_slice(&self.buf);
        }
    }
}

#[derive(Clone)]
pub(super) struct MakeCapture(Arc<Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for MakeCapture {
    type Writer = CaptureWriter;
    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter {
            buf: Vec::new(),
            sink: self.0.clone(),
        }
    }
}

/// Installs the process-wide capture on first call and returns the buffer.
pub(super) fn captured_logs() -> Arc<Mutex<Vec<u8>>> {
    CAPTURE
        .get_or_init(|| {
            let sink = Arc::new(Mutex::new(Vec::new()));
            let subscriber = tracing_subscriber::fmt()
                .with_writer(MakeCapture(sink.clone()))
                .with_max_level(tracing::Level::INFO)
                .with_ansi(false)
                .finish();
            // Only this module installs one, so it cannot lose the race to
            // another test; if some future test adds a subscriber, this
            // returns Err and the capture tests would fail loudly rather
            // than silently asserting on an empty buffer.
            tracing::subscriber::set_global_default(subscriber)
                .expect("no other global tracing subscriber in the test binary");
            sink
        })
        .clone()
}

/// The captured log text so far.
pub(super) fn captured_text(sink: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&sink.lock().unwrap()).to_string()
}

pub(super) fn tmp_home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-wfsched-")
        .tempdir()
        .expect("tempdir")
}

pub(super) fn manifest() -> CompanyManifest {
    toml::from_str(
        r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [policy]
        mode = "full"
        "#,
    )
    .expect("parse manifest")
}

/// Unix millis for a UTC civil minute, via the cron module's own conversion.
pub(super) fn millis_at(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
    let mut probe = 0u64;
    loop {
        let c = CivilTime::from_unix_millis(probe);
        if (c.year, c.month, c.day) == (year, month, day) {
            break;
        }
        probe += 86_400_000;
        if probe > 4_102_444_800_000 {
            panic!("date out of probe range");
        }
    }
    probe + (hour as u64) * 3_600_000 + (minute as u64) * MINUTE_MS
}

/// What one recorded scheduled run carries.
#[derive(Clone, Debug)]
pub(super) struct Recorded {
    pub(super) company: String,
    pub(super) workflow: String,
    pub(super) global: bool,
    pub(super) input: Value,
}

/// A [`WorkflowRunner`] that records every run instead of executing one, and
/// can be held open so a test controls exactly when a run completes.
pub(super) struct RecordingRunner {
    started: Arc<Mutex<Vec<Recorded>>>,
    completed: Arc<AtomicUsize>,
    /// When set, a run parks until the test adds a permit.
    gate: Option<Arc<Semaphore>>,
    /// The delivery rows every run reports back, so a test can drive the
    /// scheduler's undelivered-report path.
    deliveries: Vec<DeliveryReport>,
}

impl RecordingRunner {
    pub(super) fn new() -> (Arc<Self>, Arc<Mutex<Vec<Recorded>>>, Arc<AtomicUsize>) {
        let started = Arc::new(Mutex::new(Vec::new()));
        let completed = Arc::new(AtomicUsize::new(0));
        let runner = Arc::new(Self {
            started: started.clone(),
            completed: completed.clone(),
            gate: None,
            deliveries: Vec::new(),
        });
        (runner, started, completed)
    }

    pub(super) fn gated(
        gate: Arc<Semaphore>,
    ) -> (Arc<Self>, Arc<Mutex<Vec<Recorded>>>, Arc<AtomicUsize>) {
        let started = Arc::new(Mutex::new(Vec::new()));
        let completed = Arc::new(AtomicUsize::new(0));
        let runner = Arc::new(Self {
            started: started.clone(),
            completed: completed.clone(),
            gate: Some(gate),
            deliveries: Vec::new(),
        });
        (runner, started, completed)
    }

    /// A runner whose every run reports `deliveries` back.
    pub(super) fn with_deliveries(
        deliveries: Vec<DeliveryReport>,
    ) -> (Arc<Self>, Arc<AtomicUsize>) {
        let completed = Arc::new(AtomicUsize::new(0));
        let runner = Arc::new(Self {
            started: Arc::new(Mutex::new(Vec::new())),
            completed: completed.clone(),
            gate: None,
            deliveries,
        });
        (runner, completed)
    }
}

#[async_trait]
impl WorkflowRunner for RecordingRunner {
    async fn run(
        &self,
        company: &CompanyId,
        workflow: &WorkflowFile,
        input: Value,
        ctx: &crate::ports::WorkflowRunContext,
    ) -> crate::Result<WorkflowRun> {
        self.started.lock().unwrap().push(Recorded {
            company: company.as_ref().to_string(),
            workflow: workflow.id.clone(),
            global: workflow.global,
            input,
        });
        if let Some(gate) = &self.gate {
            // Issue #383: a parked run races its gate against the stop
            // signal, mirroring what the real runner does with the engine
            // future. Without this the double would ignore a cancel and the
            // scheduler test below could not observe one.
            tokio::select! {
                permit = gate.acquire() => permit.expect("gate open").forget(),
                () = ctx.cancel.cancelled() => {
                    return Ok(WorkflowRun {
                        output: Value::Null,
                        pending_approvals: Vec::new(),
                        deliveries: Vec::new(),
                        cancelled: true,
                        nodes: Vec::new(),
                        notices: Vec::new(),
                        board: Vec::new(),
                        blocked_nodes: Vec::new(),
                        approvals: Vec::new(),
                    });
                }
            }
        }
        self.completed.fetch_add(1, Ordering::SeqCst);
        Ok(WorkflowRun {
            output: Value::Null,
            pending_approvals: Vec::new(),
            deliveries: self.deliveries.clone(),
            cancelled: false,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        })
    }
}

/// A [`WorkflowRunner`] whose every run fails, so a test can drive the
/// scheduler's `Err` arm — the outcome that used to leave nothing durable
/// behind at all (issue #228).
pub(super) struct FailingRunner {
    message: String,
    attempts: Arc<AtomicUsize>,
}

impl FailingRunner {
    pub(super) fn new(message: &str) -> (Arc<Self>, Arc<AtomicUsize>) {
        let attempts = Arc::new(AtomicUsize::new(0));
        let runner = Arc::new(Self {
            message: message.to_string(),
            attempts: attempts.clone(),
        });
        (runner, attempts)
    }
}

#[async_trait]
impl WorkflowRunner for FailingRunner {
    async fn run(
        &self,
        _company: &CompanyId,
        _workflow: &WorkflowFile,
        _input: Value,
        _ctx: &crate::ports::WorkflowRunContext,
    ) -> crate::Result<WorkflowRun> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(crate::error::OpenCompanyError::Config(self.message.clone()))
    }
}

/// A minimal valid graph body, optionally scheduled on `cron`.
pub(super) fn body(id: &str, cron: Option<&str>) -> String {
    let schedule = cron
        .map(|c| format!("schedule = \"{c}\"\n"))
        .unwrap_or_default();
    format!(
        r#"
id = "{id}"
name = "{id}"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
{schedule}
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "done"
"#
    )
}

pub(super) fn overlay(id: &str, cron: Option<&str>) -> OverlayWorkflow {
    OverlayWorkflow {
        id: id.to_string(),
        toml: body(id, cron),
    }
}

pub(super) fn global(id: &str, cron: &str) -> WorkflowFile {
    let mut workflow = crate::company::parse_workflow(&body(id, Some(cron))).unwrap();
    workflow.global = true;
    workflow
}

/// Builds a registered company whose only workflows are record overlays —
/// the console-created shape, with no source directory at all.
pub(super) async fn company_with_overlays(
    home: &std::path::Path,
    id: &str,
    overlays: Vec<OverlayWorkflow>,
    runner: Option<Arc<dyn WorkflowRunner>>,
    lifecycle: &str,
) -> CompanyRegistry {
    company_with_overlays_capped(home, id, overlays, runner, lifecycle, None).await
}

/// [`company_with_overlays`] with an optional per-company in-flight run cap
/// (issue #401 / #661). `cap = Some(n)` overrides the runtime's default
/// [`RunSupervisor`] with one that admits at most `n` concurrent runs, so a
/// test can drive the scheduler's at-cap path by holding `n` `begin` guards.
pub(super) async fn company_with_overlays_capped(
    home: &std::path::Path,
    id: &str,
    overlays: Vec<OverlayWorkflow>,
    runner: Option<Arc<dyn WorkflowRunner>>,
    lifecycle: &str,
    cap: Option<usize>,
) -> CompanyRegistry {
    let company = CompanyId::new(id);
    let mut runtime = RuntimeBuilder::new(home.to_path_buf(), manifest())
        .with_id(company.clone())
        .build()
        .await
        .expect("builds");
    assert!(
        runtime.source_dir().is_none(),
        "the overlay-only case must have no source dir"
    );
    if let Some(cap) = cap {
        runtime.set_run_supervisor(crate::runtime::RunSupervisor::with_limit(cap));
    }
    if let Some(runner) = runner {
        runtime.set_workflow_runner(runner);
    }

    // Persist the graph bodies (and lifecycle) the scheduler will read.
    let store = runtime.store().clone();
    let mut record: CompanyRecord = store
        .load(&company)
        .await
        .expect("loads")
        .expect("the builder materialized a record");
    record.overlay_workflows = overlays;
    record.lifecycle = lifecycle.to_string();
    store.save(&record).await.expect("saves");

    let registry = CompanyRegistry::new();
    registry.insert(company, Arc::new(runtime));
    registry
}

/// Yields until `predicate` holds, so a test can wait on a spawned run
/// without a wall-clock sleep. Panics rather than hanging if it never does.
pub(super) async fn wait_for(mut predicate: impl FnMut() -> bool) {
    for _ in 0..10_000 {
        if predicate() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("condition never became true");
}

/// Like [`wait_for`], but sleeps a little real time between polls instead of
/// `yield_now`. On a multi-thread runtime a `yield_now` spin can burn all its
/// iterations in microseconds before a task on ANOTHER worker thread has made
/// progress; a short real sleep gives that thread a chance. Used by the
/// multi-thread sibling-admission test. Panics rather than hanging.
pub(super) async fn wait_on_time(mut predicate: impl FnMut() -> bool) {
    for _ in 0..1_000 {
        if predicate() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("condition never became true");
}

/// Waits until no scheduled run still holds its in-flight claim.
///
/// **Any test that ticks a second time and expects another fire must call
/// this first.** `tick` is not a synchronisation point: it spawns the run
/// and returns without awaiting it, so whether the spawned task has been
/// polled by the next tick depends on whether some await inside `tick`
/// (the record load) happens to yield. When it does not, the previous run
/// still holds the claim, the overlap guard correctly rejects the fire, and
/// the tick returns 0 — which is the code working as designed and the test
/// being wrong. That is exactly how
/// `the_dedupe_map_does_not_grow_without_bound` passed locally and failed
/// in CI.
///
/// Waiting on the mock's `started` vec is NOT a substitute: it records the
/// run's start, not its completion, so it proves nothing about the claim.
/// This keys on the claim itself, which is the state the guard reads. Polls
/// with real sleeps rather than `yield_now` for the same reason as
/// [`wait_on_time`]: the claim is released by a task spawned onto another
/// worker thread, and a pure yield spin can exhaust its budget before that
/// thread runs.
pub(super) async fn drain(scheduler: &WorkflowScheduler) {
    wait_on_time(|| !scheduler.is_running_any()).await;
}

// --- report delivery on a scheduled run (issue #170) ---------------------

/// A row whose classification is the plausible one for its status, so a
/// fixture never claims something the two halves would contradict (a `sent`
/// row reasoned as a cold recipient, say). Tests that assert on the
/// classification itself use [`reported`] and name it.
pub(super) fn report(node: &str, status: DeliveryStatus, detail: &str) -> DeliveryReport {
    let reason = match status {
        DeliveryStatus::Sent => DeliveryReason::OwnerEmailed,
        DeliveryStatus::Pending => DeliveryReason::ParkedForApproval,
        DeliveryStatus::Skipped => DeliveryReason::RecipientNotEstablished,
        DeliveryStatus::Denied => DeliveryReason::EmailNotGranted,
        DeliveryStatus::Failed => DeliveryReason::MailTransportRefused,
    };
    reported(node, status, reason, detail)
}

/// `report`, with the classification spelled out — for the tests that care
/// which half of the row the scheduler logged.
pub(super) fn reported(
    node: &str,
    status: DeliveryStatus,
    reason: DeliveryReason,
    detail: &str,
) -> DeliveryReport {
    DeliveryReport {
        node: node.to_string(),
        kind: "email".to_string(),
        target: Some(RECIPIENT.to_string()),
        status,
        detail: detail.to_string(),
        reason,
    }
}

/// The recipient address every delivery fixture in this module addresses.
/// `.invalid` is reserved by RFC 2606 and can never resolve, so a fixture
/// that escapes into a log or a PR body names nobody.
pub(super) const RECIPIENT: &str = "recipient@example.invalid";

// ── Issue #228: the run outcome reaches the journal, not just stdout ─────

/// Every `WorkflowRunFinished` journaled for `company`.
/// Yields until the journal holds `want` finished-run records, then returns
/// them. The append happens after the run completes, so a test that reads
/// once races it.
pub(super) async fn wait_for_outcomes(
    registry: &CompanyRegistry,
    company: &str,
    want: usize,
) -> Vec<CompanyEvent> {
    for _ in 0..10_000 {
        let outcomes = run_outcomes(registry, company).await;
        if outcomes.len() >= want {
            return outcomes;
        }
        tokio::task::yield_now().await;
    }
    panic!("only ever saw fewer than {want} finished-run records");
}

pub(super) async fn run_outcomes(registry: &CompanyRegistry, company: &str) -> Vec<CompanyEvent> {
    let id = CompanyId::new(company);
    let runtime = registry.get(&id).expect("registered");
    runtime
        .events()
        .read_from(&id, EventSeq::new(0), usize::MAX)
        .await
        .expect("read journal")
        .into_iter()
        .map(|s| s.event)
        .filter(|e| matches!(e, CompanyEvent::WorkflowRunFinished { .. }))
        .collect()
}

// ── Issue #259: the tick IS the reconcile ───────────────────────────────
//
// Editing or removing a workflow deliberately ships NO scheduler change.
// The two tests below are why that is safe rather than an omission, and
// they are the pin: if someone ever caches the schedule set across ticks —
// an obvious-looking optimisation, since the record load is per-minute
// per-company — these fail, instead of a deleted workflow quietly firing
// forever in production.
//
// OpenHuman needs `reconcile_schedule_triggers_on_boot` precisely because
// it *does* persist a registration: a schedule-trigger flow binds a row in
// a separate `cron.db`, which can drift from `flows.db` and must be
// re-synced. We persist no registration at all, so there is nothing to
// drift and nothing to reconcile.

/// Replaces a registered company's overlay bodies, standing in for the
/// `PUT`/`DELETE` routes' record write.
pub(super) async fn rewrite_overlays(
    registry: &CompanyRegistry,
    id: &str,
    overlays: Vec<OverlayWorkflow>,
) {
    let company = CompanyId::new(id);
    let runtime = registry.get(&company).expect("registered");
    let store = runtime.store().clone();
    let mut record: CompanyRecord = store.load(&company).await.unwrap().unwrap();
    record.overlay_workflows = overlays;
    store.save(&record).await.unwrap();
}

/// Flips a workflow's armed state on the persisted record, the way
/// `PUT …/workflows/{wid}/enabled` does (issue #276).
pub(super) async fn set_enabled(registry: &CompanyRegistry, id: &str, wid: &str, enabled: bool) {
    let company = CompanyId::new(id);
    let runtime = registry.get(&company).expect("registered");
    let store = runtime.store().clone();
    let mut record: CompanyRecord = store.load(&company).await.unwrap().unwrap();
    record.set_workflow_enabled(wid, enabled);
    store.save(&record).await.unwrap();
}

// --- issue #241: durable claims + restart catch-up ---------------------

/// The anchor minute of a UTC civil minute, the unit the claim store speaks.
pub(super) fn minute_at(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
    millis_at(year, month, day, hour, minute) / MINUTE_MS
}

/// Issue #661 (reviewer): sibling schedules on ONE company all due at the
/// same minute must be admitted against an EXACT in-flight count, never a
/// stale one. Before the fix `begin` ran inside each fire's spawned task, so
/// with no await between one fire's spawn and the next schedule's cap read
/// every sibling saw a count that did not yet include the ones ahead of it: N
/// schedules at a cap of 1 all passed the check, all claimed their minute,
/// and N-1 were then refused at `begin` inside their task — durably burning
/// N-1 minutes. Admitting on the tick thread BEFORE the claim makes the count
/// exact: exactly `cap` fire and claim, and the refused siblings leave their
/// minutes UNCLAIMED (recoverable), nothing burned. Held (gated) runs keep
/// the slots occupied so the property is deterministic on any runtime.
pub(super) async fn assert_siblings_admit_exactly_the_cap_and_burn_no_minute(cap: usize) {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let gate = Arc::new(Semaphore::new(0));
    let (runner, started, _completed) = RecordingRunner::gated(gate.clone());
    // Four workflows, all due at the same minute on ONE company.
    let ids = ["alpha", "bravo", "charlie", "delta"];
    let overlays = ids
        .iter()
        .map(|id| overlay(id, Some("0 9 * * *")))
        .collect();
    let registry =
        company_with_overlays_capped(&home, "acme", overlays, Some(runner), "running", Some(cap))
            .await;
    let company = CompanyId::new("acme");
    let runtime = registry.get(&company).unwrap();

    let clock = Arc::new(FakeClock::new(millis_at(2026, 7, 14, 9, 0)));
    let mut scheduler = WorkflowScheduler::new(registry.clone(), clock);

    // Exactly `cap` of the four are admitted and fire this tick; the parked
    // runs hold their slots, so every later sibling is refused at `begin`.
    assert_eq!(
        scheduler.tick().await,
        cap,
        "exactly the cap fires when {} siblings are due at once (cap={cap})",
        ids.len()
    );
    // The `cap` admitted runs reach the runner and park (holding their
    // slots). Time-based wait: robust on the multi-thread runtime this test
    // also runs on.
    wait_on_time(|| started.lock().unwrap().len() == cap).await;

    // Exactly `cap` minutes are durably claimed — the refused siblings burned
    // nothing, so their anchors are still empty and a later tick can still
    // make them up.
    let mut claimed = 0;
    for id in ids {
        let schedule_id = format!("workflow-{id}");
        if runtime
            .schedule_fires()
            .latest_fire(&company, &schedule_id)
            .await
            .unwrap()
            .is_some()
        {
            claimed += 1;
        }
    }
    assert_eq!(
        claimed,
        cap,
        "exactly {cap} minute(s) claimed; the {} refused siblings burned none",
        ids.len() - cap
    );

    // Release the parked runs and let them settle so the test tears down
    // cleanly. Time-based (robust on the multi-thread runtime); the
    // exact-count assertions above already hold regardless.
    gate.add_permits(cap);
    wait_on_time(|| !scheduler.is_running_any()).await;
}

// --- issue #661 (F2): a transient failure DEFERS the first-sight catch-up,
// it does not forfeit it ----------------------------------------------------

pub(super) use crate::error::OpenCompanyError;
pub(super) use crate::ports::ScheduleFireStore;

/// An in-memory [`ScheduleFireStore`] that can be told to fail its first N
/// `latest_fire` reads and/or its first N `claim_fire` writes, then behaves
/// normally. The double for issue #661 F2: a transient store error on the
/// first-sight catch-up must drop the `caught_up` latch so a later tick
/// re-attempts the make-up, rather than one flaky call forfeiting it for the
/// life of the process. `seed` presets an anchor WITHOUT consuming a fail
/// budget, so a test can arrange a genuine missed fire and still arm the
/// failure it wants to observe.
pub(super) struct FlakyFires {
    claims: Mutex<HashMap<(String, String), HashSet<u64>>>,
    fail_latest: AtomicUsize,
    fail_claim: AtomicUsize,
}

impl FlakyFires {
    pub(super) fn new() -> Self {
        Self {
            claims: Mutex::new(HashMap::new()),
            fail_latest: AtomicUsize::new(0),
            fail_claim: AtomicUsize::new(0),
        }
    }
    /// Arm the next `n` `latest_fire` reads to fail (armed AFTER any seeding).
    pub(super) fn arm_latest_failures(&self, n: usize) {
        self.fail_latest.store(n, Ordering::SeqCst);
    }
    /// Arm the next `n` `claim_fire` writes to fail.
    pub(super) fn arm_claim_failures(&self, n: usize) {
        self.fail_claim.store(n, Ordering::SeqCst);
    }
    /// Preset an anchor directly, bypassing the fail budgets.
    pub(super) fn seed(&self, company: &str, schedule: &str, minute: u64) {
        self.claims
            .lock()
            .unwrap()
            .entry((company.to_string(), schedule.to_string()))
            .or_default()
            .insert(minute);
    }
}

#[async_trait]
impl ScheduleFireStore for FlakyFires {
    async fn claim_fire(&self, c: &CompanyId, s: &str, m: u64) -> crate::Result<bool> {
        if self.fail_claim.load(Ordering::SeqCst) > 0 {
            self.fail_claim.fetch_sub(1, Ordering::SeqCst);
            return Err(OpenCompanyError::Store("flaky claim store".into()));
        }
        Ok(self
            .claims
            .lock()
            .unwrap()
            .entry((c.as_ref().to_string(), s.to_string()))
            .or_default()
            .insert(m))
    }
    async fn latest_fire(&self, c: &CompanyId, s: &str) -> crate::Result<Option<u64>> {
        if self.fail_latest.load(Ordering::SeqCst) > 0 {
            self.fail_latest.fetch_sub(1, Ordering::SeqCst);
            return Err(OpenCompanyError::Store("flaky claim store".into()));
        }
        Ok(self
            .claims
            .lock()
            .unwrap()
            .get(&(c.as_ref().to_string(), s.to_string()))
            .and_then(|set| set.iter().max().copied()))
    }
    async fn prune_fires_before(&self, _c: &CompanyId, _m: u64) -> crate::Result<usize> {
        Ok(0)
    }
    async fn delete_schedule_fires(&self, c: &CompanyId, s: &str) -> crate::Result<usize> {
        Ok(self
            .claims
            .lock()
            .unwrap()
            .remove(&(c.as_ref().to_string(), s.to_string()))
            .map_or(0, |set| set.len()))
    }
}

/// [`company_with_overlays`] with a caller-supplied [`ScheduleFireStore`], so a
/// test can drive the scheduler against a flaky claim store (issue #661 F2).
pub(super) async fn company_with_overlays_and_fires(
    home: &std::path::Path,
    id: &str,
    overlays: Vec<OverlayWorkflow>,
    runner: Option<Arc<dyn WorkflowRunner>>,
    lifecycle: &str,
    fires: Arc<dyn ScheduleFireStore>,
) -> CompanyRegistry {
    let company = CompanyId::new(id);
    let mut runtime = RuntimeBuilder::new(home.to_path_buf(), manifest())
        .with_id(company.clone())
        .with_schedule_fires(fires)
        .build()
        .await
        .expect("builds");
    if let Some(runner) = runner {
        runtime.set_workflow_runner(runner);
    }
    let store = runtime.store().clone();
    let mut record: CompanyRecord = store
        .load(&company)
        .await
        .expect("loads")
        .expect("the builder materialized a record");
    record.overlay_workflows = overlays;
    record.lifecycle = lifecycle.to_string();
    store.save(&record).await.expect("saves");

    let registry = CompanyRegistry::new();
    registry.insert(company, Arc::new(runtime));
    registry
}
