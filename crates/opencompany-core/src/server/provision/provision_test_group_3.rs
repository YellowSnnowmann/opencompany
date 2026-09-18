use crate::app::config::AuthMode;
use crate::company::CompanyManifest;
use crate::runtime::RuntimeBuilder;
use crate::server::router;
use crate::store::FsCompanyStore;
use axum::http::StatusCode;
use std::sync::Arc;
use tower::ServiceExt;

use super::provision_test_support_1::*;

/// Issue #605: a company provisioned from a manifest that names no tier is
/// recorded on `auto`, explicitly.
///
/// This is the one creation path with no template behind it — `serve` and the
/// desktop app both read a `companies/*/company.toml`, and every shipped preset
/// declares `mode`. So this is where the "new companies get `auto`" half of
/// #605 is actually delivered.
///
/// Asserting `auto` is also what pins the change as *doing something*: the serde
/// default is still `supervised`, deliberately (see `Policy::mode`), so a
/// regression that dropped the provisioning write would record `supervised`
/// here and fail rather than quietly reverting the feature.
#[tokio::test]
async fn a_provisioned_company_with_no_stated_tier_is_recorded_on_auto() {
    let home_dir = home();
    let state = platform_state(home_dir.path(), None);
    let app = router(state.clone());

    let response = app
        .oneshot(provision_req(
            Some(PLATFORM_SECRET),
            "[company]\nname = \"Acme\"\n",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    assert_eq!(
        recorded_mode(&state, "acme").await,
        crate::company::PROVISIONED_POLICY_MODE,
        "a manifest that states no tier must be recorded on the provisioning \
         default, explicitly — not left to the serde default"
    );
    assert_ne!(
        crate::company::PROVISIONED_POLICY_MODE,
        crate::company::Policy::default().mode,
        "if these ever coincide this test proves nothing — it would pass with \
         the provisioning write deleted"
    );
}

/// ...and a manifest that *does* state a tier keeps it, whichever tier it is.
///
/// **Preserve, never widen**, which is the property the whole of #605 turns on.
/// Walked over `POLICY_MODES` rather than spot-checked, so a fifth tier cannot
/// silently escape the guarantee the way `auto` escaped the prose tier lists in
/// #660: the day someone adds one, this covers it without being edited.
///
/// `supervised` is the sharp case and the reason this is a walk and not a single
/// `readonly` assertion — it is the value the serde default *also* produces, so
/// a broken "did the author declare a mode?" check is invisible on every other
/// tier and caught only here.
#[tokio::test]
async fn a_provisioned_company_keeps_whatever_tier_it_states() {
    let home_dir = home();
    let state = platform_state(home_dir.path(), None);
    let app = router(state.clone());

    let mut checked = 0;
    for mode in crate::company::POLICY_MODES {
        let name = format!("Acme {mode}");
        let manifest = format!("[company]\nname = \"{name}\"\n[policy]\nmode = \"{mode}\"\n");
        let response = app
            .clone()
            .oneshot(provision_req(Some(PLATFORM_SECRET), &manifest))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "provisioning `{mode}` failed"
        );

        let id = json_body(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            recorded_mode(&state, &id).await,
            *mode,
            "`{mode}` was stated in the manifest and must survive provisioning \
             untouched"
        );
        checked += 1;
    }
    assert_eq!(
        checked,
        crate::company::POLICY_MODES.len(),
        "the walk skipped a tier"
    );
}

// ---------------------------------------------------------------------------
// Host-wide auth mode override
// ---------------------------------------------------------------------------

/// A host-wide sign-in override set before provisioning (by setup, or flipped
/// live afterward) must reach a company provisioned *after* the change, the
/// same way it reaches every company built at boot — see
/// `AppState::auth_mode_override`. Provisioning built the runtime without
/// threading it through, so an operator who locked the host to `email` after
/// setup still got a provisioned tenant honoring its own manifest mode.
#[tokio::test]
async fn a_host_wide_auth_override_reaches_a_company_provisioned_after_it_is_set() {
    let home_dir = home();
    let state = platform_state(home_dir.path(), None);
    state.set_auth_mode_override(Some(AuthMode::Email));
    let app = router(state.clone());

    let manifest = "[company]\nname = \"Acme\"\n[users]\nmode = \"wallet\"\n";
    let response = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), manifest))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let id = CompanyId::new("acme");
    let runtime = state.registry().get(&id).expect("company is registered");
    assert_eq!(
        runtime.auth_mode(),
        AuthMode::Email,
        "the host-wide override set before provisioning must beat the \
         manifest's own mode, exactly as it does for a company built at boot"
    );
}

