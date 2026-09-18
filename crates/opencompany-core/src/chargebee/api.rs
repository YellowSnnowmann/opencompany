//! The billing operations issue #788 scopes, expressed against Chargebee's
//! REST API v2 and returning the compact projections in [`super::types`].
//!
//! This layer knows Chargebee and nothing about agents. The toolbelt bridge in
//! [`crate::harness::chargebee`] wraps each function below as an agent-callable
//! tool; keeping the split means the API shapes can be tested without a harness,
//! and the tool descriptions can change without touching the wire format.
//!
//! Arguments are validated here, before any network call, whenever the check is
//! one Chargebee would also make. That is not redundancy: a local rejection can
//! name the valid set, whereas Chargebee's own error arrives after a round trip
//! and, for the agent, after a turn that looked like it was working.

use crate::error::{OpenCompanyError, Result};
use serde_json::Value;

use super::client::{ChargebeeClient, Form};
use super::types::{
    CreateCustomerArgs, CustomerSummary, GetInvoiceArgs, InvoiceSummary, ListInvoicesArgs,
    SendInvoiceArgs,
};

/// Builds the invalid-argument error used for every local validation failure.
pub(crate) fn invalid(message: impl Into<String>) -> OpenCompanyError {
    OpenCompanyError::Chargebee {
        status: 0,
        code: "invalid_arguments".to_string(),
        message: message.into(),
    }
}

/// Percent-encodes a path segment.
///
/// Customer and invoice ids reach the URL path and originate in agent input, so
/// a value containing `/` or `?` would otherwise re-target the request at a
/// different endpoint.
fn urlencode(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Whether a failure is Chargebee refusing `net_term_days` because the site has
/// no payment-terms feature.
///
/// Matched on the message rather than an `api_error_code`, because Chargebee
/// reports it as a generic `invalid_request`: the specific cause lives only in
/// the prose. Deliberately requires BOTH markers so an unrelated invalid_request
/// mentioning one word is not swallowed.
fn mentions_payment_terms(error: &OpenCompanyError) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("net_term_days") && text.contains("payment terms")
}

/// Pulls a required object out of a Chargebee response.
///
/// Every write below reads one named object (`customer`, `invoice`) out of the
/// reply. Defaulting a missing one to `Null` and projecting it anyway yields a
/// record with an **empty id** that looks successful, and the next call spends
/// it — `customer_id=` on an invoice create, which Chargebee answers with a
/// confusing parameter error far from the real cause. So an absent object is an
/// error here, where it can name what was expected.
fn require<'a>(body: &'a Value, key: &str) -> Result<&'a Value> {
    body.get(key).filter(|v| v.is_object()).ok_or_else(|| {
        // The body goes to the log, not into the message. This one PARSED, so
        // unlike the client's unusable-body case it is a real Chargebee object
        // — which is exactly why it must not be quoted back: a reply that was
        // missing its `invoice` still carries whatever else Chargebee sent
        // about the customer, and this message reaches the model's context and
        // the durable transcript.
        tracing::warn!(
            expected = key,
            body = %body.to_string().chars().take(200).collect::<String>(),
            "[chargebee] reply carried no `{key}` object"
        );
        OpenCompanyError::Chargebee {
            status: 0,
            code: "unexpected_response".to_string(),
            message: format!(
                "Chargebee's reply carried no `{key}` object. The reply is in the host log."
            ),
        }
    })
}

/// Pulls a required *array* out of a Chargebee response, or fails.
///
/// A successful empty `list` means "no rows" — that is the real answer and
/// stays real. A missing or non-array `list` means the reply's shape moved,
/// and projecting that as an empty result would be a confident false negative
/// about a billing system (an agent answering "no invoices" to a site that may
/// hold any number of them). `paypal::api::list_transactions` makes the same
/// call for the same reason.
fn require_array<'a>(body: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    body.get(key).and_then(Value::as_array).ok_or_else(|| {
        tracing::warn!(
            expected = key,
            body = %body.to_string().chars().take(200).collect::<String>(),
            "[chargebee] reply carried no `{key}` array"
        );
        OpenCompanyError::Chargebee {
            status: 0,
            code: "unexpected_response".to_string(),
            message: format!(
                "Chargebee's reply carried no `{key}` array. The reply is in the host log."
            ),
        }
    })
}

