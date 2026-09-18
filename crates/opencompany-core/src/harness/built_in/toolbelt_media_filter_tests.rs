use super::toolbelt_test_helpers_tests::*;
use super::*;
use std::collections::HashSet;

#[test]
fn filter_allow_all_is_identity() {
    let ws = Path::new("/tmp/oc-toolbelt-filter");
    let security = test_security(ws, PolicyMode::Supervised);
    let mut tools = shell_tools(
        security.clone(),
        native_runtime(),
        Some(ShellAudit::disabled()),
        ws,
    );
    tools.extend(code_tools(security, ws));
    // Own the names before `tools` moves into the filter.
    let before: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
    let after_tools = filter_by_capabilities(tools, &CapabilityFilter::AllowAll);
    let after: Vec<String> = after_tools.iter().map(|t| t.name().to_string()).collect();
    assert_eq!(before, after, "AllowAll must be identity");
}

/// The `media` toolbelt (issue #109) exposes exactly the three OpenHuman
/// media tools, each mapped to the `media` namespace so the capability filter
/// gates them as one real-money family. Only built under the `media` feature.
#[cfg(feature = "media")]
#[test]
fn media_tools_expose_expected_names_and_namespace() {
    let ws = Path::new("/tmp/oc-toolbelt-media");
    let backend = MediaBackend {
        backend_url: "https://api.tinyhumans.ai".to_string(),
        auth_token: "managed-token".to_string(),
    };
    let tools = media_tools(&backend, ws);
    let got = names(&tools);
    for expected in [
        "media_generate_image",
        "media_generate_video",
        "media_list_models",
    ] {
        assert!(got.contains(&expected), "missing {expected}: {got:?}");
    }
    assert_eq!(got.len(), 3, "media tools drifted: {got:?}");
    for tool in &tools {
        assert_eq!(
            namespace_of(tool.name()),
            Some("media"),
            "media_tools leaked a non-media tool: {}",
            tool.name()
        );
    }
}

/// The managed credential never lands in a `MediaBackend` debug trace.
#[test]
fn media_backend_debug_redacts_the_token() {
    let backend = MediaBackend {
        backend_url: "https://api.tinyhumans.ai".to_string(),
        auth_token: "super-secret".to_string(),
    };
    let shown = format!("{backend:?}");
    assert!(!shown.contains("super-secret"), "token leaked: {shown}");
    assert!(shown.contains("<redacted>"), "{shown}");
    assert!(shown.contains("api.tinyhumans.ai"), "{shown}");
}

/// A backend URL that is not exactly `https` must never reach
/// `IntegrationClient::new`: the client attaches the managed token, so an
/// `http://` override would send it in the clear. Fail closed with no tools.
#[cfg(feature = "media")]
#[test]
fn a_non_https_media_backend_wires_no_tools() {
    let ws = Path::new("/tmp/oc-toolbelt-media-http");
    for url in [
        "http://api.tinyhumans.ai",
        "ftp://api.tinyhumans.ai",
        "not-a-url",
    ] {
        let backend = MediaBackend {
            backend_url: url.to_string(),
            auth_token: "managed-token".to_string(),
        };
        let tools = media_tools(&backend, ws);
        assert!(
            tools.is_empty(),
            "backend `{url}` must not construct any media tool"
        );
    }
}

#[test]
fn filter_deny_drops_mapped_but_keeps_intrinsic() {
    // Mix a real intrinsic tool (`file_read`, unmapped) with mapped exec
    // tools; a full namespace deny must keep only the intrinsic one.
    let ws = Path::new("/tmp/oc-toolbelt-deny");
    let security = test_security(ws, PolicyMode::Supervised);
    let mut tools: Vec<Box<dyn Tool>> = shell_tools(
        security.clone(),
        native_runtime(),
        Some(ShellAudit::disabled()),
        ws,
    );
    tools.extend(code_tools(security.clone(), ws));
    tools.extend(web_tools(security.clone(), Vec::new(), ws));
    // `file_read` has no mapped namespace → intrinsic → always kept.
    tools.push(Box::new(oh::tools::FileReadTool::new(security)));

    let deny: HashSet<&'static str> = ["shell", "code", "web"].into_iter().collect();
    let kept = filter_by_capabilities(tools, &CapabilityFilter::DenyNamespaces(deny));
    let kept_names = names(&kept);
    assert_eq!(
        kept_names,
        vec!["file_read"],
        "only the intrinsic tool must survive a full deny: {kept_names:?}"
    );
}

