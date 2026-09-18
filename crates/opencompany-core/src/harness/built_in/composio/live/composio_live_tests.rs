use super::*;

use std::sync::Mutex;

use crate::ports::types::CompanyId;
use crate::ports::usage::UsageSample;

#[derive(Default)]
struct AuthorizeBackend {
    posts: Vec<(String, Value)>,
    connections: Vec<Value>,
    fail_authorize: bool,
    fail_status: bool,
}

struct AuthorizeFixture {
    config: TenantComposio,
    state: Arc<Mutex<AuthorizeBackend>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for AuthorizeFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn authorize_fixture() -> AuthorizeFixture {
    use axum::{
        Router,
        extract::State,
        http::HeaderMap,
        routing::{get, post},
    };

    let state = Arc::new(Mutex::new(AuthorizeBackend::default()));
    let app = Router::new()
            .route(
                "/agent-integrations/composio/authorize",
                post(async |State(state): State<Arc<Mutex<AuthorizeBackend>>>, headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
                    let mut state = state.lock().unwrap();
                    state.posts.push((headers["authorization"].to_str().unwrap().to_string(), body.clone()));
                    if state.fail_authorize {
                        return axum::Json(json!({ "success": false, "error": "authorize unavailable" }));
                    }
                    let id = format!("connection-{}", state.posts.len());
                    state.connections.push(json!({ "id": id, "toolkit": body["toolkit"], "status": "INITIATED" }));
                    axum::Json(json!({ "success": true, "data": {
                        "connectionId": id, "connectUrl": format!("https://connect.composio.dev/{id}")
                    } }))
                }),
            )
            .route(
                "/agent-integrations/composio/connections",
                get(async |State(state): State<Arc<Mutex<AuthorizeBackend>>>| {
                    let state = state.lock().unwrap();
                    if state.fail_status {
                        axum::Json(json!({ "success": false, "error": "connection status unavailable" }))
                    } else {
                        axum::Json(json!({ "success": true, "data": { "connections": state.connections } }))
                    }
                }),
            )
            .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    AuthorizeFixture {
        config: TenantComposio::new(url, Credential::from_value("token-a"), Vec::new()),
        state,
        server,
    }
}

