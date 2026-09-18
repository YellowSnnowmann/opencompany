use super::*;

/// Multiple `{"type":"refusal",…}` parts in the same array-shaped
/// `content`. `extract_array_refusal_text`'s `find_map` stops at the
/// first match, so only the first part's text is recovered — the
/// analogous `extract_content_text` concatenates every `"text"`-typed
/// part instead of stopping at the first, so the refusal path must do
/// the same or it silently truncates the provider's own visible safety
/// response (CodeRabbit review on #1779, comment 3878506287).
#[test]
fn a_refusal_wins_and_is_not_truncated_when_content_array_has_multiple_refusal_parts() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "refusal", "refusal": "I can't help with that. " },
                    { "type": "refusal", "refusal": "Here's why." }
                ]
            }
        }]
    });
    let resp = model_response_from_payload(payload).expect("refusal turn parses");
    assert_eq!(resp.text(), "I can't help with that. Here's why.");
    assert!(resp.message.tool_calls.is_empty());
}

/// A mixed array: a `"text"`-typed part alongside a `{"type":"refusal",…}`
/// part in the same `content` array. `extract_content_text` concatenates
/// only the text part, so `content` is already nonempty by the time the
/// refusal-precedence block is reached — without checking for an array
/// refusal independent of `content`'s emptiness, the block is skipped
/// entirely and the turn "succeeds" with just the leaked text fragment,
/// silently discarding the provider's actual safety response (Codex
/// review on #1779, comment 3875001349).
#[test]
fn a_refusal_wins_over_leaked_text_when_content_array_mixes_text_and_refusal_parts() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "Sure, here's a start: " },
                    { "type": "refusal", "refusal": "I can't help with that." }
                ]
            }
        }]
    });
    let resp = model_response_from_payload(payload).expect("refusal turn parses");
    assert_eq!(resp.text(), "I can't help with that.");
    assert!(resp.message.tool_calls.is_empty());
}

/// An array-shaped `content` with only a `"text"`-typed part (no
/// `{"type":"refusal",…}` part at all) alongside a nonempty *scalar*
/// `message.refusal` sibling field. `extract_content_text` concatenates
/// the text part, so `content` is nonempty; `extract_array_refusal_text`
/// finds no refusal-typed part, so `array_refusal` is `None`. Gating the
/// refusal-precedence block on `content.is_empty() || array_refusal.is_some()`
/// alone therefore skips the block entirely and never even looks at the
/// scalar `message.refusal` field, leaking the text fragment as the
/// answer instead of surfacing the provider's actual safety response
/// (Codex review on #1779, comment 3875101974).
#[test]
fn a_scalar_refusal_wins_over_leaked_text_in_array_shaped_content() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "Sure, here's a start: " }
                ],
                "refusal": "I can't help with that."
            }
        }]
    });
    let resp = model_response_from_payload(payload).expect("refusal turn parses");
    assert_eq!(resp.text(), "I can't help with that.");
    assert!(resp.message.tool_calls.is_empty());
}

/// The mixed-array refusal case above (Codex review comment 3875001349)
/// only reproduced with `finish_reason: "stop"`. The refusal-precedence
/// block used to be gated on a `stop`-only check, so the identical
/// payload with `finish_reason: "content_filter"` — arguably the *more*
/// likely finish reason a real content-policy refusal ends with —
/// skipped the block entirely: `content` was already nonempty from the
/// leaked text part, so the empty-response check at the bottom accepted
/// it and returned the leaked lead-in as if it were the whole answer,
/// silently
/// discarding the refusal. A refusal is a completed decision, not a
/// partial one, so its precedence must not depend on `finish_reason`
/// (Codex review on #1779, comment 3875167298).
#[test]
fn a_refusal_wins_over_leaked_text_regardless_of_finish_reason() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "content_filter",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "Sure, here's a start: " },
                    { "type": "refusal", "refusal": "I can't help with that." }
                ]
            }
        }]
    });
    let resp = model_response_from_payload(payload).expect("refusal turn parses");
    assert_eq!(resp.text(), "I can't help with that.");
    assert!(resp.message.tool_calls.is_empty());
}

