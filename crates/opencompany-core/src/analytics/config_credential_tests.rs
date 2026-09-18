//! Credential-presence and reporting-toggle tests, split out of
//! `config_tests.rs` (topic split, >750 lines).

use super::*;
use crate::app::config::MapEnv;

/// A collector address that resolves nowhere. Every reporting test needs
/// one now: there is no default endpoint left to fall back to.
const TEST_ENDPOINT: &str = "https://collector.invalid/track";

/// A fully configured reporting environment, which `pairs` then overrides.
///
/// It takes three variables where it used to take one, and that is the
/// shape of the change: an OpenPanel deployment configures a client id, a
/// client secret and the address of the collector it self-hosts.
fn configured(pairs: &[(&str, &str)]) -> MapEnv {
    let mut all = vec![
        (CLIENT_ID_ENV, "not-a-real-client-id"),
        (CLIENT_SECRET_ENV, "not-a-real-client-secret"),
        (ENDPOINT_ENV, TEST_ENDPOINT),
    ];
    all.extend_from_slice(pairs);
    MapEnv::new(all)
}

/// **The decision the GPL posture rests on.** A self-hosted instance that
/// has been handed a working credential — which is the easiest way to get
/// this wrong, because a credential looks like consent — still sends
/// nothing.
#[test]
fn a_self_hosted_instance_is_silent_even_with_a_credential() {
    assert_eq!(
        resolve(Deployment::SelfHosted, &configured(&[])),
        Decision::Silent(Silence::NotHosted)
    );
}

#[test]
fn a_desktop_instance_is_silent_even_with_a_credential() {
    assert_eq!(
        resolve(Deployment::Desktop, &configured(&[])),
        Decision::Silent(Silence::NotHosted)
    );
}

#[test]
fn a_hosted_tenant_with_a_credential_reports() {
    let decision = resolve(Deployment::HostedTenant, &configured(&[]));
    assert!(decision.reports(), "{decision:?}");
    match decision {
        Decision::Report {
            endpoint,
            credentials,
        } => {
            assert_eq!(endpoint, TEST_ENDPOINT);
            assert_eq!(credentials.expose_id(), "not-a-real-client-id");
            assert_eq!(credentials.expose_secret(), "not-a-real-client-secret");
        }
        other => panic!("{other:?}"),
    }
}

/// A hosted tenant with nothing configured is misconfigured, not reporting
/// to nowhere — and the reason says so.
#[test]
fn a_hosted_tenant_without_a_credential_is_silent() {
    assert_eq!(
        resolve(Deployment::HostedTenant, &MapEnv::default()),
        Decision::Silent(Silence::NoCredentials)
    );
}

/// **Half a credential is a misconfiguration, and the reason names the half
/// that is missing.**
///
/// OpenPanel authenticates a write client with an id *and* a secret, so
/// there is no useful state in between. This is the shape a half-finished
/// secret rollout has — the id is in the manifest, the secret is still in
/// the vault — and telling that operator "no credential is configured"
/// while `OPENCOMPANY_ANALYTICS_CLIENT_ID` is plainly set in their env file
/// sends them to look at the wrong variable.
#[test]
fn half_a_credential_says_which_half_is_missing() {
    let only_id = MapEnv::new([
        (CLIENT_ID_ENV, "not-a-real-client-id"),
        (ENDPOINT_ENV, TEST_ENDPOINT),
    ]);
    assert_eq!(
        resolve(Deployment::HostedTenant, &only_id),
        Decision::Silent(Silence::NoClientSecret)
    );
    assert!(
        Silence::NoClientSecret
            .as_str()
            .contains("OPENCOMPANY_ANALYTICS_CLIENT_SECRET"),
        "the reason must name the variable to set: {}",
        Silence::NoClientSecret.as_str()
    );

    let only_secret = MapEnv::new([
        (CLIENT_SECRET_ENV, "not-a-real-client-secret"),
        (ENDPOINT_ENV, TEST_ENDPOINT),
    ]);
    assert_eq!(
        resolve(Deployment::HostedTenant, &only_secret),
        Decision::Silent(Silence::NoClientId)
    );
    assert!(
        Silence::NoClientId
            .as_str()
            .contains("OPENCOMPANY_ANALYTICS_CLIENT_ID"),
        "the reason must name the variable to set: {}",
        Silence::NoClientId.as_str()
    );
}

/// Blank is absent for both halves, and for the same reason it is for the
/// switch: a secret mounted from a file arrives with a trailing newline
/// more often than not, and a launcher that exports an empty variable has
/// configured nothing.
#[test]
fn a_blank_half_is_no_half() {
    for blank in ["   ", "\n", "\t\n "] {
        assert_eq!(
            resolve(
                Deployment::HostedTenant,
                &configured(&[(CLIENT_SECRET_ENV, blank)])
            ),
            Decision::Silent(Silence::NoClientSecret),
            "a secret of {blank:?} must not read as configured"
        );
        assert_eq!(
            resolve(
                Deployment::HostedTenant,
                &configured(&[(CLIENT_ID_ENV, blank)])
            ),
            Decision::Silent(Silence::NoClientId),
            "an id of {blank:?} must not read as configured"
        );
    }
}

