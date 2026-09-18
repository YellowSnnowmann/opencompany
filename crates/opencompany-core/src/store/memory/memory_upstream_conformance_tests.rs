//! The upstream driver-conformance suite, run against the providers **this
//! host constructs**.
//!
//! The vendored tinymemory workspace already proves its adapters uphold the
//! contract (`adapters/remote/src/conformance_test.rs`,
//! `core/src/store/factories_provider_test.rs`) — but no OpenCompany lane runs
//! that workspace, and none of it goes through `open_driver`. Namespace-driver
//! defects #1201 and #1238 shipped through exactly that gap: the driver's own
//! suite was green upstream while the driver this host binds misbehaved.
//!
//! So these run `tinymemory_conformance::assert_provider` — the same eleven
//! behavioural assertions — against providers built the way production builds
//! them: a [`MemoryDriverConfig`] through [`open_driver`], nothing constructed
//! by hand. The embedded test binds the real `namespace` store on a tempdir;
//! the hosted tests bind each HTTP adapter against an in-process double
//! speaking that vendor's own wire shapes, ported from the vendored
//! `adapters/remote/src/conformance_test.rs` (keep them in step with it when
//! the adapters' dialects change).
//!
//! Each hosted engine also gets a **facade round-trip**: traces, facts and
//! context driven through [`BoundMemory`]'s ports over real HTTP. The facades
//! encode records into a JSON envelope inside `content` and re-derive
//! everything on decode — the surface #1201 corrupted — and nothing else
//! exercises that path against the hosted dialects.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::{Json, Router};
use serde_json::{Value, json};
use tinymemory::registry::DriverClass;
use tinymemory_api::provider::MemoryProvider;

use super::BoundMemory;
use super::driver::{MemoryDriverConfig, MemoryMode, RemoteDeployment, open_driver};
use crate::ports::{CompanyId, CompressedTrace, ContextChunk, FactKind, FactRecord};

/// Opens a driver through the production path and fails the test on refusal.
pub(super) fn open(config: &MemoryDriverConfig) -> (Arc<dyn MemoryProvider>, DriverClass) {
    open_driver(config)
        .expect("the driver must bind")
        .expect("this config names a driver, so `None` is a routing bug")
}

/// The suite skips every write-path assertion for a non-retaining driver, so a
/// provider (or a broken double behind it) that dropped writes would let
/// `assert_provider` pass having proved almost nothing. Guard first.
pub(super) async fn assert_retains_then_conforms(provider: Arc<dyn MemoryProvider>) {
    assert!(
        tinymemory_conformance::retains_writes(provider.as_ref()).await,
        "driver `{}` must retain writes, or the conformance run is vacuous",
        provider.driver_id()
    );
    tinymemory_conformance::assert_provider(provider).await;
}