/// Projects Chargebee's invoice object onto [`InvoiceSummary`].
fn summarize_invoice(invoice: &Value, payment_url: Option<String>) -> InvoiceSummary {
    let num = |key: &str| invoice.get(key).and_then(Value::as_i64).unwrap_or(0);
    InvoiceSummary {
        id: invoice
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        customer_id: invoice
            .get("customer_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: invoice
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        currency_code: invoice
            .get("currency_code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        total_in_minor_units: num("total"),
        amount_due_in_minor_units: num("amount_due"),
        amount_paid_in_minor_units: num("amount_paid"),
        due_date: invoice.get("due_date").and_then(Value::as_i64),
        line_items: invoice
            .get("line_items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|li| li.get("description").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        payment_url,
        // Only `send_invoice` can observe a replay; a fetched or listed invoice
        // is never one.
        replayed_earlier_invoice: false,
    }
}

/// Projects Chargebee's customer object onto [`CustomerSummary`].
fn summarize_customer(customer: &Value) -> CustomerSummary {
    let text = |key: &str| {
        customer
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let name = match (text("first_name"), text("last_name")) {
        (Some(first), Some(last)) => Some(format!("{first} {last}")),
        (Some(one), None) | (None, Some(one)) => Some(one),
        (None, None) => None,
    };
    CustomerSummary {
        id: customer
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        email: text("email"),
        name,
        company: text("company"),
    }
}

/// Looks a customer up by email, returning `None` when no record matches.
pub async fn get_customer(
    client: &ChargebeeClient,
    email: &str,
) -> Result<Option<CustomerSummary>> {
    let email = email.trim();
    if email.is_empty() {
        return Err(invalid("`email` is required"));
    }
    let mut query = Form::new();
    // Chargebee filters take an operator suffix: a bare `email=` is IGNORED
    // rather than rejected, which would return an unrelated customer as if it
    // were a match — the worst possible failure for a tool that decides whether
    // to create one.
    query.push("email[is]", email);
    query.push("limit", "1");
    let body = client.get("/customers", &query).await?;
    Ok(body
        .get("list")
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("customer"))
        .map(summarize_customer))
}

/// Creates a customer.
pub async fn create_customer(
    client: &ChargebeeClient,
    args: CreateCustomerArgs,
) -> Result<CustomerSummary> {
    let email = args.email.trim();
    if email.is_empty() {
        return Err(invalid("`email` is required"));
    }
    let mut form = Form::new();
    form.push("email", email);
    if let Some(name) = args
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // Chargebee has no single `name` field. Splitting on the first space
        // keeps "Alan" whole and "Ada Byron" correct; a middle name lands in
        // `last_name`, which is wrong in a way nobody is harmed by.
        match name.split_once(' ') {
            Some((first, last)) => {
                form.push("first_name", first);
                form.push("last_name", last);
            }
            None => form.push("first_name", name),
        }
    }
    form.push_opt("company", args.company);
    let body = client.post_form("/customers", &form, None).await?;
    Ok(summarize_customer(require(&body, "customer")?))
}

/// Returns the customer for `email`, creating one when no record matches.
async fn resolve_or_create_customer(
    client: &ChargebeeClient,
    email: &str,
    name: Option<String>,
) -> Result<CustomerSummary> {
    if let Some(found) = get_customer(client, email).await? {
        return Ok(found);
    }
    create_customer(
        client,
        CreateCustomerArgs {
            email: email.to_string(),
            name,
            company: None,
        },
    )
    .await
}

/// Raises a hosted page where the customer can settle what they owe.
///
/// Best-effort by design: this is a second call after the invoice already
/// exists, and a site without a configured gateway (or without the hosted-page
/// feature) refuses it. Failing the whole tool at that point would report "no
/// invoice" for an invoice that was in fact created — the worst answer
/// available. So a failure logs and yields `None`, and the caller says the
/// invoice was raised without a link.
async fn payment_url(
    client: &ChargebeeClient,
    customer_id: &str,
    currency: &str,
) -> Option<String> {
    let mut form = Form::new();
    form.push("customer[id]", customer_id);
    form.push("currency_code", currency);
    match client
        .post_form("/hosted_pages/collect_now", &form, None)
        .await
    {
        Ok(body) => body
            .get("hosted_page")
            .and_then(|p| p.get("url"))
            .and_then(Value::as_str)
            .map(str::to_string),
        Err(e) => {
            tracing::warn!(%customer_id, error = %e, "[chargebee] could not raise a payment link");
            None
        }
    }
}

/// Derives an idempotency key from the request itself.
///
/// # Why a key is always sent, even when the caller supplied none
///
/// The runtime's at-most-once guard covers **approval replay**: an approved
/// effect is recorded executed before it is performed, so re-approving does not
/// re-send. It does not cover **transport retry**, which is the failure that
/// actually duplicates an invoice — the request reaches Chargebee, the response
/// is lost to a timeout, the tool reports failure, and the agent (or an
/// operator reading that failure) sends again. The customer receives two
/// invoices.
///
/// The key was an optional tool argument, which in practice meant absent: a
/// model has no reason to invent one, and every send observed in testing
/// omitted it. Deriving one from the request body closes that by default. It is
/// deliberately derived from the REQUEST rather than from the approved effect —
/// the effect id is not reachable here, because an approved call is re-issued
/// by the model through the ordinary tool path (`redispatch_granted_call`)
/// rather than executed by the runtime with the effect in scope.
///
/// The trade this makes is explicit: two byte-identical invoices raised inside
/// Chargebee's key-retention window collapse to one. That is why a replay is
/// reported back rather than passed off as a new invoice — see
/// [`InvoiceSummary::replayed_earlier_invoice`] — and why a caller who means to
/// bill twice can pass a distinct `idempotency_key`.
///
/// FNV-1a rather than `DefaultHasher`, so the key is stable **as a value**, not
/// merely within one process. `DefaultHasher`'s output is explicitly not
/// guaranteed across Rust releases, which would mean a host upgraded mid-retry
/// — or two hosts of one company behind a load balancer — deriving different
/// keys for the same invoice and billing the customer twice. That is precisely
/// the failure this function exists to prevent, so the hash cannot be one whose
/// stability is a footnote about the toolchain. The field separators keep
/// `("ab","c")` from colliding with `("a","bc")`.
fn derived_idempotency_key(form: &Form) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(PRIME);
        }
    };
    for (key, value) in form.pairs() {
        eat(key.as_bytes());
        eat(b"=");
        eat(value.as_bytes());
        eat(b"&");
    }
    format!("oc-invoice-{hash:016x}")
}