/// And a credential that merely *arrived* with surrounding whitespace is
/// used, trimmed, rather than put into a header with a newline in it — which
/// `reqwest` rejects outright when it builds the request.
#[test]
fn a_credential_is_trimmed() {
    match resolve(
        Deployment::HostedTenant,
        &configured(&[
            (CLIENT_ID_ENV, "  not-a-real-client-id\n"),
            (CLIENT_SECRET_ENV, "\tnot-a-real-client-secret\n"),
        ]),
    ) {
        Decision::Report { credentials, .. } => {
            assert_eq!(credentials.expose_id(), "not-a-real-client-id");
            assert_eq!(credentials.expose_secret(), "not-a-real-client-secret");
        }
        other => panic!("{other:?}"),
    }
}

/// **A credential that cannot go in a header is silence with a reason.**
///
/// This is new with OpenPanel and is a consequence of where the credential
/// now travels. Mixpanel's token rode in the JSON body, where any string is
/// legal, so a mangled one was simply refused by the collector. These two
/// ride in `openpanel-client-id` / `openpanel-client-secret` headers, and
/// `reqwest` refuses to *build* a request whose header value holds a control
/// byte — so a secret with an embedded newline (`kubectl create secret` over
/// a wrapped file is the usual way one arrives) would install a tracker that
/// never constructs a single request, forever, behind a `debug!` nobody has
/// enabled. Trimming does not save it: the newline is in the middle.
#[test]
fn a_credential_that_cannot_go_in_a_header_is_silence() {
    for mangled in [
        "not-a-real\nclient-secret",
        "not-a-real\rclient-secret",
        "not a real client secret",
        "not-a-real-client-secret\u{0}",
        "not-a-r\u{e9}al-client-secret",
    ] {
        assert_eq!(
            resolve(
                Deployment::HostedTenant,
                &configured(&[(CLIENT_SECRET_ENV, mangled)])
            ),
            Decision::Silent(Silence::UnusableCredential),
            "a secret of {mangled:?} must not resolve to a report that cannot be built"
        );
        assert_eq!(
            resolve(
                Deployment::HostedTenant,
                &configured(&[(CLIENT_ID_ENV, mangled)])
            ),
            Decision::Silent(Silence::UnusableCredential),
            "an id of {mangled:?} must not resolve to a report that cannot be built"
        );
    }

    // The control, without which "reject everything" would pass: the shapes
    // an OpenPanel client actually has still report. Opaque generated
    // tokens — hex, base64url, a uuid, a prefixed key.
    for real_shaped in [
        "0f8b1c2d3e4f5a6b7c8d9e0f1a2b3c4d",
        "op_sk_9zQx-4Kd_7Yb2Lp0",
        "550e8400-e29b-41d4-a716-446655440000",
        "YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXo=",
    ] {
        assert!(
            resolve(
                Deployment::HostedTenant,
                &configured(&[
                    (CLIENT_ID_ENV, real_shaped),
                    (CLIENT_SECRET_ENV, real_shaped)
                ])
            )
            .reports(),
            "{real_shaped:?} is the shape a real credential has and must still report"
        );
    }
}

/// And the reason never quotes the credential it rejected, for the same
/// reason the endpoint reason does not quote the endpoint.
#[test]
fn the_unusable_credential_reason_never_quotes_the_credential() {
    let reason = Silence::UnusableCredential.as_str();
    let printed = format!("{:?} {reason}", Silence::UnusableCredential);
    assert!(
        !printed.to_ascii_lowercase().contains("not-a-real"),
        "the reason leaked the credential: {printed}"
    );
    assert!(
        reason.contains("header"),
        "the reason must say what is wrong with it: {reason}"
    );
}

/// **There is no default endpoint, and an absent one is silence with its
/// own reason.**
///
/// This replaced `https://api.mixpanel.com/track`, and dropping the default
/// rather than re-pointing it is the deliberate half of that. OpenPanel is
/// self-hosted: its address is whatever the operator runs it at, and any
/// address this crate picked would be somebody else's collector. A tenant
/// that configured a credential but no endpoint would then have shipped its
/// telemetry to a third party nobody named — which is the accident
/// `Silence::UnusableEndpoint` already refuses to make from the other
/// direction.
#[test]
fn an_absent_endpoint_is_silence_rather_than_a_default() {
    let decision = resolve(
        Deployment::HostedTenant,
        &MapEnv::new([
            (CLIENT_ID_ENV, "not-a-real-client-id"),
            (CLIENT_SECRET_ENV, "not-a-real-client-secret"),
        ]),
    );
    assert_eq!(decision, Decision::Silent(Silence::NoEndpoint));
    assert!(!decision.reports());
    assert!(
        Silence::NoEndpoint
            .as_str()
            .contains("OPENCOMPANY_ANALYTICS_ENDPOINT"),
        "the reason must name the variable to set: {}",
        Silence::NoEndpoint.as_str()
    );
}