/// Traces, facts and context through the decorator's ports — the JSON-envelope
/// encode/decode path (#1201's shape) — against whatever engine is bound.
pub(super) async fn facade_round_trip(provider: Arc<dyn MemoryProvider>, class: DriverClass) {
    let bound = BoundMemory::bind(provider, class).expect("bind");
    let company = CompanyId::new("acme");

    let memory = bound.memory();
    memory
        .save_trace(
            &company,
            CompressedTrace {
                cycle_id: "cycle-1".into(),
                // A card-shaped digit run in purchase-y prose, deliberately:
                // #1201 was the embedded scrubber redacting digits INTO BROKEN
                // JSON, so the whole record silently vanished. The contract
                // pinned here is survival, not verbatim digits — an engine
                // with a PII scrubber (the embedded driver, post-#1248) is
                // entitled to redact the card number out of the *content*; it
                // is never entitled to corrupt the envelope around it. #1248's
                // own pin covers the sharper half (Luhn-valid `at_millis`
                // stamps round-trip untouched); this one proves the decode
                // path over every engine this host binds. On the hosted
                // engines, which scrub nothing, the digits also come back —
                // but that is engine policy, so it is not asserted.
                // nosemgrep: coderabbit.pii.credit-card-number-dashed -- test
                // vector, not a credential; see the note above.
                summary: "ordered part 4111 1111 1111 1111 for the lathe".into(),
                at_millis: 1_700_000_000_000,
            },
        )
        .await
        .expect("save_trace");
    let traces = memory.recent_traces(&company, 10).await.expect("traces");
    assert_eq!(traces.len(), 1, "the trace must survive the envelope");
    assert_eq!(
        traces[0].cycle_id, "cycle-1",
        "the record's identity must survive"
    );
    assert!(
        traces[0].summary.contains("ordered part") && traces[0].summary.contains("for the lathe"),
        "the non-sensitive prose must survive whatever the engine's scrubbing policy is; got: {:?}",
        traces[0].summary
    );

    let facts = bound.facts();
    facts
        .upsert(
            &company,
            &FactRecord {
                id: "fact-1".into(),
                kind: FactKind::Fact,
                title: "supplier".into(),
                body: "lathe parts come from Initech".into(),
                source: "cto".into(),
                updated_at_millis: 1_700_000_000_001,
            },
        )
        .await
        .expect("fact upsert");
    let listed = facts.list(&company, None, None).await.expect("fact list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].body, "lathe parts come from Initech");

    // Characters a hosted engine sanitises out of `content` (tinymemory#80).
    // The envelope escapes them precisely so this assertion can hold, and the
    // assertion lives here — in the shared facade path — rather than beside
    // the escaping, because the escaping is only worth anything if the record
    // survives the whole way to an engine and back. Against live Supermemory
    // this failed before the escape landed: `U+FFFD` came back missing from
    // the middle of the body, so the record decoded to a string one character
    // shorter than the one that was stored, with nothing raising an error.
    //
    // `U+0000` rides along as the control. JSON already escapes it, so it
    // survived even before the fix — which is what makes it useful here: if a
    // later change to the envelope broke escaping wholesale, the two would
    // fail together, and if only `U+FFFD` fails then the strip is engine-side.
    //
    // Under its OWN company, and deleted afterwards. A hosted engine retains
    // what this writes, so a fact left in `acme` is still there on the next
    // run — and the `listed.len() == 1` above would then fail on a second run
    // of a suite that passed on the first, blaming the engine for the test's
    // own litter.
    let awkward = CompanyId::new("acme-awkward-content");
    for (label, character) in [("nul", '\u{0}'), ("replacement", '\u{FFFD}')] {
        let body = format!("before{character}after");
        let id = format!("fact-awkward-{label}");
        facts
            .upsert(
                &awkward,
                &FactRecord {
                    id: id.clone(),
                    kind: FactKind::Fact,
                    title: format!("awkward {label}"),
                    body: body.clone(),
                    source: "cto".into(),
                    updated_at_millis: 1_700_000_000_002,
                },
            )
            .await
            .unwrap_or_else(|error| panic!("{label} fact upsert: {error}"));
        let listed = facts.list(&awkward, None, None).await.expect("fact list");
        let stored = listed.iter().find(|fact| fact.id == id);
        assert_eq!(
            stored.map(|fact| fact.body.as_str()),
            Some(body.as_str()),
            "{label}: the record must read back as it was written, not with the \
             character removed from the middle of it"
        );
        facts
            .delete(&awkward, &id)
            .await
            .unwrap_or_else(|error| panic!("{label} cleanup: {error}"));
    }

    let context = bound.context();
    context
        .put(
            &company,
            ContextChunk {
                label: "notes/supplier".into(),
                body: "the quick brown fox".into(),
            },
        )
        .await
        .expect("context put");
    let hits = context
        .search(&company, "quick brown", 10)
        .await
        .expect("context search");
    assert_eq!(
        hits.len(),
        1,
        "context must be recallable through the engine"
    );

    // The port-level `(addr, label)` contract, over whatever engine is bound
    // (issue #1300). Every backend that implements `ContextStore` directly —
    // fs, sqlite, mongodb — proves these in its own test module; the engines
    // reached through `ProviderContextStore` did not, because nothing ran the
    // HOST's port suite against them. That gap matters precisely for the
    // hosted drivers: `delete_label` is a read-merge-write over `get`/`put`,
    // which on supermemory, mem0 and cognee is three HTTP calls against an
    // engine whose `get` and `list` shapes this host does not control. A
    // divergence there is a claim silently lost or a body reaped while
    // another label still points at it — and it would have been invisible
    // until an operator noticed a memory missing.
    //
    // Run against `bound.context()` rather than a fresh binding so the
    // envelope, namespace and decode path under test are the ones the rest of
    // this function already exercised.
    crate::store::conformance::assert_identical_body_two_labels(bound.context()).await;
    crate::store::conformance::assert_delete_label_scoped(bound.context()).await;
}

// ── Vendor doubles ───────────────────────────────────────────────────────────
//
// Ported from the vendored `adapters/remote/src/conformance_test.rs` at
// tinymemory 38a34d2 — the shapes are each vendor's own, and the doubles
// retain what they are sent, which is what lets the suite's write-path
// assertions actually run. Auth headers are ignored: what the adapter *sends*
// is pinned upstream; these tests are about what this host binds.

/// A record as one of the vendor doubles holds it.
#[derive(Clone, Debug)]
pub(super) struct Row {
    pub(super) id: String,
    pub(super) content: String,
    pub(super) metadata: Value,
    /// The `containerTag` the adapter sent at create time (Supermemory only;
    /// Mem0 rows carry an empty one).
    pub(super) tag: String,
}

/// The doubles' shared store: `id -> Row`, plus a counter for fresh ids.
#[derive(Default, Debug)]
pub(super) struct Backend {
    pub(super) rows: BTreeMap<String, Row>,
    next: usize,
}

impl Backend {
    pub(super) fn fresh_id(&mut self) -> String {
        self.next += 1;
        format!("rec-{}", self.next)
    }
}

pub(super) type Store = Arc<Mutex<Backend>>;

/// Serves `app` on an ephemeral port and returns its base URL.
pub(super) async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    endpoint
}

