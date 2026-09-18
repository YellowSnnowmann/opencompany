//! SSRF-guard and IO-helper tests: which endpoints and redirect targets
//! a probe may reach, and the pure helpers behind it (split out of
//! `probe_tests.rs`).

use super::*;

// ---- the SSRF guard -----------------------------------------------------

pub(super) const LOCAL_OFFERED: ProbePolicy = ProbePolicy {
    allow_loopback: true,
};
const SERVER_SIDE: ProbePolicy = ProbePolicy {
    allow_loopback: false,
};

#[test]
fn an_ordinary_endpoint_is_allowed() {
    assert_eq!(
        check_endpoint("https://api.openai.com/v1", SERVER_SIDE),
        Ok(())
    );
    assert_eq!(check_endpoint("https://8.8.8.8/v1", SERVER_SIDE), Ok(()));
}

#[test]
fn only_http_and_https_are_probeable() {
    assert_eq!(
        check_endpoint("file:///etc/passwd", SERVER_SIDE),
        Err(EndpointRefusal::Scheme)
    );
    assert_eq!(
        check_endpoint("gopher://acme.test/v1", SERVER_SIDE),
        Err(EndpointRefusal::Scheme)
    );
    assert_eq!(
        check_endpoint("api.openai.com/v1", SERVER_SIDE),
        Err(EndpointRefusal::Unparseable)
    );
}

#[test]
fn the_cloud_metadata_address_is_refused_wherever_it_is_offered() {
    // 169.254.169.254 is where a container's credentials live. There is no
    // deployment on which a company's model endpoint is there.
    for policy in [LOCAL_OFFERED, SERVER_SIDE] {
        assert_eq!(
            check_endpoint("http://169.254.169.254/latest/meta-data/", policy),
            Err(EndpointRefusal::LinkLocal)
        );
    }
}

#[test]
fn link_local_is_refused_in_both_address_families() {
    assert_eq!(
        check_endpoint("http://169.254.1.1/v1", LOCAL_OFFERED),
        Err(EndpointRefusal::LinkLocal)
    );
    assert_eq!(
        check_endpoint("http://[fe80::1]/v1", LOCAL_OFFERED),
        Err(EndpointRefusal::LinkLocal)
    );
}

#[test]
fn an_ipv4_mapped_ipv6_address_gets_the_ipv4_answer() {
    // Checking only the v6 shape is how `::ffff:169.254.169.254` reaches a
    // metadata service through a guard that looks like it works.
    assert_eq!(
        check_endpoint("http://[::ffff:169.254.169.254]/v1", LOCAL_OFFERED),
        Err(EndpointRefusal::LinkLocal)
    );
    assert_eq!(
        check_endpoint("http://[::ffff:127.0.0.1]:11434/v1", SERVER_SIDE),
        Err(EndpointRefusal::Loopback)
    );
}

#[test]
fn loopback_is_an_explicit_allowance_not_a_hole() {
    // Allowed only where the local-runtime category is offered, because
    // that is exactly what Ollama needs.
    assert_eq!(
        check_endpoint("http://127.0.0.1:11434/v1", LOCAL_OFFERED),
        Ok(())
    );
    assert_eq!(
        check_endpoint("http://[::1]:11434/v1", LOCAL_OFFERED),
        Ok(())
    );
    assert_eq!(
        check_endpoint("http://127.0.0.1:11434/v1", SERVER_SIDE),
        Err(EndpointRefusal::Loopback)
    );
}

#[test]
fn private_and_carrier_grade_ranges_are_refused() {
    for addr in [
        "http://10.0.0.5/v1",
        "http://192.168.1.10/v1",
        "http://172.16.0.1/v1",
        "http://100.64.0.1/v1",
        "http://0.0.0.0/v1",
    ] {
        assert_eq!(
            check_endpoint(addr, LOCAL_OFFERED),
            Err(EndpointRefusal::PrivateNetwork),
            "{addr}"
        );
    }
    assert_eq!(
        check_endpoint("http://[fc00::1]/v1", LOCAL_OFFERED),
        Err(EndpointRefusal::PrivateNetwork)
    );
}

