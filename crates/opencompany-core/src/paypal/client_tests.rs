use super::*;

/// A cached token must never outlive the grant it came from. The margin is
/// a subtraction with a floor, and a floor applied to a grant shorter than
/// itself inverts the whole point of the margin.
#[test]
fn a_token_is_never_trusted_past_its_own_expiry() {
    // The ordinary case: nine hours, a minute of margin.
    assert_eq!(token_lifetime(32400), 32340);
    // The margin still applies well above the floor.
    assert_eq!(token_lifetime(300), 240);
    // Below the margin the floor would run past expiry — clamped instead.
    for expires_in in [0, 1, 10, 30, 59, 60, 89] {
        assert!(
            token_lifetime(expires_in) <= expires_in,
            "a {expires_in}s grant was trusted for {}s",
            token_lifetime(expires_in),
        );
    }
}

fn config() -> PaypalConfig {
    PaypalConfig {
        client_id: "AY_client".to_string(),
        client_secret: "EL_secret".to_string(),
        environment: PaypalEnvironment::Sandbox,
    }
}

#[test]
fn debug_never_renders_either_half_of_the_credential() {
    let rendered = format!("{:?}", config());
    assert!(!rendered.contains("AY_client"), "{rendered}");
    assert!(!rendered.contains("EL_secret"), "{rendered}");
    assert!(rendered.contains("Sandbox"), "{rendered}");
}

#[test]
fn a_client_debug_does_not_reach_into_its_config() {
    let client = PaypalClient::new(config()).expect("builds");
    let rendered = format!("{client:?}");
    assert!(!rendered.contains("EL_secret"), "{rendered}");
    assert!(rendered.contains("sandbox.paypal.com"), "{rendered}");
}

#[tokio::test]
async fn a_token_is_reused_rather_than_refetched() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();

    let app = axum::Router::new().fallback(axum::routing::any(move || {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            (
                axum::http::StatusCode::OK,
                [("content-type", "application/json")],
                r#"{"access_token":"tok_1","expires_in":32400}"#,
            )
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = PaypalClient::with_base_url(config(), format!("http://{addr}")).expect("builds");
    assert_eq!(client.token().await.expect("first"), "tok_1");
    assert_eq!(client.token().await.expect("second"), "tok_1");
    assert_eq!(client.token().await.expect("third"), "tok_1");
    // Three calls, ONE trip: without the cache every API call would pay for
    // a token, and PayPal rate-limits that endpoint separately.
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn bad_credentials_name_the_environment_not_just_invalid_client() {
    let app = axum::Router::new().fallback(axum::routing::any(|| async {
        (
            axum::http::StatusCode::UNAUTHORIZED,
            [("content-type", "application/json")],
            r#"{"error":"invalid_client","error_description":"Client Authentication failed"}"#,
        )
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = PaypalClient::with_base_url(config(), format!("http://{addr}")).expect("builds");
    let message = client
        .token()
        .await
        .expect_err("401 is an error")
        .to_string();
    // "invalid_client" alone reads as a typo; the commonest real cause is
    // sandbox keys pointed at live, which only the environment reveals.
    assert!(message.contains("sandbox"), "{message}");
    assert!(
        message.contains("Client Authentication failed"),
        "{message}"
    );
    server.abort();
}

#[tokio::test]
async fn a_path_that_could_move_the_host_is_refused_before_any_request() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // A live stub, so "was refused" is proved by the request never arriving
    // rather than by an error that a connection failure would also produce.
    // The count also covers the TOKEN call: a rejected path must not spend a
    // credential fetch either.
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let app = axum::Router::new().fallback(axum::routing::any(move || {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            (
                axum::http::StatusCode::OK,
                [("content-type", "application/json")],
                r#"{"access_token":"tok","expires_in":32400,"balances":[]}"#,
            )
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = PaypalClient::with_base_url(config(), format!("http://{addr}")).expect("builds");

    for path in [
        // The one that matters: concatenated onto the base this reads as
        // userinfo `api.paypal.com` at host `evil.com`, and the bearer token
        // goes to whoever owns that name.
        "@evil.com/v1/reporting/balances",
        "/v1/reporting/balances@evil.com",
        // Not absolute — pastes straight onto the host name.
        "v1/reporting/balances",
        "evil.com/v1",
        // Protocol-relative.
        "//evil.com/v1",
        // The query is a separate argument; a path carrying its own would
        // silently merge with or truncate it.
        "/v1/reporting/balances?start_date=x",
        "/v1/reporting/balances#frag",
        // Header/URL splitting.
        "/v1/reporting/ balances",
        "/v1/reporting/\nbalances",
    ] {
        let error = client
            .get(path, &[])
            .await
            .expect_err(&format!("{path:?} must be refused"));
        assert!(
            matches!(&error, OpenCompanyError::Paypal { code, .. } if code == "invalid_path"),
            "{path:?} was refused, but for the wrong reason: {error}",
        );
        // And the path itself is not echoed back into the transcript.
        assert!(!error.to_string().contains("evil.com"), "{error}");
    }

    // Nothing reached the network at all — not the request, and not the
    // token fetch that would have preceded it.
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    // The guard rejects; it does not reject everything. A real path still
    // goes through, or the check would be indistinguishable from a break.
    client
        .get("/v1/reporting/balances", &[])
        .await
        .expect("a plain absolute path is still allowed");
    assert!(hits.load(Ordering::SeqCst) > 0);
    server.abort();
}

#[tokio::test]
async fn a_body_paypal_did_not_describe_is_logged_rather_than_relayed() {
    // Symmetric with the Chargebee client: PayPal's own `message` is a
    // classified failure the agent should read, but a body carrying none is
    // unidentified text on a payments API and this string reaches the
    // model's context and the durable transcript.
    let app = axum::Router::new().fallback(axum::routing::any(|| async {
        (
            axum::http::StatusCode::BAD_GATEWAY,
            [("content-type", "text/html")],
            "<html>upstream error for sb-ml643z@business.example.com, balance 5000.00</html>",
        )
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = PaypalClient::with_base_url(config(), format!("http://{addr}")).expect("builds");
    // The token call fails on the same body, which is the path that runs
    // first — both fallbacks share `unparsed_body_message`.
    let message = client
        .get("/v1/reporting/balances", &[])
        .await
        .expect_err("502 is an error")
        .to_string();
    assert!(!message.contains("business.example.com"), "{message}");
    assert!(!message.contains("5000.00"), "{message}");
    assert!(message.contains("host log"), "{message}");
    server.abort();
}
