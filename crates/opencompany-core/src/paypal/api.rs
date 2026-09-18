//! The two read operations issue #789 scopes: wallet balance and recent
//! transactions.
//!
//! Deliberately read-only. #789 lists `send_payment` as optional and requires a
//! scoping decision before implementation; moving money is not something to
//! ship on an "optional" line in an issue.

use serde::Serialize;
use serde_json::Value;

use super::client::PaypalClient;
use crate::error::{OpenCompanyError, Result};

/// One currency's balance in the account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Balance {
    /// ISO 4217 code.
    pub currency_code: String,
    /// The amount as PayPal reports it — a decimal STRING, e.g. `"4320.50"`.
    ///
    /// Kept as text rather than parsed into a float: this value is rendered to
    /// an operator, and `4320.50` through an `f64` is how a balance acquires a
    /// trailing `0000001`. Nothing here does arithmetic on it.
    pub available: String,
    /// Funds not yet available, same format.
    pub withheld: String,
    /// Whether this is the account's primary currency.
    pub primary: bool,
}

/// One transaction, projected down from PayPal's very large record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Transaction {
    /// PayPal's transaction id.
    pub id: String,
    /// ISO 8601, as PayPal reports it.
    pub date: String,
    /// Signed decimal string — negative for money leaving the account.
    pub amount: String,
    /// ISO 4217 code.
    pub currency_code: String,
    /// `S` success, `P` pending, `V` reversed, `D` denied.
    pub status: String,
    /// The counterparty's name or email, when PayPal supplies one.
    pub counterparty: Option<String>,
    /// The payer-supplied note, when present.
    pub note: Option<String>,
}

/// Rewrites PayPal's opaque "Data for the given start date is not available"
/// into something the caller can act on.
///
/// PayPal serves transaction data on a lag — a completed payment takes up to
/// three hours to appear — and rejects any window whose start is inside that
/// gap. Its own message says only that data "is not available", which reads as
/// "there were no transactions" rather than "ask for an earlier window", so an
/// agent handed it starts guessing at timeframes instead of moving the start
/// date back. That is exactly what happened in testing: a start date of today
/// failed, and the agent asked the operator to pick a different period rather
/// than knowing what to do.
///
/// Only this one error is rewritten, and the original text is kept alongside the
/// remedy so nothing is hidden.
fn explain_unavailable_window(error: OpenCompanyError) -> OpenCompanyError {
    let OpenCompanyError::Paypal {
        status,
        code,
        message,
    } = &error
    else {
        return error;
    };
    // Matched on PayPal's specific sentence, not a bare "not available".
    // The looser test also caught unrelated failures — "The requested resource
    // is not available" is a 404, and answering it with advice about start
    // dates sends the agent adjusting timeframes for a problem that has nothing
    // to do with them.
    if !message
        .to_ascii_lowercase()
        .contains("data for the given start date is not available")
    {
        return error;
    }
    OpenCompanyError::Paypal {
        status: *status,
        code: code.clone(),
        message: format!(
            "{message} PayPal publishes transactions on a delay of up to 3 hours, so a window \
             that starts today may have no data yet — retry with a `start_date` at least a day \
             earlier. The window must also span no more than 31 days."
        ),
    }
}

