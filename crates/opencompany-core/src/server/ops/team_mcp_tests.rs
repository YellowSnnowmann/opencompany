//! The per-teammate MCP permissions read, and the `?agent=` lens it has to agree
//! with — driven over the real router, so the store key a handler writes is the
//! key the other one reads.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::company::CompanyManifest;
use crate::mcp::policy as mcp_policy;
use crate::ports::types::{CompanyId, SecretValue};
use crate::ports::{CompanyRecord, SecretStore};

const COMPANY: &str = "acme";
const SERVER: &str = "notion";

/// A company that declares one MCP server, with a teammate whose own grant does
/// not reach it — the not-reached block a page has to render rather than hide.
fn manifest() -> CompanyManifest {
    toml::from_str(
        r#"
[company]
name = "Acme"

[policy]
mode = "full"

[tools]
allow = ["mcp:notion", "search"]

[[agent]]
id = "ceo"
role = "Chief Executive"

[[agent]]
id = "writer"
role = "Writer"
tools = ["search"]

[[mcp_server]]
name = "notion"
endpoint = "https://notion.example/mcp"
"#,
    )
    .expect("valid manifest")
}

#[derive(Default)]
struct MemSecrets {
    map: StdMutex<HashMap<String, String>>,
}

#[async_trait::async_trait]
impl SecretStore for MemSecrets {
    async fn get(&self, _company: &CompanyId, key: &str) -> crate::Result<Option<SecretValue>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(key)
            .map(|raw| SecretValue(raw.clone())))
    }
    async fn set(&self, _company: &CompanyId, key: &str, value: SecretValue) -> crate::Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value.0);
        Ok(())
    }
}

struct Console {
    state: crate::AppState,
    secrets: Arc<MemSecrets>,
    _home: tempfile::TempDir,
}

async fn console() -> Console {
    use crate::ports::CompanyStore;

    let home = tempfile::tempdir().expect("tempdir");
    let secrets = Arc::new(MemSecrets::default());
    let id = CompanyId::new(COMPANY);

    let mut record = CompanyRecord {
        general_channel: Default::default(),
        overlay_desk_hive: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        id: id.clone(),
        manifest: manifest(),
        ledger: Vec::new(),
        lifecycle: "running".to_string(),
        setup: None,
        overlay_agents: Vec::new(),
        overlay_desk_members: Vec::new(),
        overlay_desk_order: Vec::new(),
        overlay_desks: Vec::new(),
        overlay_workflows: Vec::new(),
        overlay_budgets: Vec::new(),
        overlay_policy: None,
        overlay_tool_grants: None,
        overlay_desk_tools: Default::default(),
        disabled_workflows: Vec::new(),
        template_provenance: None,
        name_confirmed: false,
        activation_completed_at: None,
        created_at_millis: None,
    };
    record.manifest = manifest();
    crate::store::FsCompanyStore::new(home.path().to_path_buf())
        .save(&record)
        .await
        .expect("company saved");

    let runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_id(id.clone())
        .with_secrets(secrets.clone() as Arc<dyn SecretStore>)
        .build()
        .await
        .expect("runtime built");
    let state = crate::AppState::new(crate::AppConfig::default());
    state.registry().insert(id, Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, COMPANY).await;
    crate::server::test_support::seed_fixed_member(&state, COMPANY).await;

    Console {
        state,
        secrets,
        _home: home,
    }
}

