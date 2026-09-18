//! End-to-end contract for the packaged desktop host: bundled preset → local
//! Axum runtime → an authenticated console API with **no sign-in in it**.
//!
//! The desktop runs `AuthMode::None`. There is no login screen, no operator
//! mailbox and no session: `resolve_principal` answers with the company's
//! implicit local owner before it ever looks for a cookie, so the first request
//! the shell makes is already authenticated. This asserts both halves of that —
//! that the console API answers a bare request, and that the login routes
//! refuse rather than quietly offering a second way in.
//!
//! Note which path this exercises: `start_local`, not the shipped
//! `src-tauri/src/embedded.rs`. The two must agree about the mode, because this
//! is the function whose name someone will copy from.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use opencompany::desktop::{DEFAULT_PRESET_ID, start_local};

async fn request(address: &str, request: String) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").unwrap().1
}

#[tokio::test]
async fn desktop_preset_boots_and_needs_no_sign_in() {
    let home = tempfile::tempdir().unwrap();
    let runtime = start_local(home.path(), DEFAULT_PRESET_ID).await.unwrap();
    let config = runtime.config();
    let address = config.api_url.strip_prefix("http://").unwrap();

    let health = request(
        address,
        "GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n".to_string(),
    )
    .await;
    assert!(health.contains(" 200 "), "{health}");

    // THE assertion: no cookie, no bearer, no prior request — and the scoped
    // company API answers anyway. This is what the whole mode buys, and it is
    // the request the shell makes on its first paint.
    let company = request(
        address,
        format!(
            "GET /api/v1/companies/{} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            config.company,
        ),
    )
    .await;
    assert!(company.starts_with("HTTP/1.1 200"), "{company}");

    // The person that request was attributed to is a real record, not a
    // principal invented per request — everything that keys off `UserRecord::id`
    // depends on it, and `local:owner` is the identity `LoginIdentity::Local`
    // mints. Asked through the API the console itself uses.
    let me = request(
        address,
        format!(
            "GET /api/v1/companies/{}/auth/me HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            config.company,
        ),
    )
    .await;
    assert!(me.starts_with("HTTP/1.1 200"), "{me}");
    let who: serde_json::Value = serde_json::from_str(body(&me)).unwrap();
    assert_eq!(who["email"], "local:owner", "{who}");
    assert_eq!(
        who["role"], "admin",
        "the owner of the machine owns the company: {who}"
    );

    // And there is no second way in to drift from. A magic link asked for here
    // is refused by mode rather than answered with the same silent 202 an
    // email-mode host gives a stranger — the desktop has no mailbox to send it
    // to and no session carrier to bring it home in.
    let login = request(
        address,
        {
            let payload = r#"{"email":"someone@example.com"}"#;
            format!(
                "POST /api/v1/companies/{}/auth/request HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                config.company,
                payload.len(),
                payload,
            )
        },
    )
    .await;
    assert!(login.starts_with("HTTP/1.1 409"), "{login}");
    let refusal: serde_json::Value = serde_json::from_str(body(&login)).unwrap();
    assert_eq!(refusal["mode"], "none", "{refusal}");
}
