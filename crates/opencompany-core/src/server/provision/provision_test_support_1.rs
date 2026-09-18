//! End-to-end axum tests for provisioning, per-tenant auth, lifecycle controls,
//! quotas, and webhook emission. All offline (default build, no features).

use crate::company::CompanyManifest;
use crate::ports::Brain;
pub(super) use crate::ports::types::{
    CompanyEvent, CompanyId, CompanyRecord, CompanySummary, CompressedTrace, CycleRequest,
    CycleResult, Effect, EffectGroup, EventSeq, LedgerEntry, OutboundMessage, TokenUsage,
};
use crate::ports::{CompanyStore, CycleHost};
use crate::runtime::RuntimeBuilder;
use crate::server::platform_auth::{PlatformAuthConfig, PlatformClaims, UnsignedTenantVerifier};
use crate::server::router;
use crate::store::FsCompanyStore;
use crate::{AppConfig, AppState};
use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use std::collections::HashSet;
use std::sync::Arc;
use tower::ServiceExt;

pub(super) const PLATFORM_SECRET: &str = "plat-secret";

pub(super) const ACME_TOML: &str = "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n";

pub(super) fn home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("oc-provision-")
        .tempdir()
        .expect("tempdir")
}

pub(super) fn platform_state(home: &std::path::Path, max_per_tenant: Option<usize>) -> AppState {
    let verifier = Arc::new(UnsignedTenantVerifier::new(PLATFORM_SECRET));
    AppState::new(AppConfig::default())
        .with_home(home.to_path_buf())
        .with_platform_auth(PlatformAuthConfig::new(verifier))
        .with_quota(None, max_per_tenant)
}

/// A platform state bound to a routable address rather than loopback, for
/// exercising the same none-mode refusal `serve --company` applies at boot.
pub(super) fn routable_platform_state(home: &std::path::Path) -> AppState {
    let verifier = Arc::new(UnsignedTenantVerifier::new(PLATFORM_SECRET));
    AppState::new(AppConfig {
        bind: "0.0.0.0:8080".to_string(),
        ..AppConfig::default()
    })
    .with_home(home.to_path_buf())
    .with_platform_auth(PlatformAuthConfig::new(verifier))
}

/// A platform state in shared-single-DB mode for the workload tenant
/// `namespace` (its `OPENCOMPANY_TENANT_ID`). The configured namespace — not the
/// request's acting tenant — is authoritative for the id prefix and the
/// ownership record, so ids and owners stay workload-local and survive boot
/// hydration, which filters the `owners` rows by this same value.
pub(super) fn namespaced_state(home: &std::path::Path, namespace: &str) -> AppState {
    let verifier = Arc::new(UnsignedTenantVerifier::new(PLATFORM_SECRET));
    AppState::new(AppConfig {
        tenant_namespace: Some(namespace.to_string()),
        ..AppConfig::default()
    })
    .with_home(home.to_path_buf())
    .with_platform_auth(PlatformAuthConfig::new(verifier))
}

/// Mints a tenant principal through the `cfg(test)` unsigned codec.
///
/// What these tests are about is what a *verified* tenant token may reach —
/// scopes, the allow-list, cross-tenant ownership — which is independent of how
/// the bearer was authenticated. The codec keeps them running with no signing
/// machinery; a shipped build accepts this shape from nobody.
pub(super) fn tenant_token(tenant: &str, scopes: &[&str]) -> String {
    UnsignedTenantVerifier::tenant_token(&PlatformClaims {
        tenant: tenant.to_string(),
        scopes: scopes.iter().map(|s| s.to_string()).collect::<HashSet<_>>(),
        companies: None,
    })
}

pub(super) fn provision_req(token: Option<&str>, toml: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/companies")
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .header("content-type", "text/plain");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(toml.to_string())).unwrap()
}

pub(super) fn get_req(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::empty()).unwrap()
}

