use super::composio_test_support::*;
use crate::ports::types::CompanyId;
use axum::http::StatusCode;
#[cfg(feature = "composio")]
use axum::{Json, Router};
use serde_json::json;

/// The ops tests that are decidable only in a build carrying `composio`.
///
/// Gathered under one module so a CI lane can *name* them. A feature-gated
/// test's default fate in this repo is "compiled by `Check
/// (--all-features)`, executed by nothing" (issue #770), and the composio
/// lane has to select by filter rather than run this whole module: with the
/// feature on, `an_admin_is_unaffected` dials `api.tinyhumans.ai` for real
/// (issue #801). One filter on this module runs every gated test here and
/// none of that, and a gated test added later is picked up by joining the
/// module rather than by remembering to edit `ci.yml`.
#[cfg(feature = "composio")]
mod gated_tests {
    use super::*;
    use crate::server::ops::composio::{drop_dangling_defaults, group_by_toolkit};

    /// Issue #404: the per-toolkit shape the tile grid reads is a fold over the
    /// per-connection rows, and the fold must not lose an account or flip a
    /// boolean. Pure — no live backend needed.
    #[test]
    fn grouping_keeps_every_account_and_ors_their_connected_state() {
        use crate::harness::composio::ComposioConnectionRow;

        let row = |id: &str, toolkit: &str, connected: bool, account: Option<&str>| {
            ComposioConnectionRow {
                id: id.to_string(),
                toolkit: toolkit.to_string(),
                status: if connected { "ACTIVE" } else { "INITIATED" }.to_string(),
                connected,
                created_at: None,
                account: account.map(str::to_string),
            }
        };

        let out = group_by_toolkit(
            vec![
                row("c1", "gmail", false, Some("a@acme.test")),
                row("c2", "gmail", true, Some("b@acme.test")),
                row("c3", "slack", false, None),
            ],
            &Default::default(),
        );

        assert_eq!(out.len(), 2, "one entry per toolkit");
        assert_eq!(out[0].toolkit, "gmail");
        assert!(
            out[0].connected,
            "a toolkit is connected when ANY of its accounts is — the second row \
             here, which a first-row-wins fold would have missed"
        );
        assert_eq!(
            out[0]
                .accounts
                .iter()
                .map(|a| (a.id.as_str(), a.connected))
                .collect::<Vec<_>>(),
            vec![("c1", false), ("c2", true)],
            "both accounts survive, in the order the rows arrived"
        );
        assert_eq!(out[1].toolkit, "slack");
        assert!(!out[1].connected, "no active account, so not connected");
        assert_eq!(out[1].accounts.len(), 1);

        // Nothing pinned: nothing is marked, and no default is reported. This
        // is the shape #819 asks the console to render honestly, and it stays
        // the shape until somebody chooses.
        assert!(out.iter().all(|dto| dto.default_connection_id.is_none()));
        assert!(
            out.iter()
                .flat_map(|dto| dto.accounts.iter())
                .all(|account| !account.is_default),
            "an unchosen account is never marked as the default"
        );
    }

    /// Issue #820: once a company has chosen, the choice is reported on the
    /// toolkit **and** marked on the one account it names — the console needs
    /// both to draw a list with one row marked and the rest offering to become
    /// it.
    #[test]
    fn grouping_marks_the_chosen_account_and_only_that_one() {
        use crate::harness::composio::ComposioConnectionRow;

        let row = |id: &str, toolkit: &str| ComposioConnectionRow {
            id: id.to_string(),
            toolkit: toolkit.to_string(),
            status: "ACTIVE".to_string(),
            connected: true,
            created_at: None,
            account: None,
        };
        let defaults: crate::company::composio::ComposioDefaults =
            [("gmail".to_string(), "c2".to_string())]
                .into_iter()
                .collect();

        let out = group_by_toolkit(
            vec![row("c1", "gmail"), row("c2", "gmail"), row("c3", "slack")],
            &defaults,
        );

        assert_eq!(out[0].default_connection_id.as_deref(), Some("c2"));
        assert_eq!(
            out[0]
                .accounts
                .iter()
                .map(|a| (a.id.as_str(), a.is_default))
                .collect::<Vec<_>>(),
            vec![("c1", false), ("c2", true)],
            "exactly one account carries the mark"
        );
        assert!(
            out[1].default_connection_id.is_none(),
            "a choice made for gmail says nothing about slack: {:?}",
            out[1].default_connection_id
        );
    }

