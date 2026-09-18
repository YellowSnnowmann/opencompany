use super::*;

/// [`WORKFLOW_TOOL_SLUGS`] is a derivative of
/// [`toolbelt::namespace_of`](crate::harness::toolbelt::namespace_of), not a
/// second source of truth: every row's namespace must be what `namespace_of`
/// returns for that slug, and must be one the invoker actually wires
/// ([`WORKFLOW_TOOL_NAMESPACES`]). A toolbelt tool added or re-namespaced
/// without updating this table fails here rather than silently changing what
/// the create-time copilot (issue #753) can ground the model in.
#[test]
fn the_slug_table_agrees_with_namespace_of() {
    for (slug, namespace) in WORKFLOW_TOOL_SLUGS {
        assert_eq!(
            toolbelt::namespace_of(slug),
            Some(*namespace),
            "slug `{slug}` is listed under `{namespace}` but namespace_of disagrees"
        );
        assert!(
            WORKFLOW_TOOL_NAMESPACES.contains(namespace),
            "slug `{slug}`'s namespace `{namespace}` is not a wired workflow namespace"
        );
    }
}

/// [`WORKFLOW_TOOL_CATALOG`] is a strict companion of
/// [`WORKFLOW_TOOL_SLUGS`], not a second source of truth: it names the exact
/// same slugs (both directions), every row's namespace is what
/// [`toolbelt::namespace_of`] returns and is a wired workflow namespace, and
/// no capability line or required-arg name is blank. A tool added to (or
/// dropped from) the slug table without a matching catalogue edit fails here
/// rather than silently narrowing — or mis-describing — what the create-time
/// copilot (issue #813) grounds the model in.
#[test]
fn the_catalog_agrees_with_the_slug_table_and_namespace_of() {
    use std::collections::HashSet;
    let catalog: HashSet<&str> = WORKFLOW_TOOL_CATALOG.iter().map(|info| info.slug).collect();
    let table: HashSet<&str> = WORKFLOW_TOOL_SLUGS.iter().map(|(slug, _)| *slug).collect();
    assert_eq!(
        catalog, table,
        "the tool catalogue and the slug table must name the same slugs"
    );
    for info in WORKFLOW_TOOL_CATALOG {
        assert_eq!(
            toolbelt::namespace_of(info.slug),
            Some(info.namespace),
            "catalogue slug `{}` is listed under `{}` but namespace_of disagrees",
            info.slug,
            info.namespace
        );
        assert!(
            WORKFLOW_TOOL_NAMESPACES.contains(&info.namespace),
            "catalogue slug `{}`'s namespace `{}` is not a wired workflow namespace",
            info.slug,
            info.namespace
        );
        assert!(
            !info.capability.trim().is_empty(),
            "catalogue slug `{}` has an empty capability line",
            info.slug
        );
        assert!(
            info.required_args.iter().all(|arg| !arg.trim().is_empty()),
            "catalogue slug `{}` has an empty required-arg name",
            info.slug
        );
    }
}

