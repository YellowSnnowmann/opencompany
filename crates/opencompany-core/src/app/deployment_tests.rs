use super::*;
use crate::app::config::MapEnv;

/// The load-bearing default. Everything about the analytics posture rests on
/// an unconfigured process being self-hosted, so this is pinned rather than
/// left to `#[derive(Default)]` being read correctly by the next person.
#[test]
fn an_undeclared_deployment_is_self_hosted() {
    assert_eq!(
        Deployment::from_env(&MapEnv::default()),
        Deployment::SelfHosted
    );
    assert_eq!(Deployment::default(), Deployment::SelfHosted);
}

#[test]
fn a_declaration_wins_over_the_tenant_inference() {
    let env = MapEnv::new([
        ("OPENCOMPANY_DEPLOYMENT", "desktop"),
        ("OPENCOMPANY_TENANT_ID", "acme"),
    ]);
    assert_eq!(Deployment::from_env(&env), Deployment::Desktop);
}

#[test]
fn a_tenant_namespace_names_a_hosted_tenant() {
    let env = MapEnv::new([("OPENCOMPANY_TENANT_ID", "acme")]);
    assert_eq!(Deployment::from_env(&env), Deployment::HostedTenant);
}

/// A **whitespace-only** declaration is absent, not a declaration.
///
/// `"\n"` is not empty, so the length filter alone let it through to
/// `parse`, which trims it to `""`, matches no arm and answers
/// `SelfHosted` — costing a hosted tenant the inference it would otherwise
/// have had. A launcher that mounts this variable from a file hands it over
/// with a trailing newline more often than not, so this is the ordinary
/// shape rather than a contrived one, and `analytics::config::resolve`
/// already trims before deciding — the two readers disagreed about the same
/// input.
#[test]
fn a_whitespace_only_declaration_is_absent() {
    for blank in ["\n", " ", "\t\n ", "\r\n"] {
        assert_eq!(
            Deployment::from_env(&MapEnv::new([(DEPLOYMENT_ENV, blank)])),
            Deployment::SelfHosted,
            "with nothing else to go on: {blank:?}"
        );
        // The point of the fix: the tenant inference survives it.
        assert_eq!(
            Deployment::from_env(&MapEnv::new([
                (DEPLOYMENT_ENV, blank),
                ("OPENCOMPANY_TENANT_ID", "acme"),
            ])),
            Deployment::HostedTenant,
            "a blank declaration must not outrank the tenant namespace: {blank:?}"
        );
    }
    // A real declaration still wins, padding and all.
    assert_eq!(
        Deployment::from_env(&MapEnv::new([
            (DEPLOYMENT_ENV, " desktop\n"),
            ("OPENCOMPANY_TENANT_ID", "acme"),
        ])),
        Deployment::Desktop,
    );
}

/// A typo must fall to silence, never to reporting. The dangerous direction
/// is the only one worth a test.
///
/// `OPENCOMPANY_TENANT_ID` is set on purpose: the interesting question is
/// not whether `parse` maps an unknown slug to `SelfHosted` — it plainly
/// does — but whether an unrecognised declaration **wins over** the tenant
/// inference rather than falling through to it. Without the tenant variable
/// this test passes either way and proves nothing about the fall-through,
/// which is the shape the non-Unicode leak below actually had.
#[test]
fn an_unrecognised_declaration_falls_back_to_silence() {
    for typo in [
        "hosted-tenat",
        "hosted-tennant",
        "hosted tenant",
        "Hosted-Tenent",
    ] {
        let env = MapEnv::new([
            ("OPENCOMPANY_DEPLOYMENT", typo),
            ("OPENCOMPANY_TENANT_ID", "acme"),
        ]);
        assert_eq!(
            Deployment::from_env(&env),
            Deployment::SelfHosted,
            "{typo:?} must not fall through to the tenant inference"
        );
    }
}

/// **A declaration that is set but is not text fails closed too.**
///
/// `EnvSource::get` maps a non-Unicode value to `None`, so reading through
/// it treated `OPENCOMPANY_DEPLOYMENT=<invalid bytes>` as an absent
/// declaration, fell through to the tenant inference, and returned
/// `HostedTenant` — turning reporting **on** for an install whose operator
/// had explicitly declared something. The same leak as the unrecognised
/// spelling above, by a different route, and the one direction of failure
/// this discriminator must never take.
#[cfg(unix)]
#[test]
fn a_non_unicode_declaration_falls_closed_to_self_hosted() {
    use crate::app::config::EnvSource;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    struct NonUnicodeDeclaration;
    impl EnvSource for NonUnicodeDeclaration {
        fn get_os(&self, key: &str) -> Option<OsString> {
            match key {
                // `0xff 0xfe` is not valid UTF-8 in any position; the tail
                // spells `hosted-tenant`, so a lossy read would have said
                // the operator asked for a hosted tenant.
                DEPLOYMENT_ENV => Some(OsString::from_vec(
                    [&[0xff, 0xfe][..], b"hosted-tenant"].concat(),
                )),
                TENANT_ENV => Some(OsString::from("acme")),
                _ => None,
            }
        }
    }

    // The premise: this really is a value `get` cannot see at all, and the
    // tenant inference really is standing by to answer for it.
    assert_eq!(NonUnicodeDeclaration.get(DEPLOYMENT_ENV), None);
    assert!(NonUnicodeDeclaration.get_os(DEPLOYMENT_ENV).is_some());
    assert_eq!(
        Deployment::from_env(&MapEnv::new([("OPENCOMPANY_TENANT_ID", "acme")])),
        Deployment::HostedTenant,
        "without the declaration this environment resolves to a hosted tenant, \
         so the assertion below is about the declaration and nothing else"
    );

    assert_eq!(
        Deployment::from_env(&NonUnicodeDeclaration),
        Deployment::SelfHosted,
        "a declaration set to bytes this process cannot read must not read as unset"
    );
}

/// The control for the two above: a **blank** declaration is an absent one,
/// not an unreadable one — consistent with `EnvSource::get`, with the
/// analytics switch, and with the rest of the tree. A launcher that
/// exported `OPENCOMPANY_DEPLOYMENT=` has said nothing, and must not cost a
/// hosted tenant its inference. Without this control, "everything is
/// self-hosted now" would pass the two tests above just as well.
#[test]
fn a_blank_declaration_is_treated_as_absent() {
    let env = MapEnv::new([
        ("OPENCOMPANY_DEPLOYMENT", ""),
        ("OPENCOMPANY_TENANT_ID", "acme"),
    ]);
    assert_eq!(Deployment::from_env(&env), Deployment::HostedTenant);
}

/// And the other control: a **recognised** declaration still resolves to
/// the kind it names, so the fail-closed paths above are finding malformed
/// values rather than refusing every declaration.
#[test]
fn a_recognised_declaration_still_resolves() {
    for (declared, expected) in [
        ("hosted-tenant", Deployment::HostedTenant),
        ("  Hosted-Tenant\n", Deployment::HostedTenant),
        ("hosted", Deployment::HostedTenant),
        ("desktop", Deployment::Desktop),
        ("self-hosted", Deployment::SelfHosted),
    ] {
        let env = MapEnv::new([("OPENCOMPANY_DEPLOYMENT", declared)]);
        assert_eq!(Deployment::from_env(&env), expected, "{declared:?}");
    }
}

#[test]
fn every_kind_has_a_stable_slug() {
    assert_eq!(Deployment::Desktop.as_str(), "desktop");
    assert_eq!(Deployment::SelfHosted.as_str(), "self-hosted");
    assert_eq!(Deployment::HostedTenant.as_str(), "hosted-tenant");
}