/// Creates an invoice for `customer_email`, creating the customer if needed,
/// and returns it with a payment link when one could be raised.
pub async fn send_invoice(
    client: &ChargebeeClient,
    args: SendInvoiceArgs,
) -> Result<InvoiceSummary> {
    if args.line_items.is_empty() {
        return Err(invalid("`line_items` must contain at least one entry"));
    }
    let currency = args.currency_code.trim().to_uppercase();
    if currency.is_empty() {
        return Err(invalid("`currency_code` is required, e.g. USD"));
    }
    for (i, line) in args.line_items.iter().enumerate() {
        if line.amount_in_minor_units < 1 {
            return Err(invalid(format!(
                "line_items[{i}].amount_in_minor_units must be at least 1 (amounts are in minor \
                 units — $100.00 is 10000)"
            )));
        }
    }

    let customer =
        resolve_or_create_customer(client, args.customer_email.trim(), args.customer_name).await?;

    let mut form = Form::new();
    form.push("customer_id", &customer.id);
    form.push("currency_code", &currency);
    // Unasked-for, and load-bearing. Chargebee's default follows the customer
    // record and charges a stored card the moment the invoice exists — verified
    // against a live site, which answered `payment_method_not_present`. Two
    // reasons to override it: "send an invoice" is not "take a payment", and an
    // auto-collected invoice is already paid, which would make the "has Alan
    // paid?" flow (#788) answer itself.
    form.push("auto_collection", "off");
    form.push_opt("net_term_days", args.due_days);
    form.push_opt("invoice_note", args.invoice_note);
    for (i, line) in args.line_items.iter().enumerate() {
        form.push_indexed("charges", "description", i, &line.description);
        form.push_indexed("charges", "amount", i, line.amount_in_minor_units);
    }

    let path = "/invoices/create_for_charge_items_and_charges";
    let key = args
        .idempotency_key
        .clone()
        .unwrap_or_else(|| derived_idempotency_key(&form));
    let (body, replayed) = match client.post_form_replayable(path, &form, Some(&key)).await {
        Ok(outcome) => outcome,
        // `net_term_days` is refused outright by a site that has not enabled
        // "Payment Terms for One-Time Invoices" — a per-site feature most test
        // sites ship without. Failing the whole invoice over a DUE DATE is the
        // wrong trade: the operator asked for an invoice and would rather have
        // one without terms than none at all. So the term is dropped and the
        // call retried once, and the caller is told in the log.
        //
        // Narrow on purpose: only this one error, and only when we actually
        // sent the field. Anything else propagates untouched.
        Err(e) if args.due_days.is_some() && mentions_payment_terms(&e) => {
            tracing::warn!(
                "[chargebee] this site has not enabled payment terms for one-time invoices; \
                 raising the invoice without a due date"
            );
            let mut retry = Form::new();
            for (field, value) in form.pairs() {
                if field != "net_term_days" {
                    retry.push(field.clone(), value.clone());
                }
            }
            // A DIFFERENT key from the first attempt, deliberately. Chargebee
            // may have stored that attempt's 400 against its key, and replaying
            // a refusal would turn the recovery into the failure it exists to
            // avoid. The retry is a genuinely different request — it asks for
            // no payment terms — so it gets its own key. A derived key changes
            // on its own, since the body changed; a caller-supplied one is
            // suffixed rather than reused.
            let retry_key = match &args.idempotency_key {
                Some(supplied) => format!("{supplied}-no-terms"),
                None => derived_idempotency_key(&retry),
            };
            client
                .post_form_replayable(path, &retry, Some(&retry_key))
                .await?
        }
        Err(e) => return Err(e),
    };
    let invoice = require(&body, "invoice")?.clone();
    let url = payment_url(client, &customer.id, &currency).await;
    let mut summary = summarize_invoice(&invoice, url);
    if replayed {
        // Chargebee returned an earlier invoice verbatim, so nothing was
        // raised. Reported rather than swallowed: for a retry this is the
        // outcome you want, and for a deliberate second charge it is the one
        // fact that distinguishes "billed twice" from "billed once".
        tracing::warn!(
            invoice_id = %summary.id,
            "[chargebee] send_invoice replayed an earlier invoice for this idempotency key"
        );
        summary.replayed_earlier_invoice = true;
    }
    Ok(summary)
}