    /// Issue #820: an account revoked **at Composio** — not through this console,
    /// so nothing here saw the disconnect — leaves a choice naming a connection
    /// that no longer exists. That choice is not merely stale: it is sent on the
    /// next `composio_execute` and refused, so the toolkit stops working for
    /// every agent for a reason nothing on screen explains. The read the console
    /// polls repairs it.
    ///
    /// Driven through the handler's own helper rather than the route, because
    /// the route needs a live Composio backend and the decision under test is
    /// the one made *after* it answers. What it must not do is as load-bearing
    /// as what it must: a live choice is untouched, and a toolkit whose chosen
    /// account is gone falls back to "Composio picks" rather than being
    /// re-pointed at a sibling account nobody chose.
    #[tokio::test]
    async fn the_connections_read_forgets_a_choice_composio_no_longer_lists() {
        use crate::company::composio::{load_defaults, set_default};
        use crate::harness::composio::ComposioConnectionRow;

        let home_dir = home();
        let state = state_with_manifest(home_dir.path(), GRANTED).await;
        let runtime = runtime_of(&state, "acme");
        let (id, secrets) = (runtime.id(), runtime.secrets());

        // gmail names an account still live; slack names one revoked since.
        for (toolkit, connection) in [("gmail", "c1"), ("slack", "c_revoked")] {
            set_default(id, secrets.as_ref(), toolkit, connection)
                .await
                .unwrap();
        }

        let row = |id: &str, toolkit: &str| ComposioConnectionRow {
            id: id.to_string(),
            toolkit: toolkit.to_string(),
            status: "ACTIVE".to_string(),
            connected: true,
            created_at: None,
            account: None,
        };
        // The company still holds a slack account — just not the chosen one.
        let rows = vec![row("c1", "gmail"), row("c9", "slack")];

        let left = drop_dangling_defaults(
            runtime.as_ref(),
            &rows,
            load_defaults(id, secrets.as_ref()).await.unwrap(),
        )
        .await
        .expect("the cleanup completes");

        assert_eq!(
            left.get("gmail").map(String::as_str),
            Some("c1"),
            "a choice naming a live account is untouched"
        );
        assert!(
            !left.contains_key("slack"),
            "the choice naming a revoked account is dropped: {left:?}"
        );
        assert_eq!(
            load_defaults(id, secrets.as_ref()).await.unwrap(),
            left,
            "the repair is stored, not merely reflected in this one response — \
             otherwise the next agent turn still sends the dead id"
        );

        let out = group_by_toolkit(rows, &left);
        assert_eq!(out[1].toolkit, "slack");
        assert!(
            out[1].default_connection_id.is_none()
                && out[1].accounts.iter().all(|account| !account.is_default),
            "with its choice gone slack is unchosen again — the surviving account \
             is not silently promoted into a decision nobody made: {:?}",
            out[1]
        );
        assert_eq!(out[0].default_connection_id.as_deref(), Some("c1"));
    }

