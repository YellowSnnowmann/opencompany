use axum::http::StatusCode;
use serde_json::json;

use super::inference_test_support::*;

use crate::AppState;
use crate::ports::types::CompanyId;

#[tokio::test]
async fn disabling_keeps_the_route_and_names_the_tiers_it_parks() {
    // The departure from the plan, pinned so it is a decision rather than an
    // omission: disabling does NOT scrub. A disabled provider keeps its
    // endpoint, its label and its credential so that "stop billing this
    // account this week" is expressible, and scrubbing would make
    // re-enabling a re-configuration.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "key": "sk-not-a-real-key", "model": "acme-model" })),
    )
    .await;
    send(
        &state,
        "PUT",
        "/api/v1/company/inference/routes",
        Some(json!({ "routes": { "reasoning-v1": "acme:gpt-5" } })),
    )
    .await;

    // `acme` auto-became the company default (X1) on that first add, so
    // disabling it needs confirmation like any other in-use row.
    let (status, resp, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/acme/enabled",
        Some(json!({ "enabled": false, "confirmInUse": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    // The explicitly routed tier **and** the three unset ones. `acme` is
    // this company's only provider, so it is also what every unrouted
    // workload was going through — switching it off moves those to managed,
    // and a response naming only the explicit route would have said nothing
    // about a change of who pays for the other three.
    let mut named: Vec<String> = resp["affectedTiers"]
        .as_array()
        .expect("affectedTiers is a list")
        .iter()
        .map(|t| t.as_str().unwrap_or_default().to_string())
        .collect();
    named.sort();
    assert_eq!(
        named,
        vec![
            "agentic-v1".to_string(),
            "chat-v1".to_string(),
            "reasoning-v1".to_string(),
            "vision-v1".to_string(),
        ],
        "{raw}"
    );

    let (_, routes, _) = send(&state, "GET", "/api/v1/company/inference/routes", None).await;
    assert_eq!(
        routes["routes"]["reasoning-v1"], "acme:gpt-5",
        "the route survives so switching back on restores it: {routes}"
    );
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    let acme = dto["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "acme")
        .unwrap();
    assert_eq!(acme["enabled"], false);
    assert_eq!(
        acme["keyConfigured"], true,
        "a disabled provider keeps its credential"
    );
}

#[tokio::test]
async fn removing_the_managed_key_does_not_take_another_row_s_key_with_it() {
    // `inference/key` is ONE address that two rows can read through their
    // own legacy fallback: entry zero's, and managed's. Which of them owns
    // it depends on what entry zero's kind normalises to.
    //
    // Clearing it unconditionally while writing a *different* slug's slot
    // destroyed whatever else was reading it — on a company configured for
    // OpenRouter, removing the managed key silently took the OpenRouter key
    // with it and the row went from "•••• configured" to a bare host. Found
    // in a browser; pinned here.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    // Entry zero is OpenRouter, with its credential at the legacy address.
    let (status, _, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({ "provider": "openrouter", "key": TOKEN })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    let (status, _, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/managed/key",
        Some(json!({ "key": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    let zero = dto["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "openrouter")
        .expect("entry zero is still listed");
    assert_eq!(
        zero["keyConfigured"], true,
        "removing MANAGED's key must not clear a credential another row reads"
    );
}

#[tokio::test]
async fn a_managed_company_does_converge_off_the_legacy_address() {
    // The other half: when entry zero IS managed, the legacy slot is its
    // own, and writing the new address must retire the old one — otherwise
    // a secret is orphaned at an address nothing will ever clear.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({ "provider": "managed", "key": TOKEN })),
    )
    .await;
    let (status, _, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/managed/key",
        Some(json!({ "key": "sk-not-a-real-key" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");

    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(
        dto["managed"]["source"], "provider_key",
        "the new address is what answers now"
    );
}

#[tokio::test]
async fn the_default_is_explicit_and_survives_a_delete_that_is_not_it() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    for label in ["First", "Second"] {
        send(
            &state,
            "POST",
            "/api/v1/company/inference/providers",
            Some(json!({ "kind": "custom", "label": label, "baseUrl": UNREACHABLE, "model": "acme-model" })),
        )
        .await;
    }

    // X1 (2026-09-15): the first provider added auto-becomes the default,
    // with no marker the operator set explicitly — but the observable
    // answer is the same one "list order" used to give.
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(default_slug(&dto).as_deref(), Some("first"));

    // Marked, it is a thing the operator said rather than a thing that
    // happened.
    let (status, _, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/second/default",
        Some(json!({ "model": "second-model" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(default_slug(&dto).as_deref(), Some("second"));

    // Deleting the one that is NOT the default leaves the marker alone —
    // the failure the marker exists to prevent is the default moving with
    // list order, silently.
    send(
        &state,
        "DELETE",
        "/api/v1/company/inference/providers/first",
        None,
    )
    .await;
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(default_slug(&dto).as_deref(), Some("second"));
}

#[tokio::test]
async fn disabling_or_deleting_the_default_never_leaves_it_marked() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    for label in ["First", "Second"] {
        send(
            &state,
            "POST",
            "/api/v1/company/inference/providers",
            Some(json!({ "kind": "custom", "label": label, "baseUrl": UNREACHABLE, "model": "acme-model" })),
        )
        .await;
    }
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/second/default",
        Some(json!({ "model": "second-model" })),
    )
    .await;

    // Switched off: the stored marker itself is left exactly as it was
    // (X14) — see `a_delete_disable_or_key_clear_never_rewrites_the_stored_default_marker`
    // below for the direct assertion on the raw value — but the
    // **derived** `isDefault`/`default_slug` view reports no row at all
    // (round-3a review P2-3), not the first enabled provider. F6 means a
    // turn never falls back to `first` once the full default is broken —
    // it fails closed — so no row may claim to be serving in its place.
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/second/enabled",
        Some(json!({ "enabled": false, "confirmInUse": true })),
    )
    .await;
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(
        default_slug(&dto),
        None,
        "a broken full default must not be reported as served by a different row"
    );
    assert_eq!(dto["defaultChoice"]["provider"], "second");
    assert_eq!(dto["defaultChoice"]["broken"], true);

    // And a delete leaves the same derived view unchanged.
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/second/enabled",
        Some(json!({ "enabled": true })),
    )
    .await;
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/second/default",
        Some(json!({ "model": "second-model" })),
    )
    .await;
    send(
        &state,
        "DELETE",
        "/api/v1/company/inference/providers/second?confirmInUse=true",
        None,
    )
    .await;
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(
        default_slug(&dto),
        None,
        "a deleted full default's row is gone entirely — still no fallback claims it"
    );
    assert_eq!(dto["defaultChoice"]["broken"], true);
}

/// Keys rework (#2306), decision D-never-clear-default (X14, 2026-09-15):
/// disabling, clearing the key of, or deleting the provider
/// `inference/default` names never rewrites that **stored** value.
/// Round-3a review P2-3: the derived `isDefault`/`defaultChoice` view
/// reports the break honestly instead — no row falls back to claiming
/// `isDefault` in the broken default's place. This is the regression
/// `disabling_or_deleting_the_default_never_leaves_it_marked` above
/// cannot catch, because it only reads that derived view (which
/// already looked the same whether or not the raw marker was cleared).
#[tokio::test]
async fn a_delete_disable_or_key_clear_never_rewrites_the_stored_default_marker() {
    use crate::company::inference::store;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let id = CompanyId::new("acme");

    for label in ["First", "Second"] {
        send(
            &state,
            "POST",
            "/api/v1/company/inference/providers",
            Some(json!({ "kind": "custom", "label": label, "baseUrl": UNREACHABLE, "key": "sk-not-a-real-key", "model": "acme-model" })),
        )
        .await;
    }
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/second/default",
        Some(json!({ "model": "second-model" })),
    )
    .await;

    async fn raw_marker(state: &AppState, id: &CompanyId) -> Option<String> {
        let runtime = state.registry().get(id).expect("registered");
        let secrets = runtime.secrets();
        store::load_default_slug(id, secrets.as_ref())
            .await
            .unwrap()
    }
    assert_eq!(raw_marker(&state, &id).await.as_deref(), Some("second"));

    // Disabling it: the stored marker is untouched. `second` is in use
    // (it is the default), so the guard needs confirmation.
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/second/enabled",
        Some(json!({ "enabled": false, "confirmInUse": true })),
    )
    .await;
    assert_eq!(
        raw_marker(&state, &id).await.as_deref(),
        Some("second"),
        "a disable must not rewrite inference/default"
    );

    // Clearing its key (edit with an empty key): still untouched. Also
    // guarded, for the same reason.
    send(
        &state,
        "PUT",
        "/api/v1/company/inference/providers/second",
        Some(json!({ "key": "", "confirmInUse": true })),
    )
    .await;
    assert_eq!(
        raw_marker(&state, &id).await.as_deref(),
        Some("second"),
        "a key clear must not rewrite inference/default"
    );

    // Deleting it: still untouched, even though no row now answers to it.
    send(
        &state,
        "DELETE",
        "/api/v1/company/inference/providers/second?confirmInUse=true",
        None,
    )
    .await;
    assert_eq!(
        raw_marker(&state, &id).await.as_deref(),
        Some("second"),
        "a delete must not rewrite inference/default"
    );

    // The derived view still degrades gracefully — this is what the
    // console's status read and banner are for. No row claims to be the
    // default in `second`'s place (round-3a review P2-3), and the status
    // still names `second` as the (broken) stored choice rather than
    // hiding it.
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(default_slug(&dto), None);
    assert_eq!(dto["defaultChoice"]["provider"], "second");
    assert_eq!(dto["defaultChoice"]["broken"], true);
}

#[tokio::test]
async fn a_provider_that_is_switched_off_cannot_be_made_the_default() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "model": "acme-model" })),
    )
    .await;
    // `acme` auto-became the default on that add (X1), so disabling it
    // needs confirmation.
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/acme/enabled",
        Some(json!({ "enabled": false, "confirmInUse": true })),
    )
    .await;

    let (status, _, _) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/acme/default",
        Some(json!({ "model": "acme-model" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---- 2c: a model is required everywhere ---------------------------------

#[tokio::test]
async fn setting_a_default_requires_a_model() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(
            json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "model": "acme-1" }),
        ),
    )
    .await;

    let (status, err, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/acme/default",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{raw}");
    assert!(
        err["error"].as_str().unwrap().contains("Choose a model"),
        "{err}"
    );
}

#[tokio::test]
async fn a_default_model_may_not_be_a_tier_name_or_contain_spaces() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(
            json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "model": "acme-1" }),
        ),
    )
    .await;

    for bad in ["chat-v1", "test model", &"x".repeat(257)] {
        let (status, _, raw) = send(
            &state,
            "POST",
            "/api/v1/company/inference/providers/acme/default",
            Some(json!({ "model": bad })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {raw}");
    }
    let (status, _, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/acme/default",
        Some(json!({ "model": "x".repeat(256) })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "256 chars is exactly the bound: {raw}"
    );
}

#[tokio::test]
async fn an_add_without_a_model_is_refused_before_anything_is_written() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (status, _, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "key": "sk-not-a-real-key" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!raw.contains("sk-not-a-real-key"), "{raw}");
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert!(dto["providers"].as_array().unwrap().is_empty());

    let (status, _, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "key": "sk-not-a-real-key", "model": "acme-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
}

#[tokio::test]
async fn editing_a_providers_model_is_no_longer_silently_dropped() {
    // Round-2 console review: `EditProvider` used to take only a `models`
    // map, so the console's `model` field was silently ignored while a
    // success toast showed.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(
            json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "model": "acme-1" }),
        ),
    )
    .await;

    let (status, resp, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/inference/providers/acme",
        Some(json!({ "model": "acme-2" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    let acme = resp["status"]["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "acme")
        .unwrap();
    assert_eq!(acme["model"], "acme-2");
    assert_eq!(acme["models"]["chat-v1"], "acme-2");

    // `acme` auto-became the default (X1), so its model moved with the
    // row (2c: editing the default row's model moves the default too).
    assert_eq!(resp["status"]["defaultChoice"]["model"], "acme-2");
}
