use super::provider_tenant_tests::MemSecrets;
use super::provider_test_helpers_tests::*;
use super::*;

#[tokio::test]
async fn tenant_turn_preserves_a_provider_status() {
    let (base_url, server) = spawn_rejection(axum::http::StatusCode::TOO_MANY_REQUESTS).await;
    let company = CompanyId::new("acme");
    let secrets: Arc<dyn SecretStore> = Arc::new(MemSecrets::default());
    let mut manifest = manifest_inference("openai_compatible");
    manifest.base_url = Some(base_url);
    let provider = TenantProvider::new(company, secrets, manifest, None);

    let err = provider
        .invoke(
            &(),
            ModelRequest {
                model: Some("provider/model".to_string()),
                ..user_request("hi")
            },
        )
        .await
        .expect_err("the stub rate-limits the turn");
    server.abort();
    let InferenceError::Provider(error) = err else {
        panic!("expected a provider error, got {err}");
    };
    assert_eq!(error.status, Some(429));
    assert!(error.retryable);
    assert_eq!(error.raw, None);
}

/// Reachability and usability are different questions, and `probe` asks
/// only the first.
///
/// The turn path must not promote `reasoning` into a reply — that is this
/// PR — and `probe` shares that parser, so a reasoning-only payload fails
/// its empty-turn check. For a *probe* that failure is still a yes: the
/// credential, base URL and model name reached a server that completed a
/// chat turn. It deliberated and ran out of room, which is a statement
/// about the budget, not the connection.
///
/// An earlier revision of this branch asserted the opposite, and named the
/// risk in its own doc: `probe` sends `ping` with `max_tokens: 16`, little
/// enough that a well-behaved reasoning model burns it on deliberation
/// routinely. For the DeepSeek-style models this PR exists for that is the
/// *expected* shape, so rejecting it re-created the bug #1779 fixed — a
/// connection every real turn reached fine, reported as broken, with the
/// setup wizard refusing to move past it (Codex review on #2068). The fix
/// is the probe-specific tolerance that doc pointed at, not promotion in
/// the shared parser.
/// The reasoning tolerance covers the empty turn and nothing else.
///
/// A payload that asked for an action and got it wrong fails the parser for
/// a *different* reason than "carried nothing", and tolerating that would
/// return `Ok(())` before the unoffered-tool-call guard below could run —
/// passing an endpoint that cannot complete a bare `ping`. This probe
/// advertises no tools at all, so a declared action is already wrong no
/// matter what else rides alongside it (Codex review on #2068).
///
/// Reasoning is present in both payloads here, so the tolerance is what is
/// under test rather than the absence of its trigger.
#[tokio::test]
async fn the_reasoning_tolerance_does_not_excuse_a_broken_tool_call() {
    for message in [
        // Requested an action, and the entry is unparseable.
        serde_json::json!({
            "role": "assistant",
            "content": null,
            "reasoning": "42 is the answer.",
            "tool_calls": [{ "no_name_here": true }]
        }),
        // Declared an action that never arrived.
        serde_json::json!({
            "role": "assistant",
            "content": null,
            "reasoning": "42 is the answer."
        }),
    ] {
        let declares_only = message.get("tool_calls").is_none();
        let url = if declares_only {
            spawn_stub_choice(serde_json::json!({
                "message": message,
                "finish_reason": "tool_calls"
            }))
            .await
        } else {
            spawn_stub_message(message).await
        };

        let company = CompanyId::new("acme");
        let secrets = MemSecrets::default();
        let mut manifest = manifest_inference("openai_compatible");
        manifest.base_url = Some(url);
        let decl = inference::resolve_effective(&company, &manifest, None, &secrets)
            .await
            .unwrap()
            .unwrap();

        let err = probe(&decl, "stub-model", None)
            .await
            .expect_err("a broken tool call must fail the probe even with reasoning present");
        assert!(
            !err.to_string().contains("42"),
            "reasoning text must not leak into the probe error, got: {err}"
        );
    }
}