fn authorize_tool(config: &TenantComposio, company: &str) -> Arc<dyn Tool> {
    Arc::from(
        composio_tools(
            config,
            ComposioMetering {
                company: CompanyId::new(company),
                agent: "ceo".to_string(),
                meter: None,
            },
        )
        .into_iter()
        .find(|tool| tool.name() == "composio_authorize")
        .unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authorize_clones_share_one_pending_handoff_under_a_race() {
    for attempt in 0..5 {
        let fixture = authorize_fixture().await;
        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        let mut racers = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let tool = authorize_tool(&fixture.config.clone(), "acme");
            let barrier = barrier.clone();
            let args = json!({ "toolkit": "gmail" });
            racers.spawn(async move {
                barrier.wait().await;
                tool.execute(args).await
            });
        }
        while let Some(result) = racers.join_next().await {
            let result = result.unwrap().unwrap();
            assert!(!result.is_error, "{}", result.output());
            assert!(result.output().contains("connection-1"));
        }
        assert_eq!(
            fixture.state.lock().unwrap().posts.len(),
            1,
            "attempt {attempt}: racing cloned tools must open one handoff"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_authorize_does_not_block_an_unrelated_toolkit() {
    use axum::{Router, extract::State, routing::post};

    #[derive(Default)]
    struct ParallelBackend {
        slow_started: tokio::sync::Notify,
        release_slow: tokio::sync::Notify,
    }

    async fn authorize(
        State(state): State<Arc<ParallelBackend>>,
        axum::Json(body): axum::Json<Value>,
    ) -> axum::Json<Value> {
        let toolkit = body["toolkit"].as_str().unwrap().to_string();
        if toolkit == "gmail" {
            state.slow_started.notify_one();
            state.release_slow.notified().await;
        }
        axum::Json(json!({ "success": true, "data": {
                "connectionId": format!("connection-{toolkit}"),
                "connectUrl": format!("https://connect.composio.dev/{toolkit}")
            } }))
    }

    let state = Arc::new(ParallelBackend::default());
    let app = Router::new()
        .route("/agent-integrations/composio/authorize", post(authorize))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = TenantComposio::new(url, Credential::from_value("token-a"), Vec::new());
    let gmail = authorize_tool(&config, "acme");
    let gmail_call =
        tokio::spawn(async move { gmail.execute(json!({ "toolkit": "gmail" })).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.slow_started.notified(),
    )
    .await
    .expect("the slow request must reach the backend");

    let slack = authorize_tool(&config, "acme");
    let slack_result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        slack.execute(json!({ "toolkit": "slack" })),
    )
    .await;
    let slack_completed = match slack_result {
        Ok(result) => {
            let result = result.unwrap();
            assert!(!result.is_error, "{}", result.output());
            true
        }
        Err(_) => false,
    };
    state.release_slow.notify_one();
    let gmail_result = gmail_call.await.unwrap().unwrap();
    assert!(!gmail_result.is_error, "{}", gmail_result.output());
    assert_eq!(
        usize::from(slack_completed),
        1,
        "an authorization for one toolkit must not wait for another toolkit's network call"
    );
    server.abort();
}

#[tokio::test]
async fn authorize_reconnects_after_a_terminal_or_missing_connection() {
    for status in [
        "ACTIVE",
        "CONNECTED",
        "EXPIRED",
        "FAILED",
        "ERROR",
        "INACTIVE",
        "DISCONNECTED",
        "missing",
    ] {
        let fixture = authorize_fixture().await;
        let tool = authorize_tool(&fixture.config, "acme");
        let first = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
        assert!(!first.is_error, "{}", first.output());
        {
            let mut state = fixture.state.lock().unwrap();
            if status == "missing" {
                state.connections.clear();
            } else {
                state.connections[0]["status"] = json!(status);
            }
        }
        let second = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
        assert!(!second.is_error, "{status}: {}", second.output());
        assert!(
            second.output().contains("connection-2"),
            "{status}: {}",
            second.output()
        );
        assert_eq!(
            fixture.state.lock().unwrap().posts.len(),
            2,
            "{status} must permit a fresh handoff"
        );
        let repeated = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
        assert!(!repeated.is_error, "{}", repeated.output());
        assert_eq!(second.output(), repeated.output());
        assert_eq!(fixture.state.lock().unwrap().posts.len(), 2);
    }
}

#[tokio::test]
async fn authorize_unknown_or_unreadable_status_does_not_start_another_handoff() {
    let fixture = authorize_fixture().await;
    let tool = authorize_tool(&fixture.config, "acme");
    let first = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
    assert!(!first.is_error);
    fixture.state.lock().unwrap().connections[0]["status"] = json!("UNRECOGNIZED");
    let unknown = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
    assert!(unknown.is_error);
    assert!(unknown.output().contains("no new handoff"));
    fixture.state.lock().unwrap().fail_status = true;
    let unreadable = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
    assert!(unreadable.is_error);
    assert_eq!(fixture.state.lock().unwrap().posts.len(), 1);
    {
        let mut state = fixture.state.lock().unwrap();
        state.fail_status = false;
        state.connections[0]["status"] = json!("PENDING");
    }
    let recovered = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
    assert!(!recovered.is_error);
    assert_eq!(first.output(), recovered.output());
    assert_eq!(fixture.state.lock().unwrap().posts.len(), 1);
}

#[tokio::test]
async fn authorize_failed_post_is_not_cached() {
    let fixture = authorize_fixture().await;
    let tool = authorize_tool(&fixture.config, "acme");
    fixture.state.lock().unwrap().fail_authorize = true;
    let first = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
    assert!(first.is_error);
    fixture.state.lock().unwrap().fail_authorize = false;
    let second = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
    assert!(!second.is_error, "{}", second.output());
    assert_eq!(fixture.state.lock().unwrap().posts.len(), 2);
    let third = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
    assert!(!third.is_error);
    assert_eq!(second.output(), third.output());
    assert_eq!(fixture.state.lock().unwrap().posts.len(), 2);
}

#[tokio::test]
async fn authorize_expires_pending_handoffs_at_the_documented_lifetime() {
    let fixture = authorize_fixture().await;
    let tool = authorize_tool(&fixture.config, "acme");
    let first = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
    assert!(!first.is_error);
    for pending in fixture
        .config
        .authorizations
        .lock()
        .await
        .pending
        .values_mut()
    {
        pending.started = tokio::time::Instant::now() - AUTHORIZE_HANDOFF_LIFETIME;
    }
    let second = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
    assert!(!second.is_error);
    assert!(second.output().contains("connection-2"));
    assert_eq!(fixture.state.lock().unwrap().posts.len(), 2);
}

#[tokio::test]
async fn authorize_keys_isolate_companies_credentials_and_extra_parameters() {
    let fixture = authorize_fixture().await;
    let first = authorize_tool(&fixture.config, "acme");
    let another_company = authorize_tool(&fixture.config.clone(), "another-company");
    let initial = json!({ "toolkit": "gmail", "extra_params": { "scope_hint": "one", "nested": { "a": 1, "b": 2 } } });
    assert!(!first.execute(initial.clone()).await.unwrap().is_error);
    assert!(!first.execute(json!({ "extra_params": { "nested": { "b": 2, "a": 1 }, "scope_hint": "one" }, "toolkit": "gmail" })).await.unwrap().is_error);
    assert_eq!(fixture.state.lock().unwrap().posts.len(), 1);
    assert!(
        !another_company
            .execute(initial.clone())
            .await
            .unwrap()
            .is_error
    );
    let mut rotated = fixture.config.clone();
    rotated.credential = Credential::from_value("token-b");
    assert!(
        !authorize_tool(&rotated, "acme")
            .execute(initial.clone())
            .await
            .unwrap()
            .is_error
    );
    assert!(
        !first
            .execute(json!({ "toolkit": "gmail", "extra_params": { "scope_hint": "two" } }))
            .await
            .unwrap()
            .is_error
    );
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.posts.len(), 4);
    assert_eq!(state.posts[2].0, "Bearer token-b");
    let debug = format!("{:?}", fixture.config);
    assert!(!debug.contains("connection-1"));
    assert!(!debug.contains("token-a"));
}

#[tokio::test]
async fn authorize_cache_refuses_capacity_without_starting_an_untracked_handoff() {
    let fixture = authorize_fixture().await;
    let tool = authorize_tool(&fixture.config, "acme");
    for i in 0..MAX_PENDING_AUTHORIZATIONS {
        let result = tool
            .execute(json!({ "toolkit": "gmail", "extra_params": { "account_hint": i } }))
            .await
            .unwrap();
        assert!(!result.is_error, "{}", result.output());
    }
    let refused = tool.execute(json!({ "toolkit": "gmail", "extra_params": { "account_hint": MAX_PENDING_AUTHORIZATIONS } })).await.unwrap();
    assert!(refused.is_error);
    assert!(refused.output().contains("too many cached OAuth handoffs"));
    assert_eq!(
        fixture.state.lock().unwrap().posts.len(),
        MAX_PENDING_AUTHORIZATIONS
    );
    for pending in fixture
        .config
        .authorizations
        .lock()
        .await
        .pending
        .values_mut()
    {
        pending.started = tokio::time::Instant::now() - AUTHORIZE_HANDOFF_LIFETIME;
    }
    let retried = tool.execute(json!({ "toolkit": "gmail" })).await.unwrap();
    assert!(!retried.is_error);
    assert_eq!(fixture.config.authorizations.lock().await.pending.len(), 1);
}

#[test]
fn execute_keys_are_canonical_and_scoped_to_the_action_and_actor() {
    let mut metering = ComposioMetering {
        company: CompanyId::new("acme"),
        agent: "ceo".into(),
        meter: None,
    };
    let body = json!({
        "tool": "GMAIL_SEND_EMAIL",
        "arguments": { "to": "ops@acme.test", "content": { "subject": "hi", "body": "hello" } }
    });
    let reordered = json!({
        "arguments": { "content": { "body": "hello", "subject": "hi" }, "to": "ops@acme.test" },
        "tool": "GMAIL_SEND_EMAIL"
    });
    let key = execute_idempotency_key(&metering, &body).unwrap();
    assert_eq!(key, execute_idempotency_key(&metering, &reordered).unwrap());
    assert!(key.starts_with("oc-composio-v1-"));
    assert!(!key.contains("ops@acme.test"));
    for changed in [
        json!({ "tool": "SLACK_POST_MESSAGE", "arguments": body["arguments"] }),
        json!({ "tool": body["tool"], "arguments": { "to": "other@acme.test" } }),
        json!({ "tool": body["tool"], "arguments": body["arguments"], "connectionId": "another-account" }),
    ] {
        assert_ne!(key, execute_idempotency_key(&metering, &changed).unwrap());
    }
    metering.company = CompanyId::new("another-company");
    assert_ne!(key, execute_idempotency_key(&metering, &body).unwrap());
    metering.company = CompanyId::new("acme");
    metering.agent = "another-agent".into();
    assert_ne!(key, execute_idempotency_key(&metering, &body).unwrap());
}

#[tokio::test]
async fn pinned_execute_preserves_the_key_across_the_post_oauth_retry() {
    use axum::{Router, http::HeaderMap, routing::post};

    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&seen);
    let app = Router::new().route(
        "/agent-integrations/composio/execute",
        post(
            async move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
                let mut requests = captured.lock().unwrap();
                requests.push((headers, body));
                let first = requests.len() == 1;
                axum::Json(json!({
                    "success": true,
                    "data": {
                        "successful": !first,
                        "data": {},
                        "error": if first { Some(POST_OAUTH_AUTH_ERROR) } else { None }
                    }
                }))
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (tool, meter) = tool_over(&url, &[("gmail", "ca-billing")]);
    let result = tool
        .execute(json!({
            "tool": "GMAIL_SEND_EMAIL", "arguments": { "to": "ops@acme.test" }
        }))
        .await
        .unwrap();
    server.abort();
    assert!(!result.is_error, "{}", result.output());
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].0["idempotency-key"],
        requests[1].0["idempotency-key"]
    );
    assert_eq!(requests[0].0["authorization"], "Bearer token");
    assert_eq!(requests[0].1, requests[1].1);
    assert_eq!(requests[0].1["connectionId"], "ca-billing");
    assert_eq!(meter.samples.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn managed_execute_refuses_transport_and_envelope_failures_and_scrubs_tokens() {
    use axum::{Router, http::StatusCode, routing::post};
    use std::sync::atomic::{AtomicUsize, Ordering};

    for (status, body) in [
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "rejected Bearer token" }),
        ),
        (
            StatusCode::OK,
            json!({ "success": false, "error": "rejected Bearer token" }),
        ),
        (StatusCode::OK, json!({ "success": true, "data": null })),
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&requests);
        let app = Router::new().route(
            "/agent-integrations/composio/execute",
            post(async move || {
                captured.fetch_add(1, Ordering::SeqCst);
                (status, axum::Json(body.clone()))
            }),
        );
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (tool, meter) = tool_over(&url, &[]);
        let result = tool
            .execute(json!({
                "tool": "GMAIL_SEND_EMAIL", "arguments": { "to": "ops@acme.test" }
            }))
            .await
            .unwrap();
        server.abort();
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert!(result.is_error, "{}", result.output());
        assert!(
            !result.output().contains("Bearer token"),
            "{}",
            result.output()
        );
        assert!(meter.samples.lock().unwrap().is_empty());
    }

    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&connections);
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            captured.fetch_add(1, Ordering::SeqCst);
            drop(socket);
        }
    });
    let (tool, meter) = tool_over(&url, &[]);
    let result = tool
        .execute(json!({
            "tool": "GMAIL_SEND_EMAIL", "arguments": { "to": "ops@acme.test" }
        }))
        .await
        .unwrap();
    server.abort();
    assert!(connections.load(Ordering::SeqCst) > 0);
    assert!(result.is_error, "{}", result.output());
    assert!(meter.samples.lock().unwrap().is_empty());
}

