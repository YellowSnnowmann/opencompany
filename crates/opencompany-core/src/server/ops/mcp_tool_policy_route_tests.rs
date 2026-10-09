//! Route-level guarantees `apply_tool_policy_patch` alone cannot prove: that a
//! write is refused for an id the roster does not know, and that a
//! company-scoped reset does not also erase every teammate's own rule. Driven
//! over the real router, like `team_mcp_tests.rs`, because both checks need a
//! loaded [`CompanyRecord`] behind the route.

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

    let record = CompanyRecord {
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

    Console {
        state,
        secrets,
        _home: home,
    }
}

impl Console {
    async fn admin(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", crate::server::test_support::fixed_cookie(COMPANY));
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

fn row<'a>(dto: &'a Value, tool: &str) -> &'a Value {
    dto["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|row| row["tool"] == tool)
        .unwrap_or_else(|| panic!("row for {tool} in {dto}"))
}

// ---- the roster check on write ---------------------------------------------

/// A `?agent=` naming nobody on the roster must not silently persist a rule
/// under that id. The read side already 404s an unknown agent
/// (`team_mcp::read_permissions`); the write side has to match it, or a
/// mistyped or departed id reads as "restricted" while storing a rule nobody
/// on the roster will ever inherit.
#[tokio::test]
async fn a_write_for_an_unknown_agent_is_refused() {
    let console = console().await;

    let (status, body) = console
        .admin(
            "PUT",
            &policy_uri(Some("ghost")),
            Some(json!({ "tools": [{ "tool": "search_pages", "mode": "blocked" }] })),
        )
        .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Nothing was written under the unknown id — the read only 404s if the
    // write actually refused, rather than storing the rule and 404ing anyway.
    let stored = console
        .secrets
        .get(
            &CompanyId::new(COMPANY),
            &mcp_policy::tool_policies_key(SERVER),
        )
        .await
        .expect("read");
    assert!(
        stored.is_none(),
        "a refused write must leave no document behind"
    );
}

/// The roster check does not block a real teammate.
#[tokio::test]
async fn a_write_for_a_roster_agent_still_succeeds() {
    let console = console().await;

    let (status, dto) = console
        .admin(
            "PUT",
            &policy_uri(Some("writer")),
            Some(json!({ "tools": [{ "tool": "search_pages", "mode": "blocked" }] })),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{dto}");
    assert_eq!(row(&dto, "search_pages")["mode"], "blocked");
}

/// An agent-scoped reset is the documented way to clean up a departed
/// teammate's rules, so it must stay unchecked even for an id the roster no
/// longer has.
#[tokio::test]
async fn an_agent_scoped_reset_is_not_blocked_by_the_roster_check() {
    let console = console().await;

    let (status, _) = console
        .admin("DELETE", &policy_uri(Some("ghost")), None)
        .await;
    assert_eq!(status, StatusCode::OK);
}

// ---- the company reset preserving `agents` ---------------------------------

/// A company-scoped reset must not also erase a teammate's own `Blocked`
/// rule — that silently widens what they can call on the very next turn,
/// which is the opposite of what "reset the company defaults" asked for.
#[tokio::test]
async fn a_company_reset_preserves_a_teammates_own_rule() {
    let console = console().await;
    console
        .seed_inventory(&[("search_pages", mcp_policy::ToolTier::ReadOnly)])
        .await;

    let (status, _) = console
        .admin(
            "PUT",
            &policy_uri(Some("writer")),
            Some(json!({ "tools": [{ "tool": "search_pages", "mode": "blocked" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = console
        .admin(
            "PUT",
            &policy_uri(None),
            Some(json!({ "tierDefaults": { "read_only": "always_allow" } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, reset_dto) = console.admin("DELETE", &policy_uri(None), None).await;
    assert_eq!(status, StatusCode::OK, "{reset_dto}");

    // The company half of the reset did take: the tier default it wrote is
    // gone.
    let (status, company_view) = console.admin("GET", &policy_uri(None), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !company_view["tierDefaults"]["read_only"]["stored"]
            .as_bool()
            .unwrap_or(true),
        "{company_view}"
    );

    // The teammate's own rule survived the company-wide reset: the company
    // lens is still silently aware of it (as an exception), and the reset's
    // own echoed response already carries that — not just a later read.
    assert_eq!(
        row(&reset_dto, "search_pages")["differingAgents"],
        json!(["writer"]),
        "{reset_dto}"
    );
    let (status, writer_view) = console
        .admin("GET", &policy_uri(Some("writer")), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(row(&writer_view, "search_pages")["mode"], "blocked");
}

/// An unreadable document has no `agents` map to preserve, so the reset falls
/// back to the pre-existing full wipe rather than refusing outright — that
/// repair path is the only way back from a document that will not parse.
#[tokio::test]
async fn a_company_reset_still_repairs_an_unreadable_document() {
    let console = console().await;
    console
        .secrets
        .set(
            &CompanyId::new(COMPANY),
            &mcp_policy::tool_policies_key(SERVER),
            SecretValue("not json".to_string()),
        )
        .await
        .expect("seeded");

    let (status, _) = console.admin("GET", &policy_uri(None), None).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an unreadable document reads as a 409 until repaired"
    );

    let (status, _) = console.admin("DELETE", &policy_uri(None), None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = console.admin("GET", &policy_uri(None), None).await;
    assert_eq!(status, StatusCode::OK, "the reset repaired the document");
}