/// The sibling fallback field: some providers emit the reasoning-only
/// text under `reasoning_content` (array-of-parts shape) instead of
/// `reasoning`, with `reasoning` itself absent. This shape is subject to
/// the same fix as the plain `reasoning` field above — `reasoning_content`
/// is chain-of-thought too, so it must not be promoted into `content`
/// either. Exercises the array-of-parts form specifically, which the
/// plain-`reasoning` test above does not cover.
#[test]
fn reasoning_content_only_turn_errors_instead_of_promoting_array_shaped_text() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": null,
                "reasoning": "",
                "reasoning_content": [
                    { "type": "text", "text": "The answer is " },
                    { "type": "text", "text": "42." }
                ]
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("reasoning_content-only turn must not promote reasoning into content");
    let msg = err.to_string();
    assert!(
        msg.contains("neither"),
        "must be the diagnosable empty-turn error, got: {msg}"
    );
    assert!(
        !msg.contains("42"),
        "leaked reasoning text must not appear in the error, got: {msg}"
    );
}

/// An explicit `content: ""` (not `null`) is a *visible* empty response,
/// not the documented reasoning-only shape — `extract_content_text`
/// reduces both to the same empty string, so the old `content.is_empty()`
/// check could not tell them apart and promoted `reasoning` anyway. That
/// substitutes internal chain-of-thought for whatever unsupported/empty
/// response the provider actually sent, the same class of bug the
/// refusal-precedence guard above exists to prevent, just triggered by an
/// empty string instead of a populated field (CodeRabbit review on
/// #1779, comment 3877224319).
#[test]
fn explicit_empty_string_content_does_not_promote_reasoning() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": "",
                "reasoning": "The user wants help with something I should decline."
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("explicit empty-string content must not promote reasoning");
    assert!(
        !err.to_string().contains("decline"),
        "leaked reasoning must not appear in the error, got: {err}"
    );
}

/// Same gap as above, via the array-content path: a non-text content
/// array (e.g. an image-only part) extracts to an empty string too, but
/// the raw field is neither absent nor `null` — it is the provider's
/// actual (just non-text) response, and must not be silently swapped for
/// leaked reasoning (CodeRabbit review on #1779, comment 3877224319).
#[test]
fn non_text_array_content_does_not_promote_reasoning() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "image_url", "image_url": { "url": "https://example.com/x.png" } }
                ],
                "reasoning": "The user wants help with something I should decline."
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("non-text array content must not promote reasoning");
    assert!(
        !err.to_string().contains("decline"),
        "leaked reasoning must not appear in the error, got: {err}"
    );
}

