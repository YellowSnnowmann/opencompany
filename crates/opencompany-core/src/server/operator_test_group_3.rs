use super::*;
use crate::company::CompanyManifest;
use crate::ports::types::EventSeq;
use crate::server::router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::operator_test_support_1::*;
use super::operator_test_support_2::*;

/// `add_desk_member` must serialize its load-modify-save cycle against
/// `company_write_lock`, exactly like every other console load-modify-save
/// write (`put_logo`, `set_lifecycle`, `patch_company`) — otherwise it can
/// silently revert a concurrent rename: `patch_company` is guarded by
/// `company_write_lock` alone, so a desk write racing in on only the
/// unrelated `serial` cycle lock can load the pre-rename record and save
/// the whole thing back after the rename lands (PR #1875 review finding).
/// Proven the same way `put_logo_serializes_against_the_company_write_lock`
/// proves it: hold the lock externally, drive the real handler through the
/// router, and demand it cannot finish while the lock is held.
#[tokio::test]
async fn add_desk_member_serializes_against_the_company_write_lock() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(&home, desk_manifest()).await;
    let app = router(state);
    let cookie = crate::server::test_support::fixed_cookie("acme");
    let id = CompanyId::new("acme");

    let lock = company_write_lock(&id);
    let guard = lock.lock().await;

    let app_for_task = app.clone();
    let cookie_for_task = cookie.clone();
    let mut task = tokio::spawn(async move {
        app_for_task
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/company/desks/studio/members")
                    .header("cookie", &cookie_for_task)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"agent_id":"eng"}"#))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    });

    // The handler must be blocked behind the held lock — give it every
    // chance to (wrongly) race ahead before declaring it stuck.
    let raced_ahead = tokio::time::timeout(std::time::Duration::from_millis(200), &mut task)
        .await
        .is_ok();
    assert!(
        !raced_ahead,
        "add_desk_member completed while company_write_lock was held \
         elsewhere — it is not serializing its load-modify-save cycle \
         against concurrent `ops` writers"
    );

    drop(guard);
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("add_desk_member never resumed after the lock was released")
        .expect("add_desk_member task panicked");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// `set_desk_order` must serialize against `company_write_lock` too — same
/// load-modify-save shape and same finding as `add_desk_member`'s own test
/// above (PR #1875 review finding, round 9: the earlier fix covered five
/// handlers but this coverage only proved it for one).
#[tokio::test]
async fn set_desk_order_serializes_against_the_company_write_lock() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(&home, desk_manifest()).await;
    let app = router(state);
    let cookie = crate::server::test_support::fixed_cookie("acme");
    let id = CompanyId::new("acme");
    seed_overlay_eng(&app, &cookie).await;

    let lock = company_write_lock(&id);
    let guard = lock.lock().await;

    let app_for_task = app.clone();
    let cookie_for_task = cookie.clone();
    let mut task = tokio::spawn(async move {
        put_desk_order(
            &app_for_task,
            &cookie_for_task,
            "studio",
            r#"{"ordered_member_ids":["eng","ceo"]}"#,
        )
        .await
    });

    let raced_ahead = tokio::time::timeout(std::time::Duration::from_millis(200), &mut task)
        .await
        .is_ok();
    assert!(
        !raced_ahead,
        "set_desk_order completed while company_write_lock was held \
         elsewhere — it is not serializing its load-modify-save cycle \
         against concurrent `ops` writers"
    );

    drop(guard);
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("set_desk_order never resumed after the lock was released")
        .expect("set_desk_order task panicked");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// `remove_desk_member` must serialize against `company_write_lock` too
/// (PR #1875 review finding, round 9 — see
/// `set_desk_order_serializes_against_the_company_write_lock`'s own doc).
#[tokio::test]
async fn remove_desk_member_serializes_against_the_company_write_lock() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(&home, desk_manifest()).await;
    let app = router(state);
    let cookie = crate::server::test_support::fixed_cookie("acme");
    let id = CompanyId::new("acme");
    seed_overlay_eng(&app, &cookie).await;

    let lock = company_write_lock(&id);
    let guard = lock.lock().await;

    let app_for_task = app.clone();
    let cookie_for_task = cookie.clone();
    let mut task = tokio::spawn(async move {
        app_for_task
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/company/desks/studio/members/eng")
                    .header("cookie", &cookie_for_task)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    });

    let raced_ahead = tokio::time::timeout(std::time::Duration::from_millis(200), &mut task)
        .await
        .is_ok();
    assert!(
        !raced_ahead,
        "remove_desk_member completed while company_write_lock was held \
         elsewhere — it is not serializing its load-modify-save cycle \
         against concurrent `ops` writers"
    );

    drop(guard);
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("remove_desk_member never resumed after the lock was released")
        .expect("remove_desk_member task panicked");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// `create_desk` must serialize against `company_write_lock` too (PR #1875
/// review finding, round 9 — see
/// `set_desk_order_serializes_against_the_company_write_lock`'s own doc).
#[tokio::test]
async fn create_desk_serializes_against_the_company_write_lock() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(&home, desk_manifest()).await;
    let app = router(state);
    let cookie = crate::server::test_support::fixed_cookie("acme");
    let id = CompanyId::new("acme");

    let lock = company_write_lock(&id);
    let guard = lock.lock().await;

    let app_for_task = app.clone();
    let cookie_for_task = cookie.clone();
    let mut task = tokio::spawn(async move {
        app_for_task
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/company/desks")
                    .header("cookie", &cookie_for_task)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Growth","members":["eng"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    });

    let raced_ahead = tokio::time::timeout(std::time::Duration::from_millis(200), &mut task)
        .await
        .is_ok();
    assert!(
        !raced_ahead,
        "create_desk completed while company_write_lock was held \
         elsewhere — it is not serializing its load-modify-save cycle \
         against concurrent `ops` writers"
    );

    drop(guard);
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("create_desk never resumed after the lock was released")
        .expect("create_desk task panicked");
    assert_eq!(status, StatusCode::CREATED);
}

/// `delete_desk` must serialize against `company_write_lock` too (PR #1875
/// review finding, round 9 — see
/// `set_desk_order_serializes_against_the_company_write_lock`'s own doc).
#[tokio::test]
async fn delete_desk_serializes_against_the_company_write_lock() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(&home, desk_manifest()).await;
    let app = router(state);
    let cookie = crate::server::test_support::fixed_cookie("acme");
    let id = CompanyId::new("acme");

    // Create the overlay desk to delete before taking the lock — this
    // test proves serialization on the delete path, not the create path.
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/company/desks")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Growth","members":["eng"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let lock = company_write_lock(&id);
    let guard = lock.lock().await;

    let app_for_task = app.clone();
    let cookie_for_task = cookie.clone();
    let mut task = tokio::spawn(async move {
        app_for_task
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/company/desks/growth")
                    .header("cookie", &cookie_for_task)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    });

    let raced_ahead = tokio::time::timeout(std::time::Duration::from_millis(200), &mut task)
        .await
        .is_ok();
    assert!(
        !raced_ahead,
        "delete_desk completed while company_write_lock was held \
         elsewhere — it is not serializing its load-modify-save cycle \
         against concurrent `ops` writers"
    );

    drop(guard);
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("delete_desk never resumed after the lock was released")
        .expect("delete_desk task panicked");
    assert_eq!(status, StatusCode::NO_CONTENT);
}

/// A desk that declares no `hive` block still reports the numbers the
/// runtime would derive, rather than blanks.
///
/// This is the difference the whole DTO exists for: the manifest says
/// nothing, so `declared` is empty — but the desk would still run on a
/// budget and a quorum, and a console showing an empty form would be
/// describing a desk that does not exist.
#[tokio::test]
async fn desk_hive_reports_derived_numbers_for_an_undeclared_block() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let manifest: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [[agent]]\nid = \"a\"\nrole = \"A\"\n\
         [[agent]]\nid = \"b\"\nrole = \"B\"\n\
         [[agent]]\nid = \"c\"\nrole = \"C\"\n\
         [[group_chat]]\nid = \"solvers\"\nname = \"Solvers\"\nmembers = [\"a\", \"b\", \"c\"]\n",
    )
    .unwrap();
    let state = state_with_manifest(&home, manifest).await;
    let app = router(state);
    let cookie = crate::server::test_support::fixed_cookie("acme");

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/company/desks/solvers/hive")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["source"], "manifest");
    assert_eq!(body["deliberates"], true);
    // Three seats: 3 x members, and a majority that still leaves somebody out.
    assert_eq!(body["effective"]["turnBudget"], 9);
    assert_eq!(body["effective"]["quorum"], 2);
    // Nothing was declared, so the authored block is empty — which is
    // exactly what distinguishes it from an operator who wrote `9`.
    assert_eq!(body["declared"], serde_json::json!({}));
    // Every seat holds every move until a table narrows one.
    assert_eq!(body["seats"].as_array().unwrap().len(), 3);
    assert_eq!(body["seats"][0]["governed"], false);
    assert_eq!(body["seats"][0]["moves"].as_array().unwrap().len(), 9);
    assert_eq!(body["eligibleSupporters"], 3);
    assert_eq!(body["reachesQuorum"], true);
}

