use super::*;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::company::CompanyManifest;
use crate::ports::types::{CompanyId, EventSeq};
use crate::runtime::RuntimeBuilder;
use crate::server::router;
use crate::{AppConfig, AppState};

const MANIFEST: &str = "[company]\nname = \"Provisional Co\"\n\
     [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n";

fn home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("oc-company-profile-")
        .tempdir()
        .expect("tempdir")
}

/// Builds straight from an empty store — no pre-seeded [`CompanyRecord`] —
/// so this is a genuinely first-ever boot from `RuntimeBuilder::build`'s
/// own point of view (`existing: None`).
///
/// A pre-seed-then-build pattern (as `ops::policy`'s own tests use, which
/// this used to copy) makes `existing: Some(record with lifecycle:
/// "running", activation_completed_at: None)` true from the very first
/// `.build()` call — the exact shape the "running and unlatched"
/// grandfather back-fill (issue #1843) matches, which stamps
/// `name_confirmed: true` immediately regardless of what the pre-seeded
/// record actually said. That is fine for `policy`'s tests, which never
/// assert on `name_confirmed`; it silently defeats every assertion here,
/// which is exactly what `name_confirmed` starting `false` is supposed to
/// prove. Building with nothing pre-seeded is the one shape the migration
/// does not grandfather (see `RuntimeBuilder::build`'s own `None => (false,
/// None)` arm), so `name_confirmed` starts `false` as this module's tests
/// need it to.
async fn state(home: &std::path::Path) -> AppState {
    let manifest: CompanyManifest = toml::from_str(MANIFEST).unwrap();
    let id = CompanyId::new("acme");
    let runtime = RuntimeBuilder::new(home.to_path_buf(), manifest)
        .with_id(id.clone())
        .build()
        .await
        .unwrap();
    let state = AppState::new(AppConfig::default());
    state.registry().insert(id, std::sync::Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, "acme").await;
    state
}

async fn call(state: &AppState, name: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("PATCH")
        .uri("/api/v1/company")
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "name": name }).to_string()))
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn empty_name_is_refused() {
    let dir = home();
    let state = state(dir.path()).await;
    let (status, _) = call(&state, "   ").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let id = CompanyId::new("acme");
    let store = state.registry().get(&id).unwrap().store().clone();
    let reloaded = store.load(&id).await.unwrap().unwrap();
    assert!(
        !reloaded.name_confirmed,
        "a refused write must not stamp the flag"
    );
}

/// PR #1875 review finding: an unbounded name is embedded verbatim into
/// every agent's system prompt (`persona_prompt`,
/// `src/company/prompt.rs`), so one oversized paste (an accidental
/// pasted document is enough) inflates every model request until
/// context limits are exceeded and workflows stop running.
#[tokio::test]
async fn an_oversized_name_is_refused() {
    let dir = home();
    let state = state(dir.path()).await;
    let too_long = "x".repeat(COMPANY_NAME_MAX_CHARS + 1);
    let (status, _) = call(&state, &too_long).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let id = CompanyId::new("acme");
    let store = state.registry().get(&id).unwrap().store().clone();
    let reloaded = store.load(&id).await.unwrap().unwrap();
    assert_eq!(
        reloaded.manifest.company.name, "Provisional Co",
        "a refused oversized name must not be persisted"
    );
    assert!(
        !reloaded.name_confirmed,
        "a refused write must not stamp the flag"
    );
}

/// A name sitting exactly at the limit is still an ordinary rename.
#[tokio::test]
async fn a_name_at_the_limit_is_accepted() {
    let dir = home();
    let state = state(dir.path()).await;
    let at_limit = "x".repeat(COMPANY_NAME_MAX_CHARS);
    let (status, body) = call(&state, &at_limit).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], at_limit);
}

/// PR #1875 review finding: the limit must count characters, not UTF-8
/// bytes. `COMPANY_NAME_MAX_CHARS` (200) is what the API error message
/// and the console `maxLength` both advertise to the operator, so a
/// name made of 200 multibyte characters — well within what both surfaces
/// promise — must be accepted even though it is 600 bytes on the wire.
#[tokio::test]
async fn a_multibyte_name_at_the_char_limit_is_accepted() {
    let dir = home();
    let state = state(dir.path()).await;
    // "あ" is 3 UTF-8 bytes; 200 of them is 200 chars / 600 bytes.
    let at_limit = "あ".repeat(COMPANY_NAME_MAX_CHARS);
    let (status, body) = call(&state, &at_limit).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], at_limit);
}

/// The same limit still refuses a name one character past it, even when
/// every character is multibyte.
#[tokio::test]
async fn a_multibyte_name_over_the_char_limit_is_refused() {
    let dir = home();
    let state = state(dir.path()).await;
    let over_limit = "あ".repeat(COMPANY_NAME_MAX_CHARS + 1);
    let (status, _) = call(&state, &over_limit).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn sets_name_and_stamps_the_flag() {
    let dir = home();
    let state = state(dir.path()).await;
    let (status, body) = call(&state, "  Real Name  ").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Real Name");
    assert_eq!(body["nameConfirmed"], true);

    let id = CompanyId::new("acme");
    let store = state.registry().get(&id).unwrap().store().clone();
    let reloaded = store.load(&id).await.unwrap().unwrap();
    assert_eq!(reloaded.manifest.company.name, "Real Name");
    assert!(reloaded.name_confirmed);
}

#[tokio::test]
async fn the_step_event_is_journaled_only_once() {
    let dir = home();
    let state = state(dir.path()).await;
    let id = CompanyId::new("acme");
    let events = state.registry().get(&id).unwrap().events().clone();

    for name in ["First Name", "Second Name"] {
        let (status, _) = call(&state, name).await;
        assert_eq!(status, StatusCode::OK);
    }

    let stored = events
        .read_from(&id, EventSeq::new(0), usize::MAX)
        .await
        .unwrap();
    let step_events = stored
        .iter()
        .filter(|entry| {
            matches!(
                &entry.event,
                CompanyEvent::OnboardingStepCompleted {
                    step: OnboardingStep::NameConfirmed
                }
            )
        })
        .count();
    assert_eq!(step_events, 1, "two renames must journal the step once");
}

/// `require_admin` is called in the handler body rather than expressed
/// through an `AdminScopedCompany` extractor (see the module doc comment),
/// so nothing at the type level proves a plain member is refused — that
/// authority check has to be exercised over HTTP like any other route's.
#[tokio::test]
async fn a_member_is_refused() {
    let dir = home();
    let state = state(dir.path()).await;
    crate::server::test_support::seed_fixed_member(&state, "acme").await;

    let request = Request::builder()
        .method("PATCH")
        .uri("/api/v1/company")
        .header("cookie", crate::server::test_support::member_cookie("acme"))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "name": "Member's Choice" }).to_string()))
        .unwrap();
    let response = router(state.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let id = CompanyId::new("acme");
    let store = state.registry().get(&id).unwrap().store().clone();
    let reloaded = store.load(&id).await.unwrap().unwrap();
    assert_eq!(
        reloaded.manifest.company.name, "Provisional Co",
        "a refused rename must not touch the stored name"
    );
    assert!(!reloaded.name_confirmed);
}