/// A genuinely empty turn truncated by `finish_reason: "length"` (max_tokens
/// hit) is still an error — but the message must name the finish reason so
/// the truncation is diagnosable rather than hidden behind a generic string.
#[test]
fn truncated_empty_response_errors_with_finish_reason() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "length",
            "message": { "role": "assistant", "content": "" }
        }]
    });
    let err = model_response_from_payload(payload).expect_err("truncated empty turn errors");
    let msg = err.to_string();
    assert!(
        msg.contains("length"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// A reasoning-only turn truncated by `finish_reason: "length"` (max_tokens
/// hit mid chain-of-thought) must still error, even though `reasoning`
/// carries text — the reasoning-fallback exists to recover a *complete*
/// answer that only landed under `reasoning`, not to promote a cut-off
/// chain of thought into a fabricated final reply. Pre-fix, this payload
/// parsed successfully with `resp.text() == "The answer is"`, silently
/// handing a partial thought to downstream consumers as if it were the
/// finished answer (Codex review on #1779).
#[test]
fn truncated_reasoning_only_turn_errors_instead_of_promoting_partial_thought() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "length",
            "message": {
                "role": "assistant",
                "content": null,
                "reasoning": "The answer is"
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("truncated reasoning-only turn must not parse as success");
    let msg = err.to_string();
    assert!(
        msg.contains("length"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// Same as above but for `content_filter` — a filtered reasoning stream is
/// just as unfinished as a truncated one and must not be promoted either.
#[test]
fn content_filtered_reasoning_only_turn_errors() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "content_filter",
            "message": {
                "role": "assistant",
                "content": null,
                "reasoning_content": "Let's think about how to"
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("content-filtered reasoning-only turn must not parse as success");
    let msg = err.to_string();
    assert!(
        msg.contains("content_filter"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// `finish_reason: "failed"` is the documented HTTP-200-empty-response
/// silent provider failure (see docs/spec/runtime/providers.md — observed
/// on an oversized request, empty message, zero usage). The pre-fix guard
/// blocklisted only `length`/`content_filter`, so a reasoning-only turn
/// carrying `failed` still fell through and promoted whatever partial
/// reasoning the provider emitted before failing — handing downstream
/// consumers an unfinished thought as if it were the answer (Codex
/// follow-up review on #1779, comment 3860281502). Must error instead.
#[test]
fn failed_finish_reason_reasoning_only_turn_errors() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "failed",
            "message": {
                "role": "assistant",
                "content": null,
                "reasoning": "The answer is"
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("failed reasoning-only turn must not parse as success");
    let msg = err.to_string();
    assert!(
        msg.contains("failed"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// Same as `failed_finish_reason_reasoning_only_turn_errors`, but the
/// leaked text lives in the *primary* `content` field (array-shaped, the
/// form round #8 of this PR taught `extract_content_text` to parse) rather
/// than `reasoning`. `content` is extracted unconditionally at the top of
/// `model_response_from_payload`, with no `finish_reason` check of its own.
/// Pre-fix, this payload parsed successfully with the leaked lead-in
/// sentence returned as the answer, silently discarding the provider's own
/// `failed` disclaimer (CodeRabbit review on #1779, comment 3878355364).
#[test]
fn failed_finish_reason_with_leaked_array_content_errors() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "failed",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "Let me look that up for you" }
                ]
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("failed turn with leaked content must not parse as success");
    let msg = err.to_string();
    assert!(
        !msg.contains("look that up"),
        "leaked content must not appear in the error, got: {msg}"
    );
    assert!(
        msg.contains("failed"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// The legacy `finish_reason: "function_call"` shape carries the request
/// under the singular `message.function_call` field, which
/// `parse_tool_calls` never reads (it only parses the modern
/// `message.tool_calls` array). Pre-fix, `finish_reason: "function_call"`
/// used to sit in an allow-list gating a reasoning-promotion fallback, so
/// with `tool_calls` empty (nothing there to parse) and `content: null`,
/// this fell straight into that fallback and silently swapped the
/// requested action for prose — the caller never even sees a tool call
/// was dropped. Must error instead (Codex follow-up review on #1779,
/// comment 3862781739).
#[test]
fn legacy_function_call_with_reasoning_errors_instead_of_promoting() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "function_call",
            "message": {
                "role": "assistant",
                "content": null,
                "reasoning": "I should call the weather function",
                "function_call": { "name": "get_weather", "arguments": "{}" }
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("a raw legacy function_call must not be dropped for promoted reasoning");
    let msg = err.to_string();
    assert!(
        msg.contains("function_call"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// A raw `tool_calls` array can be *partially* malformed: one entry
/// parses (has `function.name`), one does not. `parse_tool_calls`'s
/// `filter_map` drops the malformed entry and returns the single valid
/// one — a nonempty `Vec`, so `tool_calls.is_empty()` alone never
/// catches it. Pre-fix, the response is returned successfully with only
/// the surviving call, silently discarding a genuinely requested action
/// (CodeRabbit review on #1779, comment 3877118065). The guard must
/// compare the raw array length against the parsed count, not just
/// check for emptiness.
#[test]
fn partially_malformed_tool_calls_array_errors_instead_of_dropping_one_call() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    { "id": "call_1", "type": "function", "function": { "name": "get_weather", "arguments": "{}" } },
                    { "id": "call_2", "type": "function", "function": { "arguments": "{}" } }
                ]
            }
        }]
    });
    let err = model_response_from_payload(payload).expect_err(
        "a partially malformed tool_calls array must not silently drop the malformed entry",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("tool call"),
        "error must name the dropped tool call for diagnosis, got: {msg}"
    );
}

/// A malformed modern `tool_calls` entry (missing `function.name`) is
/// dropped by `parse_tool_calls`'s `filter_map`, leaving the *parsed*
/// `tool_calls` empty even though the raw payload clearly requested one.
/// The raw-payload guard must catch this too, not just the legacy
/// `function_call` field, so a request that fails to parse surfaces as
/// the empty-response error rather than a promoted reasoning answer.
#[test]
fn malformed_modern_tool_call_with_reasoning_errors_instead_of_promoting() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": null,
                "reasoning": "I should call the weather function",
                "tool_calls": [{ "id": "call_1", "type": "function", "function": { "arguments": "{}" } }]
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("a malformed raw tool_calls entry must not be dropped for promoted reasoning");
    let msg = err.to_string();
    assert!(
        msg.contains("tool_calls"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// Array-shaped `content` can carry a text preamble ("Let me check
/// that…") alongside a `tool_calls` entry the model genuinely requested
/// but that fails to parse (missing `function.name`). `content` reads
/// nonempty via `extract_content_text` while `parse_tool_calls` drops the
/// call, so a check that only looks at content-or-tool_calls emptiness
/// passes and the harness would silently return just the preamble,
/// dropping the requested action entirely. The raw-payload guard must
/// catch this regardless of whether prose content is also present
/// (CodeRabbit review on #1779, comment 3872084060).
#[test]
fn malformed_tool_call_beside_array_content_preamble_errors_instead_of_silently_dropping() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "Let me check that for you." }
                ],
                "tool_calls": [{ "id": "call_1", "type": "function", "function": { "arguments": "{}" } }]
            }
        }]
    });
    let err = model_response_from_payload(payload).expect_err(
        "a malformed raw tool_calls entry beside preamble content must not be silently dropped",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("tool_calls"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// A non-array `message.tool_calls` (e.g. an object instead of a list) is
/// neither a legacy `function_call` nor something `.as_array()` accepts,
/// so pre-fix the raw-payload guard silently read it as "no call
/// present" and fell through to the reasoning fallback below — the exact
/// class of substitution the array/legacy checks above exist to prevent,
/// just for a shape neither one covers. Must error instead of promoting
/// (CodeRabbit review on #1779, comment 3872083353).
#[test]
fn non_array_tool_calls_with_reasoning_errors_instead_of_promoting() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": null,
                "reasoning": "I should call the weather function",
                "tool_calls": {}
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("a non-array raw tool_calls value must not be dropped for promoted reasoning");
    let msg = err.to_string();
    assert!(
        msg.contains("stop"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// A `finish_reason: "tool_calls"` response that carries no call body at
/// all (no `tool_calls` field, no legacy `function_call` field) must
/// error rather than promote `reasoning` into the final answer. The
/// finish reason itself asserts the model requested an action; treating
/// it as "genuinely finished" let the raw-payload guard (which only
/// checks for a *present* call) miss the case where there is no call
/// field to find. Pre-fix, this silently swapped the requested action
/// for prose (Codex review on #1779, comment 3864692178).
#[test]
fn tool_calls_finish_reason_with_missing_call_body_errors_instead_of_promoting() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": null,
                "reasoning": "I should call the weather function"
            }
        }]
    });
    let err = model_response_from_payload(payload)
        .expect_err("a tool_calls finish reason with no call body must not promote reasoning");
    let msg = err.to_string();
    assert!(
        msg.contains("tool_calls"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// Same gap, but the provider sends an explicit empty `tool_calls: []`
/// array instead of omitting the field — the raw-payload guard treats an
/// empty array as "nothing requested" (correctly, for `parse_tool_calls`
/// purposes) but that must not be read as license to promote reasoning
/// when the finish reason itself claims an action was intended.
#[test]
fn tool_calls_finish_reason_with_empty_call_array_errors_instead_of_promoting() {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": null,
                "reasoning": "I should call the weather function",
                "tool_calls": []
            }
        }]
    });
    let err = model_response_from_payload(payload).expect_err(
        "a tool_calls finish reason with an empty call array must not promote reasoning",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("tool_calls"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}

/// Same gap again, but via the *content* channel instead of `reasoning`:
/// array-shaped `content` carrying a nonempty text preamble ("Let me
/// check that…") makes `content` nonempty on its own, with no reasoning
/// fallback involved at all. `raw_tool_call_requested` reads a present
/// but empty `tool_calls: []` array the same as an absent field (both
/// "nothing requested"), so the explicit raw-payload guard never fires;
/// and because `content` is already nonempty, the final
/// content-and-tool_calls-both-empty catch-all below never fires either.
/// The response would be returned successfully with the preamble as the
/// full text and no tool call — silently dropping the action the
/// `finish_reason` itself asserts was requested (CodeRabbit review on
/// #1779, comment 3877608728).
#[test]
fn tool_calls_finish_reason_with_empty_array_beside_content_preamble_errors_instead_of_dropping_action()
 {
    let payload = serde_json::json!({
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "Let me check that for you." }
                ],
                "tool_calls": []
            }
        }]
    });
    let err = model_response_from_payload(payload).expect_err(
        "a tool_calls finish reason with an empty call array must not let a content \
         preamble stand in for the requested action",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("tool_calls"),
        "error must name finish_reason for diagnosis, got: {msg}"
    );
}
