//! Inbound tiny.place A2A surface: JSON-RPC `tasks/send`, discovery records, and
//! the human-readable skill catalog.
//!
//! This whole module is gated behind the `tinyplace` feature — with the feature
//! off no A2A routes are mounted and the default build links no crypto. When on,
//! [`router`] serves:
//!
//! ```text
//! POST /a2a/{handle}                                    -> a2a_task
//! GET  /a2a/{handle}/skill.md                           -> skill_md
//! GET  /a2a/{handle}                                    -> agent_card
//! GET  /.well-known/agent-card.json                     -> well_known_sole
//! GET  /companies/{handle}/.well-known/agent-card.json  -> well_known_platform
//! ```
//!
//! The `tasks/send` handler enforces the tiny.place trust boundary in a fixed
//! order: resolve a **discoverable** company, verify the SIWX `Authorization`
//! (skew + single-use replay protection via the host-global
//! [`NonceCache`](crate::economy::NonceCache)) before anything reaches cognition,
//! answer a `402` challenge for a priced skill lacking a valid, unspent
//! [`X402Authorization`](crate::economy::X402Authorization), refuse outright a
//! skill id the Agent Card never advertised (a different thing from one it
//! advertises for nothing), sanitize the
//! counterparty payload (a minimal promptguard pass), and only then append an
//! [`A2aTaskReceived`](crate::ports::types::CompanyEvent::A2aTaskReceived) event
//! and run one cycle. A paying customer runs under the same approval gates as any
//! other stimulus — there is no fence bypass.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::AppState;
use crate::company::CompanyManifest;
use crate::company::runtime::CompanyRuntime;
use crate::economy::client::{JsonRpcRequest, JsonRpcResponse, now_secs, sha256_hex};
use crate::economy::signer::signer_for;
use crate::economy::x402::{self, X402Authorization};
use crate::economy::{build_agent_card, render_skill_md, siwx};
use crate::error::OpenCompanyError;
use crate::ports::now_millis;
use crate::ports::types::{AgentCard, CardPayment, CompanyEvent, LedgerEntry};
use crate::server::error::ApiError;

/// How long an inbound A2A task may hold this connection — and the worker
/// running its company cycle — open before the caller is told to retry.
///
/// `tasks/send` is fully synchronous: the HTTP response IS the cycle result,
/// so a cycle that never returns (a stuck tool call, a hung provider) would
/// otherwise pin this connection, and the task behind it, forever. A paying
/// counterparty gets no other signal that anything went wrong.
const A2A_CYCLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Builds the tiny.place A2A route fragment, merged into the main router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/a2a/{handle}", post(a2a_task).get(agent_card))
        .route("/a2a/{handle}/skill.md", get(skill_md))
        .route("/.well-known/agent-card.json", get(well_known_sole))
        .route(
            "/companies/{handle}/.well-known/agent-card.json",
            get(well_known_platform),
        )
}

// ---------------------------------------------------------------------------
// Company resolution
// ---------------------------------------------------------------------------

/// Resolves a `@handle` to a running, **discoverable** company.
///
/// Scans the registry for a company whose manifest sets `[place].discoverable`
/// and whose `[company].handle` matches, falling back to the sole registered
/// company in prosumer mode when it too is discoverable. A miss is a 404. The
/// linear scan is fine at prosumer / small-platform scale; a handle index is a
/// documented follow-up.
async fn resolve_company(state: &AppState, handle: &str) -> Result<Arc<CompanyRuntime>, ApiError> {
    for id in state.registry().list() {
        let Some(runtime) = state.registry().get(&id) else {
            continue;
        };
        if let Some(record) = runtime.store.load(&id).await?
            && record.manifest.place.discoverable
            && record.manifest.company.handle.as_deref() == Some(handle)
        {
            return Ok(runtime);
        }
    }

    // Prosumer fallback: a lone discoverable company answers any handle.
    if let Some(runtime) = state.registry().sole()
        && let Some(record) = runtime.store.load(runtime.id()).await?
        && record.manifest.place.discoverable
    {
        return Ok(runtime);
    }

    Err(ApiError(OpenCompanyError::CompanyNotFound(
        handle.to_string(),
    )))
}