#[tokio::test]
async fn probe_accepts_a_reasoning_only_reply_as_proof_the_endpoint_answers() {
    let url = spawn_stub_message(serde_json::json!({
        "role": "assistant",
        "content": null,
        "reasoning": "42 is the answer."
    }))
    .await;

    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let mut manifest = manifest_inference("openai_compatible");
    manifest.base_url = Some(url);
    let mut decl = inference::resolve_effective(&company, &manifest, None, &secrets)
        .await
        .unwrap()
        .unwrap();
    decl.models
        .insert("chat-v1".to_string(), "stub-model".to_string());

    probe(&decl, "stub-model", None).await.expect(
        "a reply carrying reasoning tokens proves the endpoint completes chat turns, \
         even with no budget left for a visible answer",
    );

    // The turn path is unmoved by the probe's tolerance: the same payload
    // through the same parser is still an error, so nothing promotes the
    // chain-of-thought into a reply. Reachability is the only thing the
    // tolerance buys.
    let err = model_response_from_payload(serde_json::json!({
        "choices": [{
            "message": { "role": "assistant", "content": null, "reasoning": "42 is the answer." },
            "finish_reason": "stop"
        }]
    }))
    .expect_err("a real turn carrying only reasoning is still an empty turn");
    assert!(
        !err.to_string().contains("42"),
        "reasoning text must not leak into the turn error either, got: {err}"
    );
}

/// CodeRabbit review on #1779 (comment 3877827976): `probe` routes
/// through the shared `model_response_from_payload`, which is correct
/// for a real turn but accepts a tool-call-only reply as a success —
/// tool calls are a valid outcome when the caller offered tools. `probe`
/// offers none (`Vec::new()`), so an endpoint answering `ping` with a
/// tool call instead of prose never actually answered the bare chat turn
/// the probe exists to verify. Without an explicit check for visible
/// text, the setup wizard or console Test action would report such an
/// endpoint as reachable.
#[tokio::test]
async fn probe_rejects_tool_call_only_reply() {
    let url = spawn_stub_message(serde_json::json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [
            {
                "id": "call_1",
                "type": "function",
                "function": { "name": "lookup_weather", "arguments": "{}" }
            }
        ]
    }))
    .await;

    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let mut manifest = manifest_inference("openai_compatible");
    manifest.base_url = Some(url);
    let mut decl = inference::resolve_effective(&company, &manifest, None, &secrets)
        .await
        .unwrap()
        .unwrap();
    decl.models
        .insert("chat-v1".to_string(), "stub-model".to_string());

    let err = probe(&decl, "stub-model", None)
        .await
        .expect_err("a tool-call-only reply to a no-tools probe must not pass");
    assert!(
        err.to_string().contains("tool call"),
        "error should name why the probe failed: {err}"
    );
}

/// CodeRabbit review on #1779 (comment 3878355375): the tool-call guard
/// above only checked `content.is_empty()`, which catches a tool-call-
/// *only* reply but not a mixed one — a text preamble alongside a
/// genuinely parsed tool call. `model_response_from_payload` accepts that
/// combination for a real turn (the finish-reason-declares-an-action
/// guard only fires when `tool_calls` fails to parse), so `content` comes
/// back nonempty and the pre-fix check let it through even though the
/// probe offered no tools and the endpoint still requested one. Must
/// still fail the probe.
#[tokio::test]
async fn probe_rejects_tool_call_alongside_text_preamble() {
    let url = spawn_stub_message(serde_json::json!({
        "role": "assistant",
        "content": "Let me check that for you.",
        "tool_calls": [
            {
                "id": "call_1",
                "type": "function",
                "function": { "name": "lookup_weather", "arguments": "{}" }
            }
        ]
    }))
    .await;

    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();
    let mut manifest = manifest_inference("openai_compatible");
    manifest.base_url = Some(url);
    let mut decl = inference::resolve_effective(&company, &manifest, None, &secrets)
        .await
        .unwrap()
        .unwrap();
    decl.models
        .insert("chat-v1".to_string(), "stub-model".to_string());

    let err = probe(&decl, "stub-model", None)
        .await
        .expect_err("a tool call alongside text in a no-tools probe must not pass");
    assert!(
        err.to_string().contains("tool call"),
        "error should name why the probe failed: {err}"
    );
}

/// Issue #1811: the managed backend's raw refusal for a model id that does
/// not exist is rewritten into an actionable message that names the fix and
/// keeps the provider's own words (the bad id + the list-models hint) at the
/// end for support. No `harness` in play (the managed backend has no
/// harness-scoped config), so the fix is named as the company mapping.
#[test]
fn a_missing_model_400_becomes_actionable() {
    let raw = concat!(
        "inference returned 400 Bad Request: ",
        r#"{"error":"Model 'deepseek/deepseek-v4-pro' is not available. "#,
        r#"Use GET /openai/v1/models to list available models.","errorCode":"BAD_REQUEST"}"#,
    );
    let advice = model_unavailable_advice(
        reqwest::StatusCode::BAD_REQUEST,
        raw,
        "https://api.tinyhumans.ai/openai/v1/models",
        None,
        None,
    )
    .expect("recognised as a missing model");
    assert!(
        !advice.contains("agent's model"),
        "a built_in harness never honours `agent.model` (`Manifest::validate`), so it must \
         not be suggested as a fix: {advice}"
    );
    assert!(
        advice.contains("the company's `[inference].models` mapping"),
        "with no harness scope, the company-level mapping is the only place to fix it: \
         {advice}"
    );
    assert!(
        advice.contains("deepseek/deepseek-v4-pro"),
        "the offending id survives for support: {advice}"
    );
    assert!(
        advice.contains("GET https://api.tinyhumans.ai/openai/v1/models"),
        "the caller-supplied catalog endpoint is used: {advice}"
    );
}

