use super::*;
use crate::app::AppConfig;
use crate::company::CompanyManifest;
use crate::ports::types::CompanyEvent;
use crate::runtime::RuntimeBuilder;

/// A minimal event to drive one cycle with.
fn tick() -> CompanyEvent {
    CompanyEvent::ScheduleFired {
        cron: "* * * * *".to_string(),
        prompt: "status".to_string(),
    }
}

fn manifest() -> CompanyManifest {
    toml::from_str("[company]\nname = \"Acme\"\n").expect("valid manifest")
}

fn tmp_home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-rebuild-")
        .tempdir()
        .expect("tempdir")
}

async fn runtime(home: &std::path::Path, id: &CompanyId) -> CompanyRuntime {
    RuntimeBuilder::new(home.to_path_buf(), manifest())
        .with_id(id.clone())
        .build()
        .await
        .expect("build")
}

/// A rebuilder that always builds a fresh runtime over the handover.
struct Working {
    home: PathBuf,
}

#[async_trait]
impl RuntimeRebuilder for Working {
    async fn rebuild(&self, _state: &AppState, request: RebuildRequest) -> Result<CompanyRuntime> {
        RuntimeBuilder::new(self.home.clone(), request.manifest)
            .with_id(request.id)
            .with_handover(request.handover)
            .build()
            .await
    }
}

/// A rebuilder that always fails, so the failure path is exercised.
struct Broken;

#[async_trait]
impl RuntimeRebuilder for Broken {
    async fn rebuild(&self, _state: &AppState, _request: RebuildRequest) -> Result<CompanyRuntime> {
        Err(OpenCompanyError::Config("no inference backend".to_string()))
    }
}

async fn state_with(home: &std::path::Path, id: &CompanyId) -> AppState {
    let state = AppState::new(AppConfig::default());
    state
        .registry()
        .insert(id.clone(), Arc::new(runtime(home, id).await));
    state.set_boot_inputs(id.clone(), BootInputs::default());
    state
}

#[tokio::test]
async fn a_rebuild_swaps_the_registered_runtime_and_hands_state_over() {
    let home_dir = tmp_home();
    let home = home_dir.path();
    let id = CompanyId::new("acme");
    let state = state_with(home, &id)
        .await
        .with_rebuilder(Arc::new(Working {
            home: home.to_path_buf(),
        }));
    let before = state.registry().get(&id).expect("registered");

    let after = rebuild_company(&state, &id).await.expect("rebuilds");

    // A different runtime is registered...
    assert!(!Arc::ptr_eq(&before, &after));
    assert!(Arc::ptr_eq(
        &state.registry().get(&id).expect("registered"),
        &after
    ));
    // ...and it is accepting work, not stuck in the quiesced window.
    assert!(!after.is_quiesced());
    // The pieces a second instance must never duplicate came across intact.
    assert!(Arc::ptr_eq(before.journal(), after.journal()));
    // The gate itself, not a re-rehydrated copy: an approval waiting on a
    // person keeps its id, its parked effect and its TTL across the swap.
    assert!(Arc::ptr_eq(&before.approval_gate, &after.approval_gate));
    assert!(Arc::ptr_eq(before.events(), after.events()));
    assert!(Arc::ptr_eq(before.store(), after.store()));
    // Same serial lock, so the cycle invariant spans the swap rather than
    // lapsing at it. Read off the fields, like the gate above: every
    // production holder (`CycleRunner::run`, the desk routes, `quiesce`'s
    // drain) takes them the same way, so an accessor here would be a public
    // surface that exists only for this assertion.
    assert!(Arc::ptr_eq(&before.serial, &after.serial));
    assert!(Arc::ptr_eq(&before.per_agent, &after.per_agent));
    assert!(Arc::ptr_eq(&before.task_writes, &after.task_writes));
}

#[tokio::test]
async fn the_successor_runs_cycles_the_outgoing_runtime_now_refuses() {
    let home_dir = tmp_home();
    let home = home_dir.path();
    let id = CompanyId::new("acme");
    let state = state_with(home, &id)
        .await
        .with_rebuilder(Arc::new(Working {
            home: home.to_path_buf(),
        }));
    let outgoing = state.registry().get(&id).expect("registered");

    let successor = rebuild_company(&state, &id).await.expect("rebuilds");

    // The outgoing runtime stays quiesced forever: anything still holding an
    // Arc to it must not keep driving a company that has been replaced.
    let refused = outgoing
        .run_cycle(vec![tick()])
        .await
        .expect_err("a replaced runtime accepts no cycles");
    assert!(
        matches!(refused, OpenCompanyError::Quiescing(_)),
        "{refused}"
    );
    assert_eq!(refused.code(), "quiescing");

    successor
        .run_cycle(vec![tick()])
        .await
        .expect("the successor is live");
}

