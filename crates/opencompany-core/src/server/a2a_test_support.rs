use super::*;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;

use crate::AppConfig;
use crate::company::CompanyManifest;
use crate::economy::signer::LocalSigner;
use crate::economy::{MockTinyplaceClient, TinyplaceEconomy};
use crate::ports::types::{CompanyId, CompressedTrace, CycleRequest, CycleResult, TokenUsage};
use crate::ports::{AgentEconomy, Brain, CompanyStore, CycleHost};
use crate::runtime::RuntimeBuilder;
use crate::store::FsCompanyStore;

pub(super) const DISCOVERABLE_TOML: &str = r#"
    [company]
    name = "Acme SEO"
    output = "SEO audits"
    handle = "acme"

    [place]
    discoverable = true
    skills = [
        { id = "seo.audit", price_usd = "25.00", description = "Full audit" },
        { id = "seo.free", price_usd = "0.00" },
    ]
"#;

/// Builds an `AppState` with one discoverable company wired to a mock
/// economy, rooted at `home`, and returns the client-side signer to sign
/// inbound requests with.
pub(super) async fn seeded_state(home: &std::path::Path) -> (AppState, Arc<LocalSigner>) {
    let manifest: CompanyManifest = toml::from_str(DISCOVERABLE_TOML).unwrap();
    let id = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(FsCompanyStore::new(home.to_path_buf()));
    let signer = Arc::new(LocalSigner::generate());
    let mock = Arc::new(MockTinyplaceClient::new());
    let economy: Arc<dyn AgentEconomy> = Arc::new(
        TinyplaceEconomy::new(mock, signer.clone(), store.clone(), id.clone(), None)
            .going_public(true),
    );
    let runtime = RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(id)
        .with_economy(economy)
        .build()
        .await
        .unwrap();

    let state = AppState::new(AppConfig::default()).with_home(home.to_path_buf());
    state
        .registry()
        .insert(runtime.id().clone(), Arc::new(runtime));

    // The counterparty (client) signs with its own identity.
    let client_signer = Arc::new(LocalSigner::generate());
    (state, client_signer)
}

/// Same as [`seeded_state`], but the company cycle is driven by `brain`
/// instead of the default hosted one — for tests that need to control how
/// long (or how) a cycle runs.
pub(super) async fn seeded_state_with_brain(
    home: &std::path::Path,
    brain: Arc<dyn Brain>,
) -> (AppState, Arc<LocalSigner>) {
    let manifest: CompanyManifest = toml::from_str(DISCOVERABLE_TOML).unwrap();
    let id = CompanyId::new("acme");
    let store: Arc<dyn CompanyStore> = Arc::new(FsCompanyStore::new(home.to_path_buf()));
    let signer = Arc::new(LocalSigner::generate());
    let mock = Arc::new(MockTinyplaceClient::new());
    let economy: Arc<dyn AgentEconomy> = Arc::new(
        TinyplaceEconomy::new(mock, signer.clone(), store.clone(), id.clone(), None)
            .going_public(true),
    );
    let runtime = RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(id)
        .with_economy(economy)
        .with_brain(brain)
        .build()
        .await
        .unwrap();

    let state = AppState::new(AppConfig::default()).with_home(home.to_path_buf());
    state
        .registry()
        .insert(runtime.id().clone(), Arc::new(runtime));

    let client_signer = Arc::new(LocalSigner::generate());
    (state, client_signer)
}

/// A brain that never returns, so a test can prove the cycle it drives is
/// bounded by something other than the brain's own good behavior.
pub(super) struct HangingBrain;

#[async_trait]
impl Brain for HangingBrain {
    async fn run_cycle(
        &self,
        _req: CycleRequest,
        _host: &dyn CycleHost,
    ) -> crate::Result<CycleResult> {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        unreachable!("the cycle timeout must fire long before this wakes")
    }
}

/// A brain that answers a cycle with nothing, cheaply — for tests that
/// only care about the transport, not what cognition produces.
pub(super) struct SilentBrain;

#[async_trait]
impl Brain for SilentBrain {
    async fn run_cycle(
        &self,
        req: CycleRequest,
        _host: &dyn CycleHost,
    ) -> crate::Result<CycleResult> {
        Ok(CycleResult {
            channel_responses: Vec::new(),
            new_traces: vec![CompressedTrace::now(req.cycle_id, "silent test brain")],
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

/// Builds an `AppState` with two distinct discoverable companies, each
/// answering only its own handle — for tests of the prosumer (single-
/// company) fallback's boundary.
pub(super) async fn two_company_state(home: &std::path::Path) -> AppState {
    let state = AppState::new(AppConfig::default()).with_home(home.to_path_buf());
    for handle in ["acme", "globex"] {
        let toml_src = format!(
            r#"
            [company]
            name = "{handle}"
            output = "audits"
            handle = "{handle}"

            [place]
            discoverable = true
            skills = [
                {{ id = "seo.free", price_usd = "0.00" }},
            ]
            "#
        );
        let manifest: CompanyManifest = toml::from_str(&toml_src).unwrap();
        let id = CompanyId::new(handle);
        let runtime = RuntimeBuilder::new(home.to_path_buf(), manifest)
            .with_id(id.clone())
            .with_brain(Arc::new(SilentBrain))
            .build()
            .await
            .unwrap();
        state.registry().insert(id, Arc::new(runtime));
    }
    state
}

/// Signs a POST body for `/a2a/{handle}` and returns the SIWX header value.
pub(super) fn siwx_header(signer: &LocalSigner, handle: &str, body: &[u8], ts: i64) -> String {
    let hash = sha256_hex(body);
    let header = siwx::build_header(
        signer,
        &siwx::SiwxPayload {
            method: "POST",
            path: &format!("/a2a/{handle}"),
            timestamp: ts,
            body_hash: &hash,
        },
    );
    siwx::header_value(&header)
}

/// Builds a SIWX-signed `seo.audit` request carrying `auth` as its payment.
/// `site` varies the body so each request has its own SIWX signature.
pub(super) fn paid_request(
    client: &LocalSigner,
    auth: &X402Authorization,
    site: &str,
) -> Request<Body> {
    let rpc = JsonRpcRequest::new(
        "tasks/send",
        json!({ "skill": "seo.audit", "input": { "site": site }, "payment": auth }),
    );
    let body = serde_json::to_vec(&rpc).unwrap();
    let header = siwx_header(client, "acme", &body, now_secs());
    Request::builder()
        .method("POST")
        .uri("/a2a/acme")
        .header(AUTHORIZATION, header)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap()
}

pub(super) fn task_body(skill: &str) -> Vec<u8> {
    serde_json::to_vec(&JsonRpcRequest::new(
        "tasks/send",
        json!({ "skill": skill, "input": { "site": "x.com" } }),
    ))
    .unwrap()
}

pub(super) fn card_pricing(skills: &[(&str, &str)]) -> AgentCard {
    AgentCard {
        payment_requirements: skills
            .iter()
            .map(|(id, price)| CardPayment {
                skill_id: (*id).to_string(),
                price: (*price).to_string(),
                asset: "USDC".into(),
                network: "solana".into(),
            })
            .collect(),
        ..AgentCard::default()
    }
}
