use axum::http::StatusCode;
use serde_json::json;

use super::inference_test_support::*;
use super::*;

use crate::company::CompanyManifest;
use crate::ports::types::CompanyId;
use crate::runtime::RuntimeBuilder;
use crate::{AppConfig, AppState};

#[tokio::test]
async fn status_reports_the_default_choice_and_each_rows_model() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert!(dto["defaultChoice"].is_null());

    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(
            json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "model": "acme-1" }),
        ),
    )
    .await;
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    let acme = dto["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "acme")
        .unwrap();
    assert_eq!(acme["model"], "acme-1");
    assert_eq!(acme["modelAmbiguous"], false);
    assert_eq!(dto["defaultChoice"]["provider"], "acme");
    assert_eq!(dto["defaultChoice"]["model"], "acme-1");
}

/// Pure: the bare-slug/unset/full mappings, independent of a store.
#[test]
fn the_status_maps_a_bare_slug_default_to_a_null_model() {
    use crate::company::inference::store::{DefaultChoice, ModelChoice};

    assert!(default_choice_dto(&DefaultChoice::Unset, false).is_none());

    let bare = default_choice_dto(&DefaultChoice::ProviderOnly("acme".to_string()), false).unwrap();
    assert_eq!(bare.provider, "acme");
    assert!(bare.model.is_none());
    assert!(!bare.broken, "a bare-slug default is never reported broken");

    let full = default_choice_dto(
        &DefaultChoice::Full(ModelChoice {
            provider: "acme".to_string(),
            model: "acme/other-model".to_string(),
        }),
        false,
    )
    .unwrap();
    assert_eq!(full.provider, "acme");
    assert_eq!(full.model.as_deref(), Some("acme/other-model"));
    assert!(!full.broken);

    let full_broken = default_choice_dto(
        &DefaultChoice::Full(ModelChoice {
            provider: "acme".to_string(),
            model: "acme/other-model".to_string(),
        }),
        true,
    )
    .unwrap();
    assert!(full_broken.broken);
}

/// Round-3a review P2-3: only a *full* default can be reported broken —
/// see `default_full_broken`'s own doc for why a bare slug never is.
#[test]
fn default_full_broken_only_ever_fires_for_a_full_default() {
    use crate::company::inference::store::{DefaultChoice, ModelChoice};

    let full = |slug: &str| {
        DefaultChoice::Full(ModelChoice {
            provider: slug.to_string(),
            model: "m".to_string(),
        })
    };

    // The named provider is gone entirely.
    assert!(default_full_broken(
        &full("gone"),
        [("acme", true)].into_iter()
    ));
    // The named provider exists but is switched off.
    assert!(default_full_broken(
        &full("acme"),
        [("acme", false)].into_iter()
    ));
    // The named provider exists and is on: not broken.
    assert!(!default_full_broken(
        &full("acme"),
        [("acme", true)].into_iter()
    ));
    // A bare slug is never "broken" by this predicate, however stale.
    assert!(!default_full_broken(
        &DefaultChoice::ProviderOnly("gone".to_string()),
        std::iter::empty()
    ));
    // Unset is never broken.
    assert!(!default_full_broken(
        &DefaultChoice::Unset,
        std::iter::empty()
    ));
}

/// Pure: `ModelOnRow` collapses to the DTO's `(model, modelAmbiguous)`.
#[test]
fn an_ambiguous_row_reports_no_model_and_the_flag() {
    use crate::company::inference::store::ModelOnRow;

    assert_eq!(
        model_on_row_dto(ModelOnRow::Ambiguous(vec![
            "a".to_string(),
            "b".to_string()
        ])),
        (None, true)
    );
    assert_eq!(model_on_row_dto(ModelOnRow::None), (None, false));
    assert_eq!(
        model_on_row_dto(ModelOnRow::One("a".to_string())),
        (Some("a".to_string()), false)
    );
}

// ---- the in-use guard (docs/key-reworks/in-use-guards.md) ---------------

#[tokio::test]
async fn the_first_provider_and_model_added_becomes_the_default_with_no_opt_out() {
    // Decision D-first-default (X1, 2026-09-15): no `makeDefault` sent.
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
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(dto["defaultChoice"]["provider"], "first");
    assert_eq!(dto["defaultChoice"]["model"], "first-model");

    // A second provider never touches an existing default (X1's other half).
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Second", "baseUrl": UNREACHABLE, "model": "second-model" })),
    )
    .await;
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert_eq!(dto["defaultChoice"]["provider"], "first");
}