#[tokio::test]
async fn a_failed_rebuild_resumes_the_company_it_quiesced() {
    // The worst outcome is not a stale brain: it is a company that refuses
    // every cycle because a rebuild died between quiesce and swap.
    let home_dir = tmp_home();
    let home = home_dir.path();
    let id = CompanyId::new("acme");
    let state = state_with(home, &id).await.with_rebuilder(Arc::new(Broken));
    let outgoing = state.registry().get(&id).expect("registered");

    let err = rebuild_company(&state, &id)
        .await
        .expect_err("the rebuilder fails");
    assert!(matches!(err, OpenCompanyError::Config(_)), "{err}");

    assert!(Arc::ptr_eq(
        &state.registry().get(&id).expect("still registered"),
        &outgoing
    ));
    assert!(
        !outgoing.is_quiesced(),
        "a failed rebuild must not park the company"
    );
    outgoing
        .run_cycle(vec![tick()])
        .await
        .expect("the surviving runtime still runs cycles");
}

#[tokio::test]
async fn a_failed_rebuild_during_shutdown_keeps_the_company_quiesced() {
    // The shutdown drain (issue #986) gates every registered company against
    // new cycles. A rebuild that fails *after* the drain began must not
    // `resume()` the outgoing runtime and re-open admission on a company the
    // process is about to leave — that would admit a turn nothing waits for
    // in the seconds before exit.
    let home_dir = tmp_home();
    let home = home_dir.path();
    let id = CompanyId::new("acme");
    let state = state_with(home, &id).await.with_rebuilder(Arc::new(Broken));
    state.registry().begin_shutdown();
    let outgoing = state.registry().get(&id).expect("registered");

    let err = rebuild_company(&state, &id)
        .await
        .expect_err("the rebuilder fails");
    assert!(matches!(err, OpenCompanyError::Config(_)), "{err}");

    assert!(
        outgoing.is_quiesced(),
        "a failed rebuild during shutdown must keep the company gated"
    );
    assert!(
        outgoing.run_cycle(vec![tick()]).await.is_err(),
        "a company gated by shutdown must refuse a cycle"
    );
}

#[tokio::test]
async fn a_host_with_no_rebuilder_says_so_instead_of_quiescing() {
    let home_dir = tmp_home();
    let home = home_dir.path();
    let id = CompanyId::new("acme");
    let state = state_with(home, &id).await;

    let err = rebuild_company(&state, &id)
        .await
        .expect_err("no rebuilder is wired");
    assert!(matches!(err, OpenCompanyError::Config(_)), "{err}");
    // Crucially, the check happens before the quiesce.
    assert!(!state.registry().get(&id).expect("registered").is_quiesced());
}

/// `rebuild_company` must serialize the rebuilder's load-through-save of
/// the company record against `company_write_lock` (PR #1875 review
/// finding): `RuntimeBuilder::build` reads the persisted record near the
/// top and does not save its successor until near the bottom, and
/// without this lock a console write racing in between — a name-confirm
/// PATCH, a desk reorder — could land its save inside that window only
/// to have the rebuild's own full-record save silently revert it. Proven
/// the same way `set_lifecycle_serializes_against_the_company_write_lock`
/// (`company/runtime.rs`) proves it for that method: hold the lock
/// externally, drive the real call, and demand it cannot finish while
/// the lock is held.
#[tokio::test]
async fn rebuild_company_serializes_against_the_company_write_lock() {
    let home_dir = tmp_home();
    let home = home_dir.path();
    let id = CompanyId::new("acme");
    let state = state_with(home, &id)
        .await
        .with_rebuilder(Arc::new(Working {
            home: home.to_path_buf(),
        }));

    let lock = crate::ports::store::company_write_lock(&id);
    let guard = lock.lock().await;

    let state_for_task = state.clone();
    let id_for_task = id.clone();
    let mut task =
        tokio::spawn(async move { rebuild_company(&state_for_task, &id_for_task).await });

    // The rebuild must be blocked behind the held lock — give it every
    // chance to (wrongly) race ahead before declaring it stuck.
    let raced_ahead = tokio::time::timeout(std::time::Duration::from_millis(200), &mut task)
        .await
        .is_ok();
    assert!(
        !raced_ahead,
        "rebuild_company completed while company_write_lock was held \
         elsewhere — it is not serializing its load-through-save of the \
         company record against a concurrent writer (e.g. a racing \
         name-confirm PATCH), which can silently revert that writer's \
         save"
    );

    drop(guard);
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("rebuild_company never resumed after the lock was released")
        .expect("task panicked")
        .expect("rebuild_company failed");
}