#[derive(Default)]
struct RecordingMeter {
    samples: Mutex<Vec<UsageSample>>,
}

#[async_trait]
impl UsageMeter for RecordingMeter {
    async fn record(&self, _company: &CompanyId, sample: &UsageSample) -> crate::Result<()> {
        self.samples.lock().unwrap().push(sample.clone());
        Ok(())
    }
    async fn query(&self, _company: &CompanyId, _since: u64) -> crate::Result<Vec<UsageSample>> {
        Ok(self.samples.lock().unwrap().clone())
    }
}

/// An execute tool whose allowlist admits `gmail` only, wired to a
/// recording meter. The client is constructed but never dialled by the
/// paths under test — both return before any network call.
fn tool_with(meter: Arc<RecordingMeter>) -> ComposioExecuteTool {
    ComposioExecuteTool {
        config: Arc::new(TenantComposio::new(
            "https://example.invalid",
            Credential::from_value("token"),
            vec!["gmail".to_string()],
        )),
        toolkits: Arc::new(vec!["gmail".to_string()]),
        metering: ComposioMetering {
            company: CompanyId::new("acme"),
            agent: "ceo".to_string(),
            meter: Some(meter),
        },
    }
}

/// A call blocked by the toolkit allowlist never reaches the provider,
/// so it must not count towards `oauthCalls` — and must not invent a
/// `connections` entry for a toolkit this company cannot even use.
#[tokio::test]
async fn allowlist_rejection_records_no_sample() {
    let meter = Arc::new(RecordingMeter::default());
    let result = tool_with(meter.clone())
        .execute(json!({"tool": "SLACK_POST_MESSAGE"}))
        .await
        .expect("execute returns a result rather than erroring");
    assert!(result.is_error, "the call should be refused");
    assert!(meter.samples.lock().unwrap().is_empty());
}

