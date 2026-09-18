use super::*;

#[test]
fn debug_never_renders_the_api_key() {
    let hosting = TenantHosting {
        provider: "vercel".to_string(),
        api_key: "vc_live_SUPERSECRET".to_string(),
        team: Some("team_abc".to_string()),
    };

    let rendered = format!("{hosting:?}");

    assert!(!rendered.contains("SUPERSECRET"), "{rendered}");
    assert!(rendered.contains("<redacted>"), "{rendered}");
    assert!(rendered.contains("team_abc"), "{rendered}");
}

#[test]
fn the_fingerprint_moves_when_any_half_of_the_credential_does() {
    let base = TenantHosting {
        provider: "vercel".to_string(),
        api_key: "one".to_string(),
        team: None,
    };
    let rotated = TenantHosting {
        api_key: "two".to_string(),
        ..base.clone()
    };
    let scoped = TenantHosting {
        team: Some("team_abc".to_string()),
        ..base.clone()
    };

    let of = |value: TenantHosting| TenantHosting::fingerprint(&Some(value));

    assert_ne!(of(base.clone()), TenantHosting::fingerprint(&None));
    // A rotated key with the same provider must rebuild the roster.
    assert_ne!(of(base.clone()), of(rotated));
    assert_ne!(of(base.clone()), of(scoped));
    assert_eq!(of(base.clone()), of(base));
}

#[test]
fn a_connection_reports_its_provider_and_team() {
    let hosting = TenantHosting {
        provider: "vercel".to_string(),
        api_key: "key".to_string(),
        team: Some("team_abc".to_string()),
    };

    assert_eq!(hosting.provider(), "vercel");
    assert_eq!(hosting.team(), Some("team_abc"));
}

/// **Every tool this bridge wires is classified in
/// [`crate::policy::consequence`].** Issue #913 is the reason this exists.
///
/// `every_registered_tool_is_declared` in [`crate::harness::built_in`] is
/// the general version of this check and it cannot see these tools: it
/// enumerates a belt built by `build_agent`, and hosting is wired only when
/// a company has a *stored credential*, which no unit-test company has. So
/// the hosting belt was never covered by construction, and the standing
/// check was a hardcoded list of six names in `consequence.rs`.
///
/// A hardcoded list fails when a row is deleted. It cannot fail when the
/// vendor pin adds a tool — and [`hosting_tools`] returns
/// `Account::tools()` wholesale, so a pin bump wires new tools onto live
/// agents with nothing in this repository mentioning them. That is what
/// happened: `hosting_rollback`, `hosting_list_deployments` and
/// `hosting_domain_status` went live undeclared. Undeclared is not inert —
/// the two reads parked (an approval for a read) and `hosting_rollback`
/// classified as `Other` rather than `Publish`, which is issue #1079's
/// defect returning through the pin instead of through an edit.
///
/// Enumerating the belt is the fix, because it is the only form of this
/// check a pin bump cannot outrun.
///
/// `Account::connect` builds a client and does no I/O, so the dummy
/// credential below never leaves the process.
/// The doc comment on [`hosting_tools`] claims an unusable stored
/// credential "wires nothing and warns rather than failing the build" —
/// `Account::connect` returning `Err` must not leave a partial belt or
/// bubble a panic, it must fail closed to an empty `Vec`. `from_str` on an
/// unrecognised provider slug fails before any I/O, so this needs no
/// network and no vendor client.
#[cfg(feature = "openhuman")]
#[test]
fn an_unusable_stored_credential_wires_no_hosting_tools() {
    let config = TenantHosting {
        provider: "not-a-real-hosting-provider".to_string(),
        api_key: "whatever".to_string(),
        team: None,
    };
    let wired = hosting_tools(&config, std::path::PathBuf::from("/tmp"));
    assert!(
        wired.is_empty(),
        "an unrecognised provider must wire zero hosting tools, got: {:?}",
        wired.iter().map(|t| t.name()).collect::<Vec<_>>()
    );
}

#[cfg(feature = "openhuman")]
#[test]
fn every_wired_hosting_tool_is_declared() {
    let declared: std::collections::BTreeSet<&str> =
        crate::policy::consequence::declared_tools().collect();

    let config = TenantHosting {
        provider: DEFAULT_PROVIDER.to_string(),
        api_key: "not-a-real-key".to_string(),
        team: None,
    };
    let wired: Vec<String> = hosting_tools(&config, std::path::PathBuf::from("/tmp"))
        .iter()
        .map(|tool| tool.name().to_string())
        .collect();

    // A vacuity guard: `hosting_tools` warns and returns an empty vec when
    // the credential is unusable, and an empty belt would pass the check
    // below while proving nothing.
    assert!(
        wired.contains(&"hosting_launch_site".to_string()),
        "the hosting belt built no tools, so this check proves nothing: {wired:?}"
    );

    let undeclared: Vec<&String> = wired
        .iter()
        .filter(|name| !declared.contains(name.as_str()))
        .collect();
    assert!(
        undeclared.is_empty(),
        "these hosting tools came in with the vendor pin and are wired onto live              agents, but nobody has said what they can reach — so the gate is guessing              from their names, and a `hosting_` prefix defeats the read heuristic:              {undeclared:?}. Add them to `crate::policy::consequence::DECLARED`."
    );
}
