use super::*;

fn class_of(message: &str) -> Option<BlockerClass> {
    classify_blocker_message(message)
}

#[test]
fn a_rejected_model_id_is_infrastructure() {
    let class = class_of("dispatch failed: the model `gpt-nonexistent` does not exist or you do not have access to it")
        .expect("a rejected model id is recognised");
    assert_eq!(class.kind, BlockerKind::Infrastructure);
    assert_eq!(class.source, BlockerSource::Provider);
    assert!(class.kind.parks());
}

#[test]
fn a_bad_key_is_infrastructure() {
    let class = class_of("hosted inference returned 401: invalid api key")
        .expect("an auth failure is recognised");
    assert_eq!(class.kind, BlockerKind::Infrastructure);
    assert_eq!(class.source, BlockerSource::Provider);
}

#[test]
fn a_disconnected_integration_is_infrastructure_from_a_tool() {
    let class = class_of("tool call failed: could not connect to mcp server `slack`")
        .expect("an MCP connection failure is recognised");
    assert_eq!(class.kind, BlockerKind::Infrastructure);
    assert_eq!(class.source, BlockerSource::Tool);
}

/// The point of carrying `Transient` in the taxonomy: it is recognised, and
/// recognising it is how we know **not** to ask anybody.
#[test]
fn a_rate_limit_is_recognised_but_does_not_park() {
    let class = class_of("hosted inference returned 429: rate limit exceeded")
        .expect("a rate limit is recognised");
    assert_eq!(class.kind, BlockerKind::Transient);
    assert!(
        !class.kind.parks(),
        "a rate limit resolves itself; asking a person about it wastes their attention"
    );
}

/// Rate limiting outranks the auth row, because a 429 body routinely names
/// the key it is throttling. Getting this backwards would park a
/// self-resolving stop as a broken credential.
#[test]
fn a_throttled_key_reads_as_transient_not_as_bad_auth() {
    let class = class_of("429 too many requests for this api key").expect("recognised");
    assert_eq!(class.kind, BlockerKind::Transient);
}

/// The boundary check reads every occurrence, not just the first.
///
/// A provider line that names a request id before its status code —
/// `req-4290 … http 429` — used to stop at the `429` inside `4290`, reject
/// its trailing `0`, and never reach the real code. The miss was not
/// neutral: the auth row sits below the transient one, so the same body
/// mentioning a key would then park a self-resolving throttle as a broken
/// credential and wait for a person with nothing to fix.
#[test]
fn a_status_code_is_found_past_an_earlier_unbounded_lookalike() {
    let class = class_of("request req-4290 received HTTP 429").expect("recognised");
    assert_eq!(class.kind, BlockerKind::Transient);

    let auth_flavoured =
        class_of("request req-4290 failed: http 429 for this api key").expect("recognised");
    assert_eq!(
        auth_flavoured.kind,
        BlockerKind::Transient,
        "the real 429 must still outrank the auth row it is quoted beside"
    );
}

/// …and the boundary itself still holds: a longer number that merely starts
/// with a status code is not that status code.
#[test]
fn a_longer_number_is_not_a_status_code() {
    assert_eq!(class_of("dispatch failed on port 4010"), None);
    assert_eq!(class_of("dispatch failed: worker 4290 died"), None);
}

/// A bare `401` is enough on its own, whatever punctuation follows it.
///
/// It was not, and nothing said so: the leaf was spelled as the pair
/// `"401 "` / `" 401"` — a boundary check written into the leaf instead of
/// around it. The leading-space form made the start-boundary test read the
/// character *before* the space, a letter in every real provider message,
/// so it rejected all of them; the trailing-space form missed `401:` and a
/// line ending in `401`. Every existing 401 test passed anyway, because
/// each message also carried `invalid api key` or `unauthorized` — the
/// prose phrases were doing all the work and the status code none of it.
#[test]
fn a_bare_401_is_an_auth_blocker_whatever_follows_it() {
    for message in [
        "hosted inference returned 401: invalid credentials",
        "provider rejected the call with http 401",
        "401 returned by the upstream",
    ] {
        let class = class_of(message).unwrap_or_else(|| panic!("unrecognised: {message}"));
        assert_eq!(
            class.kind,
            BlockerKind::Infrastructure,
            "message: {message}"
        );
        assert_eq!(class.source, BlockerSource::Provider, "message: {message}");
    }
}

/// The conservative default. An error we cannot name keeps today's
/// behaviour rather than guessing at a question for somebody.
#[test]
fn an_unrecognised_failure_is_not_a_blocker() {
    assert_eq!(class_of("dispatch failed: index out of bounds"), None);
    assert_eq!(
        class_of("hand-off failed: the delegate produced nothing"),
        None
    );
    assert_eq!(class_of(""), None);
}

/// Whole phrases, not loose words — a provider body that merely mentions a
/// credential must not be read as a broken one.
#[test]
fn a_body_that_merely_mentions_a_key_is_not_an_auth_blocker() {
    assert_eq!(
        class_of("the document describes how to store an api key safely"),
        None,
        "matching the bare word `api key` would park an unrelated failure"
    );
}

#[test]
fn matching_is_case_insensitive() {
    assert!(class_of("HTTP 401 UNAUTHORIZED").is_some());
}

/// Every row promises something a person can act on, including the
/// transient row (whose promise is that there is nothing to do).
#[test]
fn every_shape_says_what_is_needed() {
    for shape in SHAPES {
        assert!(
            !shape.class.needed.trim().is_empty(),
            "a blocker with nothing in `needed` reaches a person with nothing to do"
        );
        assert!(
            !shape.leaves.is_empty(),
            "a shape with no phrases can never match"
        );
    }
    assert!(!PREREQ_BLOCKER.needed.is_empty());
    assert!(!AGENT_QUESTION_BLOCKER.needed.is_empty());
}

/// The two host-declared classes park by construction — they exist because
/// something already established a person is needed.
#[test]
fn declared_classes_park() {
    assert!(PREREQ_BLOCKER.kind.parks());
    assert!(AGENT_QUESTION_BLOCKER.kind.parks());
}

/// Two cards stalled on the same integration carry the same group key, so
/// the console folds them into one question instead of two.
#[test]
fn a_named_connection_groups_by_its_name() {
    let a = connection_group_key("could not connect to mcp server `slack`")
        .expect("a named connection groups");
    let b = connection_group_key("dispatch failed: could not connect to mcp server `slack`")
        .expect("the same connection, a different reason");
    assert_eq!(a, b);
    assert_eq!(a, "connection:slack");
    assert_ne!(
        connection_group_key("could not connect to mcp server `notion`"),
        Some(a),
        "a different server is a different question"
    );
}

/// The gate is that the reason matched a connection shape — a backticked
/// token in an auth or model-id reason is not a connection and must not
/// group those distinct stops together.
#[test]
fn a_backtick_outside_a_connection_shape_does_not_group() {
    assert_eq!(connection_group_key("unknown model `gpt-nope`"), None);
    assert_eq!(connection_group_key("invalid api key `sk-123`"), None);
    assert_eq!(
        connection_group_key("could not connect to mcp server"),
        None,
        "a connection shape with no name has nothing to group by"
    );
}

/// A backticked token before the connection marker — a tool name in the
/// prose — is not the connection; the name after the marker is.
#[test]
fn the_name_is_read_after_the_connection_marker() {
    assert_eq!(
        connection_group_key("tool `search` failed: could not connect to mcp server `slack`"),
        Some("connection:slack".to_string()),
        "the connection is the server after the marker, not the earlier tool token"
    );
}