impl Console {
    async fn send(
        &self,
        method: &str,
        uri: &str,
        cookie: String,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", cookie);
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let request = builder
            .body(match &body {
                Some(value) => Body::from(value.to_string()),
                None => Body::empty(),
            })
            .expect("request");
        let response = crate::server::router(self.state.clone())
            .oneshot(request)
            .await
            .expect("routed");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");
        let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, parsed)
    }

    async fn admin(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.send(
            method,
            uri,
            crate::server::test_support::fixed_cookie(COMPANY),
            body,
        )
        .await
    }

    async fn member(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.send(
            method,
            uri,
            crate::server::test_support::member_cookie(COMPANY),
            body,
        )
        .await
    }

    async fn seed_inventory(&self, tools: &[(&str, mcp_policy::ToolTier)]) {
        let inventory = mcp_policy::McpToolInventory {
            tools: tools
                .iter()
                .map(|(name, tier)| ((*name).to_string(), *tier))
                .collect(),
            discovered_at_millis: 5,
        };
        self.secrets
            .set(
                &CompanyId::new(COMPANY),
                &mcp_policy::tool_inventory_key(SERVER),
                SecretValue(serde_json::to_string(&inventory).expect("serializes")),
            )
            .await
            .expect("seeded");
    }
}

fn policy_uri(agent: Option<&str>) -> String {
    match agent {
        Some(agent) => {
            format!("/api/v1/company/mcp/servers/{SERVER}/tools/policy?agent={agent}")
        }
        None => format!("/api/v1/company/mcp/servers/{SERVER}/tools/policy"),
    }
}

fn permissions_uri(agent: &str) -> String {
    format!("/api/v1/company/team/{agent}/mcp/permissions")
}

fn row<'a>(dto: &'a Value, tool: &str) -> &'a Value {
    dto["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|row| row["tool"] == tool)
        .unwrap_or_else(|| panic!("row for {tool} in {dto}"))
}

fn server_block<'a>(dto: &'a Value, name: &str) -> &'a Value {
    dto["servers"]
        .as_array()
        .expect("servers")
        .iter()
        .find(|block| block["server"] == name)
        .unwrap_or_else(|| panic!("block for {name} in {dto}"))
}

// ---- the write path -------------------------------------------------------