/// A request must validate against, and build with, ONE snapshot of the
/// host-wide auth override — not two independent reads of the shared
/// `RwLock` straddling this request's own `.await`s.
///
/// `provision` used to call `state.auth_mode_override()` twice: once to
/// resolve `effective_auth_mode` for the wallet-no-wallets preflight check,
/// and again, later, to build `RuntimeBuilder::with_auth_mode_override`. A
/// concurrent `setup.rs` request can flip the override at any point via
/// `AppState::set_auth_mode_override` — including during the first read's
/// duplicate-id `company_store.load` await, which is exactly where this test
/// flips it. Before the fix, the preflight check validated an admins-only
/// manifest against the override's ORIGINAL value (`email`, which the
/// manifest satisfies), but the builder then read the override's NEW value
/// (`wallet`) and built a runtime in wallet mode with an empty
/// `[users].wallets` — a company nobody could sign into, and on a reset, the
/// only copy left once the old one was archived (issue #1828 comment
/// 3873451846). The fix reads the override once and reuses that snapshot for
/// both, so the built runtime can never disagree with what was validated.
#[tokio::test]
async fn a_concurrent_override_flip_mid_request_cannot_desync_the_check_from_the_build() {
    let home_dir = home();
    let state = platform_state(home_dir.path(), None);
    state.set_auth_mode_override(Some(AuthMode::Email));
    let app = router(state.clone());

    // Shaped exactly like `buildManifestToml` (frontend/src/lib/company-manifest.ts):
    // no `[users].mode` (defaults to `email`), admins only, no wallets.
    let manifest = "[company]\nname = \"Acme\"\n\n[users]\nadmins = [\"admin@example.com\"]\n";

    // Flips the override to `wallet` in a tight loop for the lifetime of the
    // request. The request's only `.await` before the builder reads the
    // override is the duplicate-id `company_store.load` — a real
    // `tokio::fs::read_to_string` that suspends the request task on the
    // blocking pool, giving this loop many scheduler turns to land the flip
    // inside that window before the request resumes.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flip_state = state.clone();
    let flip_stop = stop.clone();
    let flipper = tokio::spawn(async move {
        while !flip_stop.load(std::sync::atomic::Ordering::Relaxed) {
            flip_state.set_auth_mode_override(Some(AuthMode::Wallet));
            tokio::task::yield_now().await;
        }
    });

    let response = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), manifest))
        .await
        .unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    flipper.await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "the manifest was valid against the mode actually checked (`email`) and must provision \
         — a build that silently switched modes underneath a passed check must not surface as \
         a rejection either, it must surface as the desync this test is about"
    );
    let id = CompanyId::new("acme");
    let runtime = state.registry().get(&id).expect("company is registered");
    assert_eq!(
        runtime.auth_mode(),
        AuthMode::Email,
        "the auth mode the runtime was actually built with must match the one the \
         wallet-no-wallets preflight check validated against, regardless of how the shared \
         override changed mid-request — two reads of the same `RwLock` must not be able to \
         disagree with each other"
    );
}

/// A transient failure is retried and the write succeeds, so a mongo blip does
/// not turn into a refused provision.
#[tokio::test]
async fn a_transient_ownership_failure_is_retried_and_succeeds() {
    let (store, attempts) = FlakyOwnership::new(2);
    let result = super::persist_owner_with_retry(&store, &CompanyId::new("acme"), "tenant-a").await;
    assert!(result.is_ok(), "the third attempt succeeds: {result:?}");
    assert_eq!(
        attempts.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "it retried rather than giving up on the first failure"
    );
}