/// The reported defect, at the last mechanism that could have caught it: a
/// fresh company adds Anthropic, every turn goes out as `model: agentic-v1`
/// because nothing asked which model the provider should serve, and
/// Anthropic answers `not_found_error`. That wording matched none of the
/// signatures, so the repair advice returned `None` and the operator got a
/// bare 404 naming a string with no pointer to where it is configured.
#[test]
fn anthropics_not_found_error_becomes_actionable() {
    let raw = concat!(
        "inference returned 404 Not Found: ",
        r#"{"type":"error","error":{"type":"not_found_error","message":"model: agentic-v1"}}"#,
    );
    let advice = model_unavailable_advice(
        reqwest::StatusCode::NOT_FOUND,
        raw,
        "https://api.anthropic.com/v1/models",
        None,
        None,
    )
    .expect("Anthropic's typed 404 is recognised as a missing model");
    assert!(
        advice.contains("GET https://api.anthropic.com/v1/models"),
        "the advice points at the failing endpoint's own catalog: {advice}"
    );
    assert!(
        advice.contains("agentic-v1"),
        "the id that was actually sent survives for support: {advice}"
    );
}

/// Codex review on #1824: a named `built_in` harness with its own
/// `[harness.inference]` resolves independently of the company mapping
/// (`resolve_effective_scoped`), so the advice must name *that* harness
/// rather than blanket-pointing at `[inference].models` — the earlier wording
/// sent its operator to a table the failing request never consulted, and
/// separately suggested `agent.model`, which a `built_in` harness rejects
/// outright. This assertion set does not compile against the pre-fix
/// 3-argument `model_unavailable_advice`, i.e. it fails (to build) on the
/// pre-fix code exactly as it must.
#[test]
fn scoped_harness_advice_names_its_own_harness() {
    let raw = "inference returned 400 Bad Request: openai/made-up is not a valid model ID";
    let advice = model_unavailable_advice(
        reqwest::StatusCode::BAD_REQUEST,
        raw,
        "https://openrouter.ai/api/v1/models",
        Some("research-harness"),
        Some(InferenceSource::Manifest),
    )
    .expect("recognised as a missing model");
    assert!(
        advice.contains("harness `research-harness`'s own `[harness.inference].models`"),
        "the failing harness is named, not just the company: {advice}"
    );
    assert!(
        advice.contains("the company's `[inference].models`"),
        "the company fallback (when the harness declares none of its own) is still \
         mentioned: {advice}"
    );
    assert!(
        !advice.contains("agent's model"),
        "still never suggests the non-lever `agent.model`: {advice}"
    );
}

/// Codex review on #1824 (round 2): a saved console runtime override
/// outranks *both* manifest tables (`resolve_effective_scoped`'s
/// precedence — runtime > manifest > env-default), so while one is active
/// the earlier wording sent the operator to edit a `[harness.inference]` /
/// `[inference]` table that is shadowed and would not change the outcome.
/// This assertion set does not compile against the pre-fix 4-argument
/// `model_unavailable_advice` (no `source` parameter), i.e. it fails to
/// build on the pre-fix code exactly as it must.
#[test]
fn runtime_override_advice_names_the_override_not_the_shadowed_manifest() {
    let raw = "inference returned 400 Bad Request: openai/made-up is not a valid model ID";

    let scoped = model_unavailable_advice(
        reqwest::StatusCode::BAD_REQUEST,
        raw,
        "https://openrouter.ai/api/v1/models",
        Some("research-harness"),
        Some(InferenceSource::Runtime),
    )
    .expect("recognised as a missing model");
    assert!(
        scoped.contains("harness `research-harness`'s saved runtime inference override"),
        "the active override is named, not a shadowed manifest table: {scoped}"
    );
    assert!(
        !scoped.contains("update harness `research-harness`'s own `[harness.inference]"),
        "the manifest-table phrasing (the non-Runtime branch) must not be the suggested fix \
         while an override shadows it: {scoped}"
    );

    let default = model_unavailable_advice(
        reqwest::StatusCode::BAD_REQUEST,
        raw,
        "https://openrouter.ai/api/v1/models",
        None,
        Some(InferenceSource::Runtime),
    )
    .expect("recognised as a missing model");
    assert!(
        default.contains("update the saved runtime inference override"),
        "the company-scoped override is named: {default}"
    );
    assert!(
        !default.contains("update the company's `[inference].models` mapping"),
        "the manifest-table phrasing (the non-Runtime branch) must not be the suggested fix \
         while an override shadows it: {default}"
    );
}

