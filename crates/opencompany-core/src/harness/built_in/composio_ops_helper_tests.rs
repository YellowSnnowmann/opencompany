use super::*;

use std::net::SocketAddr;

use crate::company::composio::CatalogEntry;

use axum::Router;
use axum::routing::{get, post};
use serde_json::{Value, json};

/// Mock `POST /agent-integrations/composio/authorize` — returns a hosted
/// connect URL inside the backend's `{success,data}` envelope.
async fn authorize_handler() -> axum::Json<Value> {
    axum::Json(json!({
        "success": true,
        "data": { "connectUrl": "https://connect.composio.dev/abc", "connectionId": "conn-xyz" }
    }))
}

/// Mock `GET /agent-integrations/composio/connections` — gmail has one
/// active + one pending row (→ connected), slack only pending (→ not
/// connected), notion active (filtered out unless allowlisted).
///
/// The identity fields exercise each arm of the account-label precedence
/// (issue #404): `c1` publishes an email, `c2` only a blank one plus a
/// workspace, `c3` only a username, `c4` nothing at all.
async fn connections_handler() -> axum::Json<Value> {
    axum::Json(json!({
        "success": true,
        "data": { "connections": [
            {
                "id": "c1", "toolkit": "gmail", "status": "ACTIVE",
                "createdAt": "2026-08-01T10:00:00Z",
                "accountEmail": " ops@acme.test ",
                "username": "ignored-when-an-email-is-present"
            },
            {
                "id": "c2", "toolkit": "gmail", "status": "INITIATED",
                "accountEmail": "   ",
                "workspace": "Acme Workspace"
            },
            { "id": "c3", "toolkit": "slack", "status": "INITIATED", "username": "acme-bot" },
            { "id": "c4", "toolkit": "notion", "status": "ACTIVE" }
        ] }
    }))
}

/// Mock `GET /agent-integrations/composio/toolkits` — the dynamic catalog
/// shape (backend #1012): a `toolkits` allowlist plus a `catalog[]` whose
/// entries carry an `enabled` gate. `zendesk` is present but not connectable
/// and must not be advertised; the casing and whitespace on `HubSpot` must
/// normalise.
///
/// Entries carry the display metadata (`logo`, `description`, `categories`)
/// the backend actually publishes — issue #600 is that all of it was dropped
/// on the way through, so a mock that omitted it could not have caught the
/// bug.
async fn toolkits_handler() -> axum::Json<Value> {
    axum::Json(json!({
        "success": true,
        "data": {
            "toolkits": ["gmail", "slack"],
            "catalog": [
                {
                    "slug": " HubSpot ",
                    "name": "HubSpot",
                    "enabled": true,
                    "logo": " https://logos.composio.dev/api/hubspot ",
                    "description": "  CRM and marketing automation.  ",
                    "categories": ["crm", " marketing ", ""]
                },
                {
                    "slug": "gmail",
                    "name": "Gmail",
                    "enabled": true,
                    "description": "Send and read email.",
                    "categories": ["email"]
                },
                { "slug": "zendesk", "name": "Zendesk", "enabled": false },
                { "slug": "gmail", "name": "Gmail (dup)", "enabled": true }
            ]
        }
    }))
}

/// Mock toolkits route for a backend predating the dynamic catalog: the
/// plain slug allowlist and no `catalog[]` at all.
async fn legacy_toolkits_handler() -> axum::Json<Value> {
    axum::Json(json!({
        "success": true,
        "data": { "toolkits": ["Notion", "gmail", ""] }
    }))
}

async fn spawn_backend() -> String {
    spawn_backend_with(get(toolkits_handler)).await
}