#[test]
fn json_text_block_passes_through_else_wrapped() {
    let json_result = ToolResult::success(r#"{"rows": 3}"#);
    assert_eq!(
        tool_result_to_value("csv_export", json_result).unwrap(),
        json!({ "rows": 3 })
    );

    let text_result = ToolResult::success("Exported 3 rows to exports/out.csv");
    assert_eq!(
        tool_result_to_value("csv_export", text_result).unwrap(),
        json!({ "text": "Exported 3 rows to exports/out.csv" })
    );
}

#[test]
fn error_result_becomes_a_capability_error() {
    let err = tool_result_to_value("csv_export", ToolResult::error("nope")).unwrap_err();
    assert!(
        matches!(err, EngineError::Capability(ref m) if m.contains("nope")),
        "{err:?}"
    );
}

#[test]
fn ungranted_and_unknown_slugs_are_rejected_fail_closed() {
    use tinyflows::caps::ToolInvoker;
    // No `code` grant → csv_export (a `code`-namespace tool) is denied even
    // though it is wired.
    let invoker = WorkflowToolInvoker {
        tools: HashMap::new(),
        grants: vec!["web.*".to_string()],
        wiring: WorkflowToolWiring::default(),
        emergency: None,
    };
    let denied = tokio_test_block_on(invoker.invoke("csv_export", json!({}), None));
    assert!(
        matches!(denied, Err(EngineError::Capability(ref m)) if m.contains("not granted")),
        "{denied:?}"
    );
    // A slug with no toolbelt namespace is rejected as unwired.
    let unwired = tokio_test_block_on(invoker.invoke("email.send", json!({}), None));
    assert!(
        matches!(unwired, Err(EngineError::Capability(ref m)) if m.contains("not a wired")),
        "{unwired:?}"
    );
}

/// A run admitted before the switch was pulled must not keep calling tools.
///
/// Admission is taken once, for the whole run, and the workflow runner
/// disables policy gates inside it — so nothing between admission and the
/// node's own dispatch asks the flag again. Without this check a `tool_call`
/// node could send an email or move money after the stop was acknowledged.
#[test]
fn a_stopped_company_refuses_a_node_tool_call_even_on_an_admitted_run() {
    use tinyflows::caps::ToolInvoker;
    let gate = std::sync::Arc::new(crate::policy::gate::ManifestApprovalGate::new(
        crate::company::Policy {
            mode: "full".to_string(),
            always_approve: Vec::new(),
            auto_approve_under_usd: None,
            approval_ttl_hours: None,
        },
    ));
    let invoker = WorkflowToolInvoker {
        tools: HashMap::new(),
        grants: vec!["*".to_string()],
        wiring: WorkflowToolWiring::default(),
        emergency: None,
    }
    .with_emergency_gate(Some(gate.clone()));

    let running = tokio_test_block_on(invoker.invoke("csv_export", json!({}), None));
    assert!(
        matches!(running, Err(EngineError::Capability(ref m)) if m.contains("not available")),
        "not stopped, so the call reaches the tool lookup: {running:?}"
    );

    gate.set_emergency(true);
    let stopped = tokio_test_block_on(invoker.invoke("csv_export", json!({}), None));
    assert!(
        matches!(stopped, Err(EngineError::Capability(ref m)) if m.contains("is stopped")),
        "{stopped:?}"
    );
}

#[test]
fn the_search_namespace_requires_an_explicit_grant_not_a_wildcard() {
    use tinyflows::caps::ToolInvoker;
    // `*` covers ordinary namespaces but must NOT confer the priced `search`
    // family — the invoke-time gate mirrors construction (build.rs).
    let wildcard = WorkflowToolInvoker {
        tools: HashMap::new(),
        grants: vec!["*".to_string()],
        wiring: WorkflowToolWiring::default(),
        emergency: None,
    };
    let denied = tokio_test_block_on(wildcard.invoke("web_search", json!({}), None));
    assert!(
        matches!(denied, Err(EngineError::Capability(ref m)) if m.contains("not granted")),
        "{denied:?}"
    );
    // An explicit `search` grant passes the gate; the empty tool map then
    // fails the lookup with a different, later error.
    let granted = WorkflowToolInvoker {
        tools: HashMap::new(),
        grants: vec!["search".to_string()],
        wiring: WorkflowToolWiring::default(),
        emergency: None,
    };
    let looked_up = tokio_test_block_on(granted.invoke("web_search", json!({}), None));
    assert!(
        matches!(looked_up, Err(EngineError::Capability(ref m)) if m.contains("not available")),
        "{looked_up:?}"
    );
}

/// The replay arm answers a sentinel invocation on an invoker that grants
/// **nothing**, and reaches no capability doing it (issue #846).
///
/// Both halves matter and neither is provable without the other. Answering
/// on a zero-grant invoker is what proves the arm sits ABOVE the fail-closed
/// grant check — if it sat below, a continuation would have to be granted a
/// namespace for a call it does not make. And the same invoker refusing a
/// real slug in the same test is what proves the arm is a narrow sentinel
/// rather than a hole: nothing else got easier to invoke.
#[tokio::test]
async fn the_replay_sentinel_is_answered_without_a_grant_and_reaches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let audit = tempfile::tempdir().unwrap();
    let security = Arc::new(toolbelt::exec_security(
        dir.path(),
        crate::harness::policy::PolicyMode::Supervised,
    ));
    let invoker = WorkflowToolInvoker::new(
        security,
        dir.path(),
        audit.path(),
        Vec::new(),
        // No grants at all: every real slug is refused fail-closed.
        Vec::new(),
        &CapabilityFilter::AllowAll,
        None,
        None,
        test_metering(),
        WorkflowToolWiring::default(),
    );

    let recorded = json!({ "status": 201, "id": "abc" });
    let encoded = serde_json::to_string(&recorded).unwrap();
    let replayed = invoker
        .invoke(
            crate::workflows::replay::REPLAY_SLUG,
            json!({ crate::workflows::replay::REPLAY_RESULT_KEY: encoded }),
            None,
        )
        .await
        .expect("the sentinel is answered from its own arguments");
    assert_eq!(replayed, recorded);

    // The control: the same invoker still refuses an ungranted real tool.
    let refused = invoker
        .invoke("shell", json!({ "command": "id" }), None)
        .await;
    assert!(
        matches!(refused, Err(EngineError::Capability(ref m)) if m.contains("not granted")),
        "{refused:?}"
    );
}

