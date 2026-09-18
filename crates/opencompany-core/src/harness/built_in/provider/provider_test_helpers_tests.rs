use super::*;
use crate::company::Inference;
use std::collections::BTreeMap;

/// A three-segment JWT whose `exp` is `secs_from_now` in the future, so the
/// projected-file cache window is wide open for the whole test.
pub(crate) fn jwt_with_exp(secs_from_now: u64) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + secs_from_now;
    let payload = serde_json::json!({ "exp": exp }).to_string();
    let mut encoded = String::new();
    for chunk in payload.as_bytes().chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..chunk.len() + 1 {
            encoded.push(ALPHA[((n >> (18 - 6 * i)) & 0x3F) as usize] as char);
        }
    }
    format!("aGVhZGVy.{encoded}.c2ln")
}

/// The bearer a config would present right now.
pub(crate) async fn bearer_of(config: &HostedProviderConfig) -> Option<String> {
    config.credential.current().await.expect("resolves")
}

/// Build a single-user-message request the way the harness turn does.
pub(crate) fn user_request(message: &str) -> ModelRequest {
    ModelRequest {
        messages: vec![Message::user(message)],
        ..Default::default()
    }
}

/// Writes a projected-token file and returns `(dir, path-as-string)`. The
/// `TempDir` must stay alive: `TinyhumansTokenSource` selects the projected
/// tier only when the path **exists**.
pub(crate) fn projected_token_file() -> (tempfile::TempDir, String) {
    let dir = tempfile::Builder::new()
        .prefix("oc-cred-")
        .tempdir()
        .expect("tempdir");
    let path = dir.path().join("token");
    std::fs::write(&path, "projected-token").unwrap();
    let rendered = path.display().to_string();
    (dir, rendered)
}

/// A stub that records the `Authorization` header of every request it
/// answers, so a test can prove which bearer actually went out.
pub(crate) async fn spawn_auth_recorder() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::{Json, Router};

    let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let app = Router::new().route(
        "/chat/completions",
        post(move |headers: HeaderMap| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push(
                    headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string(),
                );
                Json(serde_json::json!({
                    "choices": [{ "message": { "role": "assistant", "content": "ok" } }]
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

pub(crate) fn manifest_inference(provider: &str) -> Inference {
    Inference {
        provider: Some(provider.to_string()),
        base_url: None,
        api_key_secret: None,
        models: BTreeMap::new(),
    }
}

/// Spawns an in-process OpenAI-compatible stub that echoes `marker` as the
/// completion content. The listener is bound before the task spawns, so the
/// OS accepts connections into the backlog immediately.
pub(crate) async fn spawn_stub(marker: &'static str) -> String {
    use axum::routing::post;
    use axum::{Json, Router};

    let app = Router::new().route(
        "/chat/completions",
        post(move || async move {
            Json(serde_json::json!({
                "choices": [{ "message": { "role": "assistant", "content": marker } }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

pub(crate) async fn spawn_rejection(
    status: axum::http::StatusCode,
) -> (String, tokio::task::JoinHandle<()>) {
    use axum::Router;
    use axum::routing::post;

    let app = Router::new().route(
        "/chat/completions",
        post(move || async move {
            (
                status,
                axum::Json(serde_json::json!({
                    "error": { "message": "provider refused the request" }
                })),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), server)
}

/// Spawns an in-process OpenAI-compatible stub whose `message.content` is
/// the given raw JSON value rather than a plain string — used to exercise
/// the array-of-text-parts content shape end to end.
pub(crate) async fn spawn_stub_content(content: serde_json::Value) -> String {
    use axum::routing::post;
    use axum::{Json, Router};

    let app = Router::new().route(
        "/chat/completions",
        post(move || {
            let content = content.clone();
            async move {
                Json(serde_json::json!({
                    "choices": [{
                        "finish_reason": "stop",
                        "message": { "role": "assistant", "content": content }
                    }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Spawns a stub whose whole `choices[0]` object is the given raw JSON —
/// `spawn_stub_message` below pins `finish_reason: "stop"`, so this is the
/// one that can express a payload *declaring* an action it never delivered.
pub(crate) async fn spawn_stub_choice(choice: serde_json::Value) -> String {
    use axum::routing::post;
    use axum::{Json, Router};

    let app = Router::new().route(
        "/chat/completions",
        post(move || {
            let choice = choice.clone();
            async move {
                Json(serde_json::json!({
                    "choices": [choice],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Spawns an in-process OpenAI-compatible stub whose full `message` object
/// is the given raw JSON value — used to exercise shapes `spawn_stub_content`
/// cannot, such as a reasoning-only turn (`content: null` with the visible
/// text under `reasoning`/`reasoning_content` instead).
pub(crate) async fn spawn_stub_message(message: serde_json::Value) -> String {
    use axum::routing::post;
    use axum::{Json, Router};

    let app = Router::new().route(
        "/chat/completions",
        post(move || {
            let message = message.clone();
            async move {
                Json(serde_json::json!({
                    "choices": [{
                        "finish_reason": "stop",
                        "message": message
                    }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// A `ModelChoice` builder for the pin tests below.
pub(crate) fn choice(provider: &str, model: &str) -> inference::store::ModelChoice {
    inference::store::ModelChoice {
        provider: provider.to_string(),
        model: model.to_string(),
    }
}

pub(crate) type Seen = Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

/// An OpenAI-compatible stub that records every request's `model` field
/// and `Authorization` header, in arrival order.
pub(super) async fn spawn_capturing_stub() -> (String, Seen) {
    use axum::Router;
    use axum::extract::Json as JsonExtract;
    use axum::http::HeaderMap;
    use axum::routing::post;

    let seen: Seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let capture = Arc::clone(&seen);
    let app = Router::new().route(
        "/chat/completions",
        post(
            move |headers: HeaderMap, JsonExtract(body): JsonExtract<serde_json::Value>| {
                let capture = Arc::clone(&capture);
                async move {
                    let model = body["model"].as_str().unwrap_or_default().to_string();
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    capture.lock().unwrap().push((model, auth));
                    axum::Json(serde_json::json!({
                        "choices": [{ "message": { "role": "assistant", "content": "ok" } }],
                        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
                    }))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

/// A stub that rejects every chat-completion with `status`, the way an
/// early-rotated bearer is rejected in production.
pub(crate) async fn spawn_rejecting_stub(status: axum::http::StatusCode) -> String {
    use axum::Router;
    use axum::routing::post;

    let app = Router::new().route(
        "/chat/completions",
        post(move || async move { (status, "rejected") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Spawns a stub that rejects every chat completion with a
/// provider-flavoured "model not available" 400, the shape `probe`'s
/// caller (the console "Test" button) hits when an operator has typed a
/// model id the endpoint does not serve.
pub(crate) async fn spawn_model_unavailable_stub() -> String {
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::post;

    let app = Router::new().route(
        "/chat/completions",
        post(|| async {
            (
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "error": "Model 'gpt-5.9-ghost' is not available. Use GET /v1/models to \
                              list available models."
                })
                .to_string(),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}