    /// A loopback backend for the `DELETE …/composio/connections/{id}` route
    /// test: `conn-1` is the only account this company knows about, and the
    /// backend's own delete either succeeds or fails depending on
    /// `fail_delete`, so the same mock drives both the 404 and the 502 arm of
    /// the mapping in [`super::super::disconnect_impl`].
    async fn spawn_connections_backend(
        fail_delete: bool,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use axum::extract::Path;
        use axum::response::IntoResponse;

        let deletes: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let recorded = deletes.clone();
        let app = Router::new()
            .route(
                "/agent-integrations/composio/connections",
                axum::routing::get(|| async {
                    Json(json!({
                        "success": true,
                        "data": { "connections": [
                            { "id": "conn-1", "toolkit": "gmail", "status": "ACTIVE" }
                        ] }
                    }))
                }),
            )
            .route(
                "/agent-integrations/composio/connections/{id}",
                axum::routing::delete(move |Path(id): Path<String>| {
                    let recorded = recorded.clone();
                    async move {
                        recorded.lock().unwrap().push(id);
                        if fail_delete {
                            StatusCode::INTERNAL_SERVER_ERROR.into_response()
                        } else {
                            Json(json!({ "success": true, "data": { "deleted": true } }))
                                .into_response()
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), deletes)
    }

    /// The documented split in [`super::super::disconnect_impl`]: an id this
    /// company's own read cannot see is a `404`, never a `502`, because it is a
    /// claim about the company's accounts rather than about the provider. Route
    /// tests above only reach the "no client at all" `409` arm; this drives the
    /// real mapping with a loopback backend standing in for Composio.
    #[tokio::test]
    async fn disconnect_maps_an_unknown_id_to_404_not_502() {
        let (backend, deletes) = spawn_connections_backend(false).await;
        let env = crate::test_support::EnvVarGuard::capture(&[
            crate::company::composio::TINYHUMANS_API_URL_ENV,
        ]);
        env.set(crate::company::composio::TINYHUMANS_API_URL_ENV, &backend);

        let home_dir = home();
        let state = state_with_manifest(home_dir.path(), GRANTED).await;
        send(
            &state,
            "PUT",
            "/api/v1/company/composio/token",
            Some(json!({ "token": TOKEN })),
        )
        .await;

        let (status, body, raw) = send(
            &state,
            "DELETE",
            "/api/v1/company/composio/connections/conn-does-not-exist",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{raw}");
        assert_eq!(body["code"], "not_found", "{body}");
        assert!(
            deletes.lock().unwrap().is_empty(),
            "an id outside the company's visible connections must never reach a delete call"
        );
    }

    /// The other arm of the same split: an id the company DOES hold, but the
    /// backend refuses to delete, is a `502` naming the provider failure —
    /// never a `404`, which would tell the operator to stop looking for an
    /// account that is right there in the list.
    #[tokio::test]
    async fn disconnect_maps_a_backend_failure_to_502_not_404() {
        let (backend, deletes) = spawn_connections_backend(true).await;
        let env = crate::test_support::EnvVarGuard::capture(&[
            crate::company::composio::TINYHUMANS_API_URL_ENV,
        ]);
        env.set(crate::company::composio::TINYHUMANS_API_URL_ENV, &backend);

        let home_dir = home();
        let state = state_with_manifest(home_dir.path(), GRANTED).await;
        send(
            &state,
            "PUT",
            "/api/v1/company/composio/token",
            Some(json!({ "token": TOKEN })),
        )
        .await;

        let (status, body, raw) = send(
            &state,
            "DELETE",
            "/api/v1/company/composio/connections/conn-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{raw}");
        assert_eq!(body["code"], "tinyhumans_composio_disconnect", "{body}");
        assert_eq!(
            deletes.lock().unwrap().as_slice(),
            ["conn-1"],
            "a known id must actually reach the backend's delete before failing"
        );
    }
}

/// The choice plane is wired on the same terms as the rest of the OAuth
/// plane: in the route table whatever the build, and a `409` — never a
/// `404` — when there is no usable client, since "no such connection" is a
/// claim about this company's accounts that a build without Composio cannot
/// make.
#[tokio::test]
async fn set_default_route_conflicts_without_build_or_token() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), GRANTED).await;

    let (status, body, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/composio/connections/conn-1/default",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{raw}");
    #[cfg(not(feature = "composio"))]
    assert_eq!(body["code"], "not_in_build", "{body}");
    #[cfg(feature = "composio")]
    assert_eq!(body["code"], "not_configured", "{body}");
}

/// Clearing is the deliberate exception: it takes no upstream call, so it
/// works in a build without Composio and — the case that matters — when the
/// provider is unreachable or the account is already gone. A clear that
/// needed the network would refuse exactly when it is most needed.
#[tokio::test]
async fn clearing_a_choice_needs_no_client_and_is_idempotent() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), GRANTED).await;

    let (status, body, raw) = send(
        &state,
        "DELETE",
        "/api/v1/company/composio/connections/conn-1/default",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert!(
        body["note"]
            .as_str()
            .unwrap_or_default()
            .contains("nothing changed"),
        "clearing what was never chosen says so rather than claiming a change: {body}"
    );

    // Now with something stored, the same call reports the real change and
    // leaves nothing behind.
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    crate::company::composio::set_default(
        runtime.id(),
        runtime.secrets().as_ref(),
        "gmail",
        "conn-1",
    )
    .await
    .unwrap();

    let (status, body, raw) = send(
        &state,
        "DELETE",
        "/api/v1/company/composio/connections/conn-1/default",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert!(
        body["note"]
            .as_str()
            .unwrap_or_default()
            .contains("Cleared"),
        "{body}"
    );
    assert!(
        crate::company::composio::load_defaults(runtime.id(), runtime.secrets().as_ref())
            .await
            .unwrap()
            .is_empty()
    );
}

/// Choosing the account a company acts as is an admin's decision, like
/// connecting and disconnecting one: every agent in the company acts
/// through the single answer, so it is not a per-operator preference.
#[tokio::test]
async fn a_member_cannot_choose_the_account_the_company_acts_as() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), GRANTED).await;
    let member =
        crate::server::test_support::seed_session(&state, "acme", crate::ports::UserRole::Member)
            .await;

    for method in ["PUT", "DELETE"] {
        let (status, body, raw) = send_as(
            &state,
            method,
            "/api/v1/company/composio/connections/conn-1/default",
            None,
            Auth::Cookie(member.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}: {raw}");
        assert_eq!(body["code"], "forbidden", "{method}: {body}");
    }

    // And the refusal is real: nothing was stored by either attempt.
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    assert!(
        crate::company::composio::load_defaults(runtime.id(), runtime.secrets().as_ref())
            .await
            .unwrap()
            .is_empty()
    );
}

