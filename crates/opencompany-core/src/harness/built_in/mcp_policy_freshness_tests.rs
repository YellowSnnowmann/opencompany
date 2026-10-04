//! The MCP tool-permission freshness gate, driven over the console's own write
//! route: a stored policy is a term of [`HarnessPool::ensure`]'s staleness
//! check, so an operator's permission edit reaches the roster on the company's
//! next turn rather than on the next host restart.
//!
//! Shared setup lives in [`super::built_in_test_fixtures`] and
//! [`super::built_in_test_fixtures_2`]; the route is reached through the real
//! router so the store key the handler writes is the key the fingerprint reads.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::built_in_test_fixtures::*;
use super::built_in_test_fixtures_2::*;
use super::*;

const COMPANY: &str = "acme";
const SERVER: &str = "notion";

fn manifest_declaring_a_server() -> crate::company::CompanyManifest {
    toml::from_str(
        r#"
[company]
name = "Acme"

[policy]
mode = "full"

[tools]
allow = ["mcp:*"]

[[agent]]
id = "ceo"
role = "Chief Executive"

[[mcp_server]]
name = "notion"
endpoint = "https://notion.example/mcp"
"#,
    )
    .expect("valid manifest")
}

fn decl_with(
    tool_policies: crate::company::mcp_policy::McpToolPolicies,
    tool_inventory: crate::company::mcp_policy::McpToolInventory,
) -> McpServerDecl {
    McpServerDecl {
        name: SERVER.to_string(),
        endpoint: "https://notion.example/mcp".to_string(),
        description: None,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        read_only_tools: Vec::new(),
        timeout_secs: 30,
        enabled: true,
        source: crate::company::mcp::McpSource::Manifest,
        auth: crate::company::mcp::AuthMaterial::None,
        tool_policies,
        tool_inventory,
    }
}

/// The fold is canonical: two documents holding the same decisions fingerprint
/// identically however their maps iterate, and one document fingerprints the
/// same on every read. A non-canonical fold rebuilds every roster on every
/// `ensure` instead of failing anything.
#[test]
fn the_policy_fold_is_independent_of_map_iteration_order() {
    use crate::company::mcp_policy::{ApprovalMode, McpToolPolicies, ToolPolicy, ToolTier};

    let tools = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    ];
    let row = |index: usize| ToolPolicy {
        tier: Some(ToolTier::ALL[index % ToolTier::ALL.len()]),
        mode: Some(if index.is_multiple_of(2) {
            ApprovalMode::Blocked
        } else {
            ApprovalMode::AlwaysAllow
        }),
    };

    let mut forward = McpToolPolicies::default();
    for (index, tool) in tools.iter().enumerate() {
        forward.overrides.insert((*tool).to_string(), row(index));
    }
    forward
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::Blocked);
    forward
        .tier_defaults
        .insert(ToolTier::WriteDelete, ApprovalMode::AlwaysAllow);

    let mut backward = McpToolPolicies::default();
    for (index, tool) in tools.iter().enumerate().rev() {
        backward.overrides.insert((*tool).to_string(), row(index));
    }
    backward
        .tier_defaults
        .insert(ToolTier::WriteDelete, ApprovalMode::AlwaysAllow);
    backward
        .tier_defaults
        .insert(ToolTier::ReadOnly, ApprovalMode::Blocked);

    let inventory = crate::company::mcp_policy::McpToolInventory::default();
    assert_eq!(
        mcp_fingerprint(&[decl_with(forward.clone(), inventory.clone())]),
        mcp_fingerprint(&[decl_with(backward, inventory.clone())]),
        "two maps holding the same decisions must fingerprint the same"
    );

    let decls = [decl_with(forward.clone(), inventory.clone())];
    assert_eq!(
        mcp_fingerprint(&decls),
        mcp_fingerprint(&decls),
        "one unchanged document must fingerprint the same on every read"
    );

    let mut widened = forward.clone();
    widened
        .overrides
        .insert("iota".to_string(), row(tools.len()));
    assert_ne!(
        mcp_fingerprint(&[decl_with(forward.clone(), inventory.clone())]),
        mcp_fingerprint(&[decl_with(widened, inventory.clone())]),
        "a new override must move the fingerprint"
    );
}