/// Loads a resolved company's manifest, erroring 404 when the record is missing.
async fn load_manifest(runtime: &CompanyRuntime) -> Result<CompanyManifest, ApiError> {
    runtime
        .store
        .load(runtime.id())
        .await?
        .map(|record| record.manifest)
        .ok_or_else(|| ApiError(OpenCompanyError::CompanyNotFound(runtime.id().to_string())))
}

/// Builds a resolved company's Agent Card against the host base URL.
async fn card_for(state: &AppState, runtime: &CompanyRuntime) -> Result<AgentCard, ApiError> {
    let manifest = load_manifest(runtime).await?;
    Ok(build_agent_card(&manifest, &state.config().host_base_url()))
}

// ---------------------------------------------------------------------------
// Read-only discovery routes (no SIWX)
// ---------------------------------------------------------------------------

/// `GET /a2a/{handle}` — the company's Agent Card (a directory-record convenience).
async fn agent_card(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(handle): axum::extract::Path<String>,
) -> Result<Json<AgentCard>, ApiError> {
    let runtime = resolve_company(&state, &handle).await?;
    Ok(Json(card_for(&state, &runtime).await?))
}

/// `GET /.well-known/agent-card.json` — the sole company's card (prosumer mode).
async fn well_known_sole(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<AgentCard>, ApiError> {
    let runtime = state.registry().sole().ok_or_else(|| {
        ApiError(OpenCompanyError::CompanyNotFound(
            "single-company".to_string(),
        ))
    })?;
    // Discoverability is opt-in: an undiscoverable sole company is not published
    // through the well-known card either.
    let discoverable = runtime
        .store
        .load(runtime.id())
        .await?
        .map(|record| record.manifest.place.discoverable)
        .unwrap_or(false);
    if !discoverable {
        return Err(ApiError(OpenCompanyError::CompanyNotFound(
            "single-company".to_string(),
        )));
    }
    Ok(Json(card_for(&state, &runtime).await?))
}

/// `GET /companies/{handle}/.well-known/agent-card.json` — a named company's card.
async fn well_known_platform(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(handle): axum::extract::Path<String>,
) -> Result<Json<AgentCard>, ApiError> {
    let runtime = resolve_company(&state, &handle).await?;
    Ok(Json(card_for(&state, &runtime).await?))
}

/// `GET /a2a/{handle}/skill.md` — the human- and agent-readable skill catalog.
async fn skill_md(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(handle): axum::extract::Path<String>,
) -> Result<Response, ApiError> {
    let runtime = resolve_company(&state, &handle).await?;
    let card = card_for(&state, &runtime).await?;
    let body = render_skill_md(&card);
    Ok(([(CONTENT_TYPE, "text/markdown; charset=utf-8")], body).into_response())
}

// ---------------------------------------------------------------------------
// The inbound task route
// ---------------------------------------------------------------------------

/// `POST /a2a/{handle}` — a SIWX-authenticated JSON-RPC `tasks/send`.
async fn a2a_task(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Path(handle): axum::extract::Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 1. Resolve a discoverable company; 404 otherwise.
    let runtime = match resolve_company(&state, &handle).await {
        Ok(runtime) => runtime,
        Err(err) => return err.into_response(),
    };

    // A company with no economy wired is not reachable for commerce → 503.
    if !runtime.has_economy() {
        return ApiError(OpenCompanyError::tinyplace(
            "unreachable",
            format!("@{handle} is not reachable for A2A tasks"),
        ))
        .into_response();
    }

    // Lifecycle: a paused/archived company rejects work → 409.
    if let Err(err) = runtime.ensure_running().await {
        return ApiError(err).into_response();
    }

    // 2. SIWX — verified before anything reaches cognition. A bad or missing
    // header is a 401; nothing is logged from the request until it verifies.
    let path = format!("/a2a/{handle}");
    let body_hash = sha256_hex(&body);
    let auth_header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let from = match siwx::verify(
        auth_header,
        "POST",
        &path,
        &body_hash,
        now_secs(),
        state.nonce(),
    ) {
        Ok(agent_id) => agent_id,
        Err(err) => return unauthorized(&err),
    };

    // 3. Parse the JSON-RPC `tasks/send` envelope.
    let rpc: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(rpc) => rpc,
        Err(err) => {
            return ApiError(OpenCompanyError::InvalidRequest(format!(
                "body is not a JSON-RPC request: {err}"
            )))
            .into_response();
        }
    };
    if rpc.method != "tasks/send" {
        return ApiError(OpenCompanyError::InvalidRequest(format!(
            "unsupported method `{}`; only `tasks/send` is served",
            rpc.method
        )))
        .into_response();
    }
    let skill = rpc
        .params
        .get("skill")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Pricing comes from the company's own Agent Card.
    let card = match card_for(&state, &runtime).await {
        Ok(card) => card,
        Err(err) => return err.into_response(),
    };

    // 4. Charge for the requested skill. See `classify_skill` for why an
    // unadvertised id is not the same answer as a free one.
    match classify_skill(&card, &skill) {
        SkillCharge::Unknown => {
            return ApiError(OpenCompanyError::NotFound(format!(
                "@{handle} does not offer skill `{}`",
                sanitize_text(&skill)
            )))
            .into_response();
        }
        SkillCharge::Free => {}
        SkillCharge::Priced(pay) => match extract_payment(&rpc.params) {
            None => return payment_required(&state, &runtime, pay).await,
            Some(auth) => {
                // Checked against the claimed (not yet verified) fields,
                // before `x402::verify` spends the nonce below: on a
                // multi-company host, a correctly-signed authorization
                // submitted against the wrong company's handle — or one
                // that underpays — would otherwise burn its nonce on this
                // re-challenge and could never be resubmitted, even against
                // the right company or with the right amount. A forged
                // recipient or amount is still caught by `verify`'s
                // signature check right after, since those fields are part
                // of what it signs.
                //
                // Bind the payment to THIS company: the payer must have signed a
                // `recipient` equal to our own agent id. Without this a
                // counterparty could self-sign an authorization paying anyone
                // else and still obtain priced work.
                let our_id = match signer_for(state.home(), runtime.id()).await {
                    Ok(signer) => signer.agent_id(),
                    Err(err) => return ApiError(err).into_response(),
                };
                if auth.recipient != our_id {
                    return payment_required(&state, &runtime, pay).await;
                }
                let paid = auth.amount.trim().parse::<f64>().ok();
                let price = pay.price.trim().parse::<f64>().ok();
                let sufficient = matches!(
                    (paid, price),
                    (Some(paid), Some(price))
                        if paid.is_finite() && price.is_finite() && paid >= price
                );
                if auth.asset != pay.asset || auth.network != pay.network || !sufficient {
                    // Underpaid, unparsable/non-finite, or paid in the wrong
                    // asset/network: re-challenge for the correct terms.
                    return payment_required(&state, &runtime, pay).await;
                }
                let paid = paid.expect("sufficient implies paid is Some and finite");
                if let Err(err) = x402::verify(&auth, state.x402_nonce(), now_secs()) {
                    return ApiError(err).into_response();
                }
                // Journal the inbound receipt before doing the work.
                let entry = LedgerEntry {
                    at_millis: now_millis(),
                    kind: "x402.in".to_string(),
                    amount_usd: paid,
                    memo: format!("a2a `{skill}` from {from}"),
                };
                if let Err(err) = runtime.store.append_ledger(runtime.id(), entry).await {
                    return ApiError(err).into_response();
                }
            }
        },
    }

    // 5. Promptguard: sanitize the counterparty payload before it becomes an
    // event. Deliberately minimal — a control-character strip seam, not a full
    // model-based guard.
    let task = sanitize_value(rpc.params.clone());

    // 6. Append the event and run one cycle (run_cycle persists the event),
    // bounded so a stuck cycle cannot hold this connection open forever.
    let report = match tokio::time::timeout(
        A2A_CYCLE_TIMEOUT,
        runtime.run_cycle(vec![CompanyEvent::A2aTaskReceived {
            from: from.clone(),
            task,
        }]),
    )
    .await
    {
        Ok(Ok(report)) => report,
        Ok(Err(err)) => return ApiError(err).into_response(),
        Err(_) => return cycle_timeout(),
    };

    let result = json!({
        "cycleId": report.cycle_id,
        "responses": report.responses,
    });
    (StatusCode::OK, Json(JsonRpcResponse::ok(rpc.id, result))).into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Renders a SIWX failure as a `401` in the api.md error envelope.
fn unauthorized(err: &OpenCompanyError) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": err.to_string(), "code": err.code() })),
    )
        .into_response()
}

