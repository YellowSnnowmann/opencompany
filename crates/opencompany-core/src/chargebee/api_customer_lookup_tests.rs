use super::tests::stub;
use super::*;
use crate::chargebee::types::ChargeLine;

async fn lookup(reply: &'static str) -> Result<Option<CustomerSummary>> {
    let (result, _) = stub(vec![("GET /customers", 200, reply)], |client| async move {
        get_customer(&client, "alan@tinyhumans.ai").await
    })
    .await;
    result
}

fn unexpected(result: Result<Option<CustomerSummary>>) -> bool {
    matches!(
        result,
        Err(OpenCompanyError::Chargebee { ref code, .. }) if code == "unexpected_response"
    )
}

#[tokio::test]
async fn a_reply_without_a_list_is_an_error_not_no_customer() {
    assert!(unexpected(lookup("{}").await));
    assert!(unexpected(lookup(r#"{"list":{}}"#).await));
}

#[tokio::test]
async fn an_empty_list_is_no_customer() {
    assert!(matches!(lookup(r#"{"list":[]}"#).await, Ok(None)));
}

#[tokio::test]
async fn a_row_without_a_customer_object_is_an_error() {
    assert!(unexpected(lookup(r#"{"list":[{}]}"#).await));
}

#[tokio::test]
async fn a_customer_without_an_id_is_an_error() {
    assert!(unexpected(
        lookup(r#"{"list":[{"customer":{"email":"alan@tinyhumans.ai"}}]}"#).await
    ));
    assert!(unexpected(
        lookup(r#"{"list":[{"customer":{"id":""}}]}"#).await
    ));
}

#[tokio::test]
async fn a_found_customer_is_returned() {
    let found = lookup(r#"{"list":[{"customer":{"id":"cus_1","email":"alan@tinyhumans.ai"}}]}"#)
        .await
        .expect("ok")
        .expect("found");
    assert_eq!(found.id, "cus_1");
}

#[tokio::test]
async fn an_unreadable_lookup_does_not_create_a_duplicate_customer() {
    let (result, seen) = stub(
        vec![
            ("GET /customers", 200, "{}"),
            ("POST /customers", 200, r#"{"customer":{"id":"cus_dup"}}"#),
        ],
        |client| async move {
            send_invoice(
                &client,
                SendInvoiceArgs {
                    customer_email: "alan@tinyhumans.ai".to_string(),
                    customer_name: None,
                    currency_code: "usd".to_string(),
                    line_items: vec![ChargeLine {
                        description: "Consulting".to_string(),
                        amount_in_minor_units: 10_000,
                    }],
                    due_days: None,
                    invoice_note: None,
                    idempotency_key: None,
                },
            )
            .await
        },
    )
    .await;
    assert!(result.is_err(), "{result:?}");
    assert!(
        !seen
            .iter()
            .any(|r| r.method == "POST" && r.path.contains("/customers")),
        "no customer may be created: {seen:?}"
    );
}
