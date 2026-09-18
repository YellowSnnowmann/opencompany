//! The finance **read plane**: the console's path to live Chargebee and PayPal
//! data (issue #788, #789; console design in
//! `docs/spec/runtime/finance-console.md`).
//!
//! # Why this module exists at all
//!
//! `crate::chargebee::api` and `crate::paypal::api` were reachable only from a
//! harness turn. Every operation the console's Finance section needs already
//! existed and had no HTTP address, so an operator could ask an agent "has Alan
//! paid?" and could not answer it themselves.
//!
//! # What it is not
//!
//! A **thin adapter**, deliberately: resolve the company's credentials, build
//! the provider client, call the existing `api` function, serialize its existing
//! projection. No new money types, no arithmetic, no caching, and no second
//! opinion about what a field means. If a projection is wrong for the console it
//! is wrong for the agent too, and the fix belongs in `api` where both callers
//! get it.
//!
//! The projections therefore serialize in **snake_case**, unlike the camelCase
//! DTOs the rest of the write plane returns. That is the point rather than an
//! oversight: an operator reading a `chargebee_get_invoice` result in chat and
//! the same invoice in the console sees one shape with one set of field names,
//! and `total_in_minor_units` keeps saying what unit it is in on both.
//!
//! # Three failures, three remedies
//!
//! [`FinanceError`] keeps them apart, for the reason `BillingStatus` reports
//! four flags instead of one `connected` boolean: the fixes are in three
//! different places.
//!
//! - `501 not_in_build` — the running host was compiled without the feature. No
//!   amount of configuring will help; the operator needs a different build.
//! - `409 not_configured` — no credential in this company's secret store. The
//!   fix is the connection panel on the page they are already looking at.
//! - `502 provider_error` — the credential reached the provider and the provider
//!   said no. The fix is at the provider, and its own `code` and `message` ride
//!   along so the console can say which.
//!
//! A locally rejected argument — an empty invoice id, a missing date window —
//! never reaches the network and is a `400`, not a `502`. The `api` functions
//! already mark these with `status: 0`, so the distinction is carried rather
//! than re-derived.
//!
//! # Scope
//!
//! Reads are [`ScopedCompany`]: any signed-in member. An invoice list is not
//! more sensitive than the ledger projection Finance's own Overview page
//! already shows them.
//!
//! `POST …/invoices` is [`AdminScopedCompany`], matching `PUT …/billing/chargebee`
//! — it bills a real customer real money, and there is no route here that undoes
//! it. A member may read every invoice and raise none.
//!
//! `POST …/test` is a read wearing a POST. It is not idempotent only in that it
//! costs a provider round-trip; the verb is what keeps a link preview, a
//! prefetch or a bookmarked URL from firing it.

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::company::runtime::CompanyRuntime;
use crate::server::error::ApiError;
use crate::server::ops::scope::{AdminScopedCompany, ScopedCompany, scoped};

/// Builds the finance read routes.
///
/// Every route is registered on **every** build, including one compiled without
/// `chargebee` or `paypal`. A missing feature answers `501 not_in_build` rather
/// than `404`, so the console can say "this host has no PayPal support" instead
/// of rendering a broken page — the same reasoning that keeps
/// `crate::company::paypal` always compiled.
pub fn router() -> Router<AppState> {
    scoped(
        "/finance/chargebee/invoices",
        get(list_invoices).post(create_invoice),
    )
    .merge(scoped(
        "/finance/chargebee/invoices/{invoice_id}",
        get(get_invoice),
    ))
    .merge(scoped("/finance/chargebee/customers", get(get_customer)))
    .merge(scoped("/finance/chargebee/test", post(test_chargebee)))
    .merge(scoped("/finance/paypal/balance", get(paypal_balance)))
    .merge(scoped(
        "/finance/paypal/transactions",
        get(paypal_transactions),
    ))
    .merge(scoped("/finance/paypal/test", post(test_paypal)))
}