/// Issue #1811 follow-up (Codex review on #1824): a direct OpenRouter,
/// Ollama, or arbitrary `openai_compatible` BYOK endpoint must get *its own*
/// catalog URL in the advice, not the TinyHumans-managed `/openai/v1/models`
/// path. Before the fix this string was hard-coded regardless of
/// `models_url`, so this assertion fails on the pre-fix code even though the
/// raw provider error here (OpenRouter's own wording) never mentions
/// `/openai/v1/models` at all.
#[test]
fn byok_provider_advice_points_at_its_own_catalog() {
    let raw = "inference returned 400 Bad Request: openai/made-up is not a valid model ID";
    let advice = model_unavailable_advice(
        reqwest::StatusCode::BAD_REQUEST,
        raw,
        "https://openrouter.ai/api/v1/models",
        None,
        None,
    )
    .expect("recognised as a missing model");
    assert!(
        advice.contains("GET https://openrouter.ai/api/v1/models"),
        "OpenRouter's own catalog endpoint is named: {advice}"
    );
    assert!(
        !advice.contains("openai/v1/models"),
        "the TinyHumans-managed path must not leak into a direct-provider hint: {advice}"
    );
}

/// The BYOK / OpenAI-compatible and OpenRouter phrasings for the same class
/// are all recognised — the signature set is the provider's wording, not a
/// catalogue of model ids.
#[test]
fn other_provider_phrasings_are_recognised() {
    for body in [
        "inference returned 404 Not Found: The model `gpt-9` does not exist",
        "inference returned 400 Bad Request: openai/made-up is not a valid model ID",
    ] {
        assert!(
            model_unavailable_advice(
                reqwest::StatusCode::BAD_REQUEST,
                body,
                "https://example.com/v1/models",
                None,
                None,
            )
            .is_some(),
            "should be recognised as a missing model: {body}"
        );
    }
}

/// A valid model that fails for another reason must pass through untouched:
/// a 401 (bad key), a 4xx about something other than a model, and any 5xx
/// (the provider's own fault — reframing it as a config error would send the
/// operator to change a model that is fine).
#[test]
fn unrelated_failures_are_left_alone() {
    assert_eq!(
        model_unavailable_advice(
            reqwest::StatusCode::UNAUTHORIZED,
            "inference returned 401 Unauthorized: invalid api key",
            "https://example.com/v1/models",
            None,
            None,
        ),
        None,
        "a bad key is not a missing model"
    );
    assert_eq!(
        model_unavailable_advice(
            reqwest::StatusCode::BAD_REQUEST,
            "inference returned 400 Bad Request: user does not exist",
            "https://example.com/v1/models",
            None,
            None,
        ),
        None,
        "a 4xx that never names a model is not a missing model"
    );
    assert_eq!(
        model_unavailable_advice(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "inference returned 500: the model host crashed",
            "https://example.com/v1/models",
            None,
            None,
        ),
        None,
        "a 5xx is the provider's fault, not the operator's config"
    );
}

/// `probe`'s repair hint must name the harness whose table the failing
/// request actually read (Codex review on #1824's #1811 follow-up):
/// `test_config` (src/server/ops/inference.rs) resolves against the
/// company's default harness and now threads its real id through, the
/// same distinction `TenantProvider::invoke` already makes for live
/// turns. Before the fix `probe` hard-coded `None` for every caller, so a
/// company whose default harness declares its own `[harness.inference]`
/// got a repair hint pointing at the company-level `[inference].models`
/// table — one its request never consulted.
#[tokio::test]
async fn probe_names_the_harness_that_owns_the_failing_config() {
    let base_url = spawn_model_unavailable_stub().await;
    let decl = inference::decl_for_probe("openai_compatible", Some(&base_url), None, None)
        .with_chosen_model("stub-model".to_string());

    let err = probe(&decl, "stub-model", Some("embedded"))
        .await
        .expect_err("the stub rejects every model");
    assert!(
        err.to_string().contains("harness `embedded`"),
        "the hint must name the owning harness: {err}"
    );

    let err = probe(&decl, "stub-model", None)
        .await
        .expect_err("the stub rejects every model");
    assert!(
        !err.to_string().contains("harness `"),
        "with no harness in play (the first-run wizard), the hint must not invent one: {err}"
    );
}