/// Renders a cycle that outran [`A2A_CYCLE_TIMEOUT`] as a `504`.
fn cycle_timeout() -> Response {
    (
        StatusCode::GATEWAY_TIMEOUT,
        Json(json!({
            "error": "the company did not finish this task in time",
            "code": "timeout",
        })),
    )
        .into_response()
}

/// What a company's Agent Card says about a requested skill id.
enum SkillCharge<'a> {
    /// Advertised above zero: the task needs a valid, unspent authorization.
    Priced(&'a CardPayment),
    /// Advertised at `0.00`, or at a price this build cannot parse. Served.
    Free,
    /// Not advertised at all, by a company that charges for its work. Refused.
    Unknown,
}

/// Classifies `skill` against the card's advertised prices.
///
/// The three answers are genuinely different and collapsing any two of them
/// gives work away. `payment_requirements` is a one-to-one projection of the
/// manifest's `[place].skills`, so an id missing from it is an id the company
/// never offered — not an id it offers for nothing. Reading "no price found" as
/// "free" let any unadvertised string buy the whole `tasks/send` path on a
/// company that prices every skill it does advertise.
///
/// An unparsable price stays free deliberately: a company that has misdeclared
/// its own price has not thereby declared a task unavailable, and the manifest
/// validator already names the mistake.
///
/// A card advertising nothing above zero charges for nothing, so every id on it
/// is free — including one it does not list. Refusing there would take A2A away
/// from companies that never opted into pricing.
///
/// Manifest validation rejects a duplicate skill id outright, so
/// `payment_requirements` should never carry two entries for the same
/// `skill`. If one somehow reaches this card anyway (an older store predating
/// that check), a priced entry always outranks a free or unparsable one for
/// the same id — the reverse would let a duplicate free entry waive a price
/// the company does charge for that skill.
fn classify_skill<'a>(card: &'a AgentCard, skill: &str) -> SkillCharge<'a> {
    let matching = || {
        card.payment_requirements
            .iter()
            .filter(|pay| pay.skill_id == skill)
    };

    match matching().find(|pay| priced_above_zero(pay)) {
        Some(pay) => SkillCharge::Priced(pay),
        None if matching().next().is_some() => SkillCharge::Free,
        None if card.payment_requirements.iter().any(priced_above_zero) => SkillCharge::Unknown,
        None => SkillCharge::Free,
    }
}