/// What a `POST …/test` reports when the credential worked.
///
/// There is no `ok: false`. A failed check is the provider's own failure and
/// renders as a `502` carrying the provider's code and message — collapsing that
/// into a `200 {ok: false}` would throw away the only part of the answer an
/// operator can act on.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    /// Always `true`. Present so the body is self-describing rather than `{}`.
    pub ok: bool,
    /// What was actually verified, in the operator's terms — including which
    /// site or environment answered, because "Connected ✓" against the wrong
    /// world is the confusion this whole surface exists to avoid.
    pub detail: String,
}

/// Why a finance call could not be answered.
///
/// A module-local rejection rather than an [`ApiError`] arm, because the three
/// codes below are specific to this surface and the crate error should not grow
/// a variant per console page. Renders the same `{ "error", "code" }` envelope
/// every other route does, so a client needs no special case.
#[derive(Debug)]
pub enum FinanceError {
    /// The feature is not compiled into this build.
    NotInBuild {
        /// `chargebee` or `paypal`.
        provider: &'static str,
    },
    /// No credential is stored for this company.
    NotConfigured {
        /// `chargebee` or `paypal`.
        provider: &'static str,
    },
    /// The provider was reached and refused, or the call never left the process
    /// because an argument was rejected (`status == 0`).
    Provider {
        /// `chargebee` or `paypal`.
        provider: &'static str,
        /// The provider's HTTP status, or `0` for a locally rejected argument.
        status: u16,
        /// The provider's own error token.
        code: String,
        /// The provider's own description.
        message: String,
    },
    /// Anything else — a secret store that would not answer, a company that
    /// could not be resolved. Mapped by [`ApiError`] as it always was.
    Api(ApiError),
}

impl From<ApiError> for FinanceError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl FinanceError {
    /// Classifies an error out of a provider `api` call.
    ///
    /// Compiled only alongside a provider, since nothing calls it on a build
    /// with neither — the tests reach it directly, which is why this is a `cfg`
    /// rather than an `allow(dead_code)` that would also hide a real orphan.
    ///
    /// Only the provider's own variants become a [`FinanceError::Provider`].
    /// Anything else — a store failure that surfaced mid-call — keeps whatever
    /// status [`ApiError`] already gives it, rather than being relabelled as the
    /// provider's fault.
    #[cfg(any(feature = "chargebee", feature = "paypal", test))]
    fn from_provider(provider: &'static str, error: crate::error::OpenCompanyError) -> Self {
        match error {
            crate::error::OpenCompanyError::Chargebee {
                status,
                code,
                message,
            }
            | crate::error::OpenCompanyError::Paypal {
                status,
                code,
                message,
            } => Self::Provider {
                provider,
                status,
                code,
                message,
            },
            other => Self::Api(ApiError(other)),
        }
    }
}