/// The production remote config for one hosted engine, pointed at a double.
pub(super) fn remote_config(driver: &str, endpoint: &str) -> MemoryDriverConfig {
    MemoryDriverConfig {
        mode: MemoryMode::Remote,
        driver_id: Some(driver.into()),
        url: Some(endpoint.into()),
        api_key: Some("test-key".into()),
        data_dir: None,
        // The doubles below speak each vendor's SELF-HOSTED dialect, ported
        // from the adapters' own tests — un-prefixed paths, `X-API-Key` where
        // the platform would want `Authorization: Token`. So the config has to
        // say so: pointing the managed client at them would 404 on paths that
        // exist only on the platform, and the failure would read as a broken
        // adapter rather than a mismatched double.
        //
        // Managed is what production defaults to, and it is covered against
        // the real services in `live_hosted` plus, offline, by
        // `the_managed_deployment_speaks_the_platform_dialect` below.
        deployment: RemoteDeployment::SelfHosted,
    }
}

/// The deployment knob must actually change the wire conversation.
///
/// This is the offline guard for a defect the live lane found: OpenCompany
/// built every remote engine with the self-hosted constructor, so a tenant
/// pointed at Mem0's or Cognee's managed platform spoke the wrong protocol
/// entirely — `X-API-Key` and un-prefixed paths at Mem0 (HTTP 404), a bearer
/// token at Cognee Cloud (HTTP 401 `Invalid header`). Supermemory hid it,
/// because it serves the same API to both deployments on one bearer
/// credential, so the one engine anybody tested against kept working.
///
/// Asserting on the *credential header* rather than the path is deliberate:
/// it is the half that cannot be papered over by a permissive router, and it
/// is what each vendor actually distinguishes its two products by.
#[tokio::test]
async fn the_deployment_selects_each_vendors_dialect() {
    use axum::http::HeaderMap;

    /// Captures the first request's headers, then fails the call.
    async fn capture(
        State(seen): State<Arc<Mutex<Vec<String>>>>,
        headers: HeaderMap,
    ) -> (axum::http::StatusCode, Json<Value>) {
        let rendered: Vec<String> = ["authorization", "x-api-key"]
            .iter()
            .filter_map(|name| {
                headers
                    .get(*name)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| format!("{name}: {v}"))
            })
            .collect();
        seen.lock().expect("lock").extend(rendered);
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"detail": "capture double"})),
        )
    }

    for (driver, deployment, expected) in [
        (
            "mem0",
            RemoteDeployment::Managed,
            "authorization: Token test-key",
        ),
        ("mem0", RemoteDeployment::SelfHosted, "x-api-key: test-key"),
        ("cognee", RemoteDeployment::Managed, "x-api-key: test-key"),
        (
            "cognee",
            RemoteDeployment::SelfHosted,
            "authorization: Bearer test-key",
        ),
    ] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .fallback(axum::routing::any(capture))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let endpoint = format!("http://{}", listener.local_addr().expect("addr"));
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

        let mut config = remote_config(driver, &endpoint);
        config.deployment = deployment;
        let (provider, _) = open(&config);
        // The call is expected to fail — the double refuses everything. What
        // is under test is the credential it carried getting there.
        let _ = provider.get("any", "thing").await;

        let headers = seen.lock().expect("lock").clone();
        assert!(
            headers.iter().any(|h| h.eq_ignore_ascii_case(expected)),
            "{driver} as {deployment:?} must authenticate with `{expected}`; sent {headers:?}"
        );
    }
}
