use axum::http::StatusCode;
use serde_json::json;

use super::inference_test_support::*;

#[cfg(feature = "openhuman")]
use crate::ports::types::CompanyId;
#[cfg(feature = "openhuman")]
use crate::runtime::RuntimeBuilder;
#[cfg(feature = "openhuman")]
use crate::{AppConfig, AppState};

#[tokio::test]
async fn clearing_the_key_of_the_default_provider_is_refused_without_confirmation() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "key": "sk-not-a-real-key", "model": "acme-1" })),
    )
    .await;

    let (status, err, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/providers/acme",
        Some(json!({ "key": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{raw}");
    assert_eq!(err["code"], "in_use");

    let (status, resp, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/providers/acme",
        Some(json!({ "key": "", "confirmInUse": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(resp["usedBy"]["default"], true);
    // Q8, generalized (X14): the confirmed clear never moves the default.
    assert_eq!(resp["status"]["defaultChoice"]["provider"], "acme");
}

#[tokio::test]
async fn a_provider_not_in_use_needs_no_confirmation() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "First", "baseUrl": UNREACHABLE, "model": "first-model" })),
    )
    .await;
    // Second is not the default (X1 never moved it there), so removing it
    // needs no confirmation and its `usedBy` is absent.
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Second", "baseUrl": UNREACHABLE, "model": "second-model" })),
    )
    .await;

    let (status, resp, raw) = send(
        &state,
        "DELETE",
        "/api/v1/company/inference/providers/second",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert!(resp.get("usedBy").is_none(), "{resp}");
}

#[tokio::test]
async fn a_route_naming_a_provider_nobody_holds_is_refused() {
    // Fail closed. Accepting it and letting the turn discover it would
    // attribute that workload's spend to whatever the fallback happened to
    // be — the same defect as resolving an unknown provider kind.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, err, _) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/routes",
        Some(json!({ "routes": { "chat-v1": "ghost:gpt-5" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err["error"].as_str().unwrap_or_default().contains("ghost"),
        "the error names the slug that resolved to nothing: {err}"
    );
}

#[tokio::test]
async fn a_tier_this_runtime_does_not_have_is_refused() {
    // The five-row trap: `coding` maps onto the same `agentic-v1` tier as
    // `agentic`, so a `coding-v1` route would write one tier's route under a
    // second name and setting one would silently change the other.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, _, _) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/routes",
        Some(json!({ "routes": { "coding-v1": "managed" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_routing_mode_is_inferred_from_the_routes() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    // **The reported defect, at the HTTP boundary.** An empty table used to
    // answer `managed` on a company whose managed chain resolves to nothing,
    // while every unset row resolved to `Resolution::Primary` — the first
    // enabled provider. The screen named one destination and the turn used
    // another.
    let (_, routes, _) = send(&state, "GET", "/api/v1/company/inference/routes", None).await;
    assert_eq!(
        routes["mode"], "unset",
        "nothing set is not a mode when Managed cannot answer"
    );

    // Give the chain something to resolve to, and the same empty table is
    // genuinely Managed — the inference is about what the company can use,
    // not about the table alone.
    send(
        &state,
        "PUT",
        "/api/v1/company/inference/managed/key",
        Some(json!({ "key": "th-not-a-real-key" })),
    )
    .await;
    let (_, routes, _) = send(&state, "GET", "/api/v1/company/inference/routes", None).await;
    assert_eq!(
        routes["mode"], "managed",
        "nothing set is managed once managed answers"
    );

    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "model": "acme-model" })),
    )
    .await;
    let (_, routes, _) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/routes",
        Some(json!({ "routes": {
            "chat-v1": "acme:gpt-5",
            "reasoning-v1": "acme:gpt-5",
            "agentic-v1": "acme:gpt-5",
            "vision-v1": "acme:gpt-5",
        }})),
    )
    .await;
    assert_eq!(routes["mode"], "own", "every row the same is own");

    let (_, routes, _) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/routes",
        Some(json!({ "routes": {
            "chat-v1": "acme:gpt-5",
            "reasoning-v1": "managed",
        }})),
    )
    .await;
    assert_eq!(routes["mode"], "advanced");
}

#[tokio::test]
async fn the_only_provider_a_company_can_use_is_routed_to() {
    // §4. Nothing authored, no managed credential, one provider added: there
    // is precisely one thing in this company that can serve a turn, so
    // routing to anything else is not a choice that exists. Without this the
    // operator adds a provider, every screen says Managed, and every turn
    // goes to the provider anyway — with no per-tier model, which is the
    // reported `404 model: agentic-v1`.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (_, added, _) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "model": "acme-model" })),
    )
    .await;
    assert_eq!(
        added["affectedTiers"].as_array().map(Vec::len),
        Some(4),
        "the write says which rows it wrote rather than leaving them to be noticed: {added}"
    );

    let (_, routes, _) = send(&state, "GET", "/api/v1/company/inference/routes", None).await;
    assert_eq!(
        routes["mode"], "own",
        "one provider on every row is own: {routes}"
    );
    assert_eq!(routes["routes"]["agentic-v1"], "acme");
}

#[tokio::test]
async fn a_provider_added_beside_managed_is_not_routed_to() {
    // Row B2, and the case the guard exists for: Managed resolves, so adding
    // a key may be for one workload, for vision only, or to compare. Writing
    // all four rows would bill the operator for everything, silently, from a
    // screen that still says Managed. The answer is to ask, which is what
    // leaving the table empty does.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    send(
        &state,
        "PUT",
        "/api/v1/company/inference/managed/key",
        Some(json!({ "key": "th-not-a-real-key" })),
    )
    .await;
    let (_, added, _) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE })),
    )
    .await;
    assert!(
        added["affectedTiers"]
            .as_array()
            .is_none_or(|tiers| tiers.is_empty()),
        "nothing was routed on the operator's behalf: {added}"
    );

    let (_, routes, _) = send(&state, "GET", "/api/v1/company/inference/routes", None).await;
    assert_eq!(
        routes["mode"], "managed",
        "the table is still empty: {routes}"
    );
}