/// The per-agent half is a term, and canonically so — it is a `BTreeMap`, which is
/// most of the reason the layer lives inside this document.
///
/// Three claims. A per-agent rule moves the hash, one document hashes the same on
/// every read, and residue an unpruned write would leave behind hashes
/// differently from no entry at all — which is why `prune` dropping the emptied
/// teammate is a requirement of the cache axis rather than tidiness.
#[test]
fn the_per_agent_half_is_a_canonical_fingerprint_term() {
    use crate::company::mcp_policy::{
        AgentToolPolicies, ApprovalMode, McpToolInventory, McpToolPolicies, ToolPolicy,
    };

    let inventory = McpToolInventory::default();
    let blank = McpToolPolicies::default();

    let rule = |mode: ApprovalMode| AgentToolPolicies {
        overrides: [(
            "search_pages".to_string(),
            ToolPolicy {
                tier: None,
                mode: Some(mode),
            },
        )]
        .into_iter()
        .collect(),
    };

    let mut writer_blocked = McpToolPolicies::default();
    writer_blocked
        .agents
        .insert("writer".to_string(), rule(ApprovalMode::Blocked));

    assert_ne!(
        mcp_fingerprint(&[decl_with(blank.clone(), inventory.clone())]),
        mcp_fingerprint(&[decl_with(writer_blocked.clone(), inventory.clone())]),
        "a per-agent rule must move the fingerprint"
    );

    let decls = [decl_with(writer_blocked.clone(), inventory.clone())];
    assert_eq!(
        mcp_fingerprint(&decls),
        mcp_fingerprint(&decls),
        "one unchanged per-agent document must fingerprint the same on every read"
    );

    // The same rule written for a different teammate is a different document.
    let mut engineer_blocked = McpToolPolicies::default();
    engineer_blocked
        .agents
        .insert("engineer".to_string(), rule(ApprovalMode::Blocked));
    assert_ne!(
        mcp_fingerprint(&[decl_with(writer_blocked.clone(), inventory.clone())]),
        mcp_fingerprint(&[decl_with(engineer_blocked, inventory.clone())]),
        "whose rule it is must be a term"
    );

    // Residue: an entry that decides nothing resolves identically and hashes
    // differently, so a reset that left it behind would rebuild every roster.
    let mut residue = McpToolPolicies::default();
    residue
        .agents
        .insert("writer".to_string(), AgentToolPolicies::default());
    assert_ne!(
        mcp_fingerprint(&[decl_with(blank.clone(), inventory.clone())]),
        mcp_fingerprint(&[decl_with(residue.clone(), inventory.clone())]),
        "residue hashes differently, which is why prune must remove it"
    );
    let mut pruned = residue;
    pruned.prune();
    assert_eq!(
        mcp_fingerprint(&[decl_with(blank, inventory.clone())]),
        mcp_fingerprint(&[decl_with(pruned, inventory)]),
        "a pruned reset must fingerprint as the document it resolves like"
    );
}

/// A re-probe that learned nothing must not rebuild the roster, so the
/// discovery timestamp is not a term — while a newly discovered tool is.
#[test]
fn only_the_inventory_names_are_a_fingerprint_term() {
    use crate::company::mcp_policy::{McpToolInventory, McpToolPolicies, ToolTier};

    let tools: std::collections::BTreeMap<String, ToolTier> =
        [("search_pages".to_string(), ToolTier::ReadOnly)]
            .into_iter()
            .collect();
    let earlier = McpToolInventory {
        tools: tools.clone(),
        discovered_at_millis: 1,
    };
    let later = McpToolInventory {
        tools: tools.clone(),
        discovered_at_millis: 2,
    };
    assert_eq!(
        mcp_fingerprint(&[decl_with(McpToolPolicies::default(), earlier)]),
        mcp_fingerprint(&[decl_with(McpToolPolicies::default(), later.clone())]),
        "a re-probe that found the same tools must not move the fingerprint"
    );

    let mut grown = later;
    grown
        .tools
        .insert("delete_page".to_string(), ToolTier::WriteDelete);
    assert_ne!(
        mcp_fingerprint(&[decl_with(
            McpToolPolicies::default(),
            McpToolInventory {
                tools,
                discovered_at_millis: 2,
            },
        )]),
        mcp_fingerprint(&[decl_with(McpToolPolicies::default(), grown)]),
        "a newly discovered tool must move the fingerprint"
    );
}