/// [`namespace_denied`] must agree with [`filter_by_capabilities`] on every
/// case: it is the standalone check `build_agent` uses to keep the sandbox
/// brief from describing a namespace the filter is about to strip from the
/// tool vector, so a mismatch between the two would let the brief and the
/// live belt disagree again — the exact bug this function exists to close.
#[test]
fn namespace_denied_agrees_with_filter_by_capabilities() {
    assert!(!namespace_denied(&CapabilityFilter::AllowAll, "shell"));
    assert!(!namespace_denied(&CapabilityFilter::AllowAll, "code"));

    let deny: HashSet<&'static str> = ["shell"].into_iter().collect();
    let filter = CapabilityFilter::DenyNamespaces(deny);
    assert!(namespace_denied(&filter, "shell"));
    assert!(!namespace_denied(&filter, "code"));

    // A namespace outside `DenyNamespaces`' set is simply not denied — it
    // is never asked to special-case a name it does not recognize.
    assert!(!namespace_denied(&filter, "web"));
}

/// [`native_caps_for_composio_brief`] must narrow the SAME way
/// [`filter_by_capabilities`] narrows the live belt: a namespace the
/// current tier denies must not appear in the list `composio_brief` uses
/// to tell the agent "use your own built-in tool for this instead of
/// Composio" — that tool is about to be stripped from the belt below.
///
/// Before this function existed, `build_agent` passed
/// `native_capabilities_on_belt(&tools)` straight through, unfiltered —
/// `tools` at that point is still the PRE-filter belt, so a denied
/// namespace's tool showed up in the brief even though the belt handed to
/// the model never carried it (PR #1946 follow-up finding).
#[test]
fn native_caps_for_composio_brief_withholds_a_denied_namespace() {
    let ws = Path::new("/tmp/oc-toolbelt-composio-native-caps");
    let security = test_security(ws, PolicyMode::Supervised);
    let tools = shell_tools(security, native_runtime(), Some(ShellAudit::disabled()), ws);

    // No tier denial: the belt's native `shell` namespace passes through.
    let admitted = native_caps_for_composio_brief(&tools, &CapabilityFilter::AllowAll);
    assert!(
        admitted.contains(&"shell"),
        "AllowAll must not withhold a namespace the belt actually has: {admitted:?}"
    );

    // Tier denies `shell` (budget exhausted / fail-closed metering, the
    // same condition that makes `filter_by_capabilities` drop every
    // `shell` tool from `tools` a few lines below in `build_agent`): the
    // brief must not credit the agent with a built-in `shell` tool either.
    let deny: HashSet<&'static str> = ["shell"].into_iter().collect();
    let filter = CapabilityFilter::DenyNamespaces(deny);
    let withheld = native_caps_for_composio_brief(&tools, &filter);
    assert!(
        !withheld.contains(&"shell"),
        "a denied namespace must not appear in the composio brief's native caps: {withheld:?}"
    );
}

/// The grant/credential resolving `wired = true` is not enough: a
/// capability tier that has denied `composio` (budget exhausted, or a
/// fail-closed metering error) must still turn the predicate off, because
/// `filter_by_capabilities` is about to strip every `composio_*` tool from
/// the belt. This is the fix for the P1 codex found on PR #1780 — before
/// it, the S1 brief and S2 deflection policy were wired from the grant
/// alone, exactly the shape `sandbox_brief_flags_withhold_a_capability_denied_namespace`
/// (PR #1670) fixed for `shell`/`code`.
#[test]
fn composio_capability_admits_withholds_when_the_tier_denies_it() {
    assert!(
        composio_capability_admits(true, &CapabilityFilter::AllowAll),
        "wired + no denial must admit"
    );
    assert!(
        !composio_capability_admits(false, &CapabilityFilter::AllowAll),
        "not wired must never admit, regardless of the tier"
    );

    let deny_composio = CapabilityFilter::DenyNamespaces(["composio"].into_iter().collect());
    assert!(
        !composio_capability_admits(true, &deny_composio),
        "wired but denied must not admit — the brief/policy must not \
         describe a surface `filter_by_capabilities` is about to strip"
    );

    // A denial of an unrelated namespace must not withhold composio.
    let deny_shell = CapabilityFilter::DenyNamespaces(["shell"].into_iter().collect());
    assert!(
        composio_capability_admits(true, &deny_shell),
        "a denial of another namespace must not withhold composio"
    );
}
