//! workflow_create: issue #540 author-time tool_call validation.

use super::test_support::*;
use super::*;

// --- #540: author-time tool_call validation ----------------------------

/// A manifest with the `assistant` roster agent AND an explicit
/// `[tools].allow`, so tool_call grant coverage can be exercised precisely.
///
/// Only the tool_call grant-coverage tests use this, and every one of them
/// is gated on `openhuman` (the tool catalogue lives behind that feature);
/// without the gate the helper is dead code at default features and trips
/// `-D warnings` in the default `Rust` CI lane.
#[cfg(feature = "openhuman")]
pub(super) fn manifest_with_allow(allow: &[&str]) -> CompanyManifest {
    let list = allow
        .iter()
        .map(|grant| format!("\"{grant}\""))
        .collect::<Vec<_>>()
        .join(", ");
    toml::from_str(&format!(
        "[company]\nname = \"Acme\"\n[tools]\nallow = [{list}]\n[[agent]]\nid = \"assistant\"\nrole = \"Assistant\"\n"
    ))
    .expect("valid manifest")
}

/// A `trigger → tool_call` draft. `slug` of `None` omits `config` entirely,
/// so the ungated slug-presence check fires. Otherwise the node carries a
/// generic `config.args` table with every workflow-tool required-arg key set
/// (issue #813), so a positive-control slug clears the required-args arm — the
/// arm checks only presence, so the extra keys are harmless and this stays
/// feature-agnostic (no catalogue reference).
pub(super) fn tool_call_draft(id: &str, name: &str, slug: Option<&str>) -> RawWorkflow {
    let mut args = toml::map::Map::new();
    for key in [
        "command",
        "edits",
        "operation",
        "data",
        "filename",
        "url",
        "path",
        "query",
    ] {
        args.insert(key.to_string(), toml::Value::String("x".to_string()));
    }
    tool_call_draft_args(id, name, slug, Some(toml::Value::Table(args)))
}

/// A `trigger → tool_call` draft with explicit control over `config.args` —
/// used to exercise the #813 required-args arm (absent args, present args)
/// directly. `args` of `None` omits the `args` table entirely.
pub(super) fn tool_call_draft_args(
    id: &str,
    name: &str,
    slug: Option<&str>,
    args: Option<toml::Value>,
) -> RawWorkflow {
    let config = slug.map(|slug| {
        let mut table = toml::map::Map::new();
        table.insert("slug".to_string(), toml::Value::String(slug.to_string()));
        if let Some(args) = &args {
            table.insert("args".to_string(), args.clone());
        }
        toml::Value::Table(table)
    });
    RawWorkflow {
        id: id.to_string(),
        name: name.to_string(),
        description: None,
        owner_desk: None,
        nodes: vec![
            RawNode {
                id: "start".to_string(),
                kind: "trigger".to_string(),
                name: "Start".to_string(),
                summary: None,
                agent: None,
                schedule: None,
                config: None,
                on_error: None,
                retry: None,
                requires_approval: None,
                repeatable: None,
                destination: None,
                postcondition: None,
                verify: None,
            },
            RawNode {
                id: "call".to_string(),
                kind: "tool_call".to_string(),
                name: "Call".to_string(),
                summary: None,
                agent: None,
                schedule: None,
                config,
                on_error: None,
                retry: None,
                requires_approval: None,
                repeatable: None,
                destination: None,
                postcondition: None,
                verify: None,
            },
        ],
        edges: vec![RawEdge {
            from: "start".to_string(),
            to: "call".to_string(),
            label: None,
        }],
    }
}

/// UNGATED: a tool_call with no `slug` is refused before any feature-specific
/// namespace resolution, so it fails the same way in every build.
#[tokio::test]
async fn tool_call_without_slug_is_invalid() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft("wf", "WF", None),
        None,
        None,
    )
    .await
    .expect_err("tool_call with no slug");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("no `slug`"), "{err}");
}

/// A slug that maps to no toolbelt namespace is unwired — the run would halt
/// on it, so the save is refused with the run gate's own wording.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn tool_call_with_bogus_slug_is_invalid() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft("wf", "WF", Some("totally_bogus")),
        None,
        None,
    )
    .await
    .expect_err("unwired slug");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(
        err.to_string().contains("not a wired workflow tool"),
        "{err}"
    );
}

/// A wired slug whose namespace the company's `[tools].allow` does not cover
/// is refused; granting that namespace lets the same slug through.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn tool_call_slug_outside_the_granted_namespace_is_invalid() {
    let company = CompanyId::new("acme");
    // `web.*` grants `web` but NOT `code`; `csv_export` is a `code` tool.
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["web.*"]),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft("wf", "WF", Some("csv_export")),
        None,
        None,
    )
    .await
    .expect_err("code slug not granted");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("does not grant"), "{err}");

    // Positive control: grant `code` and the same slug is accepted.
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["code"]),
    )));
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft("wf2", "WF2", Some("csv_export")),
        None,
        None,
    )
    .await
    .expect("code slug is granted");
}

/// Mirrors `caps/tools.rs`'s
/// `the_search_namespace_requires_an_explicit_grant_not_a_wildcard`: the
/// catch-all `*` never confers the priced `search` family, but an explicit
/// `search` grant does.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn the_search_namespace_needs_an_explicit_grant_not_a_wildcard() {
    let company = CompanyId::new("acme");
    // `*` covers ordinary namespaces but must NOT buy a managed search call.
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["*"]),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft("wf", "WF", Some("web_search")),
        None,
        None,
    )
    .await
    .expect_err("wildcard never confers search");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("does not grant"), "{err}");

    // An explicit `search` grant alongside the belt passes.
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["*", "search"]),
    )));
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft("wf2", "WF2", Some("web_search")),
        None,
        None,
    )
    .await
    .expect("explicit search grant is honored");
}

/// The shared helper gates BOTH surfaces: an update into a graph with an
/// unwired tool_call slug is refused the same way a create is.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn update_gates_tool_calls_through_the_shared_helper() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        valid_draft("wf", "WF"),
        None,
        None,
    )
    .await
    .expect("seed create");

    let err = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        tool_call_draft("wf", "WF", Some("totally_bogus")),
        None,
        None,
    )
    .await
    .expect_err("update must gate tool_calls too");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(
        err.to_string().contains("not a wired workflow tool"),
        "{err}"
    );
}