async fn spawn_backend_with(toolkits: axum::routing::MethodRouter) -> String {
    let app = Router::new()
        .route(
            "/agent-integrations/composio/authorize",
            post(authorize_handler),
        )
        .route(
            "/agent-integrations/composio/connections",
            get(connections_handler),
        )
        .route("/agent-integrations/composio/toolkits", toolkits);
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn config(url: &str, toolkits: Vec<String>) -> TenantComposio {
    TenantComposio::new(
        url.to_string(),
        Credential::from_value("tenant-token"),
        toolkits,
    )
}

#[tokio::test]
async fn authorize_returns_hosted_connect_url() {
    let url = spawn_backend().await;
    let out = authorize_connect_url(&config(&url, vec!["gmail".into()]), "gmail")
        .await
        .expect("authorize returns a connect URL");
    assert_eq!(out, "https://connect.composio.dev/abc");
}

#[tokio::test]
async fn authorize_rejects_toolkit_outside_allowlist_before_any_network_call() {
    // Backend URL is unreachable — the allowlist rejection must fire first.
    let out =
        authorize_connect_url(&config("http://127.0.0.1:1", vec!["gmail".into()]), "slack").await;
    let err = out.expect_err("a toolkit outside the allowlist must be refused");
    assert!(err.to_string().contains("allowlist"), "{err}");
}

#[tokio::test]
async fn list_connection_states_aggregates_active_and_filters_to_allowlist() {
    let url = spawn_backend().await;
    // gmail + slack allowed; notion is active upstream but not in the grant.
    let states = list_connection_states(&config(&url, vec!["gmail".into(), "slack".into()]))
        .await
        .expect("list connections");
    assert_eq!(
        states,
        vec![("gmail".to_string(), true), ("slack".to_string(), false)],
        "gmail active (one ACTIVE row), slack pending only, notion filtered out"
    );
}

/// Issue #404: the detail view needs the account behind a connection, not
/// just that one exists. Pins the whole projection — per-connection rows
/// (two for gmail, where the fold gives one), the raw status, the account
/// label precedence, and the `(toolkit, id)` order — against the same
/// allowlist filter the fold applies.
#[tokio::test]
async fn list_connections_detailed_projects_each_account_with_its_identity() {
    let url = spawn_backend().await;
    let rows = list_connections_detailed(&config(&url, vec!["gmail".into(), "slack".into()]))
        .await
        .expect("list connections");

    // Compared as whole rows rather than as a tuple projection, so a field
    // added to `ComposioConnectionRow` later cannot slip past this
    // assertion unexamined.
    let expect = |id: &str,
                  toolkit: &str,
                  status: &str,
                  connected: bool,
                  created_at: Option<&str>,
                  account: Option<&str>| ComposioConnectionRow {
        id: id.to_string(),
        toolkit: toolkit.to_string(),
        status: status.to_string(),
        connected,
        created_at: created_at.map(str::to_string),
        account: account.map(str::to_string),
    };
    assert_eq!(
        rows,
        vec![
            // Email wins over the username the same row carries, and is
            // trimmed.
            expect(
                "c1",
                "gmail",
                "ACTIVE",
                true,
                Some("2026-08-01T10:00:00Z"),
                Some("ops@acme.test"),
            ),
            // A blank email is not an email: falls through to the workspace.
            // Kept as its own row rather than folded into c1 — this is the
            // "two Gmail accounts" case a disconnect has to tell apart.
            expect(
                "c2",
                "gmail",
                "INITIATED",
                false,
                None,
                Some("Acme Workspace"),
            ),
            // Username is the last resort.
            expect("c3", "slack", "INITIATED", false, None, Some("acme-bot")),
        ],
        "one row per connection, sorted by (toolkit, id); notion filtered out \
         by the allowlist exactly as the fold filters it"
    );
}

/// The fold the tile grid and the reconciliation probe read must keep
/// meaning what it meant before #404 widened the call underneath it —
/// `connected` is still "any account active", not "the first one".
#[tokio::test]
async fn the_per_toolkit_fold_still_summarises_the_detailed_rows() {
    let url = spawn_backend().await;
    let cfg = config(&url, vec!["gmail".into(), "slack".into()]);
    let rows = list_connections_detailed(&cfg).await.expect("rows");
    let states = list_connection_states(&cfg).await.expect("states");

    let folded: std::collections::BTreeMap<String, bool> =
        rows.into_iter().fold(Default::default(), |mut acc, r| {
            let e = acc.entry(r.toolkit).or_insert(false);
            *e = *e || r.connected;
            acc
        });
    assert_eq!(
        states,
        folded.into_iter().collect::<Vec<_>>(),
        "the states route is exactly the OR-fold of the detailed rows"
    );
}

/// Issue #404 + #403: an id this company's own reads will not show must not
/// be deletable by naming it. The mock serves no DELETE route at all, so a
/// request that got as far as dialling would fail loudly rather than pass —
/// the refusal has to come from the guard, before the call.
#[tokio::test]
async fn disconnect_refuses_an_id_outside_this_companys_visible_connections() {
    let url = spawn_backend().await;
    // `c4` (notion) is a real, active connection upstream — but this
    // company's manifest does not grant notion, so no read here surfaces
    // it. That is the case the guard exists for: the bearer *could* delete
    // it, and the allowlist must be a boundary rather than a display filter.
    let err = delete_connection(&config(&url, vec!["gmail".into()]), "c4")
        .await
        .expect_err("a connection outside the grant is not deletable");
    // The *variant* is the assertion, not the message: it is what decides
    // the status code the console sees, and asserting only on the string
    // is what let a refusal ship as a `502`.
    assert!(
        matches!(err, DisconnectError::NotFound(_)),
        "a refused id must be NotFound, not an upstream failure: {err:?}"
    );

    // And an id that exists nowhere at all fails the same way.
    let err = delete_connection(&config(&url, vec!["gmail".into()]), "nope")
        .await
        .expect_err("an unknown id is not deletable");
    assert!(
        matches!(err, DisconnectError::NotFound(_)),
        "unexpected error: {err:?}"
    );
}

/// An empty / whitespace id is refused before the list is even fetched —
/// and as a client mistake, not as an unreachable backend.
#[tokio::test]
async fn disconnect_refuses_a_blank_id_before_any_network_call() {
    // Unreachable backend — the argument check must fire first. If it did
    // not, this would surface as `Upstream`, which is what the assertion
    // below rules out.
    let err = delete_connection(&config("http://127.0.0.1:1", vec!["gmail".into()]), "  ")
        .await
        .expect_err("a blank id is refused");
    assert!(
        matches!(err, DisconnectError::NotFound(_)),
        "unexpected error: {err:?}"
    );
}

/// Issue #820: an account that is not usable cannot be the one agents act
/// as. `c2` is a real gmail connection of this company's, and `INITIATED` —
/// pinning it would route every gmail send to an account that cannot send,
/// which is worse than the unpinned behaviour it replaces. So the refusal is
/// a product decision, not a validation nicety, and it is asserted with the
/// store: a refusal that still wrote would be a broken toolkit with a
/// reassuring error message.
///
/// The two blunter refusals share the test because they share the guard, and
/// the assertion that matters for all three is the same one — nothing
/// reached [`crate::company::composio::set_default`].
#[tokio::test]
async fn pinning_an_account_that_cannot_send_is_refused_and_stores_nothing() {
    use crate::company::composio::load_defaults;
    use crate::ports::types::CompanyId;
    use crate::store::FsSecretStore;

    let url = spawn_backend().await;
    let dir = tempfile::Builder::new()
        .prefix("oc-composio-pin-")
        .tempdir()
        .expect("tempdir");
    let secrets = FsSecretStore::new(dir.path());
    let company = CompanyId::new("acme");
    let cfg = config(&url, vec!["gmail".into(), "slack".into()]);

    let err = set_default_connection(&cfg, &company, &secrets, "c2")
        .await
        .expect_err("an account that is not connected cannot be pinned");
    // `NotFound` and not `Upstream`: the backend answered fine, and the
    // console must render this as the operator's mistake with the fix in it
    // ("re-authorize it"), not as a provider outage.
    assert!(
        matches!(err, DisconnectError::NotFound(_)),
        "unexpected error: {err:?}"
    );
    assert!(
        err.to_string().contains("INITIATED") && err.to_string().contains("not connected"),
        "the message names the status the operator has to fix: {err}"
    );

    // An id belonging to nobody, and an id belonging to this company under a
    // toolkit its manifest does not grant — the same boundary
    // `delete_connection` draws, so a pin cannot reach what no read shows.
    for id in ["nope", "c4", "   "] {
        match set_default_connection(&cfg, &company, &secrets, id).await {
            Err(DisconnectError::NotFound(_)) => {}
            other => panic!("`{id}` must be refused as NotFound, got {other:?}"),
        }
    }

    assert!(
        load_defaults(&company, &secrets)
            .await
            .expect("defaults read")
            .is_empty(),
        "a refused pin must not be stored — the whole point is that the next \
         agent turn is unchanged"
    );

    // The control: `c1` is the same toolkit, ACTIVE, and goes through. Without
    // it a guard that refused everything would pass every assertion above.
    let toolkit = set_default_connection(&cfg, &company, &secrets, "c1")
        .await
        .expect("an active account is pinnable");
    assert_eq!(toolkit, "gmail", "the pinned toolkit is reported back");
    assert_eq!(
        load_defaults(&company, &secrets)
            .await
            .expect("defaults read")
            .get("gmail")
            .map(String::as_str),
        Some("c1")
    );
}

/// The console's open-mode source (issue #397): the backend's real catalog,
/// normalised. Connectable entries only, trimmed + lowercased, de-duplicated,
/// sorted.
#[tokio::test]
async fn list_catalog_toolkits_returns_the_backends_connectable_catalog() {
    let url = spawn_backend().await;
    let catalog = list_catalog_toolkits(&config(&url, Vec::new()))
        .await
        .expect("catalog fetch");
    assert_eq!(
        catalog.iter().map(|e| e.slug.as_str()).collect::<Vec<_>>(),
        vec!["gmail", "hubspot"],
        "connectable entries only, normalised, de-duplicated and sorted"
    );
}

/// Issue #600: the display metadata the backend publishes reaches the
/// caller instead of being reduced to a slug.
///
/// This is the regression test for the defect itself. Every field asserted
/// here was present in the response and discarded by a single
/// `.map(|entry| entry.slug)`, which is why the console had nothing to
/// group by, nothing to brand with, and nothing to search but the slug.
#[tokio::test]
async fn list_catalog_toolkits_carries_the_display_metadata() {
    let url = spawn_backend().await;
    let catalog = list_catalog_toolkits(&config(&url, Vec::new()))
        .await
        .expect("catalog fetch");

    let hubspot = catalog
        .iter()
        .find(|e| e.slug == "hubspot")
        .expect("hubspot is connectable");
    assert_eq!(hubspot.name, "HubSpot");
    assert_eq!(hubspot.description, "CRM and marketing automation.");
    assert_eq!(
        hubspot.logo.as_deref(),
        Some("https://logos.composio.dev/api/hubspot"),
        "the logo URL is what lets a tile be branded rather than a text row"
    );
    assert_eq!(
        hubspot.categories,
        vec!["crm".to_string(), "marketing".to_string()],
        "categories are trimmed and emptied-out entries dropped, but otherwise \
         forwarded verbatim — the console buckets them, not this layer"
    );

    let gmail = catalog
        .iter()
        .find(|e| e.slug == "gmail")
        .expect("gmail is connectable");
    assert_eq!(gmail.description, "Send and read email.");
    assert_eq!(
        gmail.logo, None,
        "an unpublished logo is None, not an empty string the console would \
         render as a broken image"
    );
    assert_eq!(
        gmail.name, "Gmail",
        "the FIRST entry for a slug wins, matching the de-duplication the slug \
         set used to do — not the later `Gmail (dup)`"
    );
}

/// A backend predating the dynamic catalog sends no `catalog[]`. Its plain
/// slug allowlist is used rather than reporting an empty catalog — which the
/// console would (correctly) render as a degraded fallback.
#[tokio::test]
async fn list_catalog_toolkits_falls_back_to_the_plain_allowlist() {
    let url = spawn_backend_with(get(legacy_toolkits_handler)).await;
    let catalog = list_catalog_toolkits(&config(&url, Vec::new()))
        .await
        .expect("catalog fetch");
    assert_eq!(
        catalog,
        vec![
            CatalogEntry::from_slug("gmail"),
            CatalogEntry::from_slug("notion"),
        ],
        "slug-only entries: the backend published nothing else, and the console \
         renders these with its own typography rather than dropping them"
    );
}

/// An unreachable backend is an error, never a quietly-empty catalog — the
/// caller has to be able to tell "nothing is permitted" from "I could not
/// ask".
#[tokio::test]
async fn list_catalog_toolkits_surfaces_a_fetch_failure() {
    let out = list_catalog_toolkits(&config("http://127.0.0.1:1", Vec::new())).await;
    out.expect_err("an unreachable backend must not read as an empty catalog");
}

#[tokio::test]
async fn list_connection_states_empty_allowlist_admits_every_toolkit() {
    let url = spawn_backend().await;
    let states = list_connection_states(&config(&url, Vec::new()))
        .await
        .expect("list connections");
    assert_eq!(
        states,
        vec![
            ("gmail".to_string(), true),
            ("notion".to_string(), true),
            ("slack".to_string(), false),
        ]
    );
}
