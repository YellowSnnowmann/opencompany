use super::{provider_failure_sentence, turn_failure_notice};

/// The failure that put a wall of provider JSON into company chat, verbatim
/// from the 1/9 round (issue #2016). None of it may reach the operator, and
/// what they read instead has to say whether waiting will help.
#[cfg(feature = "openhuman")]
#[test]
fn a_rate_limit_reaches_the_operator_as_a_sentence_not_a_payload() {
    let raw = concat!(
        "turn for 'frontend_engineer': inference returned 429 Too Many Requests: ",
        r#"{"error":{"message":"Provider returned error","code":429,"metadata":"#,
        r#"{"raw":"deepseek/deepseek-chat is temporarily rate-limited upstream. "#,
        r#"Please retry shortly, or add your own key to accumulate your rate limits: "#,
        r#"https://openrouter.ai/settings/integrations","provider_name":"DeepInfra"}}}"#,
    );
    let notice = turn_failure_notice(raw);

    assert!(notice.contains("rate-limiting"), "{notice}");
    for leaked in ["{", "openrouter.ai", "deepseek", "DeepInfra", "429"] {
        assert!(
            !notice.contains(leaked),
            "the provider payload leaked {leaked:?} into chat: {notice}"
        );
    }
}

/// An empty inference is its own message: the harness has already retried it
/// by the time this is written, so leading with "try again" is wrong advice
/// and the cause is worth naming.
#[test]
fn an_empty_inference_says_so() {
    let notice = turn_failure_notice(concat!(
        "inference response carried neither choices[0].message.content nor tool_calls ",
        "(finish_reason: failed; choices: 1; usage: in=0 out=0 total=0)"
    ));

    assert!(notice.contains("empty response"), "{notice}");
    assert!(
        !notice.contains("finish_reason"),
        "diagnostics leaked into chat: {notice}"
    );
}

/// Quota exhaustion is not wait-and-retry — somebody has to go and fix the
/// account — so it must not be worded like a transient blip.
#[cfg(feature = "openhuman")]
#[test]
fn quota_exhaustion_points_at_the_account() {
    let notice = turn_failure_notice(concat!(
        "inference returned 429 Too Many Requests: ",
        r#"{"error":{"message":"insufficient balance"}}"#
    ));

    assert!(notice.contains("quota or credit"), "{notice}");
    assert!(notice.contains("Settings"), "{notice}");
}

/// A status our own error format hides from `structured_http_status`. With
/// it invisible, a 402 fell through to the `Retryable` default and was
/// reported as "temporarily unavailable" — telling an operator to wait for
/// something that will never clear on its own.
#[cfg(feature = "openhuman")]
#[test]
fn a_status_only_our_own_prefix_carries_is_still_classified() {
    let notice = turn_failure_notice("inference returned 402 Payment Required: no credit");

    assert!(notice.contains("rejected the request"), "{notice}");
}

/// The guard against over-claiming. `classify_provider_failure` falls back
/// to `Retryable` for text it recognizes nothing in, so a tool that ran out
/// of wall-clock would otherwise be reported as a provider outage.
#[test]
fn a_failure_that_is_not_the_providers_is_not_blamed_on_it() {
    let raw = "the tool call exceeded its wall-clock budget";

    assert!(
        provider_failure_sentence(raw).is_none(),
        "a non-provider failure must not be classified as one"
    );
    let notice = turn_failure_notice(raw);
    assert!(notice.contains("something went wrong"), "{notice}");
    assert!(!notice.contains("provider"), "{notice}");
}

/// Whatever the cause, the operator is told the turn left nothing behind —
/// the one fact they need in order to decide whether to re-send.
#[test]
fn every_notice_states_that_nothing_was_half_done() {
    for raw in [
        "inference returned 429 Too Many Requests: rate limited",
        "inference response carried neither choices[0].message.content nor tool_calls",
        "something else entirely",
    ] {
        assert!(
            turn_failure_notice(raw).contains("Nothing was left half-done"),
            "missing for: {raw}"
        );
    }
}