/// The isolation claim over the real route: a per-agent write reaches only that
/// teammate, and the company lens still reads what it read before — plus the
/// count of the exception it is now silent about.
#[tokio::test]
async fn a_per_agent_write_leaves_the_company_document_alone() {
    let console = console().await;
    console
        .seed_inventory(&[("search_pages", mcp_policy::ToolTier::ReadOnly)])
        .await;

    let (status, _) = console
        .admin(
            "PUT",
            &policy_uri(None),
            Some(json!({ "tierDefaults": { "read_only": "always_allow" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, writer) = console
        .admin(
            "PUT",
            &policy_uri(Some("writer")),
            Some(json!({ "tools": [{ "tool": "search_pages", "mode": "blocked" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{writer}");
    assert_eq!(writer["agent"], "writer");
    assert_eq!(row(&writer, "search_pages")["mode"], "blocked");
    assert_eq!(row(&writer, "search_pages")["source"], "agent_pinned");
    assert!(
        writer["note"].is_string(),
        "a per-agent write reaches agents on the next turn, so it carries the note: {writer}"
    );

    let (status, company) = console.admin("GET", &policy_uri(None), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        row(&company, "search_pages")["mode"],
        "always_allow",
        "the company document must be untouched: {company}"
    );
    assert_eq!(
        row(&company, "search_pages")["differingAgents"],
        json!(["writer"])
    );
}

/// A per-agent reset clears that teammate and nothing else, so the two scopes
/// cannot undo each other.
#[tokio::test]
async fn an_agent_scoped_reset_clears_only_that_teammate() {
    let console = console().await;
    console
        .seed_inventory(&[("search_pages", mcp_policy::ToolTier::ReadOnly)])
        .await;

    for agent in ["writer", "ceo"] {
        let (status, _) = console
            .admin(
                "PUT",
                &policy_uri(Some(agent)),
                Some(json!({ "tools": [{ "tool": "search_pages", "mode": "blocked" }] })),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, reset) = console
        .admin("DELETE", &policy_uri(Some("writer")), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(
        row(&reset, "search_pages")["source"],
        "server_inherited",
        "{reset}"
    );

    let (_, ceo) = console.admin("GET", &policy_uri(Some("ceo")), None).await;
    assert_eq!(row(&ceo, "search_pages")["mode"], "blocked");

    let raw = console
        .secrets
        .get(
            &CompanyId::new(COMPANY),
            &mcp_policy::tool_policies_key(SERVER),
        )
        .await
        .expect("read")
        .expect("stored")
        .0;
    assert!(!raw.contains("writer"), "{raw}");
    assert!(raw.contains("ceo"), "{raw}");
}

/// A per-agent tier is a `400` on the wire, not a silently dropped field.
#[tokio::test]
async fn a_per_agent_tier_is_a_400() {
    let console = console().await;

    let (status, body) = console
        .admin(
            "PUT",
            &policy_uri(Some("writer")),
            Some(json!({ "tools": [{ "tool": "search_pages", "tier": "read_only" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = console
        .admin(
            "PUT",
            &policy_uri(Some("writer")),
            Some(json!({ "tierDefaults": { "read_only": "blocked" } })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

/// An unreadable document is a `409` in either scope, and the company-scoped
/// reset stays the one way back.
#[tokio::test]
async fn an_unreadable_document_is_a_409_in_either_scope() {
    let console = console().await;
    console
        .secrets
        .set(
            &CompanyId::new(COMPANY),
            &mcp_policy::tool_policies_key(SERVER),
            SecretValue("{not json".to_string()),
        )
        .await
        .expect("seeded");

    for agent in [None, Some("writer")] {
        let (status, body) = console.admin("GET", &policy_uri(agent), None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], "policy_unreadable");
    }

    let (status, body) = console
        .admin("DELETE", &policy_uri(Some("writer")), None)
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an agent-scoped reset must read first, so it cannot repair a damaged document: {body}"
    );

    let (status, _) = console.admin("DELETE", &policy_uri(None), None).await;
    assert_eq!(status, StatusCode::OK, "the company reset is the repair");
    let (status, _) = console
        .admin("GET", &policy_uri(Some("writer")), None)
        .await;
    assert_eq!(status, StatusCode::OK);
}

/// Reading who can call what changes nothing, so it is member-open in both
/// scopes. Writing settles something on behalf of the company, so it is not.
#[tokio::test]
async fn a_member_may_read_the_agent_lens_but_may_not_write_it() {
    let console = console().await;

    let (status, _) = console
        .member("GET", &policy_uri(Some("writer")), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = console
        .member("GET", &permissions_uri("writer"), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = console
        .member(
            "PUT",
            &policy_uri(Some("writer")),
            Some(json!({ "tools": [{ "tool": "search_pages", "mode": "blocked" }] })),
        )
        .await;
    assert_ne!(status, StatusCode::OK);
}

// ---- the aggregate read --------------------------------------------------

/// Every configured server, reached or not: a page that hid what a teammate
/// cannot reach could not answer the question it exists for.
#[tokio::test]
async fn the_aggregate_read_lists_reached_and_unreached_servers() {
    let console = console().await;
    console
        .seed_inventory(&[("search_pages", mcp_policy::ToolTier::ReadOnly)])
        .await;

    let (status, ceo) = console.admin("GET", &permissions_uri("ceo"), None).await;
    assert_eq!(status, StatusCode::OK, "{ceo}");
    assert_eq!(server_block(&ceo, SERVER)["reached"], true);
    assert!(server_block(&ceo, SERVER)["grantNeeded"].is_null());
    assert_eq!(ceo["approvalsPark"], false);

    let (status, writer) = console.admin("GET", &permissions_uri("writer"), None).await;
    assert_eq!(status, StatusCode::OK, "{writer}");
    let block = server_block(&writer, SERVER);
    assert_eq!(block["reached"], false);
    assert_eq!(
        block["grantNeeded"], "mcp:notion",
        "the block must name the exact grant: {writer}"
    );
    assert_eq!(writer["requested"], json!(["search"]));
}

/// The anti-drift guard: one tool, two routes, one answer. The aggregate exists
/// so the console never re-implements the resolution, which only holds if it
/// cannot disagree with the per-server read.
#[tokio::test]
async fn the_aggregate_agent_read_resolves_the_same_source_as_the_per_server_read() {
    let console = console().await;
    console
        .seed_inventory(&[
            ("search_pages", mcp_policy::ToolTier::ReadOnly),
            ("delete_page", mcp_policy::ToolTier::WriteDelete),
        ])
        .await;

    let (status, _) = console
        .admin(
            "PUT",
            &policy_uri(None),
            Some(json!({ "tools": [{ "tool": "delete_page", "mode": "blocked" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // A stored widening the clamp discards, so `AgentClamped` has to survive both
    // readers.
    let (status, _) = console
        .admin(
            "PUT",
            &policy_uri(Some("ceo")),
            Some(json!({ "tools": [
                { "tool": "delete_page", "mode": "always_allow" },
                { "tool": "search_pages", "mode": "blocked" }
            ] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (_, per_server) = console.admin("GET", &policy_uri(Some("ceo")), None).await;
    let (_, aggregate) = console.admin("GET", &permissions_uri("ceo"), None).await;
    let block = server_block(&aggregate, SERVER);

    for tool in ["search_pages", "delete_page"] {
        let a = row(&per_server, tool);
        let b = block["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|row| row["tool"] == tool)
            .unwrap_or_else(|| panic!("aggregate row for {tool}"));
        assert_eq!(a["mode"], b["mode"], "{tool}");
        assert_eq!(a["source"], b["source"], "{tool}");
        assert_eq!(a["agentMode"], b["agentMode"], "{tool}");
        assert_eq!(a["effectiveTier"], b["effectiveTier"], "{tool}");
    }
    assert_eq!(row(&per_server, "delete_page")["source"], "agent_clamped");
}

/// The state per-agent policy creates and nothing else does — reported where it
/// is created.
#[tokio::test]
async fn a_server_a_teammate_can_call_nothing_on_is_reported_as_such() {
    let console = console().await;
    console
        .seed_inventory(&[
            ("search_pages", mcp_policy::ToolTier::ReadOnly),
            ("delete_page", mcp_policy::ToolTier::WriteDelete),
        ])
        .await;

    let (_, before) = console.admin("GET", &permissions_uri("ceo"), None).await;
    assert_eq!(server_block(&before, SERVER)["fullyRefused"], false);

    let (status, _) = console
        .admin(
            "PUT",
            &policy_uri(Some("ceo")),
            Some(json!({ "tools": [
                { "tool": "search_pages", "mode": "blocked" },
                { "tool": "delete_page", "mode": "blocked" }
            ] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (_, after) = console.admin("GET", &permissions_uri("ceo"), None).await;
    assert_eq!(server_block(&after, SERVER)["fullyRefused"], true);

    let (_, writer) = console.admin("GET", &permissions_uri("writer"), None).await;
    assert_eq!(
        server_block(&writer, SERVER)["fullyRefused"],
        false,
        "one teammate's refusal is not another's"
    );
}

/// One damaged document degrades its own block, never the page.
#[tokio::test]
async fn an_unreadable_document_degrades_one_block_only() {
    let console = console().await;
    console
        .secrets
        .set(
            &CompanyId::new(COMPANY),
            &mcp_policy::tool_policies_key(SERVER),
            SecretValue("{not json".to_string()),
        )
        .await
        .expect("seeded");

    let (status, dto) = console.admin("GET", &permissions_uri("ceo"), None).await;
    assert_eq!(status, StatusCode::OK, "{dto}");
    let block = server_block(&dto, SERVER);
    assert_eq!(block["unreadable"], true);
    assert_eq!(block["tools"], json!([]));
}

#[tokio::test]
async fn an_unknown_teammate_is_a_404() {
    let console = console().await;
    let (status, body) = console.admin("GET", &permissions_uri("nobody"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found");
}