/// A backend that is genuinely down returns the error, which is what the route
/// turns into a refusal. The bound matters: this must not retry forever with a
/// caller waiting on the request.
#[tokio::test]
async fn a_persistent_ownership_failure_gives_up_and_reports_it() {
    let (store, attempts) = FlakyOwnership::new(usize::MAX);
    let result = super::persist_owner_with_retry(&store, &CompanyId::new("acme"), "tenant-a").await;
    assert!(
        result.is_err(),
        "a write that never succeeds must be reported, not swallowed — swallowing it is \
         issue #1050"
    );
    assert_eq!(
        attempts.load(std::sync::atomic::Ordering::SeqCst),
        super::OWNERSHIP_WRITE_ATTEMPTS,
        "bounded: a caller is waiting on this request"
    );
}

/// The happy path costs exactly one write — the retry must not multiply the
/// normal case.
#[tokio::test]
async fn a_successful_ownership_write_is_attempted_once() {
    let (store, attempts) = FlakyOwnership::new(0);
    super::persist_owner_with_retry(&store, &CompanyId::new("acme"), "tenant-a")
        .await
        .expect("writes first time");
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// The property this whole helper exists for: a `status()` failure right
/// after a successful build must not leave the company unregistered.
///
/// Before the fix, `provision` read `status()` BEFORE calling
/// `state.registry().insert(...)` — so on this exact failure, the runtime a
/// successful `build()` had just constructed was discarded, never reaching
/// the registry. The company's `CompanyRecord` was already durably saved
/// (`build()`'s own `store.save`, unaffected by a later `load` failing), so a
/// retry's duplicate-id check (`company_store.load(&id)` in `provision.rs`)
/// would find that record and refuse with `company_exists` — forever, for an
/// id nothing had ever registered and no request could ever create or reach
/// again (issue #1828 comment 3866132497). This test calls the extracted
/// `register_and_report_status` directly — the exact function `provision`
/// calls — so it exercises the real ordering, not a re-implementation of it.
#[tokio::test]
async fn register_and_report_status_registers_even_when_the_status_read_fails() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let id = CompanyId::new("acme");
    let runtime = build_runtime_with_status_read_failing(&home, &id).await;

    let state = platform_state(&home, None);
    let result = super::register_and_report_status(&state, &id, "tenant-a", runtime).await;

    assert!(
        result.is_err(),
        "the status() read was made to fail — the function must report that, not paper over it"
    );
    assert!(
        state.registry().get(&id).is_some(),
        "the company was fully built (its record is durably saved) before the failing \
         status() read ran, so it must already be registered and addressable — a status() \
         failure is a response-body problem, not proof the company doesn't exist"
    );
}

/// The direct, HTTP-level consequence of the property above: a caller that
/// gets an error back from a status()-read failure right after a successful
/// build can still address the company through the ordinary status route —
/// it is not the permanent, unrecoverable lockout a pre-fix retry would hit.
#[tokio::test]
async fn a_company_registered_despite_a_failed_status_read_is_addressable() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let id = CompanyId::new("acme");
    let runtime = build_runtime_with_status_read_failing(&home, &id).await;

    let state = platform_state(&home, None);
    let result = super::register_and_report_status(&state, &id, "tenant-a", runtime).await;
    assert!(result.is_err());

    let app = router(state);
    let response = app
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "registered despite the failed status() read during provisioning, so a plain \
         status lookup afterward must succeed"
    );
}

