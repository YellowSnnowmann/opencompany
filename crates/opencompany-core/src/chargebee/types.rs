//! Configuration and argument types for the Chargebee billing tools.
//!
//! Every money-carrying field is named `*_in_minor_units` and typed `i64` on
//! purpose. Chargebee's API is minor-unit based (`amount = 10000` is $100.00 for
//! a two-decimal currency), and an agent filling a field called plain `amount`
//! from the prompt "invoice Alan $100" will write `100` — a $1.00 invoice that
//! succeeds, returns a plausible invoice object, and is wrong by two orders of
//! magnitude. The unit lives in the field name so the mistake has to be made
//! deliberately.
//!
//! This deviates from the tool signatures sketched in issue #788 (`amount: 100`).
//! The deviation is the point: a float dollar amount also invites binary
//! rounding on money, and integer minor units are what Chargebee actually takes.

use serde::{Deserialize, Serialize};

pub use crate::company::billing::{API_KEY_SECRET, SITE_SECRET};

/// Connection settings for one company's Chargebee site.
///
/// `Debug` is hand-written to redact the key — see the impl below.
#[derive(Clone)]
pub struct ChargebeeConfig {
    /// The Chargebee site slug, i.e. the `acme` in `acme.chargebee.com`.
    pub site: String,
    /// The site's API key, sent as the HTTP Basic username.
    pub api_key: String,
}

/// Prints the site and **redacts the key**.
///
/// Not a nicety. This struct is reachable from `HarnessDeps`, which is a large
/// aggregate that debugging code prints wholesale; a derived `Debug` puts a live
/// Chargebee API key into any log line that ever formats one. Caught by a test
/// that asserted the key could not reach a `Debug` rendering — and, before this
/// impl, it could.
impl std::fmt::Debug for ChargebeeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChargebeeConfig")
            .field("site", &self.site)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl ChargebeeConfig {
    /// The API v2 base URL for this site, without a trailing slash.
    pub fn base_url(&self) -> String {
        format!("https://{}.chargebee.com/api/v2", self.site)
    }
}

/// One ad-hoc line item on an invoice.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChargeLine {
    /// Line-item text shown on the invoice.
    pub description: String,
    /// The charge amount in the currency's minor unit (cents for USD).
    /// Chargebee rejects anything below 1.
    pub amount_in_minor_units: i64,
}

/// Arguments for `chargebee_send_invoice`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SendInvoiceArgs {
    /// The customer's email. Resolved to a Chargebee customer, and one is
    /// created if no match exists — so the agent never has to ask the operator
    /// for an internal id it could not know (issue #788, TC-05).
    pub customer_email: String,
    /// Display name for a customer that has to be created. Ignored when the
    /// customer already exists — renaming is not a side effect of invoicing.
    #[serde(default)]
    pub customer_name: Option<String>,
    /// ISO 4217 code, e.g. `USD`. Required: Chargebee will not infer it for a
    /// site with more than one enabled currency, and inferring it here would
    /// mean guessing what money the operator meant.
    pub currency_code: String,
    /// At least one line item.
    pub line_items: Vec<ChargeLine>,
    /// Days until the invoice falls due.
    #[serde(default)]
    pub due_days: Option<i64>,
    /// Free-text note stored on the invoice.
    #[serde(default)]
    pub invoice_note: Option<String>,
    /// Optional `chargebee-idempotency-key`. A retried agent turn that reuses
    /// this key gets the original invoice back instead of billing twice.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// Arguments for `chargebee_get_invoice`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GetInvoiceArgs {
    /// The Chargebee invoice id.
    pub invoice_id: String,
}

/// Arguments for `chargebee_list_invoices`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ListInvoicesArgs {
    /// Restrict to one customer by email. Resolved to a customer id first.
    #[serde(default)]
    pub customer_email: Option<String>,
    /// Chargebee invoice status: `paid`, `posted`, `payment_due`, `not_paid`,
    /// `voided`, or `pending`.
    #[serde(default)]
    pub status: Option<String>,
    /// Page size, 1-100. Chargebee's own default is 10.
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Arguments for `chargebee_get_customer`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GetCustomerArgs {
    /// The email to look up.
    pub email: String,
}

/// Arguments for `chargebee_create_customer`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateCustomerArgs {
    /// Billing email. The identity the other tools resolve against.
    pub email: String,
    /// Full name, split on the first space into Chargebee's first/last fields.
    #[serde(default)]
    pub name: Option<String>,
    /// Company name, e.g. `Acme Corp`.
    #[serde(default)]
    pub company: Option<String>,
}

/// A compact projection of a Chargebee invoice, returned to the agent instead
/// of the raw API object.
///
/// Chargebee's invoice payload is ~40 fields plus nested line items, billing
/// address and empty collections. Handing that to a model verbatim costs a
/// large amount of context for one invoice and buries the three facts an
/// operator asked about. Anything omitted is still reachable — the agent can
/// call `chargebee_get_invoice` — but the default answer stays legible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InvoiceSummary {
    /// Chargebee's invoice id.
    pub id: String,
    /// The customer this invoice belongs to.
    pub customer_id: String,
    /// `paid`, `payment_due`, `posted`, `voided`, …
    pub status: String,
    /// ISO 4217 code.
    pub currency_code: String,
    /// Invoice total, in minor units.
    pub total_in_minor_units: i64,
    /// Outstanding balance, in minor units.
    pub amount_due_in_minor_units: i64,
    /// Settled so far, in minor units.
    pub amount_paid_in_minor_units: i64,
    /// Unix seconds, when Chargebee reports one.
    pub due_date: Option<i64>,
    /// Line-item descriptions, in order.
    pub line_items: Vec<String>,
    /// A hosted page where the customer can pay, when one could be raised.
    ///
    /// `None` is not a failure: the payment page is a second API call after the
    /// invoice exists, and an invoice that was created but whose link could not
    /// be raised is still a real invoice the operator should hear about.
    pub payment_url: Option<String>,
    /// Set when Chargebee **replayed** an earlier invoice for this idempotency
    /// key rather than raising a new one.
    ///
    /// Serialised only when true, so the ordinary result is unchanged. It has to
    /// be reported at all because a replayed response is byte-identical to the
    /// original: without this flag a deliberate second identical invoice that
    /// was deduped reads exactly like a successful new one, which is a silent
    /// failure to bill.
    #[serde(skip_serializing_if = "is_false")]
    pub replayed_earlier_invoice: bool,
}

/// `skip_serializing_if` predicate — `bool::not` takes `self` by value and so
/// cannot be used here.
fn is_false(value: &bool) -> bool {
    !*value
}

/// A compact projection of a Chargebee customer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CustomerSummary {
    /// Chargebee's customer id.
    pub id: String,
    /// Billing email, when set.
    pub email: Option<String>,
    /// Display name, assembled from first/last.
    pub name: Option<String>,
    /// Company name, when set.
    pub company: Option<String>,
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