/// Whether this requirement names a price the company actually charges.
fn priced_above_zero(pay: &CardPayment) -> bool {
    pay.price
        .trim()
        .parse::<f64>()
        .map(|price| price > 0.0)
        .unwrap_or(false)
}

/// Builds the `402` challenge naming the price and the company's own address.
async fn payment_required(
    state: &AppState,
    runtime: &CompanyRuntime,
    pay: &CardPayment,
) -> Response {
    let recipient = match signer_for(state.home(), runtime.id()).await {
        Ok(signer) => signer.agent_id(),
        Err(err) => return ApiError(err).into_response(),
    };
    let challenge = json!({
        "amount": pay.price,
        "recipient": recipient,
        "asset": pay.asset,
        "network": pay.network,
    });
    (StatusCode::PAYMENT_REQUIRED, Json(challenge)).into_response()
}

/// Extracts an [`X402Authorization`] from a `payment` param, if present and valid.
fn extract_payment(params: &Value) -> Option<X402Authorization> {
    let payment = params.get("payment")?;
    serde_json::from_value(payment.clone()).ok()
}

/// Strips control characters (keeping ordinary whitespace) from counterparty
/// text so an injected escape/marker never reaches the brain verbatim.
fn sanitize_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .collect()
}

/// Recursively sanitizes every string in a JSON value.
fn sanitize_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(sanitize_text(&s)),
        Value::Array(items) => Value::Array(items.into_iter().map(sanitize_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, sanitize_value(v)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
#[path = "a2a_test_support.rs"]
mod test_support;
#[cfg(test)]
#[path = "a2a_pricing_tests.rs"]
mod tests_pricing;
#[cfg(test)]
#[path = "a2a_routing_tests.rs"]
mod tests_routing;
#[cfg(test)]
#[path = "a2a_x402_tests.rs"]
mod tests_x402;