/// Fetches one invoice by id.
pub async fn get_invoice(client: &ChargebeeClient, args: GetInvoiceArgs) -> Result<InvoiceSummary> {
    let id = args.invoice_id.trim();
    if id.is_empty() {
        return Err(invalid("`invoice_id` is required"));
    }
    let body = client
        .get(&format!("/invoices/{}", urlencode(id)), &Form::new())
        .await?;
    Ok(summarize_invoice(require(&body, "invoice")?, None))
}

/// Lists invoices, optionally narrowed to one customer and/or status.
pub async fn list_invoices(
    client: &ChargebeeClient,
    args: ListInvoicesArgs,
) -> Result<Vec<InvoiceSummary>> {
    let mut query = Form::new();
    if let Some(email) = args
        .customer_email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // An email that matches nobody must return "no invoices", not "every
        // invoice on the site" — which is what dropping an unresolvable filter
        // would do.
        let Some(customer) = get_customer(client, email).await? else {
            return Ok(Vec::new());
        };
        query.push("customer_id[is]", customer.id);
    }
    query.push_opt("status[is]", args.status);
    query.push_opt("limit", args.limit);

    let body = client.get("/invoices", &query).await?;
    let rows = require_array(&body, "list")?;
    Ok(rows
        .iter()
        .filter_map(|row| row.get("invoice"))
        .map(|invoice| summarize_invoice(invoice, None))
        .collect())
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