// ── in-use guards (#2306): confirmInUse gates a clear/switch ──────
//
// The signal is `composio/mode` (`src/company/composio.rs::MODE_KEY`),
// per in-use-guards.md §2's surfaces table: clearing
// `composio/tinyhumans/key` reports `surfaces: ["composio"]` iff the
// mode currently reads `"managed"`; clearing/switching
// `composio/byok/key` reports it iff the mode currently reads `"byok"`.
// A company connection pin (`composio/defaults`) is no longer the
// signal, so these tests drive the guard by setting the mode — directly,
// or through `store_token`/`store_api_key`, which are what set it in
// practice.

/// A managed-token clear is refused with `409 in_use` while the company
/// is still on the managed route (the default with nothing else
/// stored), and nothing is written.
#[tokio::test]
async fn clearing_the_managed_token_while_mode_is_managed_is_refused_without_confirmation() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "clearguard", GRANTED).await;
    let runtime = runtime_of(&state, "clearguard");
    crate::company::composio::store_token(runtime.id(), runtime.secrets().as_ref(), TOKEN)
        .await
        .unwrap();
    assert_eq!(
        crate::company::composio::load_mode(runtime.id(), runtime.secrets().as_ref())
            .await
            .unwrap(),
        crate::company::composio::ComposioMode::Managed,
        "managed is the default mode with nothing else stored"
    );

    let (status, body, raw) = send_for(
        &state,
        "clearguard",
        "PUT",
        "/api/v1/company/composio/token",
        Some(json!({ "token": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{raw}");
    assert_eq!(body["code"], "in_use", "{body}");
    assert_eq!(
        body["error"], "Composio's key is used by Composio.",
        "the message follows in-use-guards.md §2's fixed sentence: {body}"
    );
    assert_eq!(
        body["usedBy"],
        json!({ "surfaces": ["composio"] }),
        "{body}"
    );

    // Refused means nothing changed.
    assert_eq!(
        crate::company::composio::load_tinyhumans_key(runtime.id(), runtime.secrets().as_ref())
            .await
            .unwrap()
            .as_deref(),
        Some(TOKEN),
        "a refused clear must not have written anything"
    );
}

/// The same clear, with `confirmInUse: true`, proceeds and echoes the
/// `usedBy` it would have refused with (in-use-guards.md §3).
#[tokio::test]
async fn a_confirmed_clear_of_the_managed_token_succeeds_and_echoes_used_by() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "clearconfirmed", GRANTED).await;
    let runtime = runtime_of(&state, "clearconfirmed");
    crate::company::composio::store_token(runtime.id(), runtime.secrets().as_ref(), TOKEN)
        .await
        .unwrap();

    let (status, body, raw) = send_for(
        &state,
        "clearconfirmed",
        "PUT",
        "/api/v1/company/composio/token",
        Some(json!({ "token": "", "confirmInUse": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(
        body["usedBy"],
        json!({ "surfaces": ["composio"] }),
        "{body}"
    );
    assert_eq!(
        crate::company::composio::load_tinyhumans_key(runtime.id(), runtime.secrets().as_ref())
            .await
            .unwrap(),
        None,
        "the confirmed clear must have landed"
    );
}