#[tokio::test]
async fn the_draft_probe_refuses_the_metadata_address() {
    // The SSRF answer, made explicitly rather than inherited. That range is
    // where a container's credentials live and a company's model endpoint is
    // never there.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, resp, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/probe",
        Some(json!({ "baseUrl": "http://169.254.169.254/latest/meta-data", "key": "sk-not-a-real-key" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(resp["ok"], false);
    assert_eq!(resp["class"], "endpoint");
}

// --- Issue #266: a save the running brain cannot honour --------------------

/// On a host with no harness reachable, a saved config is *never* "restart
/// pending" — the echo brain is where this build ends up no matter how many
/// times it is restarted, and telling the operator otherwise would send them
/// bouncing a process for nothing.
///
/// This is the default build, so it is also the guard that keeps the flag
/// from firing on every self-hosted instance that simply has no local
/// inference compiled in.
#[tokio::test]
async fn a_save_is_not_restart_pending_when_no_restart_would_help() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, resp, _) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({ "provider": "openrouter", "key": TOKEN })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // The config landed...
    assert_eq!(resp["status"]["source"], "runtime");
    assert_eq!(resp["status"]["keyConfigured"], true);
    // ...and the runtime is on the echo brain, but no harness is reachable
    // here, so there is nothing a restart would change.
    assert_eq!(resp["status"]["cognition"], "echo");
    assert_eq!(resp["status"]["restartRequired"], false);
    assert!(
        resp["note"]
            .as_str()
            .unwrap_or_default()
            .contains("no restart needed"),
        "{}",
        resp["note"]
    );

    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(dto["restartRequired"], false);
}

/// A company with nothing configured has nothing stranded, so the flag is
/// off even before any save.
#[tokio::test]
async fn an_unconfigured_company_is_not_restart_pending() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(dto["source"], "managed");
    assert_eq!(dto["restartRequired"], false);
}

