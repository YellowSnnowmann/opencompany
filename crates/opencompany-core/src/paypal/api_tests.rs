use super::*;

use crate::company::paypal::PaypalEnvironment;
use crate::paypal::client::{PaypalClient, PaypalConfig};

/// A client pointed at a stub serving `body` for every PayPal path.
///
/// The token endpoint answers too, so an operation under test goes through
/// the same auth path it does in production rather than a client with the
/// credential step skipped. Abort the returned handle when done.
async fn stub_client(body: &'static str) -> (PaypalClient, tokio::task::JoinHandle<()>) {
    let handler = move |uri: axum::http::Uri| async move {
        let payload = if uri.path().contains("oauth2/token") {
            r#"{"access_token":"tok","expires_in":32400}"#
        } else {
            body
        };
        (
            axum::http::StatusCode::OK,
            [("content-type", "application/json")],
            payload,
        )
    };
    let app = axum::Router::new().fallback(axum::routing::any(handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let client = PaypalClient::with_base_url(
        PaypalConfig {
            client_id: "AY_id".to_string(),
            client_secret: "EL_secret".to_string(),
            environment: PaypalEnvironment::Sandbox,
        },
        format!("http://{addr}"),
    )
    .expect("client builds");
    (client, server)
}

#[tokio::test]
async fn a_balance_keeps_paypals_decimal_string_verbatim() {
    // Through an f64 this becomes 4320.500000000001 on some inputs. It is
    // rendered to an operator and never computed on, so it stays text.
    // Driven through `get_wallet_balance` itself, against a stub serving
    // PayPal's real response shape. An earlier version of this test rebuilt
    // the projection inline and asserted on its own copy — which passes
    // whatever the production projection does, including not existing.
    let (client, server) = stub_client(
        r#"{"balances":[{
            "currency":"USD","primary":true,
            "available_balance":{"currency_code":"USD","value":"4320.50"},
            "withheld_balance":{"currency_code":"USD","value":"12.30"}
        },{
            "currency":"EUR","primary":false,
            "available_balance":{"currency_code":"EUR","value":"0.00"},
            "withheld_balance":{"currency_code":"EUR","value":"0.00"}
        }]}"#,
    )
    .await;
    let balances = get_wallet_balance(&client).await.expect("balances");
    server.abort();

    assert_eq!(balances.len(), 2);
    // Through an f64 this becomes 4320.500000000001 on some inputs. It is
    // rendered to an operator and never computed on, so it stays text.
    assert_eq!(balances[0].available, "4320.50");
    assert_eq!(balances[0].withheld, "12.30");
    assert_eq!(balances[0].currency_code, "USD");
    assert!(balances[0].primary);
    assert_eq!(balances[1].currency_code, "EUR");
    assert!(!balances[1].primary);
}

#[tokio::test]
async fn a_reply_without_a_balances_array_is_an_error_not_an_empty_wallet() {
    // An account always has balances, so a reply without them is a broken
    // integration — reporting "no funds" would be a confident lie about
    // money. The transaction query follows the same rule: an empty array
    // is a real empty history, but a missing array is not.
    let (client, server) = stub_client(r#"{"name":"INTERNAL","debug_id":"x"}"#).await;
    let err = get_wallet_balance(&client)
        .await
        .expect_err("a missing array is an error");
    server.abort();
    let rendered = err.to_string();
    assert!(rendered.contains("balances"), "{rendered}");
    // And the body itself is logged, not relayed into the transcript.
    assert!(!rendered.contains("debug_id"), "{rendered}");
}

#[tokio::test]
async fn a_balance_with_no_amount_is_an_error_rather_than_a_fabricated_zero() {
    // These fields used to default to "0.00". A response whose shape drifted
    // therefore reported a funded wallet as EMPTY — not a degraded answer but
    // a confident wrong one, which an agent relays and an operator acts on.
    for (label, body) in [
        (
            "no available_balance at all",
            r#"{"balances":[{"currency":"USD","primary":true,
                "withheld_balance":{"currency_code":"USD","value":"12.30"}}]}"#,
        ),
        (
            "an available_balance with no value",
            r#"{"balances":[{"currency":"USD","primary":true,
                "available_balance":{"currency_code":"USD"},
                "withheld_balance":{"currency_code":"USD","value":"12.30"}}]}"#,
        ),
        (
            "an empty-string value",
            r#"{"balances":[{"currency":"USD","primary":true,
                "available_balance":{"currency_code":"USD","value":""},
                "withheld_balance":{"currency_code":"USD","value":"12.30"}}]}"#,
        ),
        (
            "no withheld_balance",
            r#"{"balances":[{"currency":"USD","primary":true,
                "available_balance":{"currency_code":"USD","value":"4320.50"}}]}"#,
        ),
    ] {
        let (client, server) = stub_client(body).await;
        let err = match get_wallet_balance(&client).await {
            Ok(balances) => {
                panic!("{label}: a missing amount must not become a number: {balances:?}")
            }
            Err(err) => err,
        };
        server.abort();
        let rendered = err.to_string();
        // The report names WHICH part of the shape moved, so the fix is
        // findable rather than "PayPal broke".
        assert!(rendered.contains("balance.value"), "{label}: {rendered}");
        assert!(!rendered.contains("0.00"), "{label}: {rendered}");
    }
}