pub(super) fn post_req(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::empty()).unwrap()
}

pub(super) fn chat_req(uri: &str, token: Option<&str>, text: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    } else {
        // No explicit credential: sign in as the harness admin, since chat now
        // requires a principal like everything else.
        builder = builder.header("cookie", crate::server::test_support::fixed_cookie("acme"));
    }
    builder
        .body(Body::from(format!(r#"{{"text":"{text}"}}"#)))
        .unwrap()
}

pub(super) async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Builds a JSON-envelope provision request naming an explicit id.
pub(super) fn provision_req_json(token: Option<&str>, toml: &str, id: &str) -> Request<Body> {
    let body = serde_json::json!({ "manifest_toml": toml, "id": id }).to_string();
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/companies")
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(body)).unwrap()
}

// ---------------------------------------------------------------------------
// Emergency stop (issue #86)
// ---------------------------------------------------------------------------

/// A `POST` carrying a JSON body, for the step-up-confirmed emergency routes.
pub(super) fn json_post_req(
    uri: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

// ---------------------------------------------------------------------------
// Webhooks
// ---------------------------------------------------------------------------

/// A brain that emits one supervised effect per operator message (parks under a
/// explicit request), so a cycle produces an `approval.requested` webhook.
pub(super) struct EffectBrain {
    pub(super) effect: Effect,
}

#[async_trait]
impl Brain for EffectBrain {
    async fn run_cycle(
        &self,
        req: CycleRequest,
        host: &dyn CycleHost,
    ) -> crate::Result<CycleResult> {
        let mut responses = Vec::new();
        for event in &req.events {
            if let CompanyEvent::OperatorMessage { text, .. } = event {
                host.park_effect(self.effect.clone()).await?;
                responses.push(OutboundMessage {
                    message_id: None,
                    task_id: None,
                    outputs: Vec::new(),
                    channel: "operator".into(),
                    agent: None,
                    text: format!("handled: {text}"),
                    steps: Vec::new(),
                    reply_to: None,
                    mentions: Vec::new(),
                });
            }
        }
        Ok(CycleResult {
            channel_responses: responses,
            new_traces: vec![CompressedTrace::now(&req.cycle_id, "effect cycle")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

// ---------------------------------------------------------------------------
// Issue #605 — the tier a provisioned company is recorded on
// ---------------------------------------------------------------------------

/// The tier `id` was persisted with, read back off the stored record rather
/// than off the response.
///
/// The record is what matters here: it is the manifest a rebuild re-reads and
/// the only place a platform-provisioned tenant's tier is written down at all,
/// since it has no `company.toml` anywhere on disk.
pub(super) async fn recorded_mode(state: &AppState, id: &str) -> String {
    let id = CompanyId::new(id);
    let runtime = state.registry().get(&id).expect("company is registered");
    runtime
        .store()
        .load(&id)
        .await
        .expect("store readable")
        .expect("record exists")
        .manifest
        .policy
        .mode
}

// ── issue #1050: the durable ownership write ────────────────────────────────

/// An [`OwnershipStore`](crate::store::select::OwnershipStore) that fails its
/// first `fail_first` `set_owner` calls, then succeeds — the transient blip
/// (mongo election, timeout) issue #1050 names as the cause.
pub(super) struct FlakyOwnership {
    fail_first: std::sync::Mutex<usize>,
    attempts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl FlakyOwnership {
    pub(super) fn new(fail_first: usize) -> (Self, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Self {
                fail_first: std::sync::Mutex::new(fail_first),
                attempts: attempts.clone(),
            },
            attempts,
        )
    }
}

#[async_trait::async_trait]
impl crate::store::select::OwnershipStore for FlakyOwnership {
    async fn set_owner(&self, _id: &CompanyId, _tenant: &str) -> crate::Result<()> {
        self.attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut left = self.fail_first.lock().unwrap();
        if *left > 0 {
            *left -= 1;
            return Err(crate::error::OpenCompanyError::Config(
                "transient ownership write failure".into(),
            ));
        }
        Ok(())
    }
    async fn remove_owner(&self, _id: &CompanyId) -> crate::Result<()> {
        Ok(())
    }
    async fn owners(&self) -> crate::Result<Vec<(CompanyId, String)>> {
        Ok(Vec::new())
    }
}

// ── issue #1828 comment 3866132497: register before the status() read ──────

/// A `CompanyStore` that fails its `fail_on`-th `load` call (1-indexed) and
/// otherwise delegates to `inner` for everything, including every `save`.
/// Models a transient read blip (a mongo hiccup) landing right after the
/// write it would be reading back already succeeded.
struct FlakyLoadStore {
    inner: Arc<dyn CompanyStore>,
    fail_on: usize,
    load_calls: std::sync::atomic::AtomicUsize,
}

impl FlakyLoadStore {
    fn new(inner: Arc<dyn CompanyStore>, fail_on: usize) -> Self {
        Self {
            inner,
            fail_on,
            load_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl CompanyStore for FlakyLoadStore {
    async fn load(&self, id: &CompanyId) -> crate::Result<Option<CompanyRecord>> {
        let n = self
            .load_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if n == self.fail_on {
            return Err(crate::error::OpenCompanyError::Config(
                "transient store read failure".into(),
            ));
        }
        self.inner.load(id).await
    }
    async fn save(&self, record: &CompanyRecord) -> crate::Result<()> {
        self.inner.save(record).await
    }
    async fn list(&self) -> crate::Result<Vec<CompanySummary>> {
        self.inner.list().await
    }
    async fn append_ledger(&self, id: &CompanyId, entry: LedgerEntry) -> crate::Result<()> {
        self.inner.append_ledger(id, entry).await
    }
}

/// Builds a runtime the same way `provision`'s `builder.build()` does, over a
/// store whose SECOND `load` call fails. The first `load` is `build()`'s own
/// "is this a rebuild" check (empty registry here, so it finds nothing and
/// proceeds as a fresh boot); the second is whichever caller reads next —
/// in production that is `register_and_report_status`'s `runtime.status()`.
/// `build()` itself must still succeed: its own durable `store.save` is
/// unaffected, so by the time this returns the `CompanyRecord` is already on
/// disk regardless of what a later read does.
pub(super) async fn build_runtime_with_status_read_failing(
    home: &std::path::Path,
    id: &CompanyId,
) -> crate::runtime::CompanyRuntime {
    let inner = Arc::new(FsCompanyStore::new(home.to_path_buf()));
    let store = Arc::new(FlakyLoadStore::new(inner, 2));
    let manifest: CompanyManifest = toml::from_str(ACME_TOML).unwrap();
    RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(id.clone())
        .with_store(store)
        .build()
        .await
        .expect("build succeeds — its own save is unaffected by a later load failing")
}

// ── issue #1828 comment 3875046440: finish archive cleanup even when the ──
// ── post-write status() read fails ─────────────────────────────────────

/// A runtime that is already registered (as if provisioned normally), then
/// rebuilt over the SAME durable record with a store whose THIRD `load` call
/// fails. The first `load` is `build()`'s own rebuild check (finds the
/// already-provisioned record and inherits it); the second is inside
/// `set_lifecycle` (`CompanyRuntime::set_lifecycle`, `src/company/runtime.rs`)
/// reading the current record before it flips `lifecycle` to `"archived"` and
/// saves it; the third is `transition`'s own post-`set_lifecycle`
/// `runtime.status()` read. `set_lifecycle`'s `store.save` — like `build()`'s
/// — is unaffected by a later `load` failing, so by the time this third read
/// fails, `lifecycle: "archived"` is already durably on disk.
pub(super) async fn build_runtime_with_archive_status_read_failing(
    home: &std::path::Path,
    id: &CompanyId,
) -> crate::runtime::CompanyRuntime {
    let inner = Arc::new(FsCompanyStore::new(home.to_path_buf()));
    let store = Arc::new(FlakyLoadStore::new(inner, 3));
    let manifest: CompanyManifest = toml::from_str(ACME_TOML).unwrap();
    RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(id.clone())
        .with_store(store)
        .build()
        .await
        .expect("build succeeds — its own save is unaffected by a later load failing")
}

// ── issue #1828 comment 3875203599: the response's own OK is proof enough ──

/// Same rebuild-over-the-existing-record shape as
/// `build_runtime_with_archive_status_read_failing` above, but with a store
/// whose FOURTH `load` call fails instead of its third. The first three
/// loads are unchanged (`build()`'s rebuild check, `set_lifecycle`'s own
/// load, `transition`'s post-`set_lifecycle` `status()`) and all SUCCEED
/// here, so `transition` returns an ordinary `200` whose body already
/// confirms `lifecycle: "archived"`. The fourth load is `archive`'s own
/// extra, redundant `runtime.status()` re-read on top of that.
pub(super) async fn build_runtime_with_redundant_archive_read_failing(
    home: &std::path::Path,
    id: &CompanyId,
) -> crate::runtime::CompanyRuntime {
    let inner = Arc::new(FsCompanyStore::new(home.to_path_buf()));
    let store = Arc::new(FlakyLoadStore::new(inner, 4));
    let manifest: CompanyManifest = toml::from_str(ACME_TOML).unwrap();
    RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(id.clone())
        .with_store(store)
        .build()
        .await
        .expect("build succeeds — its own save is unaffected by a later load failing")
}

// ── issue #1828 comment 3875297944: retry the reconciliation read itself ──
// ── instead of treating one blip on it as proof the archive never landed ──

/// A `CompanyStore` that fails every `load` call whose 1-indexed call number
/// is in `fail_on` and otherwise delegates to `inner`. Unlike `FlakyLoadStore`
/// above (which fails exactly one call), this can make two calls in a row
/// fail — modeling `transition`'s own post-`set_lifecycle` status() read AND
/// one or more attempts of `archive`'s retrying reconciliation read landing
/// back to back.
pub(super) struct FlakyLoadStoreOnCalls {
    inner: Arc<dyn CompanyStore>,
    fail_on: HashSet<usize>,
    load_calls: std::sync::atomic::AtomicUsize,
}

impl FlakyLoadStoreOnCalls {
    pub(super) fn new(
        inner: Arc<dyn CompanyStore>,
        fail_on: impl IntoIterator<Item = usize>,
    ) -> Self {
        Self {
            inner,
            fail_on: fail_on.into_iter().collect(),
            load_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl CompanyStore for FlakyLoadStoreOnCalls {
    async fn load(&self, id: &CompanyId) -> crate::Result<Option<CompanyRecord>> {
        let n = self
            .load_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if self.fail_on.contains(&n) {
            return Err(crate::error::OpenCompanyError::Config(
                "transient store read failure".into(),
            ));
        }
        self.inner.load(id).await
    }
    async fn save(&self, record: &CompanyRecord) -> crate::Result<()> {
        self.inner.save(record).await
    }
    async fn list(&self) -> crate::Result<Vec<CompanySummary>> {
        self.inner.list().await
    }
    async fn append_ledger(&self, id: &CompanyId, entry: LedgerEntry) -> crate::Result<()> {
        self.inner.append_ledger(id, entry).await
    }
}

/// Builds a runtime the same way `build_runtime_with_archive_status_read_failing`
/// does, but over a store whose 3rd AND 4th `load` calls both fail. The 3rd is
/// `transition`'s own post-`set_lifecycle` status() read (so its response
/// still comes back non-`OK`, same as the single-failure case above); the 4th
/// is the FIRST attempt of `archive`'s retrying reconciliation read
/// (`archive_reconcile_status`) — made to fail too, so only the retry's
/// SECOND attempt (the 5th load) succeeds. Proves the fix is an actual retry,
/// not a lone lucky first try landing where the old single read used to.
pub(super) async fn build_runtime_with_archive_status_read_failing_twice(
    home: &std::path::Path,
    id: &CompanyId,
) -> crate::runtime::CompanyRuntime {
    let inner = Arc::new(FsCompanyStore::new(home.to_path_buf()));
    let store = Arc::new(FlakyLoadStoreOnCalls::new(inner, [3, 4]));
    let manifest: CompanyManifest = toml::from_str(ACME_TOML).unwrap();
    RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(id.clone())
        .with_store(store)
        .build()
        .await
        .expect("build succeeds — its own save is unaffected by a later load failing")
}

// ── codex review on #1943, PR comment 3894439358: durable ownership before ─
// ── the irreversible registry removal ────────────────────────────────────
// ── codex review on #1943, PR comment 3894439351: conditional eviction ────
// ── against a runtime that has since replaced the one confirmed archived ──
//
// Both tests below call `evict_registry_and_ownership` directly — the
// sequencing `evict_archived_company` delegates to — rather than through the
// HTTP `archive` route. Constructing the exact race each finding describes
// (a persisted-store failure landing mid-eviction; a rebuild swap landing in
// the window between a caller observing `"archived"` and eviction actually
// running) through the router would mean synchronizing two concurrent
// requests around one specific await point — the direct call lets the test
// construct each "the race already happened" state deterministically instead.

/// An [`OwnershipStore`](crate::store::select::OwnershipStore) whose
/// `remove_owner` always fails — models a persisted-store hiccup landing
/// exactly on eviction's durable ownership cleanup step.
pub(super) struct FailingOwnershipRemoval {
    pub(super) remove_owner_calls: std::sync::atomic::AtomicUsize,
}

impl FailingOwnershipRemoval {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            remove_owner_calls: std::sync::atomic::AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl crate::store::select::OwnershipStore for FailingOwnershipRemoval {
    async fn set_owner(&self, _id: &CompanyId, _tenant: &str) -> crate::Result<()> {
        Ok(())
    }
    async fn remove_owner(&self, _id: &CompanyId) -> crate::Result<()> {
        self.remove_owner_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(crate::error::OpenCompanyError::Config(
            "transient ownership removal failure".into(),
        ))
    }
    async fn owners(&self) -> crate::Result<Vec<(CompanyId, String)>> {
        Ok(Vec::new())
    }
}

/// A bare, registered `CompanyRuntime` for the eviction unit tests below —
/// its own tempdir so two calls for the "same" id never share a durable
/// record, which would blur two instances that must stay distinct `Arc`s.
pub(super) async fn evict_test_runtime(id: &CompanyId) -> Arc<crate::runtime::CompanyRuntime> {
    let home_dir = home();
    let manifest: CompanyManifest = toml::from_str(ACME_TOML).unwrap();
    Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest)
            .with_id(id.clone())
            .build()
            .await
            .expect("runtime builds"),
    )
}

// ---------------------------------------------------------------------------
// Lifecycle authority: who may move a company, not merely who may address it
// ---------------------------------------------------------------------------

/// A `POST` carrying a signed-in human's session cookie.
pub(super) fn post_req_as(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap()
}

/// A `POST` carrying a session cookie and a JSON step-up body.
pub(super) fn json_post_req_as(uri: &str, cookie: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("cookie", cookie)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Provisions `acme` and returns the router plus a seeded session of `role`.
pub(super) async fn company_with_session(
    home: &std::path::Path,
    role: crate::ports::UserRole,
) -> (axum::Router, String) {
    let state = platform_state(home, None);
    let app = router(state.clone());
    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    let cookie = crate::server::test_support::seed_session(&state, "acme", role).await;
    (app, cookie)
}