/// Round-3a review, P0: "no stored default" is not "the first provider
/// ever" — every company that predates this rework has no stored default,
/// so testing only `Unset` would silently reroute an existing company's
/// traffic onto the next thing an operator "tried out". A company with an
/// existing row must not auto-default a second one.
#[tokio::test]
async fn adding_a_second_provider_to_a_non_empty_company_never_auto_defaults() {
    use crate::company::inference::store;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    // Planted straight into the store — a row from before this feature
    // existed, with no default marker of any kind, which is exactly the
    // pre-rework shape the P0 bug mishandled.
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    store::put_provider(
        runtime.id(),
        runtime.secrets().as_ref(),
        store::ProviderDraft {
            slug: "already-here".into(),
            label: "Already here".into(),
            kind: "custom".into(),
            base_url: UNREACHABLE.into(),
            models: BTreeMap::from([("chat".to_string(), "existing-model".to_string())]),
            enabled: true,
        },
    )
    .await
    .unwrap();

    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Second", "baseUrl": UNREACHABLE, "model": "second-model" })),
    )
    .await;
    let (_, dto, raw) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert!(
        dto["defaultChoice"].is_null(),
        "a company that already had a provider row must not auto-default its next add: {raw}"
    );
}

/// Round-3a review, P0: a legacy entry-zero company (the flat
/// `inference/config` slot every pre-rework company already resolves
/// through) adding its first *console* provider must not auto-default —
/// entry zero is already an effective default, so this is not that
/// company's first provider.
#[tokio::test]
async fn adding_a_provider_to_a_legacy_entry_zero_company_never_auto_defaults() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    send(
        &state,
        "PUT",
        "/api/v1/company/inference",
        Some(json!({ "provider": "openai_compatible", "baseUrl": UNREACHABLE })),
    )
    .await;
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Second", "baseUrl": UNREACHABLE, "model": "second-model" })),
    )
    .await;
    let (_, dto, raw) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert!(
        dto["defaultChoice"].is_null(),
        "a legacy entry-zero company's next add must not auto-default: {raw}"
    );
}

/// Round-3a review, P0: same guard, for a company whose inference comes
/// from a manifest `[inference]` section rather than a console row.
#[tokio::test]
async fn adding_a_provider_to_a_manifest_inference_company_never_auto_defaults() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(
        &home,
        "acme",
        r#"[company]
name = "Acme"
[policy]
mode = "full"

[inference]
provider = "openai_compatible"
base_url = "http://127.0.0.1:9/v1"
"#,
    )
    .await;

    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(json!({ "kind": "custom", "label": "Second", "baseUrl": UNREACHABLE, "model": "second-model" })),
    )
    .await;
    let (_, dto, raw) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert!(
        dto["defaultChoice"].is_null(),
        "a manifest-inference company's first console add must not auto-default: {raw}"
    );
}