/// A live console over the same secret store the harness resolves from.
async fn console(home: &std::path::Path, secrets: Arc<dyn SecretStore>) -> crate::AppState {
    use crate::ports::CompanyStore;

    let id = CompanyId::new(COMPANY);
    let mut rec = record();
    rec.manifest = manifest_declaring_a_server();
    crate::store::FsCompanyStore::new(home.to_path_buf())
        .save(&rec)
        .await
        .expect("company saved");
    let runtime =
        crate::runtime::RuntimeBuilder::new(home.to_path_buf(), manifest_declaring_a_server())
            .with_id(id.clone())
            .with_secrets(secrets)
            .build()
            .await
            .expect("runtime built");
    let state = crate::AppState::new(crate::AppConfig::default());
    state.registry().insert(id, Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, COMPANY).await;
    state
}

async fn put_policy(state: &crate::AppState, body: Value) -> StatusCode {
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/company/mcp/servers/{SERVER}/tools/policy"))
        .header("cookie", crate::server::test_support::fixed_cookie(COMPANY))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    crate::server::router(state.clone())
        .oneshot(request)
        .await
        .expect("routed")
        .status()
}

/// Blocking a tool through the console route moves the MCP fingerprint, so the
/// next `ensure` rebuilds the roster and the deny list the agent is attached
/// with carries the refusal. A write that decides nothing must not move it — the
/// fingerprint tracks the resolved document, not the fact that a key was
/// written.
#[tokio::test]
async fn a_tool_policy_write_moves_the_mcp_fingerprint() {
    let secrets: Arc<dyn SecretStore> = Arc::new(MemSecrets::default());
    let home = tempfile::tempdir().expect("tempdir");
    let state = console(home.path(), secrets.clone()).await;

    let mut rec = record();
    rec.manifest = manifest_declaring_a_server();
    let mut deps = deps_with_plan(home.path(), Arc::new(MockContext), None, None);
    deps.secrets = Some(secrets.clone());

    let pool = HarnessPool::new();
    pool.ensure(&rec, &deps).await.expect("first ensure");
    let before = pool
        .mcp_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");

    // Resetting a row nobody decided about prunes to the same empty document.
    let status = put_policy(&state, json!({ "tools": [{ "tool": "search_pages" }] })).await;
    assert_eq!(status, StatusCode::OK);
    pool.ensure(&rec, &deps).await.expect("no-op ensure");
    assert_eq!(
        pool.mcp_fingerprint_of(&rec.id).await,
        Some(before),
        "a policy write that decides nothing must not rebuild the roster"
    );

    let status = put_policy(
        &state,
        json!({ "tools": [{ "tool": "delete_page", "mode": "blocked" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    pool.ensure(&rec, &deps).await.expect("post-write ensure");
    let after = pool
        .mcp_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");
    assert_ne!(
        before, after,
        "a tool-permission write must move the staleness fingerprint"
    );
    assert_eq!(
        pool.resident_companies().await,
        1,
        "same company, rebuilt in place"
    );

    // A second load of the stored document, through a second set of maps.
    pool.ensure(&rec, &deps).await.expect("final ensure");
    assert_eq!(
        pool.mcp_fingerprint_of(&rec.id).await,
        Some(after),
        "re-reading an unchanged policy must not rebuild the roster"
    );

    let decls = pool.resolve_effective_mcp(&rec, &deps).await;
    let denied =
        crate::harness::mcp::embed_servers_for_agent(&decls, "ceo", &["mcp:*".to_string()]);
    assert!(
        !denied.is_empty(),
        "the granted server must still reach the agent"
    );
    assert!(
        crate::company::mcp_policy::blocked_tool_names(
            &decls
                .iter()
                .find(|decl| decl.name == SERVER)
                .expect("declared server resolved")
                .tool_policies,
            &crate::company::mcp_policy::McpToolInventory::default(),
        )
        .contains(&"delete_page".to_string()),
        "the blocked tool must be denied on the attachment the rebuild produced"
    );
}

/// The same freshness promise over the `?agent=` route: a per-agent write moves
/// the fingerprint, and a per-agent reset that decides nothing does not.
///
/// The company-wide case is asserted above; this is the one the new lens creates,
/// and it is the only thing that makes `NEXT_TURN_NOTE` on that response true.
#[tokio::test]
async fn a_per_agent_policy_write_moves_the_mcp_fingerprint() {
    let secrets: Arc<dyn SecretStore> = Arc::new(MemSecrets::default());
    let home = tempfile::tempdir().expect("tempdir");
    let state = console(home.path(), secrets.clone()).await;

    let mut rec = record();
    rec.manifest = manifest_declaring_a_server();
    let mut deps = deps_with_plan(home.path(), Arc::new(MockContext), None, None);
    deps.secrets = Some(secrets.clone());

    let pool = HarnessPool::new();
    pool.ensure(&rec, &deps).await.expect("first ensure");
    let before = pool
        .mcp_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");

    let status = put_agent_policy(
        &state,
        "ceo",
        json!({ "tools": [{ "tool": "delete_page", "mode": "blocked" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    pool.ensure(&rec, &deps).await.expect("post-write ensure");
    let after = pool
        .mcp_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");
    assert_ne!(
        before, after,
        "a per-agent permission write must move the staleness fingerprint"
    );

    pool.ensure(&rec, &deps).await.expect("no-op ensure");
    assert_eq!(
        pool.mcp_fingerprint_of(&rec.id).await,
        Some(after),
        "re-reading an unchanged per-agent document must not rebuild the roster"
    );

    // The rebuild carries the refusal, and carries it for that teammate only.
    let decls = pool.resolve_effective_mcp(&rec, &deps).await;
    let denied = |agent: &str| {
        crate::company::mcp_policy::blocked_tool_names_for_agent(
            &decls
                .iter()
                .find(|decl| decl.name == SERVER)
                .expect("declared server resolved")
                .tool_policies,
            &crate::company::mcp_policy::McpToolInventory::default(),
            agent,
        )
    };
    assert_eq!(denied("ceo"), vec!["delete_page".to_string()]);
    assert!(denied("engineer").is_empty());

    // Resetting the same teammate returns the document to where it started, so the
    // fingerprint returns with it rather than drifting one write at a time.
    let status = reset_agent_policy(&state, "ceo").await;
    assert_eq!(status, StatusCode::OK);
    pool.ensure(&rec, &deps).await.expect("post-reset ensure");
    assert_eq!(
        pool.mcp_fingerprint_of(&rec.id).await,
        Some(before),
        "a pruned per-agent reset must fingerprint as the document it started from"
    );
}

async fn put_agent_policy(state: &crate::AppState, agent: &str, body: Value) -> StatusCode {
    let request = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/v1/company/mcp/servers/{SERVER}/tools/policy?agent={agent}"
        ))
        .header("cookie", crate::server::test_support::fixed_cookie(COMPANY))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    crate::server::router(state.clone())
        .oneshot(request)
        .await
        .expect("routed")
        .status()
}

async fn reset_agent_policy(state: &crate::AppState, agent: &str) -> StatusCode {
    let request = Request::builder()
        .method("DELETE")
        .uri(format!(
            "/api/v1/company/mcp/servers/{SERVER}/tools/policy?agent={agent}"
        ))
        .header("cookie", crate::server::test_support::fixed_cookie(COMPANY))
        .body(Body::empty())
        .expect("request");
    crate::server::router(state.clone())
        .oneshot(request)
        .await
        .expect("routed")
        .status()
}
