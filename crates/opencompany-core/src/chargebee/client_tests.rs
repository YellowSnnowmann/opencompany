use super::*;

#[test]
fn push_opt_omits_none_rather_than_sending_blank() {
    let mut form = Form::new();
    form.push("customer_id", "acme");
    form.push_opt("net_term_days", None::<i64>);
    form.push_opt("invoice_note", Some("Q3 retainer"));

    let keys: Vec<&str> = form.pairs().iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["customer_id", "invoice_note"]);
}

#[test]
fn indexed_encoding_is_per_field_not_per_object() {
    let mut form = Form::new();
    for (i, (desc, amount)) in [("Pro plan", 50_000), ("Setup", 2_500)].iter().enumerate() {
        form.push_indexed("charges", "description", i, desc);
        form.push_indexed("charges", "amount", i, amount);
    }

    let pairs: Vec<(&str, &str)> = form
        .pairs()
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("charges[description][0]", "Pro plan"),
            ("charges[amount][0]", "50000"),
            ("charges[description][1]", "Setup"),
            ("charges[amount][1]", "2500"),
        ]
    );
}

/// A body of exactly the shape that must not be quoted back: an HTML error
/// page from something in front of Chargebee, carrying a customer address
/// and an amount.
const LEAKY_BODY: &str = "<html><body>Gateway error for alan@tinyhumans.ai — invoice \
                          INV-0042, USD 100.00, request 9f3c-aa71</body></html>";

#[test]
fn a_client_debug_does_not_render_its_api_key() {
    // The client holds its own copy of the key, so redacting only
    // `ChargebeeConfig` leaves the same leak one level down.
    let client = ChargebeeClient::new(ChargebeeConfig {
        site: "acme-test".to_string(),
        api_key: "live_supersecret".to_string(),
    })
    .expect("builds");
    let rendered = format!("{client:?}");
    assert!(!rendered.contains("live_supersecret"), "{rendered}");
    assert!(rendered.contains("acme-test"), "{rendered}");
}

#[test]
fn an_unparseable_body_is_logged_rather_than_put_in_the_error() {
    // This message reaches `ToolResult::error`, so it lands in the model's
    // context and the turn's durable transcript. What an unparseable body
    // contains is unknown by construction — see `unparsed_body_message`.
    let message = unparsed_body_message(502, LEAKY_BODY);
    for secret in [
        "alan@tinyhumans.ai",
        "INV-0042",
        "100.00",
        "9f3c-aa71",
        "<html>",
    ] {
        assert!(
            !message.contains(secret),
            "`{secret}` must not reach the transcript: {message}"
        );
    }
    // The agent still learns the fact it can act on.
    assert!(message.contains("502"), "{message}");
    assert!(message.contains("host log"), "{message}");
}

#[test]
fn the_same_rule_applies_to_a_success_whose_body_is_not_an_object() {
    let rendered = err_body(200, "unexpected_response", LEAKY_BODY).to_string();
    assert!(!rendered.contains("alan@tinyhumans.ai"), "{rendered}");
    assert!(!rendered.contains("INV-0042"), "{rendered}");
}

#[tokio::test]
async fn chargebees_own_error_message_is_still_relayed_verbatim() {
    // The narrow half of the rule: a CLASSIFIED Chargebee failure names a
    // business outcome the model must read, and withholding it would leave
    // the agent unable to tell a refused request from a broken integration.
    let app = axum::Router::new().fallback(axum::routing::any(|| async {
        (
            axum::http::StatusCode::BAD_REQUEST,
            [("content-type", "application/json")],
            r#"{"api_error_code":"param_wrong_value","message":"currency_code : INR is not enabled for this site"}"#,
        )
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = ChargebeeClient::with_base_url(
        ChargebeeConfig {
            site: "test".to_string(),
            api_key: "cb_key".to_string(),
        },
        format!("http://{addr}"),
    )
    .expect("client builds");
    let message = client
        .get("/invoices/inv_1", &Form::new())
        .await
        .expect_err("400 is an error")
        .to_string();
    assert!(message.contains("INR is not enabled"), "{message}");
    server.abort();
}

#[tokio::test]
async fn a_replayed_post_is_reported_to_the_caller() {
    // Chargebee answers a repeated idempotency key with the ORIGINAL
    // response, so the body alone cannot distinguish a replay from a fresh
    // write. Only this header can.
    let app = axum::Router::new().fallback(axum::routing::any(|| async {
        (
            axum::http::StatusCode::OK,
            [
                ("content-type", "application/json"),
                ("chargebee-idempotency-replayed", "true"),
            ],
            r#"{"invoice":{"id":"inv_1"}}"#,
        )
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = ChargebeeClient::with_base_url(
        ChargebeeConfig {
            site: "test".to_string(),
            api_key: "cb_key".to_string(),
        },
        format!("http://{addr}"),
    )
    .expect("client builds");
    let (_body, replayed) = client
        .post_form_replayable("/invoices", &Form::new(), Some("key-1"))
        .await
        .expect("200");
    assert!(replayed, "the replay header must be surfaced");
    server.abort();
}