/// A blank endpoint is an absent one, not a broken one: a launcher that
/// exports an empty variable has configured nothing, and the reason it gets
/// should send it to set the variable rather than to fix its value.
#[test]
fn a_blank_endpoint_is_absent_rather_than_unusable() {
    assert_eq!(
        resolve(
            Deployment::HostedTenant,
            &configured(&[(ENDPOINT_ENV, "  \n")])
        ),
        Decision::Silent(Silence::NoEndpoint)
    );
}

/// `off` outranks the deployment kind. The platform can switch a tenant off
/// without rebuilding it.
#[test]
fn off_outranks_a_hosted_deployment() {
    assert_eq!(
        resolve(
            Deployment::HostedTenant,
            &configured(&[(ENABLE_ENV, "off")])
        ),
        Decision::Silent(Silence::OptedOut)
    );
}

/// The self-hoster's opt-in, which is the only way a non-hosted install ever
/// reports.
#[test]
fn a_self_hoster_can_opt_in() {
    assert!(resolve(Deployment::SelfHosted, &configured(&[(ENABLE_ENV, "on")])).reports());
}

/// A typo must not opt anybody in.
#[test]
fn a_misspelled_switch_does_not_opt_in() {
    assert_eq!(
        resolve(Deployment::SelfHosted, &configured(&[(ENABLE_ENV, "onn")])),
        Decision::Silent(Silence::Unreadable)
    );
}

/// **And a typo must not fail to opt anybody out.** This is the direction
/// that used to leak: an unreadable value fell through to the deployment
/// default, so a hosted tenant whose operator meant `off` and typed `of`
/// carried on reporting, with a boot line that said "reporting to …" and
/// gave them no reason to look again.
#[test]
fn a_misspelled_opt_out_does_not_keep_a_hosted_tenant_reporting() {
    for typo in ["of", "offf", "disabled", "0.0", "nope"] {
        let decision = resolve(Deployment::HostedTenant, &configured(&[(ENABLE_ENV, typo)]));
        assert_eq!(
            decision,
            Decision::Silent(Silence::Unreadable),
            "{typo:?} must not leave a hosted tenant reporting"
        );
        assert!(!decision.reports(), "{typo:?}");
    }
}

/// **A switch that is set but is not text fails closed too.**
///
/// `EnvSource::get` maps a non-Unicode value to `None`, so reading through
/// it would have treated `OPENCOMPANY_ANALYTICS=<invalid bytes>` as an
/// absent switch and left a hosted tenant reporting — the same leak as the
/// unreadable spelling, by a different route.
#[cfg(unix)]
#[test]
fn a_non_unicode_switch_is_unreadable_rather_than_absent() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    struct NonUnicodeSwitch;
    impl EnvSource for NonUnicodeSwitch {
        fn get_os(&self, key: &str) -> Option<OsString> {
            match key {
                ENABLE_ENV => Some(OsString::from_vec(vec![0xff, 0xfe, 0x6f, 0x6e])),
                CLIENT_ID_ENV => Some(OsString::from("not-a-real-client-id")),
                CLIENT_SECRET_ENV => Some(OsString::from("not-a-real-client-secret")),
                ENDPOINT_ENV => Some(OsString::from(TEST_ENDPOINT)),
                _ => None,
            }
        }
    }

    // The premise: this really is a value `get` cannot see at all.
    assert_eq!(NonUnicodeSwitch.get(ENABLE_ENV), None);
    assert!(NonUnicodeSwitch.get_os(ENABLE_ENV).is_some());

    assert_eq!(
        resolve(Deployment::HostedTenant, &NonUnicodeSwitch),
        Decision::Silent(Silence::Unreadable),
        "a switch set to bytes this process cannot read must not read as unset"
    );
}

/// The near-miss control: `off` really is matched case-insensitively and
/// after trimming, so the test above is finding typos rather than finding
/// every value that is not lowercase and bare.
#[test]
fn an_off_switch_is_trimmed_and_case_folded() {
    assert_eq!(
        resolve(
            Deployment::HostedTenant,
            &configured(&[(ENABLE_ENV, "  ofF\n")])
        ),
        Decision::Silent(Silence::OptedOut)
    );
}

/// The control for the two above: an **absent** switch still falls to the
/// deployment default, in both directions. Without this, "everything is
/// silent now" would pass the tests above just as well.
#[test]
fn an_absent_switch_still_falls_to_the_deployment_default() {
    assert!(resolve(Deployment::HostedTenant, &configured(&[])).reports());
    assert_eq!(
        resolve(Deployment::SelfHosted, &configured(&[])),
        Decision::Silent(Silence::NotHosted)
    );
}

/// A whitespace-only switch is an absent switch, not an unreadable one —
/// consistent with the credential and endpoint, and it must not flip a
/// hosted tenant into silence just because a launcher exported an empty
/// variable.
#[test]
fn a_blank_switch_is_treated_as_absent() {
    assert!(
        resolve(
            Deployment::HostedTenant,
            &configured(&[(ENABLE_ENV, "   ")])
        )
        .reports(),
        "a blank switch must not read as unreadable"
    );
}