#[test]
fn granted_search_refusals_name_provider_and_capability_failures() {
    let provider = WorkflowToolWiring {
        missing: [("search", MissingReason::SearchBackendNotConfigured)]
            .into_iter()
            .collect(),
        ..WorkflowToolWiring::default()
    };
    let provider_message = refusal_for("web_search", &["search".to_string()], &provider)
        .expect("missing provider refuses");
    assert!(provider_message.contains("no managed search backend"));
    // Both remedies, because either one fixes it: the company can set its
    // own provider without waiting on the platform.
    assert!(provider_message.contains("Settings → Search"));
    assert!(provider_message.contains("ask the platform operator"));

    let tier = WorkflowToolWiring {
        missing: [("search", MissingReason::CapabilityTierFiltered)]
            .into_iter()
            .collect(),
        ..WorkflowToolWiring::default()
    };
    let tier_message =
        refusal_for("web_search", &["search".to_string()], &tier).expect("tier filtering refuses");
    assert!(tier_message.contains("capability tier filtered"));
    assert!(tier_message.contains("raise the capability tier"));
}

#[test]
fn construction_only_initializes_granted_tool_families() {
    let dir = tempfile::tempdir().unwrap();
    // A SEPARATE root from the workspace: the audit sink is host-owned and
    // must never live inside the directory the exec policy sandboxes to
    // (issue #775).
    let audit = tempfile::tempdir().unwrap();
    let security = Arc::new(toolbelt::exec_security(
        dir.path(),
        crate::harness::policy::PolicyMode::Supervised,
    ));

    let none = WorkflowToolInvoker::new(
        security.clone(),
        dir.path(),
        audit.path(),
        Vec::new(),
        Vec::new(),
        &CapabilityFilter::AllowAll,
        None,
        None,
        test_metering(),
        WorkflowToolWiring::default(),
    );
    assert!(none.tools.is_empty());

    let code = WorkflowToolInvoker::new(
        security,
        dir.path(),
        audit.path(),
        Vec::new(),
        vec!["code.*".to_string()],
        &CapabilityFilter::AllowAll,
        None,
        None,
        test_metering(),
        WorkflowToolWiring {
            wired_namespaces: ["code"].into_iter().collect(),
            ..WorkflowToolWiring::default()
        },
    );
    assert!(code.tools.contains_key("apply_patch"));
    assert!(code.tools.contains_key("csv_export"));
    assert!(!code.tools.contains_key("shell"));
    assert!(!code.tools.contains_key("web_fetch"));
}

#[test]
fn search_wires_only_with_an_explicit_grant_and_a_backend() {
    let dir = tempfile::tempdir().unwrap();
    let audit = tempfile::tempdir().unwrap();
    let security = Arc::new(toolbelt::exec_security(
        dir.path(),
        crate::harness::policy::PolicyMode::Supervised,
    ));
    let backend = SearchBackend::new(
        "https://api.example.test".to_string(),
        crate::company::credentials::Credential::from_value("managed"),
        5,
    );

    // Explicit `search` grant + a backend → the metered `web_search` is wired.
    let wired = WorkflowToolInvoker::new(
        security.clone(),
        dir.path(),
        audit.path(),
        Vec::new(),
        vec!["search".to_string()],
        &CapabilityFilter::AllowAll,
        Some(&backend),
        None,
        test_metering(),
        WorkflowToolWiring {
            wired_namespaces: WORKFLOW_TOOL_NAMESPACES.into_iter().collect(),
            missing: BTreeMap::new(),
        },
    );
    assert!(wired.tools.contains_key("web_search"));

    // The catch-all `*` must NOT confer the priced search family.
    let wildcard = WorkflowToolInvoker::new(
        security.clone(),
        dir.path(),
        audit.path(),
        Vec::new(),
        vec!["*".to_string()],
        &CapabilityFilter::AllowAll,
        Some(&backend),
        None,
        test_metering(),
        WorkflowToolWiring {
            wired_namespaces: WORKFLOW_TOOL_NAMESPACES.into_iter().collect(),
            missing: BTreeMap::new(),
        },
    );
    assert!(!wildcard.tools.contains_key("web_search"));

    // Granted but uncredentialed wires nothing (fail-closed) rather than panicking.
    let uncredentialed = WorkflowToolInvoker::new(
        security,
        dir.path(),
        audit.path(),
        Vec::new(),
        vec!["search".to_string()],
        &CapabilityFilter::AllowAll,
        None,
        None,
        test_metering(),
        WorkflowToolWiring {
            wired_namespaces: WORKFLOW_TOOL_NAMESPACES.into_iter().collect(),
            missing: BTreeMap::new(),
        },
    );
    assert!(!uncredentialed.tools.contains_key("web_search"));
}