#[test]
fn a_key_is_never_sent_to_an_http_endpoint_off_this_host() {
    // `http` stays in the allowed set because the local-runtime category
    // needs it and there is no certificate to have at `localhost`. What is
    // refused is a **credential** leaving this host in the clear.
    assert_eq!(
        check_endpoint_with_credential("http://gateway.acme.test/v1", SERVER_SIDE, true),
        Err(EndpointRefusal::Cleartext)
    );
    // Without one there is nothing to leak, and this is a real shape: a
    // keyless gateway on an intranet.
    assert_eq!(
        check_endpoint_with_credential("http://gateway.acme.test/v1", SERVER_SIDE, false),
        Ok(())
    );
    // https is the point of the rule, not a coincidence of it.
    assert_eq!(
        check_endpoint_with_credential("https://gateway.acme.test/v1", SERVER_SIDE, true),
        Ok(())
    );
    // Loopback never leaves the host — by name, which is what Ollama's own
    // documentation prints, as well as by literal.
    for local in [
        "http://localhost:11434/v1",
        "http://ollama.localhost:11434/v1",
        "http://127.0.0.1:11434/v1",
        "http://[::1]:11434/v1",
        "http://[::ffff:127.0.0.1]:11434/v1",
    ] {
        assert_eq!(
            check_endpoint_with_credential(local, LOCAL_OFFERED, true),
            Ok(()),
            "{local} is this host"
        );
    }
    // And the address rules still run first: a credentialed https probe at
    // the metadata address is refused as link-local, not waved through.
    assert_eq!(
        check_endpoint_with_credential("https://169.254.169.254/v1", SERVER_SIDE, true),
        Err(EndpointRefusal::LinkLocal)
    );
}

#[test]
fn a_credentialed_request_does_not_follow_a_redirect_off_its_origin() {
    // `reqwest` strips `Authorization` when the host changes and keeps a
    // custom header, and the catalogue's one non-bearer entry sends the key
    // as `x-api-key` — so a provider that can answer `302` could name any
    // host to hand it to.
    let origin = "https://api.acme.test/v1/models";
    assert!(same_origin(origin, "https://api.acme.test/v2/models"));
    assert!(same_origin(
        origin,
        "https://API.ACME.TEST/v1/models?page=2"
    ));
    assert!(!same_origin(origin, "https://elsewhere.test/v1/models"));
    // Scheme and port are part of an origin, both ways.
    assert!(!same_origin(origin, "http://api.acme.test/v1/models"));
    assert!(!same_origin(origin, "https://api.acme.test:8443/v1/models"));
    // Unparseable is not a match: refusing costs a catalogue read, and
    // following costs the key.
    assert!(!same_origin(origin, "api.acme.test/v1/models"));
}

#[test]
fn a_redirect_target_gets_the_same_answer_as_the_first_hop() {
    // A permitted host that redirects to the metadata address is the whole
    // trick, so the address half is public for the redirect check to reuse.
    assert_eq!(check_endpoint("https://acme.test/v1", SERVER_SIDE), Ok(()));
    assert_eq!(
        check_address("169.254.169.254".parse().unwrap(), SERVER_SIDE),
        Err(EndpointRefusal::LinkLocal)
    );
}

#[test]
fn userinfo_and_ports_do_not_hide_the_host() {
    assert_eq!(
        check_endpoint("http://user:pw@169.254.169.254:80/v1", SERVER_SIDE),
        Err(EndpointRefusal::LinkLocal)
    );
    assert_eq!(
        check_endpoint("http://169.254.169.254@example.test/v1", SERVER_SIDE),
        Ok(()),
        "the authority after the last @ is the real host"
    );
}

#[test]
fn a_hostname_is_allowed_because_resolving_it_here_would_prove_nothing() {
    // A name resolved in a pure check is a DNS lookup in a pure function,
    // and the resolve can change underneath it anyway. The address check is
    // applied where the connection is actually made.
    assert_eq!(
        check_endpoint("https://localhost.acme.test/v1", SERVER_SIDE),
        Ok(())
    );
}

// ---- the IO half's pure helpers ----------------------------------------

#[test]
fn the_loopback_allowance_is_tied_to_the_local_runtime_category() {
    // Not a free-standing `true`. If the catalogue ever stops offering a
    // local runtime, the reason for the allowance is gone and so is the
    // allowance — one place to change rather than five call sites.
    assert_eq!(
        default_policy().allow_loopback,
        !catalogue::LOCAL_RUNTIMES.is_empty()
    );
}

#[test]
fn a_guard_refusal_keeps_the_credential() {
    // The SSRF guard answers a question about the address. Treating it as
    // an auth failure would delete a key over a typo in a URL.
    let failure = ProbeFailure::refused(EndpointRefusal::LinkLocal);
    assert_eq!(failure.class, ProbeClass::Endpoint);
    assert!(!failure.class.destroys_credential());
}

#[test]
fn model_ids_are_read_from_the_openai_shape_and_from_a_bare_array() {
    let wrapped = r#"{"data":[{"id":"gpt-5"},{"id":"gpt-5-mini"}]}"#;
    assert_eq!(parse_model_ids(wrapped), vec!["gpt-5", "gpt-5-mini"]);
    let bare = r#"[{"id":"llama3"}]"#;
    assert_eq!(parse_model_ids(bare), vec!["llama3"]);
}