/// Issue #266, reproduced at the route: a company built with a harness pool
/// but **no** inference source boots onto the echo brain with an unwired
/// workflow runner. Storing a credential afterwards updates the secret store
/// and nothing else — the brain is chosen in `RuntimeBuilder::build` and this
/// one already ran.
///
/// So the save must report `restartRequired`, the note must say restart
/// rather than "next turn", and `POST …/workflows/{id}/run` must stop
/// claiming the deployment lacks workflow execution when what it lacks is a
/// restart.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn configuring_inference_after_boot_reports_restart_required() {
    use crate::harness::HarnessPool;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();

    // Build the company the way the serve path does — with a harness pool
    // attached — but with no inference source of any kind. That is the boot
    // this issue is about.
    let id = CompanyId::new("acme");
    let runtime = RuntimeBuilder::new(home.clone(), manifest())
        .with_id(id.clone())
        .with_harness(std::sync::Arc::new(HarnessPool::new()))
        .build()
        .await
        .unwrap();
    // The bug's precondition, asserted rather than assumed: the harness was
    // available and the company still landed on the offline brain.
    assert_eq!(
        runtime.cognition().path,
        "echo",
        "expected the no-inference boot to select the echo brain"
    );
    assert!(
        runtime.workflow_runner().is_none(),
        "expected the no-inference boot to leave the workflow runner unwired"
    );

    let state = AppState::new(AppConfig::default());
    state.registry().insert(id, std::sync::Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, "acme").await;

    // Configure inference, exactly as the console does.
    let (status, resp, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({
            "provider": "openai_compatible",
            "baseUrl": "https://stub.invalid/v1",
            "key": TOKEN,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(resp["status"]["source"], "runtime");
    assert_eq!(resp["status"]["keyConfigured"], true);
    // The config resolves now; the running brain still does not know it.
    assert_eq!(resp["status"]["cognition"], "echo");
    assert_eq!(resp["status"]["restartRequired"], true, "{raw}");
    // A pool is attached on this build, so the design path is reachable —
    // the flag the setup dialog reads to keep the "set up a model" CTA.
    assert_eq!(resp["status"]["harnessReachable"], true, "{raw}");

    let note = resp["note"].as_str().unwrap_or_default();
    assert!(note.contains("restart"), "note must say restart: {note}");
    assert!(
        !note.contains("next turn"),
        "note must not promise the next turn: {note}"
    );
    assert!(!raw.contains(TOKEN), "PUT response leaked the token: {raw}");

    // The flag is a property of the runtime, not of the mutation, so a plain
    // read reports it too — this is what keeps the warning on screen after
    // the toast is gone.
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(dto["restartRequired"], true);

    // Second surface: the run route no longer blames the deployment.
    let (status, err, _) = send(
        &state,
        "POST",
        "/api/v1/company/workflows/daily/run",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err["code"], "restart_required");
    assert!(
        err["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Restart"),
        "{err}"
    );

    // Reverting to managed un-strands the company: there is no longer a
    // saved config waiting on a restart, so the flag clears.
    let (_, resp, _) = send(&state, "DELETE", "/api/v1/company/inference", None).await;
    assert_eq!(resp["status"]["restartRequired"], false);

    // #514: ...and with nothing pending BUT the harness reachable here,
    // the run route no longer 404s "no workflow execution" — it names the
    // real dead end, that this company never configured inference. (Before
    // #514 this asserted 404 `not_wired`; that was the third, unhandled
    // cause the console read as a permanent gap and degraded to read-only.)
    let (status, err, raw) = send(
        &state,
        "POST",
        "/api/v1/company/workflows/daily/run",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{raw}");
    assert_eq!(err["code"], "inference_required");
}
