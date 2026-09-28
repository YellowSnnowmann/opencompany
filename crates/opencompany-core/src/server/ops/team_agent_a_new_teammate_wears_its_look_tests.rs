use axum::http::StatusCode;
use serde_json::json;

use super::team_agent_test_support::*;

const LOOK: &str = r#""avatar": "mascot:animated", "mascotMode": "static", "mascotCostume": "habibi", "mascotSkinColor": "mint", "mascotHandColor": "charcoal""#;

fn robin(extra: &str) -> serde_json::Value {
    serde_json::from_str(&format!(
        r#"{{"name": "Robin", "role": "Support", {extra}}}"#
    ))
    .unwrap()
}

/// A teammate created with a look is born wearing it: the response, the roster
/// row and the detail read all say so, so the console draws the new card in it
/// from the first frame instead of flashing the default until a second write.
#[tokio::test]
async fn a_teammate_is_born_wearing_the_look_it_was_created_with() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), ROSTER).await;

    let (status, created) = send(&state, "POST", "/api/v1/company/team", Some(robin(LOOK))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap().to_string();

    let (_, roster) = send(&state, "GET", "/api/v1/company/team", None).await;
    let row = roster
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id.as_str())
        .expect("the new teammate is on the roster")
        .clone();
    let (_, detail) = get_agent(&state, &id).await;

    for (label, body) in [("created", &created), ("roster", &row), ("detail", &detail)] {
        assert_eq!(body["avatar"], "mascot:animated", "{label}: {body}");
        assert_eq!(body["mascotMode"], "static", "{label}: {body}");
        assert_eq!(body["mascotCostume"], "habibi", "{label}: {body}");
        assert_eq!(body["mascotSkinColor"], "mint", "{label}: {body}");
        assert_eq!(body["mascotHandColor"], "charcoal", "{label}: {body}");
    }
}

/// Only what was chosen is stored: a partial look leaves the rest at the file's
/// own default rather than writing four values.
#[tokio::test]
async fn a_partial_look_stores_only_what_was_chosen() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), ROSTER).await;

    let (status, created) = send(
        &state,
        "POST",
        "/api/v1/company/team",
        Some(robin(r#""mascotCostume": "glass2""#)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["mascotCostume"], "glass2", "{created}");
    for absent in ["mascotMode", "mascotSkinColor", "mascotHandColor"] {
        assert!(created.get(absent).is_none(), "{absent}: {created}");
    }
    let (_, detail) = get_agent(&state, created["id"].as_str().unwrap()).await;
    assert_eq!(detail["mascotCostume"], "glass2", "{detail}");
    assert!(detail.get("mascotSkinColor").is_none(), "{detail}");
}

/// At creation there is nothing to reset to, so omitted, `null` and blank are
/// the same "no choice" — nothing stored, and the keys stay off the wire.
#[tokio::test]
async fn omitted_null_and_blank_are_all_no_choice() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), ROSTER).await;

    for body in [
        robin(r#""mascotCostume": null"#),
        robin(r#""mascotCostume": "", "mascotSkinColor": "   ""#),
        json!({"name": "Robin", "role": "Support"}),
    ] {
        let (status, created) = send(&state, "POST", "/api/v1/company/team", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{created}");
        for absent in [
            "mascotMode",
            "mascotCostume",
            "mascotSkinColor",
            "mascotHandColor",
        ] {
            assert!(created.get(absent).is_none(), "{absent}: {created}");
        }
    }
}

/// The same closed lists as the `PATCH` route, refused with the same sentence —
/// and refused *before* anything is written, so a bad look never leaves a
/// teammate behind wearing none of it.
#[tokio::test]
async fn an_unknown_value_is_refused_naming_the_accepted_set_and_creates_nobody() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), ROSTER).await;
    let (_, before) = send(&state, "GET", "/api/v1/company/team", None).await;

    for (field, value, accepted) in [
        ("mascotMode", "paused", "static"),
        ("mascotCostume", "face_mask", "cardboard_mask"),
        ("mascotSkinColor", "chartreuse", "mint"),
        ("mascotHandColor", "#ff0000", "charcoal"),
    ] {
        let extra = format!(r#""{field}": "{value}""#);
        let (status, refused) =
            send(&state, "POST", "/api/v1/company/team", Some(robin(&extra))).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{field}={value} was accepted: {refused}"
        );
        let text = refused.to_string();
        assert!(
            text.contains("Pick one of") && text.contains(accepted),
            "{field}: {text}"
        );
    }

    let (_, after) = send(&state, "GET", "/api/v1/company/team", None).await;
    assert_eq!(
        before.as_array().unwrap().len(),
        after.as_array().unwrap().len(),
        "a refused look must not create a teammate"
    );
}

/// The look decides nothing about what the company can reach, so — like the
/// face — any member may create a teammate with one. Verified as a member,
/// because a rule checked only as an admin passes identically against no rule.
#[tokio::test]
async fn a_member_may_create_a_teammate_with_a_look() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), ROSTER).await;
    crate::server::test_support::seed_fixed_member(&state, "acme").await;

    let (status, created) = send_as(
        &state,
        "POST",
        "/api/v1/company/team",
        Some(robin(LOOK)),
        crate::server::test_support::member_cookie("acme"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["mascotCostume"], "habibi", "{created}");
}

/// An older console sends none of it, and gets exactly the response it always
/// did: no face, no look, nothing new on the wire.
#[tokio::test]
async fn a_create_request_with_no_look_behaves_as_it_always_did() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), ROSTER).await;

    let (status, created) = send(
        &state,
        "POST",
        "/api/v1/company/team",
        Some(json!({"name": "Robin", "role": "Support"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert!(created.get("avatar").is_none(), "{created}");
    for absent in [
        "mascotMode",
        "mascotCostume",
        "mascotSkinColor",
        "mascotHandColor",
    ] {
        assert!(created.get(absent).is_none(), "{absent}: {created}");
    }
}