/// A workflow node searches through the company's own provider when it has
/// one — and reaches it on a deployment with no managed backend at all,
/// which is exactly the self-hosted case the BYO surface exists for.
#[test]
fn a_company_provider_serves_workflow_search_and_replaces_the_managed_tool() {
    let dir = tempfile::tempdir().unwrap();
    let audit = tempfile::tempdir().unwrap();
    let security = Arc::new(toolbelt::exec_security(
        dir.path(),
        crate::harness::policy::PolicyMode::Supervised,
    ));
    let tenant = TenantSearch::for_test("exa", Some("tenant-key"), None);
    let backend = SearchBackend::new(
        "https://api.example.test".to_string(),
        crate::company::credentials::Credential::from_value("managed"),
        5,
    );

    // No managed backend, a company provider → search still works.
    let byo_only = WorkflowToolInvoker::new(
        security.clone(),
        dir.path(),
        audit.path(),
        Vec::new(),
        vec!["search".to_string()],
        &CapabilityFilter::AllowAll,
        None,
        Some(&tenant),
        test_metering(),
        WorkflowToolWiring {
            wired_namespaces: WORKFLOW_TOOL_NAMESPACES.into_iter().collect(),
            missing: BTreeMap::new(),
        },
    );
    assert!(byo_only.tools.contains_key("web_search"));
    assert!(byo_only.tools.contains_key("exa_get_contents"));

    // Both configured → the company's own provider wins, and the node can
    // no longer reach the platform's metered surface at all.
    let both = WorkflowToolInvoker::new(
        security,
        dir.path(),
        audit.path(),
        Vec::new(),
        vec!["search".to_string()],
        &CapabilityFilter::AllowAll,
        Some(&backend),
        Some(&tenant),
        test_metering(),
        WorkflowToolWiring {
            wired_namespaces: WORKFLOW_TOOL_NAMESPACES.into_iter().collect(),
            missing: BTreeMap::new(),
        },
    );
    assert!(both.tools.contains_key("exa_find_similar"));
}

#[test]
fn wiring_namespaces_match_constructed_tool_namespaces() {
    let dir = tempfile::tempdir().unwrap();
    let audit = tempfile::tempdir().unwrap();
    let security = Arc::new(toolbelt::exec_security(
        dir.path(),
        crate::harness::policy::PolicyMode::Supervised,
    ));
    let wiring = WorkflowToolWiring {
        wired_namespaces: WORKFLOW_TOOL_NAMESPACES.into_iter().collect(),
        missing: BTreeMap::new(),
    };
    let invoker = WorkflowToolInvoker::new(
        security,
        dir.path(),
        audit.path(),
        Vec::new(),
        vec!["*".to_string(), "search".to_string()],
        &CapabilityFilter::AllowAll,
        Some(&SearchBackend::new(
            "https://api.example.test".to_string(),
            crate::company::credentials::Credential::from_value("managed"),
            5,
        )),
        None,
        test_metering(),
        wiring.clone(),
    );
    let constructed: BTreeSet<&str> = invoker
        .tools
        .keys()
        .filter_map(|slug| toolbelt::namespace_of(slug))
        .collect();
    assert_eq!(constructed, wiring.wired_namespaces);
}

/// A throwaway [`SearchMetering`] for the construction tests — the tool is
/// never executed here, so the company/agent/meter values are inert.
fn test_metering() -> SearchMetering {
    SearchMetering {
        company: crate::ports::types::CompanyId::new("test"),
        agent: "workflow:test".to_string(),
        meter: None,
    }
}

/// Minimal blocking bridge so the fail-closed checks (which never touch the
/// tool map) can be unit-tested without a full tokio runtime import churn.
fn tokio_test_block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(fut)
}