/// A malformed call is rejected before the client is touched, so it is
/// likewise not a metered OAuth call.
#[tokio::test]
async fn missing_tool_argument_records_no_sample() {
    let meter = Arc::new(RecordingMeter::default());
    let result = tool_with(meter.clone())
        .execute(json!({}))
        .await
        .expect("execute returns a result rather than erroring");
    assert!(result.is_error, "the call should be refused");
    assert!(meter.samples.lock().unwrap().is_empty());
}

// ── which account the call acts as (issue #820) ──────────────────
//
// These assert on the **wire body**, not on a return value, because the
// whole of #820 is a field that was missing from it: a test that only
// checked the result would have passed before the change and after it.

/// Every execute body a stub backend saw.
type Bodies = Arc<Mutex<Vec<Value>>>;

/// A backend that records each `POST …/composio/execute` body and
/// answers with a successful, empty result.
async fn spawn_execute_recorder() -> (String, Bodies) {
    use axum::Router;
    use axum::routing::post;

    let bodies: Bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    let app = Router::new().route(
        "/agent-integrations/composio/execute",
        post(async move |axum::Json(body): axum::Json<Value>| {
            seen.lock().unwrap().push(body);
            axum::Json(json!({
                "success": true,
                "data": { "data": {"ok": true}, "successful": true, "error": null }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), bodies)
}

/// An execute tool over `url`, admitting gmail + slack, carrying
/// `defaults` as the company's pins.
fn tool_over(url: &str, defaults: &[(&str, &str)]) -> (ComposioExecuteTool, Arc<RecordingMeter>) {
    let meter = Arc::new(RecordingMeter::default());
    let toolkits = vec!["gmail".to_string(), "slack".to_string()];
    let config = TenantComposio::new(
        url.to_string(),
        Credential::from_value("token"),
        toolkits.clone(),
    )
    .with_defaults(
        defaults
            .iter()
            .map(|(t, id)| (t.to_string(), id.to_string()))
            .collect(),
    );
    (
        ComposioExecuteTool {
            config: Arc::new(config),
            toolkits: Arc::new(toolkits),
            metering: ComposioMetering {
                company: CompanyId::new("acme"),
                agent: "ceo".to_string(),
                meter: Some(Arc::clone(&meter) as Arc<dyn UsageMeter>),
            },
        },
        meter,
    )
}

/// The ordinary company — one account per toolkit, nothing pinned —
/// must send exactly the body it sent before #820, with no connection
/// id at all. Sending one would change which account Composio resolves
/// for every existing company.
#[tokio::test]
async fn an_unpinned_call_names_no_connection() {
    let (url, bodies) = spawn_execute_recorder().await;
    let (tool, meter) = tool_over(&url, &[]);

    let result = tool
        .execute(json!({"tool": "GMAIL_SEND_EMAIL", "arguments": {"to": "a@b.test"}}))
        .await
        .expect("execute returns a result");
    assert!(!result.is_error, "the call should succeed: {result:?}");

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["tool"], json!("GMAIL_SEND_EMAIL"));
    assert!(
        bodies[0].get("connectionId").is_none(),
        "an unpinned call must carry no connection id: {}",
        bodies[0]
    );
    assert_eq!(meter.samples.lock().unwrap().len(), 1, "still metered");
}

/// The point of the issue: a company that said "send as billing@" has
/// that carried to the backend, which forwards it to Composio as the
/// connected account.
#[tokio::test]
async fn a_pinned_toolkit_sends_its_connection_id() {
    let (url, bodies) = spawn_execute_recorder().await;
    let (tool, meter) = tool_over(&url, &[("gmail", "ca_billing")]);

    let result = tool
        .execute(json!({"tool": "GMAIL_SEND_EMAIL", "arguments": {"to": "a@b.test"}}))
        .await
        .expect("execute returns a result");
    assert!(!result.is_error, "the call should succeed: {result:?}");

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["connectionId"], json!("ca_billing"));
    assert_eq!(
        bodies[0]["arguments"]["to"],
        json!("a@b.test"),
        "the pinned path still normalizes and forwards the arguments"
    );
    assert_eq!(
        meter.samples.lock().unwrap().len(),
        1,
        "a pinned call is metered like any other"
    );
}

/// A pin is per toolkit, so one on gmail must not reach a slack call —
/// the toolkit is derived from the slug, the same prefix the allowlist
/// is enforced on.
#[tokio::test]
async fn a_pin_does_not_leak_across_toolkits() {
    let (url, bodies) = spawn_execute_recorder().await;
    let (tool, _) = tool_over(&url, &[("gmail", "ca_billing")]);

    tool.execute(json!({"tool": "SLACK_POST_MESSAGE", "arguments": {}}))
        .await
        .expect("execute returns a result");

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].get("connectionId").is_none(),
        "slack was never pinned: {}",
        bodies[0]
    );
}

/// The allowlist is still enforced on the slug prefix before anything
/// is sent — a pin is not a way past it.
#[tokio::test]
async fn a_pin_does_not_widen_the_allowlist() {
    let (url, bodies) = spawn_execute_recorder().await;
    let (mut tool, _) = tool_over(&url, &[("notion", "ca_notion")]);
    tool.toolkits = Arc::new(vec!["gmail".to_string()]);

    let result = tool
        .execute(json!({"tool": "NOTION_CREATE_PAGE"}))
        .await
        .expect("execute returns a result");
    assert!(result.is_error, "notion is outside the allowlist");
    assert!(
        bodies.lock().unwrap().is_empty(),
        "nothing should have been sent"
    );
}