#[test]
fn a_body_that_is_not_a_catalog_is_an_empty_list_rather_than_a_failure() {
    // A 200 from something that is not a model listing is still a reachable
    // endpoint. Failing here would refuse every provider that does not
    // publish an OpenAI-shaped catalog, which the connect flow explicitly
    // supports adding.
    assert!(parse_model_ids("not json at all").is_empty());
    assert!(parse_model_ids(r#"{"models":["a"]}"#).is_empty());
    assert!(parse_model_ids(r#"{"data":[{"name":"no id here"}]}"#).is_empty());
}

#[test]
fn a_transport_failure_says_which_condition_it_was() {
    // `reqwest`'s own Display buries the cause, so a DNS failure and a
    // timeout read identically and both classify as `unknown`. These fixed
    // phrases are what let `classify` tell them apart.
    assert_eq!(classify("timeout"), ProbeClass::Timeout);
    assert_eq!(classify("connection refused"), ProbeClass::Endpoint);
    assert_eq!(
        classify("redirect not followed: unreachable"),
        ProbeClass::Endpoint
    );
    assert_eq!(classify("the check did not complete"), ProbeClass::Unknown);
}

#[test]
fn the_probes_own_url_never_reaches_the_classifier() {
    // This probe's URL always ends in `/models`, so interpolating it into
    // the classifier's input makes EVERY failure contain the word "model" —
    // and a refused connection classified as a missing model id, sending the
    // operator to check a model they never typed.
    let failure = ProbeFailure::classified_as(
        "connection refused",
        "http://127.0.0.1:9/v1/models: error sending request".to_string(),
    );
    assert_eq!(failure.class, ProbeClass::Endpoint);
    assert!(
        failure.raw.contains("/models"),
        "the URL is still worth having in a log"
    );
}

#[test]
fn dns_and_refusal_are_endpoint_facts_not_unknowns() {
    for raw in [
        "connection refused",
        "no such host",
        "could not resolve host",
        "temporary failure in name resolution",
        "dns error",
        "network is unreachable",
        "connection reset by peer",
    ] {
        assert_eq!(
            classify(raw),
            ProbeClass::Endpoint,
            "`{raw}` is the clearest evidence there is that nothing is at that address"
        );
    }
}

#[test]
fn a_local_runtime_that_is_not_running_is_not_a_connection_worth_keeping() {
    // The one category-specific exception to "only `auth` rolls back". A
    // runtime that is not listening is a fact about the operator's machine
    // and their next move is to start it — not to keep a row pointing at a
    // port with nothing behind it.
    for class in [ProbeClass::Endpoint, ProbeClass::Timeout] {
        assert!(rolls_back(class, catalogue::Category::Local), "{class:?}");
    }
}

#[test]
fn the_same_class_against_a_cloud_provider_keeps_everything() {
    // And this asymmetry is the point: `endpoint` against a vendor's host
    // is a fact about the network in between — a proxy, a WAF, a slow
    // gateway — sitting between a perfectly good key and an endpoint that
    // is fine. Rolling back there is the bug the classifier exists to stop.
    for class in [ProbeClass::Endpoint, ProbeClass::Timeout, ProbeClass::Quota] {
        assert!(!rolls_back(class, catalogue::Category::Cloud), "{class:?}");
    }
}

#[test]
fn auth_rolls_back_whatever_the_category() {
    for category in [
        catalogue::Category::Cloud,
        catalogue::Category::Local,
        catalogue::Category::Cli,
    ] {
        assert!(rolls_back(ProbeClass::Auth, category), "{category:?}");
    }
}

#[test]
fn a_refusal_never_says_saved() {
    // `describe` opens every sentence but one with "Saved", which is true
    // when the row was kept. On the rollback path no row exists, and an
    // operator told it was saved while nothing appears has been lied to
    // about the one thing they can see.
    for class in [
        ProbeClass::Auth,
        ProbeClass::Endpoint,
        ProbeClass::Timeout,
        ProbeClass::Unknown,
    ] {
        let said = describe_refusal(class, "Ollama");
        assert!(!said.contains("Saved"), "{class:?}: {said}");
    }
}

#[test]
fn a_refusal_names_the_next_thing_to_do() {
    assert!(describe_refusal(ProbeClass::Endpoint, "Ollama").contains("Start it"));
    assert!(
        describe_refusal(ProbeClass::Auth, "Groq").contains("rejected the credential"),
        "the auth sentence is unchanged — it was already right"
    );
}