fn text(value: Option<&Value>, key: &str) -> Option<String> {
    value?
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Reads a money field PayPal must have supplied, or fails.
///
/// The same rule the missing-array checks below already apply, one level down:
/// **never invent a number about money.** These fields used to default to
/// `"0.00"`, so a response whose shape drifted — a renamed field, a projection
/// PayPal serves to some accounts, an object that arrives empty — reported a
/// funded wallet as empty and a real payment as a zero-value transaction. That
/// is not a degraded answer, it is a confident wrong one: an agent told the
/// balance is `0.00` says the company has no money, and an operator acts on it.
///
/// An error says "PayPal's reply did not parse", which is true and which
/// somebody can fix. `field` names the JSON path so the report identifies which
/// part of the shape moved; the reply itself goes to the log, never into the
/// message — same rule as `unparsed_body_message`.
fn money(parent: Option<&Value>, key: &str, field: &str) -> Result<String> {
    text(parent, key).ok_or_else(|| {
        tracing::warn!(
            field,
            parent = %parent.map(|value| value.to_string()).unwrap_or_else(|| "null".to_string())
                .chars().take(200).collect::<String>(),
            "[paypal] reply carried no amount where one is required"
        );
        OpenCompanyError::Paypal {
            status: 0,
            code: "unexpected_response".to_string(),
            message: format!(
                "PayPal's reply carried no `{field}`, and this host does not substitute a zero \
                 for an amount PayPal did not report. The reply is in the host log."
            ),
        }
    })
}

/// Fetches the account's balances.
pub async fn get_wallet_balance(client: &PaypalClient) -> Result<Vec<Balance>> {
    let body = client.get("/v1/reporting/balances", &[]).await?;
    let balances = body
        .get("balances")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            // Logged, not relayed — same rule as the client's
            // `unparsed_body_message` and `chargebee::api::require`.
            tracing::warn!(
                body = %body.to_string().chars().take(200).collect::<String>(),
                "[paypal] reply carried no `balances` array"
            );
            OpenCompanyError::Paypal {
                status: 0,
                code: "unexpected_response".to_string(),
                message:
                    "PayPal's reply carried no `balances` array. The reply is in the host log."
                        .to_string(),
            }
        })?;

    balances
        .iter()
        .map(|entry| {
            Ok(Balance {
                currency_code: text(Some(entry), "currency")
                    .or_else(|| text(entry.get("total_balance"), "currency_code"))
                    .unwrap_or_default(),
                available: money(
                    entry.get("available_balance"),
                    "value",
                    "balances[].available_balance.value",
                )?,
                withheld: money(
                    entry.get("withheld_balance"),
                    "value",
                    "balances[].withheld_balance.value",
                )?,
                primary: entry
                    .get("primary")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// Fetches transactions between two ISO 8601 instants.
///
/// PayPal caps the window at **31 days** and publishes on a lag of up to three
/// hours; both limits are enforced by PayPal, not here. Only the non-empty
/// check below is local — parsing ISO 8601 to pre-validate the span would mean
/// duplicating PayPal's calendar rules to save one round trip, and getting that
/// subtly wrong would reject windows PayPal would have accepted. What this does
/// instead is make PayPal's own refusal actionable: see
/// [`explain_unavailable_window`].
pub async fn list_transactions(
    client: &PaypalClient,
    start_date: &str,
    end_date: &str,
    page_size: Option<i64>,
) -> Result<Vec<Transaction>> {
    if start_date.trim().is_empty() || end_date.trim().is_empty() {
        return Err(OpenCompanyError::Paypal {
            status: 0,
            code: "invalid_arguments".to_string(),
            message: "`start_date` and `end_date` are both required, in ISO 8601. PayPal allows a \
                      window of at most 31 days and publishes on a delay of up to 3 hours."
                .to_string(),
        });
    }

    let query = vec![
        ("start_date".to_string(), start_date.trim().to_string()),
        ("end_date".to_string(), end_date.trim().to_string()),
        // Without this PayPal returns only ids and amounts — no counterparty,
        // no note — and the answer reads as a list of anonymous numbers.
        (
            "fields".to_string(),
            "transaction_info,payer_info".to_string(),
        ),
        (
            "page_size".to_string(),
            page_size.unwrap_or(20).clamp(1, 500).to_string(),
        ),
    ];

    let body = client
        .get("/v1/reporting/transactions", &query)
        .await
        .map_err(explain_unavailable_window)?;
    let rows = body
        .get("transaction_details")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            // A successful empty array means no transactions. A missing array
            // means the response shape changed, and reporting the latter as an
            // empty history would be a confident lie about money.
            tracing::warn!(
                body = %body.to_string().chars().take(200).collect::<String>(),
                "[paypal] reply carried no `transaction_details` array"
            );
            OpenCompanyError::Paypal {
                status: 0,
                code: "unexpected_response".to_string(),
                message: "PayPal's reply carried no `transaction_details` array. The reply is in the host log."
                    .to_string(),
            }
        })?;

    rows.iter()
        .map(|row| {
            let info = row.get("transaction_info");
            let payer = row.get("payer_info");
            Ok(Transaction {
                id: text(info, "transaction_id").unwrap_or_default(),
                date: text(info, "transaction_initiation_date").unwrap_or_default(),
                amount: money(
                    info.and_then(|i| i.get("transaction_amount")),
                    "value",
                    "transaction_details[].transaction_info.transaction_amount.value",
                )?,
                currency_code: text(
                    info.and_then(|i| i.get("transaction_amount")),
                    "currency_code",
                )
                .unwrap_or_default(),
                status: text(info, "transaction_status").unwrap_or_default(),
                counterparty: text(
                    payer.and_then(|p| p.get("payer_name")),
                    "alternate_full_name",
                )
                .or_else(|| text(payer, "email_address")),
                note: text(info, "transaction_note"),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