/// `archive`'s registry/owner cleanup (`src/server/provision.rs`) is gated on
/// `transition`'s whole response being `StatusCode::OK`. `set_lifecycle`
/// persists `lifecycle: "archived"` to the store BEFORE it appends the
/// `LifecycleChanged` audit event, and `transition` then re-reads `status()`
/// after that — so a failure in either the event append or that re-read
/// surfaces as a non-200 response even though the archive genuinely landed.
/// Before the fix, that left the runtime (and its owner record) still
/// registered: on a host at its per-tenant or global company quota
/// (`provision.rs` quota checks) or one that just re-lists companies, an
/// already-archived company kept occupying its slot and appearing in
/// `listCompanies()` — exactly the gap a create/reset dialog's own
/// reconciliation (`create-company-dialog.tsx`, commit 1191ad67e) cannot see
/// or fix from the client, because the client has no visibility into the
/// server's registry (issue #1828 comment 3875046440). This test provisions
/// normally, then swaps in a runtime whose post-persist status() read fails on
/// `archive`, and proves the registry entry does not survive that failure.
#[tokio::test]
async fn archive_removes_from_registry_even_when_the_status_read_fails() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let id = CompanyId::new("acme");

    // Provision normally first, so the durable record exists exactly as it
    // would for a real company (matching `build()`'s rebuild-inherit path
    // the flaky-load helper below relies on for its first `load` call).
    let state = platform_state(&home, None);
    let app = router(state.clone());
    let provisioned = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(provisioned.status(), StatusCode::CREATED);

    // Swap the registered runtime for one whose post-set_lifecycle status()
    // read is made to fail on `archive`, modeling a read blip landing right
    // after the archive write it would be reading back already succeeded.
    let flaky_runtime = build_runtime_with_archive_status_read_failing(&home, &id).await;
    state.registry().insert(id.clone(), Arc::new(flaky_runtime));

    let app = router(state.clone());
    let archived = app
        .oneshot(post_req(
            "/api/v1/companies/acme/archive",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_ne!(
        archived.status(),
        StatusCode::OK,
        "the status() read was made to fail — the response itself must report that"
    );

    assert!(
        state.registry().get(&id).is_none(),
        "set_lifecycle's own store.save already persisted lifecycle:\"archived\" before the \
         status() read failed — the registry entry must not survive a response-body problem \
         after the archive write already landed"
    );
}

/// The regression codex flagged in 890aac128 itself (PR #1828 comment
/// 3875203599): cleanup was rewritten to depend EXCLUSIVELY on a second,
/// redundant `runtime.status()` re-read — even on the ordinary path where
/// `transition`'s response already came back `200`, meaning its OWN
/// `status()` read (the third `load` above) already succeeded and already
/// confirmed `lifecycle: "archived"`. Re-reading a fourth time to reconfirm
/// what the response body already proved was pure downside: a transient
/// failure on that redundant read flipped `archived` to `false` via
/// `unwrap_or(false)`, so the handler still returned the original successful
/// `200` while leaving the archived runtime and its owner registered — a
/// reset at quota then could not provision its replacement because the
/// retired company still occupied the slot. `response.status() == OK` must
/// be sufficient on its own; the extra read exists only to reconcile
/// non-`OK` responses (the `archive_removes_from_registry_even_when_the_
/// status_read_fails` test above).
#[tokio::test]
async fn archive_removes_from_registry_when_the_response_already_confirms_it_even_if_the_redundant_read_fails()
 {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let id = CompanyId::new("acme");

    // Provision normally first, so the durable record exists exactly as it
    // would for a real company (matching `build()`'s rebuild-inherit path
    // the flaky-load helper below relies on for its first `load` call).
    let state = platform_state(&home, None);
    let app = router(state.clone());
    let provisioned = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(provisioned.status(), StatusCode::CREATED);

    // Swap the registered runtime for one whose FOURTH load — the extra
    // read `archive` performs on top of `transition`'s own status() read —
    // is made to fail. The third load (transition's) still succeeds.
    let flaky_runtime = build_runtime_with_redundant_archive_read_failing(&home, &id).await;
    state.registry().insert(id.clone(), Arc::new(flaky_runtime));

    let app = router(state.clone());
    let archived = app
        .oneshot(post_req(
            "/api/v1/companies/acme/archive",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(
        archived.status(),
        StatusCode::OK,
        "transition's own status() read (the third load) succeeded and already confirmed \
         lifecycle: \"archived\" in the response body — only the redundant fourth read was \
         made to fail"
    );

    assert!(
        state.registry().get(&id).is_none(),
        "the response itself already proved the archive landed — a transient failure on the \
         extra, redundant status() re-read must not leave an already-archived company still \
         registered and occupying its quota slot"
    );
}

/// The retry `archive_reconcile_status` adds (`src/server/provision.rs`)
/// closes the gap the single-attempt `unwrap_or(false)` left: a transient
/// failure on `archive`'s non-`OK` reconciliation read used to be
/// indistinguishable from "not archived", even though `set_lifecycle`'s own
/// `store.save` already persisted `lifecycle: "archived"` before that read
/// ever ran. Left uncorrected, the registry/owner cleanup this branch is the
/// LAST chance to run was skipped for good: the create/reset dialog's own
/// client-side reconciliation has no visibility into the server registry, so
/// if its own later status lookup succeeds it reports the reset as done and
/// never calls `archive` again (issue #1828 comment 3875297944). This test
/// makes BOTH `transition`'s own read and the reconciliation branch's first
/// attempt fail, and proves cleanup still lands once the retry's second
/// attempt succeeds.
#[tokio::test]
async fn archive_removes_from_registry_when_the_reconciliation_read_retries_past_one_blip() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let id = CompanyId::new("acme");

    let state = platform_state(&home, None);
    let app = router(state.clone());
    let provisioned = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(provisioned.status(), StatusCode::CREATED);

    let flaky_runtime = build_runtime_with_archive_status_read_failing_twice(&home, &id).await;
    state.registry().insert(id.clone(), Arc::new(flaky_runtime));

    let app = router(state.clone());
    let archived = app
        .oneshot(post_req(
            "/api/v1/companies/acme/archive",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_ne!(
        archived.status(),
        StatusCode::OK,
        "transition's own status() read (the 3rd load) was made to fail — the response itself \
         must still report that"
    );

    assert!(
        state.registry().get(&id).is_none(),
        "set_lifecycle's own store.save already persisted lifecycle:\"archived\" before either \
         read failed; the reconciliation branch's retry must recover from a single blip on its \
         first attempt (the 4th load) and still find archived on its second (the 5th), so \
         cleanup must not be permanently skipped"
    );
}

/// The retry is bounded, not unconditional trust: when the reconciliation
/// read fails on EVERY attempt (`ARCHIVE_RECONCILE_READ_ATTEMPTS` of them),
/// `archive_reconcile_status` must still report failure rather than looping
/// forever or defaulting to "archived", and the registry entry must survive
/// exactly like the pre-existing single-failure case — proving the fix adds
/// resilience to a genuine blip without papering over a store that is really
/// down.
#[tokio::test]
async fn archive_reconciliation_read_gives_up_after_its_attempt_budget_and_leaves_registry_intact()
{
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let id = CompanyId::new("acme");

    let state = platform_state(&home, None);
    let app = router(state.clone());
    let provisioned = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(provisioned.status(), StatusCode::CREATED);

    // 3rd load = transition's own read (fails, so response is non-OK); 4th,
    // 5th, 6th = every attempt archive_reconcile_status's retry budget makes.
    let inner = Arc::new(FsCompanyStore::new(home.clone()));
    let store = Arc::new(FlakyLoadStoreOnCalls::new(inner, [3, 4, 5, 6]));
    let manifest: CompanyManifest = toml::from_str(ACME_TOML).unwrap();
    let flaky_runtime = RuntimeBuilder::new(home.clone(), manifest)
        .with_id(id.clone())
        .with_store(store)
        .build()
        .await
        .expect("build succeeds — its own save is unaffected by a later load failing");
    state.registry().insert(id.clone(), Arc::new(flaky_runtime));

    let app = router(state.clone());
    let archived = app
        .oneshot(post_req(
            "/api/v1/companies/acme/archive",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_ne!(archived.status(), StatusCode::OK);

    assert!(
        state.registry().get(&id).is_some(),
        "every reconciliation attempt was made to fail — a request this inconclusive must not \
         run cleanup (the registry entry stays), but must also return promptly rather than \
         retrying without bound"
    );
}

/// #3894439358: a failed persisted-ownership removal must leave the company
/// registered, not de-registered-with-an-orphaned-row. `MaintenanceTicker::tick`
/// (`src/runtime/maintenance.rs`) only retries a company it can still see in
/// `CompanyRegistry::list` — removing the registry entry BEFORE confirming
/// the durable ownership row is actually gone strands that row the moment the
/// removal fails, with nothing left registered to ever trigger a retry.
#[tokio::test]
async fn a_failed_persisted_ownership_removal_leaves_the_company_registered_for_retry() {
    let id = CompanyId::new("acme");
    let runtime = evict_test_runtime(&id).await;

    let registry = crate::runtime::CompanyRegistry::new();
    registry.insert(id.clone(), runtime.clone());

    let failing = FailingOwnershipRemoval::new();
    let ownership: Option<Arc<dyn crate::store::select::OwnershipStore>> =
        Some(failing.clone() as Arc<dyn crate::store::select::OwnershipStore>);

    let removed = super::evict_registry_and_ownership(&registry, &ownership, &id, &runtime).await;

    assert!(
        !removed,
        "the persisted ownership removal was made to fail — eviction must report that it did \
         not complete"
    );
    assert!(
        registry.get(&id).is_some(),
        "a failed durable ownership removal must leave the company registered so the next \
         maintenance tick can retry the whole eviction — removing it here would orphan the \
         ownership row with nothing left registered to ever revisit it"
    );
    assert_eq!(
        failing
            .remove_owner_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "exactly one removal attempt for this call"
    );
}

/// #3894439351: eviction must not remove a runtime that has since replaced
/// the one it confirmed archived. `CompanyRegistry::insert` — the one choke
/// point every registration goes through, including a rebuild swap
/// (`runtime::rebuild::rebuild_company`) — can land a fresh runtime under the
/// same id in the window between a caller observing `"archived"` and the
/// eviction call actually running. This constructs that race deterministically:
/// `expected` names the ORIGINAL runtime, but the registry now holds a
/// DIFFERENT one under the same id, as if the swap had already landed.
#[tokio::test]
async fn eviction_preserves_a_runtime_that_replaced_the_one_confirmed_archived() {
    let id = CompanyId::new("acme");
    let original = evict_test_runtime(&id).await;
    let replacement = evict_test_runtime(&id).await;
    assert!(
        !Arc::ptr_eq(&original, &replacement),
        "sanity: these must be two distinct runtime instances"
    );

    let registry = crate::runtime::CompanyRegistry::new();
    // The replacement is what's actually registered — as if a rebuild swap
    // landed after `original` was observed archived but before eviction ran.
    registry.insert(id.clone(), replacement.clone());

    let ownership: Option<Arc<dyn crate::store::select::OwnershipStore>> = None;
    let removed = super::evict_registry_and_ownership(&registry, &ownership, &id, &original).await;

    assert!(
        removed,
        "no persisted ownership store was configured, so the durable half trivially succeeds \
         — only the registry conditional is under test here"
    );
    let still_registered = registry.get(&id);
    assert!(
        still_registered.is_some(),
        "the id must still be registered — eviction must not have removed it outright"
    );
    assert!(
        Arc::ptr_eq(still_registered.as_ref().unwrap(), &replacement),
        "eviction confirmed `original` archived, but `replacement` is what was actually \
         registered by the time it ran — removing by id alone would have deregistered the live \
         replacement instead of doing nothing"
    );
}

/// The ordinary case, for contrast with the two failure-mode tests above:
/// nothing raced, ownership removal succeeds (trivially — no store
/// configured), and eviction actually removes the matching registered
/// runtime.
#[tokio::test]
async fn eviction_removes_the_registered_runtime_when_nothing_raced() {
    let id = CompanyId::new("acme");
    let runtime = evict_test_runtime(&id).await;

    let registry = crate::runtime::CompanyRegistry::new();
    registry.insert(id.clone(), runtime.clone());

    let ownership: Option<Arc<dyn crate::store::select::OwnershipStore>> = None;
    let removed = super::evict_registry_and_ownership(&registry, &ownership, &id, &runtime).await;

    assert!(removed);
    assert!(
        registry.get(&id).is_none(),
        "the runtime confirmed archived is the one actually registered, so eviction must \
         remove it"
    );
}

/// The gap this section pins: `pause` stops the whole company, and it used to
/// accept any signed-in member because addressing the company was mistaken for
/// authority over it. Every other admin write on the same session refuses.
#[tokio::test]
async fn a_member_may_not_pause_the_company() {
    let home_dir = home();
    let (app, cookie) = company_with_session(home_dir.path(), crate::ports::UserRole::Member).await;

    let denied = app
        .clone()
        .oneshot(post_req_as("/api/v1/companies/acme/pause", &cookie))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    // The refusal is real, not merely reported: the company is still running.
    let status = app
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(json_body(status).await["lifecycle"], "running");
}
