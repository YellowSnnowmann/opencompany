//! A loopback MCP server behind a loopback OAuth authorization server, for
//! sign-in tests that drive the whole flow.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse as _;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

/// What the authorization server saw.
#[derive(Default)]
pub(crate) struct Seen {
    /// Every token-endpoint form, in order.
    pub(crate) token_forms: StdMutex<Vec<Vec<(String, String)>>>,
    /// Every dynamic-registration body, in order.
    pub(crate) registrations: StdMutex<Vec<Value>>,
}

/// The fixture's endpoints.
pub(crate) struct AuthServer {
    /// The MCP endpoint, which challenges every request.
    pub(crate) mcp: String,
    /// The issuer base URL.
    pub(crate) base: String,
    /// What the authorization server saw.
    pub(crate) seen: Arc<Seen>,
}

/// Serves the fixture. `registration` false leaves the dynamic-registration
/// endpoint out of the metadata.
pub(crate) async fn spawn(registration: bool, token_reply: Value) -> AuthServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Seen::default());

    let challenge =
        format!("Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\"");
    let resource = json!({ "resource": format!("{base}/mcp"), "authorization_servers": [base] });
    let mut metadata = json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "grant_types_supported": ["authorization_code", "refresh_token"],
    });
    if registration {
        metadata["registration_endpoint"] = json!(format!("{base}/register"));
    }

    let registered = Arc::clone(&seen);
    let tokens = Arc::clone(&seen);
    let app = Router::new()
        .route(
            "/mcp",
            post(move || {
                let challenge = challenge.clone();
                async move {
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        "www-authenticate",
                        HeaderValue::from_str(&challenge).unwrap(),
                    );
                    (StatusCode::UNAUTHORIZED, headers, "").into_response()
                }
            }),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(move || {
                let resource = resource.clone();
                async move { Json(resource) }
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(move || {
                let metadata = metadata.clone();
                async move { Json(metadata) }
            }),
        )
        .route(
            "/register",
            post(move |Json(body): Json<Value>| {
                let seen = Arc::clone(&registered);
                async move {
                    seen.registrations.lock().unwrap().push(body);
                    Json(json!({ "client_id": "cid-1", "client_secret": "cs-1" }))
                }
            }),
        )
        .route(
            "/token",
            post(move |body: String| {
                let seen = Arc::clone(&tokens);
                let reply = token_reply.clone();
                async move {
                    let form = url::form_urlencoded::parse(body.as_bytes())
                        .into_owned()
                        .collect();
                    seen.token_forms.lock().unwrap().push(form);
                    Json(reply)
                }
            }),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    AuthServer {
        mcp: format!("{base}/mcp"),
        base,
        seen,
    }
}

/// The `state` an authorize URL carries.
pub(crate) fn state_of(authorize_url: &str) -> String {
    url::Url::parse(authorize_url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("the authorize url carries a state")
}