#[tokio::test]
async fn a_transaction_with_no_amount_is_an_error_rather_than_a_zero_payment() {
    // Same rule for the transaction list: a payment reported as 0.00 reads
    // as a failed or free transaction, which is a lie about money rather
    // than a gap in the answer.
    let (client, server) = stub_client(
        r#"{"transaction_details":[{"transaction_info":{
            "transaction_id":"T1","transaction_status":"S",
            "transaction_initiation_date":"2026-08-01T00:00:00+0000"
        }}]}"#,
    )
    .await;
    let err = match list_transactions(
        &client,
        "2026-08-01T00:00:00Z",
        "2026-08-02T00:00:00Z",
        None,
    )
    .await
    {
        Ok(rows) => panic!("a missing amount must not become 0.00: {rows:?}"),
        Err(err) => err,
    };
    server.abort();
    let rendered = err.to_string();
    assert!(rendered.contains("transaction_amount.value"), "{rendered}");
    assert!(!rendered.contains("0.00"), "{rendered}");
}

#[tokio::test]
async fn a_reply_without_transaction_details_is_an_error_not_an_empty_history() {
    let (client, server) = stub_client(r#"{"name":"INTERNAL","debug_id":"x"}"#).await;
    let err = list_transactions(
        &client,
        "2026-08-01T00:00:00Z",
        "2026-08-02T00:00:00Z",
        None,
    )
    .await
    .expect_err("a missing array is an error");
    server.abort();
    let rendered = err.to_string();
    assert!(rendered.contains("transaction_details"), "{rendered}");
    assert!(!rendered.contains("debug_id"), "{rendered}");
}

#[test]
fn an_unavailable_window_is_explained_rather_than_relayed() {
    // PayPal's own words read as "there were no transactions", which sends
    // an agent guessing at timeframes instead of moving the start date back.
    let raw = OpenCompanyError::Paypal {
        status: 400,
        code: "INVALID_REQUEST".to_string(),
        message: "Data for the given start date is not available.".to_string(),
    };
    let explained = explain_unavailable_window(raw).to_string();
    assert!(
        explained.contains("Data for the given start date"),
        "{explained}"
    );
    assert!(explained.contains("3 hours"), "{explained}");
    assert!(explained.contains("start_date"), "{explained}");

    // Every other failure passes through untouched — this must not become a
    // catch-all that buries unrelated PayPal errors under a date hint.
    let other = OpenCompanyError::Paypal {
        status: 401,
        code: "NOT_AUTHORIZED".to_string(),
        message: "Authorization failed due to insufficient permissions.".to_string(),
    };
    let untouched = explain_unavailable_window(other).to_string();
    assert!(!untouched.contains("3 hours"), "{untouched}");

    // And neither does an unrelated failure that merely says "not
    // available" — a 404 answered with advice about start dates sends the
    // agent adjusting timeframes for a problem that has nothing to do with
    // them.
    let missing = OpenCompanyError::Paypal {
        status: 404,
        code: "RESOURCE_NOT_FOUND".to_string(),
        message: "The requested resource is not available.".to_string(),
    };
    let relayed = explain_unavailable_window(missing).to_string();
    assert!(!relayed.contains("3 hours"), "{relayed}");
    assert!(!relayed.contains("start_date"), "{relayed}");
}

#[tokio::test]
async fn a_missing_date_is_rejected_before_any_request() {
    use crate::company::paypal::PaypalEnvironment;
    use crate::paypal::client::{PaypalClient, PaypalConfig};
    // Port 0 never listens, so anything that reached the network would fail
    // fast rather than hang — the rejection must happen before that.
    let client = PaypalClient::with_base_url(
        PaypalConfig {
            client_id: "id".into(),
            client_secret: "secret".into(),
            environment: PaypalEnvironment::Sandbox,
        },
        "http://127.0.0.1:0".into(),
    )
    .expect("builds");

    let err = list_transactions(&client, "", "2026-08-13T00:00:00Z", None)
        .await
        .expect_err("an empty start is rejected");
    assert!(err.to_string().contains("31 days"), "got: {err}");
}