impl IntoResponse for FinanceError {
    fn into_response(self) -> Response {
        let (status, code, message, extra) = match self {
            Self::NotInBuild { provider } => (
                StatusCode::NOT_IMPLEMENTED,
                "not_in_build",
                format!(
                    "This host was built without {provider} support, so there is nothing to \
                     configure. A build with the `{provider}` feature is needed."
                ),
                json!({ "provider": provider }),
            ),
            Self::NotConfigured { provider } => (
                StatusCode::CONFLICT,
                "not_configured",
                format!(
                    "No {provider} credentials are stored for this company. Connect the account \
                     first."
                ),
                json!({ "provider": provider }),
            ),
            // `status: 0` means the call never reached the network — the
            // argument was rejected here. That is the caller's bad request, not
            // an upstream failure, and calling it a 502 would send an operator
            // to check the provider's status page over a blank invoice id.
            Self::Provider {
                provider,
                status: 0,
                code,
                message,
            } => (
                StatusCode::BAD_REQUEST,
                "invalid_arguments",
                message,
                json!({ "provider": provider, "providerCode": code, "providerStatus": 0 }),
            ),
            Self::Provider {
                provider,
                status,
                code,
                message,
            } => (
                StatusCode::BAD_GATEWAY,
                "provider_error",
                message,
                json!({
                    "provider": provider,
                    "providerCode": code,
                    "providerStatus": status,
                }),
            ),
            Self::Api(error) => return error.into_response(),
        };

        let mut body = json!({ "error": message, "code": code });
        if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            body.extend(extra.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        (status, Json(body)).into_response()
    }
}

/// Reads a stored secret, treating empty as absent.
///
/// The same rule as `billing::read`, and deliberately the same one: a key
/// written as `""` by a clear is *absent*, and a surface that treated it as
/// present would build a client that authenticates as nobody.
///
/// Only a provider half calls this, so it is compiled only when one is present.
#[cfg(any(feature = "chargebee", feature = "paypal"))]
async fn read(runtime: &CompanyRuntime, key: &str) -> Result<Option<String>, ApiError> {
    Ok(runtime
        .secrets()
        .get(runtime.id(), key)
        .await?
        .map(|value| value.expose().to_string())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

/// `?status=&customerEmail=&limit=` on the invoice list.
///
/// Still deserialized on a build without Chargebee — the route exists there and
/// answers `501` — so the fields are written and never read. That is the
/// `501`-not-`404` decision showing through, not an unused struct.
#[cfg_attr(not(feature = "chargebee"), allow(dead_code))]
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InvoiceQuery {
    /// Chargebee invoice status: `paid`, `posted`, `payment_due`, …
    #[serde(default)]
    status: Option<String>,
    /// Narrow to one customer, by email. Resolved to a customer id by `api`.
    #[serde(default)]
    customer_email: Option<String>,
    /// Page size, 1-100.
    #[serde(default)]
    limit: Option<i64>,
}

/// `?email=` on the customer lookup.
#[derive(Debug, Default, Deserialize)]
struct CustomerQuery {
    #[serde(default)]
    email: Option<String>,
}

/// `?since=&until=&limit=` on the transaction list.
///
/// Both instants are **required** and passed to PayPal verbatim. Defaulting them
/// here would mean a clock in this module and a second opinion about PayPal's
/// three-hour publication lag; the console owns the window because the console
/// is what has to explain it to the operator (see `finance-console.md`).
#[cfg_attr(not(feature = "paypal"), allow(dead_code))]
#[derive(Debug, Default, Deserialize)]
struct TransactionQuery {
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

// --- Chargebee ------------------------------------------------------------

/// `GET …/finance/chargebee/invoices`
async fn list_invoices(
    company: ScopedCompany,
    Query(query): Query<InvoiceQuery>,
) -> Result<Response, FinanceError> {
    chargebee::list_invoices(&company.runtime, query).await
}

/// `GET …/finance/chargebee/invoices/{invoice_id}`
async fn get_invoice(
    company: ScopedCompany,
    axum::extract::Path(params): axum::extract::Path<std::collections::HashMap<String, String>>,
) -> Result<Response, FinanceError> {
    // Both scope forms route here, and they carry different path params (`id` +
    // `invoice_id` versus `invoice_id` alone), so the invoice is read by name
    // rather than by tuple position.
    let invoice_id = params.get("invoice_id").cloned().unwrap_or_default();
    chargebee::get_invoice(&company.runtime, invoice_id).await
}

/// `GET …/finance/chargebee/customers?email=`
async fn get_customer(
    company: ScopedCompany,
    Query(query): Query<CustomerQuery>,
) -> Result<Response, FinanceError> {
    chargebee::get_customer(&company.runtime, query.email.unwrap_or_default()).await
}

/// `POST …/finance/chargebee/test`
async fn test_chargebee(company: ScopedCompany) -> Result<Json<TestResult>, FinanceError> {
    chargebee::test(&company.runtime).await
}

/// `POST …/finance/chargebee/invoices` — admin only. Bills a real customer.
async fn create_invoice(
    company: AdminScopedCompany,
    Json(body): Json<serde_json::Value>,
) -> Result<Response, FinanceError> {
    chargebee::create_invoice(&company.runtime, body).await
}

// --- PayPal ---------------------------------------------------------------

/// `GET …/finance/paypal/balance`
async fn paypal_balance(company: ScopedCompany) -> Result<Response, FinanceError> {
    paypal::balance(&company.runtime).await
}

/// `GET …/finance/paypal/transactions?since=&until=&limit=`
async fn paypal_transactions(
    company: ScopedCompany,
    Query(query): Query<TransactionQuery>,
) -> Result<Response, FinanceError> {
    paypal::transactions(&company.runtime, query).await
}

/// `POST …/finance/paypal/test`
async fn test_paypal(company: ScopedCompany) -> Result<Json<TestResult>, FinanceError> {
    paypal::test(&company.runtime).await
}

// --- The provider halves, each present only when its feature is -----------
//
// Two modules per provider with one signature set, so the handlers above are
// written once and neither reads `cfg!`. The absent half answers
// `501 not_in_build` for every route, which is what makes a build without the
// feature a page that explains itself rather than a 404.

#[cfg(not(feature = "chargebee"))]
mod chargebee {
    use super::*;

    fn absent<T>() -> Result<T, FinanceError> {
        Err(FinanceError::NotInBuild {
            provider: "chargebee",
        })
    }

    pub(super) async fn list_invoices(
        _runtime: &CompanyRuntime,
        _query: InvoiceQuery,
    ) -> Result<Response, FinanceError> {
        absent()
    }
    pub(super) async fn get_invoice(
        _runtime: &CompanyRuntime,
        _invoice_id: String,
    ) -> Result<Response, FinanceError> {
        absent()
    }
    pub(super) async fn get_customer(
        _runtime: &CompanyRuntime,
        _email: String,
    ) -> Result<Response, FinanceError> {
        absent()
    }
    pub(super) async fn create_invoice(
        _runtime: &CompanyRuntime,
        _body: serde_json::Value,
    ) -> Result<Response, FinanceError> {
        absent()
    }
    pub(super) async fn test(_runtime: &CompanyRuntime) -> Result<Json<TestResult>, FinanceError> {
        absent()
    }
}

#[cfg(feature = "chargebee")]
mod chargebee {
    use super::*;

    use crate::chargebee::client::ChargebeeClient;
    use crate::chargebee::types::{
        API_KEY_SECRET, ChargebeeConfig, GetInvoiceArgs, ListInvoicesArgs, SITE_SECRET,
        SendInvoiceArgs,
    };

    /// Builds a client from the company's stored credentials.
    ///
    /// Both halves or neither, for the reason `TenantChargebee::resolve` gives:
    /// a site with no key cannot be called, and a key pointed at the wrong site
    /// fails in a way that reads like a bad key.
    async fn client(runtime: &CompanyRuntime) -> Result<(ChargebeeClient, String), FinanceError> {
        let (Some(site), Some(api_key)) = (
            read(runtime, SITE_SECRET).await?,
            read(runtime, API_KEY_SECRET).await?,
        ) else {
            return Err(FinanceError::NotConfigured {
                provider: "chargebee",
            });
        };
        let client = ChargebeeClient::new(ChargebeeConfig {
            site: site.clone(),
            api_key,
        })
        .map_err(|e| FinanceError::from_provider("chargebee", e))?;
        Ok((client, site))
    }

    /// Serializes a projection verbatim. See the module header on snake_case.
    fn ok<T: serde::Serialize>(value: T) -> Result<Response, FinanceError> {
        Ok(Json(serde_json::to_value(value).map_err(ApiError::from)?).into_response())
    }

    // Each operation is a pair: the route half, which resolves the company's
    // credentials, and an `_on` half taking a client. The split is what lets a
    // test drive the real serialization and the real error classification
    // against a stub transport (`ChargebeeClient::with_base_url`) without a
    // secret store standing in the way — and it keeps credential resolution in
    // exactly one place rather than at the top of five functions.

    pub(super) async fn list_invoices(
        runtime: &CompanyRuntime,
        query: InvoiceQuery,
    ) -> Result<Response, FinanceError> {
        let (client, _) = client(runtime).await?;
        list_invoices_on(&client, query).await
    }

    pub(super) async fn list_invoices_on(
        client: &ChargebeeClient,
        query: InvoiceQuery,
    ) -> Result<Response, FinanceError> {
        let invoices = crate::chargebee::api::list_invoices(
            client,
            ListInvoicesArgs {
                customer_email: query.customer_email,
                status: query.status,
                limit: query.limit,
            },
        )
        .await
        .map_err(|e| FinanceError::from_provider("chargebee", e))?;
        ok(invoices)
    }

    pub(super) async fn get_invoice(
        runtime: &CompanyRuntime,
        invoice_id: String,
    ) -> Result<Response, FinanceError> {
        let (client, _) = client(runtime).await?;
        get_invoice_on(&client, invoice_id).await
    }

    pub(super) async fn get_invoice_on(
        client: &ChargebeeClient,
        invoice_id: String,
    ) -> Result<Response, FinanceError> {
        let invoice = crate::chargebee::api::get_invoice(client, GetInvoiceArgs { invoice_id })
            .await
            .map_err(|e| FinanceError::from_provider("chargebee", e))?;
        ok(invoice)
    }

    pub(super) async fn get_customer(
        runtime: &CompanyRuntime,
        email: String,
    ) -> Result<Response, FinanceError> {
        let (client, _) = client(runtime).await?;
        let customer = crate::chargebee::api::get_customer(&client, &email)
            .await
            .map_err(|e| FinanceError::from_provider("chargebee", e))?;
        // `None` is a real answer — "nobody with that email" — not a 404. The
        // console renders it as an empty result on a filter, and a 404 would
        // read as a broken route.
        ok(customer)
    }

    pub(super) async fn create_invoice(
        runtime: &CompanyRuntime,
        body: serde_json::Value,
    ) -> Result<Response, FinanceError> {
        // Deserialized here rather than in the handler signature so a malformed
        // body is a `400` in this surface's own envelope, next to every other
        // rejection, instead of axum's bare rejection text.
        let args: SendInvoiceArgs =
            serde_json::from_value(body).map_err(|e| FinanceError::Provider {
                provider: "chargebee",
                status: 0,
                code: "invalid_arguments".to_string(),
                message: e.to_string(),
            })?;
        let (client, _) = client(runtime).await?;
        let invoice = crate::chargebee::api::send_invoice(&client, args)
            .await
            .map_err(|e| FinanceError::from_provider("chargebee", e))?;
        ok(invoice)
    }

    pub(super) async fn test(runtime: &CompanyRuntime) -> Result<Json<TestResult>, FinanceError> {
        let (client, site) = client(runtime).await?;
        test_on(&client, &site).await
    }

    pub(super) async fn test_on(
        client: &ChargebeeClient,
        site: &str,
    ) -> Result<Json<TestResult>, FinanceError> {
        // The cheapest authenticated call the API has: one invoice, which
        // proves the key, the site and the network in a single round trip
        // without creating anything.
        let invoices = crate::chargebee::api::list_invoices(
            client,
            ListInvoicesArgs {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| FinanceError::from_provider("chargebee", e))?;
        Ok(Json(TestResult {
            ok: true,
            detail: format!(
                "Authenticated against the {site} Chargebee site and listed {} invoice(s).",
                invoices.len()
            ),
        }))
    }
}

#[cfg(not(feature = "paypal"))]
mod paypal {
    use super::*;

    fn absent<T>() -> Result<T, FinanceError> {
        Err(FinanceError::NotInBuild { provider: "paypal" })
    }

    pub(super) async fn balance(_runtime: &CompanyRuntime) -> Result<Response, FinanceError> {
        absent()
    }
    pub(super) async fn transactions(
        _runtime: &CompanyRuntime,
        _query: TransactionQuery,
    ) -> Result<Response, FinanceError> {
        absent()
    }
    pub(super) async fn test(_runtime: &CompanyRuntime) -> Result<Json<TestResult>, FinanceError> {
        absent()
    }
}

#[cfg(feature = "paypal")]
mod paypal {
    use super::*;

    use crate::company::paypal::{
        CLIENT_ID_SECRET, CLIENT_SECRET_SECRET, ENVIRONMENT_SECRET, PaypalEnvironment,
    };
    use crate::paypal::client::{PaypalClient, PaypalConfig};

    /// Builds a client from the company's stored credentials.
    async fn client(
        runtime: &CompanyRuntime,
    ) -> Result<(PaypalClient, PaypalEnvironment), FinanceError> {
        let (Some(client_id), Some(client_secret)) = (
            read(runtime, CLIENT_ID_SECRET).await?,
            read(runtime, CLIENT_SECRET_SECRET).await?,
        ) else {
            return Err(FinanceError::NotConfigured { provider: "paypal" });
        };
        // Absent reads as sandbox, never live — the same default `PaypalEnvironment`
        // has, for the same reason: guessing wrong must read fake money rather
        // than point at real money.
        let environment = read(runtime, ENVIRONMENT_SECRET)
            .await?
            .map(|raw| PaypalEnvironment::parse(&raw))
            .unwrap_or_default();
        let client = PaypalClient::new(PaypalConfig {
            client_id,
            client_secret,
            environment,
        })
        .map_err(|e| FinanceError::from_provider("paypal", e))?;
        Ok((client, environment))
    }

    fn ok<T: serde::Serialize>(value: T) -> Result<Response, FinanceError> {
        Ok(Json(serde_json::to_value(value).map_err(ApiError::from)?).into_response())
    }

    pub(super) async fn balance(runtime: &CompanyRuntime) -> Result<Response, FinanceError> {
        let (client, _) = client(runtime).await?;
        let balances = crate::paypal::api::get_wallet_balance(&client)
            .await
            .map_err(|e| FinanceError::from_provider("paypal", e))?;
        ok(balances)
    }

    pub(super) async fn transactions(
        runtime: &CompanyRuntime,
        query: TransactionQuery,
    ) -> Result<Response, FinanceError> {
        let (client, _) = client(runtime).await?;
        // Passed through empty when absent: `api::list_transactions` already
        // rejects an empty window with the message that names PayPal's 31-day
        // cap and 3-hour lag, and duplicating that check here would give the
        // same mistake two different wordings.
        let transactions = crate::paypal::api::list_transactions(
            &client,
            query.since.as_deref().unwrap_or_default(),
            query.until.as_deref().unwrap_or_default(),
            query.limit,
        )
        .await
        .map_err(|e| FinanceError::from_provider("paypal", e))?;
        ok(transactions)
    }

    pub(super) async fn test(runtime: &CompanyRuntime) -> Result<Json<TestResult>, FinanceError> {
        let (client, environment) = client(runtime).await?;
        // A balance read exercises the whole chain: the token exchange with the
        // client id and secret, then a bearer call. A token fetch alone would
        // pass with a credential that has no reporting scope.
        let balances = crate::paypal::api::get_wallet_balance(&client)
            .await
            .map_err(|e| FinanceError::from_provider("paypal", e))?;
        Ok(Json(TestResult {
            ok: true,
            detail: format!(
                "Authenticated against PayPal {} and read {} currency balance(s).",
                environment.as_str(),
                balances.len()
            ),
        }))
    }
}

#[cfg(test)]
#[path = "finance_tests.rs"]
mod tests;