/// Installing a grammar takes effect, is reported back derived, and is
/// undone by a reset — without the manifest ever being rewritten.
#[tokio::test]
async fn a_move_grammar_installs_and_resets() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let manifest: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [[agent]]\nid = \"a\"\nrole = \"A\"\n\
         [[agent]]\nid = \"b\"\nrole = \"B\"\n\
         [[agent]]\nid = \"c\"\nrole = \"C\"\n\
         [[group_chat]]\nid = \"solvers\"\nname = \"Solvers\"\nmembers = [\"a\", \"b\", \"c\"]\n",
    )
    .unwrap();
    let state = state_with_manifest(&home, manifest).await;
    let app = router(state);
    let cookie = crate::server::test_support::fixed_cookie("acme");

    let install = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/company/desks/solvers/hive")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"quorum":2,"moves":{"a":["propose","support"],"b":["object","evidence"],"c":["support"]}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(install.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(install.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["source"], "overlay");
    assert_eq!(body["effective"]["quorum"], 2);
    // `b` was narrowed to object/evidence, and still keeps the three no
    // table can take away.
    let b = body["seats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|seat| seat["agentId"] == "b")
        .unwrap()
        .clone();
    assert_eq!(b["governed"], true);
    let b_moves: Vec<String> = serde_json::from_value(b["moves"].clone()).unwrap();
    assert!(b_moves.contains(&"commit".to_string()));
    assert!(b_moves.contains(&"question".to_string()));
    assert!(b_moves.contains(&"defer".to_string()));
    assert!(!b_moves.contains(&"propose".to_string()));
    // a and c may support or propose; b may not. Two clears a quorum of two.
    assert_eq!(body["eligibleSupporters"], 2);
    assert_eq!(body["reachesQuorum"], true);

    let reset = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/company/desks/solvers/hive")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reset.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(reset.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    // Back to the blueprint, which declared nothing.
    assert_eq!(body["source"], "manifest");
    assert_eq!(body["declared"], serde_json::json!({}));
    assert_eq!(body["eligibleSupporters"], 3);
}

