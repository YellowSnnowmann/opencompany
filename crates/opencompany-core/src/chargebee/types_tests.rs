use super::*;

#[test]
fn base_url_is_the_site_api_v2_root() {
    let cfg = ChargebeeConfig {
        site: "acme-test".to_string(),
        api_key: "cb_test_key".to_string(),
    };
    assert_eq!(cfg.base_url(), "https://acme-test.chargebee.com/api/v2");
}

#[test]
fn debug_never_renders_the_api_key() {
    // Reachable from `HarnessDeps`, which debugging code prints wholesale —
    // a derived Debug put a live key in any log line that formatted one.
    let rendered = format!(
        "{:?}",
        ChargebeeConfig {
            site: "acme-test".to_string(),
            api_key: "cb_live_SUPERSECRET".to_string(),
        }
    );
    assert!(rendered.contains("<redacted>"), "{rendered}");
    assert!(!rendered.contains("SUPERSECRET"), "{rendered}");
    assert!(
        rendered.contains("acme-test"),
        "the site is not secret: {rendered}"
    );
}

#[test]
fn a_bare_amount_does_not_satisfy_a_line_item() {
    // The whole point of the naming convention: "$100" becoming `100`
    // must not deserialize into a field that means cents.
    assert!(
        serde_json::from_str::<ChargeLine>(r#"{"description":"Consulting","amount":100}"#).is_err(),
        "a bare `amount` must not satisfy ChargeLine"
    );
    let ok: ChargeLine =
        serde_json::from_str(r#"{"description":"Consulting","amount_in_minor_units":10000}"#)
            .expect("explicit minor units parse");
    assert_eq!(ok.amount_in_minor_units, 10_000);
}

#[test]
fn send_invoice_needs_only_email_currency_and_lines() {
    let args: SendInvoiceArgs = serde_json::from_str(
        r#"{"customer_email":"alan@tinyhumans.ai","currency_code":"USD",
            "line_items":[{"description":"Consulting","amount_in_minor_units":10000}]}"#,
    )
    .expect("minimal args parse");
    assert_eq!(args.customer_email, "alan@tinyhumans.ai");
    assert_eq!(args.due_days, None);
    assert_eq!(args.customer_name, None);
    assert_eq!(args.line_items.len(), 1);
}