#[tokio::test]
async fn deleting_the_default_provider_is_refused_without_confirmation() {
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
        "DELETE",
        "/api/v1/company/inference/providers/acme",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{raw}");
    assert_eq!(err["code"], "in_use");
    assert!(err["error"].as_str().unwrap().contains("Acme"), "{err}");
    assert_eq!(err["usedBy"]["default"], true);

    // The row must still be there: a refusal writes nothing.
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/inference", None).await;
    assert!(
        dto["providers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["slug"] == "acme"),
        "{dto}"
    );

    // Confirmed, it proceeds and echoes what it broke.
    let (status, resp, raw) = send(
        &state,
        "DELETE",
        "/api/v1/company/inference/providers/acme?confirmInUse=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(resp["usedBy"]["default"], true);
}

/// Keys rework, issue #2306, slice 3a: a provider named by an agent's own
/// pin is `usedBy` on every status read and refused on delete without
/// confirmation — the counterpart of the `default` guard above, now that
/// `Agent.provider` exists to name.
#[tokio::test]
async fn a_provider_pinned_by_an_agent_is_used_by_that_agent() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let id = CompanyId::new("acme");
    let pinned_manifest: CompanyManifest = toml::from_str(
        r#"[company]
name = "Acme"
[policy]
mode = "full"

[[agent]]
id = "researcher"
role = "Researcher"
provider = "acme"
model = "test-model-large"

[[agent]]
id = "writer"
role = "Writer"
"#,
    )
    .unwrap();
    save_record(&home, &id, &pinned_manifest).await;
    let runtime = RuntimeBuilder::new(home.clone(), pinned_manifest)
        .with_id(id.clone())
        .build()
        .await
        .unwrap();
    let state = AppState::new(AppConfig::default());
    state.registry().insert(id, std::sync::Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, "acme").await;

    // The provider does not exist yet — no row, no default, no pin can
    // resolve — so nothing is used yet.
    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(
            json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "key": "sk-not-a-real-key", "model": "test-model-large" }),
        ),
    )
    .await;

    // Status names the pinning agent on the row itself.
    let (_, dto, raw) = send(&state, "GET", "/api/v1/company/inference", None).await;
    let acme = dto["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "acme")
        .unwrap();
    let agent_ids: Vec<&str> = acme["usedBy"]["agents"]
        .as_array()
        .unwrap_or_else(|| panic!("no usedBy.agents on {acme}: {raw}"))
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(agent_ids, vec!["researcher"], "{acme}");
    assert_eq!(
        acme["usedBy"]["agents"][0]["name"], "Researcher",
        "the display name, not the bare id: {acme}"
    );
    // The writer, which names no pair, must not appear.
    assert!(
        !agent_ids.contains(&"writer"),
        "an agent with no pair must not be counted: {acme}"
    );

    // Deleting the pinned provider is refused the same way the default is.
    let (status, err, raw) = send(
        &state,
        "DELETE",
        "/api/v1/company/inference/providers/acme",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{raw}");
    assert_eq!(err["code"], "in_use");
    assert_eq!(
        err["usedBy"]["agents"][0]["id"], "researcher",
        "the refusal must name the pinning agent: {err}"
    );

    // Confirmed, it proceeds and echoes the same agent.
    let (status, resp, raw) = send(
        &state,
        "DELETE",
        "/api/v1/company/inference/providers/acme?confirmInUse=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(resp["usedBy"]["agents"][0]["id"], "researcher");
}

/// Bug KR-L2-01 (live E2E): the same guard, but the pin is set through the
/// real write path an operator actually uses — `PATCH …/team/{id}` on a
/// manifest teammate with no pair of its own yet, which stores an
/// `AgentOverride` rather than editing `company.toml`. `usedBy.agents`
/// must see it exactly as it sees a manifest-declared pair.
#[tokio::test]
async fn a_provider_pinned_through_the_team_patch_route_is_used_by_that_agent() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let id = CompanyId::new("acme");
    let manifest: CompanyManifest = toml::from_str(
        r#"[company]
name = "Acme"
[policy]
mode = "full"

[[agent]]
id = "researcher"
role = "Researcher"
"#,
    )
    .unwrap();
    save_record(&home, &id, &manifest).await;
    let runtime = RuntimeBuilder::new(home, manifest)
        .with_id(id)
        .build()
        .await
        .unwrap();
    let state = AppState::new(AppConfig::default());
    state
        .registry()
        .insert(CompanyId::new("acme"), std::sync::Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, "acme").await;

    send(
        &state,
        "POST",
        "/api/v1/company/inference/providers",
        Some(
            json!({ "kind": "custom", "label": "Acme", "baseUrl": UNREACHABLE, "key": "sk-not-a-real-key", "model": "test-model-large" }),
        ),
    )
    .await;

    // The pin is set through the team PATCH route, not the manifest.
    let (status, patched, raw) = send(
        &state,
        "PATCH",
        "/api/v1/company/team/researcher",
        Some(json!({ "provider": "acme", "model": "test-model-large" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(patched["provider"], "acme", "{patched}");

    let (_, dto, raw) = send(&state, "GET", "/api/v1/company/inference", None).await;
    let acme = dto["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "acme")
        .unwrap();
    let agent_ids: Vec<&str> = acme["usedBy"]["agents"]
        .as_array()
        .unwrap_or_else(|| panic!("no usedBy.agents on {acme}: {raw}"))
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(agent_ids, vec!["researcher"], "{acme}");

    let (status, err, raw) = send(
        &state,
        "DELETE",
        "/api/v1/company/inference/providers/acme",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{raw}");
    assert_eq!(err["code"], "in_use");
    assert_eq!(err["usedBy"]["agents"][0]["id"], "researcher", "{err}");
}

#[tokio::test]
async fn disabling_the_default_provider_is_refused_without_confirmation() {
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
        "/api/v1/company/inference/providers/acme/enabled",
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{raw}");
    assert_eq!(err["code"], "in_use");

    let (status, resp, raw) = send(
        &state,
        "POST",
        "/api/v1/company/inference/providers/acme/enabled",
        Some(json!({ "enabled": false, "confirmInUse": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(resp["usedBy"]["default"], true);
}