/// PR #1875 review finding (second pass): a manifest snapshot taken
/// *before* `quiesce()` — rather than under the `company_write_lock` this
/// function already holds by the time it calls the rebuilder — leaves a
/// window `quiesce()`'s own wait can outlast: `quiesce()` blocks on
/// `serial`, not on `company_write_lock`, so a writer like `PUT …/logo`
/// (load-modify-save under `company_write_lock`) can complete entirely
/// while `quiesce()` is still draining an in-flight cycle — long before
/// this function reaches its own lock. `RuntimeBuilder::build` treats
/// every manifest field but `[workflows].enabled` as seed-authoritative,
/// i.e. taken from the snapshot handed to it rather than re-read from the
/// store, so a stale snapshot's full-record save silently reverts that
/// write. This reproduces the race directly: hold `serial` to stand in
/// for the in-flight cycle, let the write land while `rebuild_company` is
/// parked in `quiesce()`, then release it and demand the write survived.
#[tokio::test]
async fn a_rebuild_does_not_revert_a_manifest_write_that_lands_during_quiesce() {
    let home_dir = tmp_home();
    let home = home_dir.path();
    let id = CompanyId::new("acme");
    let seed: CompanyManifest =
        toml::from_str("[company]\nname = \"Acme\"\nlogo_url = \"old-logo\"\n")
            .expect("valid manifest");
    let outgoing = Arc::new(
        RuntimeBuilder::new(home.to_path_buf(), seed)
            .with_id(id.clone())
            .build()
            .await
            .expect("build"),
    );
    let state = AppState::new(AppConfig::default());
    state.registry().insert(id.clone(), outgoing.clone());
    state.set_boot_inputs(id.clone(), BootInputs::default());
    let state = state.with_rebuilder(Arc::new(Working {
        home: home.to_path_buf(),
    }));

    // Stand in for a cycle already in flight: `quiesce()` cannot return
    // until this is released.
    let in_flight = outgoing.serial.clone().lock_owned().await;

    let state_for_task = state.clone();
    let id_for_task = id.clone();
    let task = tokio::spawn(async move { rebuild_company(&state_for_task, &id_for_task).await });

    // Give the spawned task every chance to reach the blocked
    // `quiesce()` await before the write below lands. A fixed yield
    // count only hopes the spawned task got scheduled in time; this
    // instead polls `is_quiesced()`, which `quiesce()` sets *before*
    // ever awaiting `serial` (`CompanyRuntime::quiesce`'s own doc), so
    // seeing it flip proves `rebuild_company` actually reached that
    // point rather than merely having had the chance to (CodeRabbit,
    // PR #1875 review). Bounded so a real regression here fails fast
    // instead of hanging the suite.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !outgoing.is_quiesced() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("rebuild_company never reached quiesce() before the timeout");

    // The concurrent write: lock, load-modify-save, unlock — exactly the
    // shape `PUT …/logo` takes (`src/server/ops/company_logo.rs`).
    {
        let write_lock = company_write_lock(&id);
        let _guard = write_lock.lock().await;
        let mut record = outgoing
            .store()
            .load(&id)
            .await
            .expect("load")
            .expect("record exists");
        record.manifest.company.logo_url = Some("new-logo".to_string());
        outgoing.store().save(&record).await.expect("save");
    }

    // Let the "in-flight cycle" finish, unblocking `quiesce()`.
    drop(in_flight);

    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("rebuild_company never finished")
        .expect("task panicked")
        .expect("rebuild_company failed");

    let persisted = outgoing
        .store()
        .load(&id)
        .await
        .expect("load")
        .expect("record exists");
    assert_eq!(
        persisted.manifest.company.logo_url.as_deref(),
        Some("new-logo"),
        "the rebuild's own save must not revert a manifest write that \
         landed under company_write_lock while quiesce() was still \
         draining the in-flight cycle"
    );
}

#[tokio::test]
async fn rebuilding_an_unregistered_company_is_a_not_found() {
    let state = AppState::new(AppConfig::default());
    let err = rebuild_company(&state, &CompanyId::new("nobody"))
        .await
        .expect_err("nothing to rebuild");
    assert!(matches!(err, OpenCompanyError::CompanyNotFound(_)), "{err}");
}