/// The runtime refuses exactly what a manifest carrying the same block
/// would be refused for — and in the same words.
#[tokio::test]
async fn installing_an_unreachable_quorum_is_refused() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let manifest: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [[agent]]\nid = \"a\"\nrole = \"A\"\n\
         [[agent]]\nid = \"b\"\nrole = \"B\"\n\
         [[agent]]\nid = \"c\"\nrole = \"C\"\n\
         [[group_chat]]\nid = \"solvers\"\nname = \"Solvers\"\nmembers = [\"a\", \"b\", \"c\"]\n",
    )
    .unwrap();
    let state = state_with_manifest(&home, manifest).await;
    let app = router(state);
    let cookie = crate::server::test_support::fixed_cookie("acme");

    // Only `a` may deposit a supporter, but the quorum asks for two — the
    // room could never decide anything however much it agreed.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/company/desks/solvers/hive")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"quorum":2,"moves":{"a":["propose"],"b":["object"],"c":["object"]}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // An unknown move kind, and an id that is not on the desk, are refused
    // for the same reason: both fail *open* at runtime, handing the seat
    // every move and letting the desk quietly go on voting.
    for body in [
        r#"{"moves":{"a":["shrug"]}}"#,
        r#"{"moves":{"ghost":["propose"]}}"#,
    ] {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/company/desks/solvers/hive")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "body {body} was accepted"
        );
    }
}

/// The structural rows a console draws its activity graph from.
///
/// Before these, "who created this desk" and "who moved this seat" were
/// answerable only from a live frame that does not survive a reload.
#[tokio::test]
async fn desk_lifecycle_is_journaled() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(&home, desk_manifest()).await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    let events = runtime.events();
    let app = router(state);
    let cookie = crate::server::test_support::fixed_cookie("acme");

    for (method, uri, body) in [
        (
            "POST",
            "/api/v1/company/team",
            Some(r#"{"name":"Dana","role":"Analyst"}"#),
        ),
        (
            "POST",
            "/api/v1/company/desks",
            Some(r#"{"name":"Growth","members":["eng"]}"#),
        ),
        (
            "POST",
            "/api/v1/company/desks/growth/members",
            Some(r#"{"agent_id":"ceo"}"#),
        ),
        (
            "PUT",
            "/api/v1/company/desks/growth/hive",
            Some(r#"{"quorum":1}"#),
        ),
        ("DELETE", "/api/v1/company/desks/growth/hive", None),
        ("DELETE", "/api/v1/company/desks/growth/members/ceo", None),
        ("DELETE", "/api/v1/company/desks/growth", None),
    ] {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", &cookie);
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let res = app
            .clone()
            .oneshot(req.body(body.map_or(Body::empty(), Body::from)).unwrap())
            .await
            .unwrap();
        assert!(
            res.status().is_success(),
            "{method} {uri} answered {}",
            res.status()
        );
    }

    let rows = events
        .read_from(&CompanyId::new("acme"), EventSeq::new(0), 500)
        .await
        .unwrap();
    let kinds: Vec<&str> = rows.iter().map(|row| row.event.kind()).collect();
    assert!(kinds.contains(&"DeskCreated"), "kinds: {kinds:?}");
    assert!(kinds.contains(&"DeskDeleted"), "kinds: {kinds:?}");
    assert!(kinds.contains(&"TeammateAdded"), "kinds: {kinds:?}");
    assert_eq!(
        kinds.iter().filter(|k| **k == "DeskHiveConfigured").count(),
        2,
        "one row for install and one for reset: {kinds:?}"
    );
    assert_eq!(
        kinds.iter().filter(|k| **k == "DeskMembersChanged").count(),
        2,
        "one row for the add and one for the remove: {kinds:?}"
    );
}

/// Deleting a desk takes its installed grammar with it.
///
/// Left behind, an overlay desk re-created with the same id silently
/// inherits a table nobody installed on it.
#[tokio::test]
async fn deleting_a_desk_drops_its_installed_grammar() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(&home, desk_manifest()).await;
    let app = router(state);
    let cookie = crate::server::test_support::fixed_cookie("acme");

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/company/desks")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Growth","members":["eng","ceo"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    let installed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/company/desks/growth/hive")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"moves":{"eng":["propose","support"]}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(installed.status(), StatusCode::OK);

    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/company/desks/growth")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    // Re-create the same id; it must come back ungoverned.
    let again = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/company/desks")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Growth","members":["eng","ceo"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::CREATED);

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/company/desks/growth/hive")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["source"], "default");
    for seat in body["seats"].as_array().unwrap() {
        assert_eq!(
            seat["governed"], false,
            "a re-created desk inherited a grammar"
        );
    }
}
