//! The autonomy-tier write plane: `GET`/`PUT`/`DELETE {scope}/policy`
//! (issue #562).
//!
//! An operator drowning in approval cards had no way to stop it. The tier is
//! read from `[policy].mode` in the company manifest, and nothing in the console
//! read or wrote it — so the only ways to change it were editing a
//! version-controlled file and redeploying, or, on a hosted tenant where the
//! manifest is a read-only boot snapshot baked into the image, nothing at all.
//!
//! This is the company-scoped twin of the per-teammate budget surface in
//! [`team`](crate::server::ops::team), and it keeps the same three rules for the
//! same reasons:
//!
//! - **Admin-only, and attributed.** Loosening the approval gate is a privilege
//!   boundary at least as sharp as raising a spend cap, so both writes go through
//!   [`require_admin`](crate::server::users::admin::require_admin) and stamp who
//!   did it and when. The console renders that attribution: a tier that can be
//!   loosened anonymously is not much of a gate.
//! - **Absent is not empty.** An omitted key leaves that field on the manifest's
//!   value; `"alwaysApprove": []` is an operator deliberately clearing the
//!   always-ask list. Those are different stored states and different
//!   behaviours, so the body uses a double option and an empty `{}` is a `422`
//!   rather than a silent no-op.
//! - **Reset is its own verb.** `DELETE` drops the override so the manifest's
//!   `[policy]` applies again, which no `PUT` body can express — writing the
//!   manifest's *current* values would pin them, and the manifest can change
//!   under a rebuild.
//!
//! ## Why this writes an overlay and not the manifest
//!
//! A rebuild re-persists `record.manifest` **from the seed**, merging only
//! `[workflows].enabled`; every other field is seed-authoritative, *"and for
//! `[tools]` / `[policy]` that is a security property"* (`runtime::builder`). A
//! manifest write would be wiped by the next rebuild **and** would contradict
//! that invariant. So this route writes
//! [`PolicyOverride`](crate::ports::types::PolicyOverride), which
//! [`CompanyRecord::effective_policy`](crate::ports::types::CompanyRecord::effective_policy)
//! resolves *ahead* of the manifest at read time.
//!
//! ## When a change takes effect, stated precisely
//!
//! On the company's **next turn**, not on the turn already running.
//! `ApprovalPolicy` is built once per roster build, and
//! `HarnessPool::ensure` rebuilds the roster when the policy fingerprint moves
//! (issue #562's `effective_policy_fingerprint`). So the write lands, the next `ensure`
//! rebuilds, and the following turn runs on the new tier — an in-flight turn
//! finishes under the old one. Since "stop the flood **now**" is the motivating
//! complaint, the response says so rather than leaving an operator to discover
//! it: see [`PolicyDto::takes_effect`].

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::company::{POLICY_MODES, Policy};
use crate::error::OpenCompanyError;
use crate::policy::DEFAULT_TTL_MILLIS;
use crate::ports::now_millis;
use crate::ports::store::company_write_lock;
use crate::ports::types::{Actor, ActorKind, CompanyRecord, PolicyOverride};
use crate::server::error::ApiError;
use crate::server::ops::team::double_option;
use crate::server::ops::{ScopedCompany, scoped};
use crate::server::users::admin::require_admin;

/// What the console tells an operator about when a tier change bites.
///
/// A constant rather than prose invented at the call site, so the two write
/// routes and the read route cannot describe the timing differently.
///
/// `pub(crate)` for the same reason [`PolicyDto`] is: the GraphQL suite asserts
/// the field against **this** string rather than a copy of it, so a test cannot
/// keep passing while the two surfaces quote different timings at an operator.
pub(crate) const TAKES_EFFECT: &str =
    "on the next turn — a turn already running finishes under the previous tier";

/// The largest `alwaysApprove` list `PUT {scope}/policy` accepts.
///
/// Well past any real always-ask list — this build declares a few dozen tools
/// (`known_tools` above) — so it bounds the write without narrowing what an
/// operator can actually express. The list is stored verbatim and re-served on
/// every `GET`, so an unbounded one is a standing cost on every read, not just
/// the write that set it.
const MAX_ALWAYS_APPROVE_ENTRIES: usize = 200;

/// The largest single `alwaysApprove` entry `PUT {scope}/policy` accepts, in
/// bytes. A gateable tool name is a short identifier (`payment.send`); this
/// leaves ample room for one still unknown to this build while refusing an
/// entry that could not be a tool name by any stretch.
const MAX_ALWAYS_APPROVE_ENTRY_LEN: usize = 200;

/// Builds the policy route fragment.
pub fn router() -> Router<AppState> {
    scoped(
        "/policy",
        get(read_policy).put(set_policy).delete(clear_policy),
    )
}

/// One tier as the console renders it: the value, and what it means in the
/// operator's own words rather than in tier names.
///
/// The descriptions live here rather than in the frontend because they describe
/// *this* runtime's behaviour — `auto`'s line in particular is the contract
/// `Consequence::parks_under_auto` implements — and a copy in TypeScript would
/// drift from the gate it claims to describe.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TierDto {
    /// The `[policy].mode` word.
    pub(crate) value: &'static str,
    /// The operator-facing label.
    pub(crate) label: &'static str,
    /// What choosing it means, in consequences rather than in tier vocabulary.
    pub(crate) description: &'static str,
}

/// The operator-facing text for every tier this console knows how to describe,
/// in increasing order of autonomy.
///
/// **Not** what gets offered — [`selectable_tiers`] filters this by
/// [`POLICY_MODES`], so the console offers exactly what the runtime accepts.
/// The two are separate because they move independently: `auto` is issue #560,
/// landing in its own PR, and hard-coding it here would either offer a tier the
/// gate would silently downgrade to `supervised` (if #562 landed first) or need
/// a cross-PR edit to appear (if #560 did).
///
/// With the filter, neither happens: an entry ahead of the runtime is inert, and
/// the day `auto` joins `POLICY_MODES` it appears in the console with no further
/// change. An entry *behind* the runtime is the real error, and
/// `every_runtime_tier_has_console_text` fails on it.
const TIER_TEXT: &[TierDto] = &[
    TierDto {
        value: "readonly",
        label: "Read-only",
        // **B-023.** "spend nothing" was read as a spend control, and it is not
        // one. What `readonly` gates is `Reach` (`policy::consequence`): it
        // denies everything that is not `Reach::Nothing`
        // (`denied_under_readonly`), which is three distinct things and the
        // sentence has to carry all three: `Consequence` (state changes, a
        // counterparty reached, arbitrary code or address), `Money` (a billed
        // tool call) and `ExternalRead` (a third party's own data read with the
        // company's connected credential — no change, no bill, and still
        // denied). So billed *tool calls* really are denied — but inference is
        // not a tool call and reaches neither gate, so the agents still think
        // and the company is still billed for it.
        //
        // The old sentence therefore promised a bill of zero and delivered one,
        // and said nothing about `ExternalRead` — which is precisely what an
        // operator choosing this tier wants to know is blocked. Both halves are
        // named here rather than in the console, for the reason this table
        // exists: the prose lives beside the gate it describes so it cannot
        // drift from it.
        //
        // **The first sentence has to stand alone.** The pill renders
        // `leadSentence(description)` and nothing else (`autonomy-pill.tsx`),
        // on the argument that half a claim about what the agents may do is
        // worse than none. So the lead states only the authority — which is
        // wholly true — and makes no claim about spending at all, where the old
        // copy made a false one. The billing caveat lands in the second
        // sentence, which the menu and the settings page both render in full.
        description: "The agents can look at things but change nothing, contact nobody, and use \
                      no connected account. Billed tool calls are refused too — but the agents \
                      still think, and the company is billed for that.",
    },
    TierDto {
        value: "supervised",
        label: "Supervised",
        description: "Conservative execution restrictions. Approval prompts are explicit through \
                      request_approval while policy HITL is disabled.",
    },
    TierDto {
        value: "auto",
        label: "Auto",
        description: "Balanced execution autonomy. Approval prompts are explicit through \
                      request_approval while policy HITL is disabled.",
    },
    TierDto {
        value: "full",
        label: "Full",
        description: "Broadest execution autonomy. Approval prompts are explicit through \
                      request_approval while policy HITL is disabled.",
    },
];

/// The tiers this company can actually be set to: [`TIER_TEXT`] narrowed to the
/// modes [`POLICY_MODES`] accepts, in `POLICY_MODES` order.
///
/// Driving the order from `POLICY_MODES` rather than from `TIER_TEXT` keeps one
/// list authoritative about what exists; `TIER_TEXT` is only authoritative about
/// how it reads.
fn selectable_tiers() -> Vec<&'static TierDto> {
    tiers_for(POLICY_MODES)
}

/// [`selectable_tiers`] over an explicit mode list.
///
/// Split out so the filter can be tested against a list that is not
/// `POLICY_MODES`. That is not ceremony: `TIER_TEXT` and `POLICY_MODES` now hold
/// the same four tiers (issue #560 landed `auto`), so a test driven off
/// `POLICY_MODES` alone can no longer tell a filtered list from an unfiltered
/// one — deleting the filter would leave every such assertion passing. A test
/// that has stopped discriminating looks exactly like coverage, which is worse
/// than no test at all.
fn tiers_for(modes: &[&str]) -> Vec<&'static TierDto> {
    modes
        .iter()
        .filter_map(|mode| TIER_TEXT.iter().find(|tier| tier.value == *mode))
        .collect()
}

/// The policy surface as the console reads it.
///
/// `pub(crate)` so the GraphQL layer can answer from **this** derivation rather
/// than recomputing the tier from the record (issue #1070). Two surfaces
/// resolving one value independently is how they come to disagree — a company
/// with a console override would report the overridden tier on one and the
/// manifest's on the other, and a caller has no way to tell which it got.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PolicyDto {
    /// The tier actually in force.
    pub(crate) mode: String,
    /// The always-ask list actually in force. The operator's real lever: it
    /// wins over every tier, `full` included.
    pub(crate) always_approve: Vec<String>,
    /// The spend threshold actually in force. `None` means every spend parks.
    pub(crate) auto_approve_under_usd: Option<f64>,
    /// The deadline actually in force, including the runtime default.
    pub(crate) approval_ttl_hours: u64,
    /// The manifest's tier, so the console can show what "reset" would restore
    /// rather than describing it abstractly.
    pub(crate) manifest_mode: String,
    /// The manifest's always-ask list, for the same reason.
    pub(crate) manifest_always_approve: Vec<String>,
    /// The manifest's spend threshold, before any console override.
    pub(crate) manifest_auto_approve_under_usd: Option<f64>,
    /// The manifest's deadline, if it explicitly names one.
    pub(crate) manifest_approval_ttl_hours: Option<u64>,
    /// Whether an operator override is in force. Distinct from comparing the
    /// values: an override that happens to match the manifest is still an
    /// override, and still what `DELETE` would remove.
    pub(crate) overridden: bool,
    /// Who set the override, if one is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) set_by: Option<String>,
    /// When it was set (epoch millis), if one is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) set_at_millis: Option<u64>,
    /// The selectable tiers with their operator-facing consequences.
    pub(crate) tiers: Vec<&'static TierDto>,
    /// When a change bites. Stated because "stop the flood now" is what an
    /// operator comes here to do, and this is not quite that.
    pub(crate) takes_effect: &'static str,
    /// Whether the live gate currently turns policy — the tier,
    /// `always_approve`, the spend cap — into approval requests, as opposed to
    /// allowing everything the hard denials (`readonly`, the emergency stop)
    /// do not already refuse.
    ///
    /// Read from [`ManifestApprovalGate::policy_hitl_enabled`]
    /// (`crate::policy::gate`) rather than assumed, so the console's claim
    /// about its own always-ask list tracks the gate it describes instead of
    /// a copy of today's build state — see this module's own doc comment for
    /// why a copy in TypeScript drifts.
    pub(crate) policy_hitl_enabled: bool,
    /// Every tool name this build's approval gate can match, for the console's
    /// "is this a real tool?" note (issue #1423).
    ///
    /// The complete registry, not the granted-and-wired subset
    /// (`/workflows/tool-slugs`): the gate matches a tool call by name, so an
    /// entry naming a wired agent tool (`hosting_launch_site`,
    /// `publish_artifact`) is a legitimate fence and must not be called a
    /// mistake just because it cannot be a workflow node. Sourced from
    /// `consequence::declared_tools`, which
    /// `every_registered_tool_is_declared` proves covers every tool a live
    /// agent can call.
    pub(crate) known_tools: Vec<String>,
}

impl PolicyDto {
    pub(crate) fn build(record: &CompanyRecord, policy_hitl_enabled: bool) -> Self {
        let effective: Policy = record.effective_policy();
        let manifest = &record.manifest.policy;
        Self {
            mode: effective.mode,
            always_approve: effective.always_approve,
            auto_approve_under_usd: effective.auto_approve_under_usd,
            approval_ttl_hours: effective
                .approval_ttl_hours
                .unwrap_or(DEFAULT_TTL_MILLIS / (60 * 60 * 1000)),
            manifest_mode: manifest.mode.clone(),
            manifest_always_approve: manifest.always_approve.clone(),
            manifest_auto_approve_under_usd: manifest.auto_approve_under_usd,
            manifest_approval_ttl_hours: manifest.approval_ttl_hours,
            overridden: record.overlay_policy.is_some(),
            set_by: record.overlay_policy.as_ref().map(|o| o.set_by.id.clone()),
            set_at_millis: record.overlay_policy.as_ref().map(|o| o.at_millis),
            tiers: selectable_tiers(),
            takes_effect: TAKES_EFFECT,
            policy_hitl_enabled,
            known_tools: {
                let mut tools: Vec<String> = crate::policy::consequence::declared_tools()
                    .map(str::to_owned)
                    .collect();
                // Deterministic over the wire; the console compares membership,
                // not order, but a stable list is easier to read and to test.
                tools.sort();
                tools
            },
        }
    }
}

/// The set-policy body.
///
/// Both fields are **double options** so "leave this alone" and "set it to this"
/// stay apart on the wire:
///
/// | body | parses as | means |
/// |---|---|---|
/// | `{"mode": "auto"}` | `Some(Some("auto"))` | run `auto`; leave the list alone |
/// | `{"alwaysApprove": []}` | `Some(Some([]))` | clear the always-ask list |
/// | `{"mode": null}` | `Some(None)` | stop overriding the tier |
/// | `{}` | *rejected* | — |
///
/// `#[serde(default)]` on each field makes an omitted key legal individually —
/// otherwise setting the tier would force the caller to restate the whole list —
/// but a body that sets *neither* is refused by [`set_policy`] rather than
/// stored, because a row with both fields `None` says nothing while still
/// rendering in the console as "overridden".
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetPolicy {
    #[serde(default, deserialize_with = "double_option")]
    mode: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    always_approve: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "double_option")]
    auto_approve_under_usd: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    approval_ttl_hours: Option<Option<u64>>,
}

/// `GET {scope}/policy` — the tier in force, what the manifest would restore,
/// and the selectable tiers with their consequences.
async fn read_policy(company: ScopedCompany) -> Result<Json<PolicyDto>, crate::server::Rejection> {
    let record = load_record(&company).await?;
    Ok(Json(PolicyDto::build(
        &record,
        company.runtime.approval_gate.policy_hitl_enabled(),
    )))
}

/// `PUT {scope}/policy` — set the tier and/or the always-ask list. Admin-only,
/// attributed, in force on the next turn.
async fn set_policy(
    company: ScopedCompany,
    State(state): State<AppState>,
    headers: HeaderMap,
    crate::server::graphql::auth::MaybePeer(peer): crate::server::graphql::auth::MaybePeer,
    Json(body): Json<SetPolicy>,
) -> Result<Json<PolicyDto>, crate::server::Rejection> {
    let admin = require_admin(&headers, &state, &company.runtime, peer).await?;

    if body.mode.is_none()
        && body.always_approve.is_none()
        && body.auto_approve_under_usd.is_none()
        && body.approval_ttl_hours.is_none()
    {
        return Err(refusal(
            "Nothing to set. Send a policy field — or `DELETE` \
             this endpoint to go back to the manifest's policy.",
        )
        .into());
    }

    // Validate against the same list `company.toml` is validated against, so a
    // tier the console accepts is one the manifest would have accepted too. An
    // A stored unknown mode falls back to the manifest for version-skew safety,
    // but a fresh write must still be refused: accepting it would leave the
    // console showing a tier the gate was not running.
    if let Some(Some(mode)) = &body.mode
        && !POLICY_MODES.contains(&mode.as_str())
    {
        return Err(refusal(&format!(
            "`mode` must be one of {} — you sent `{mode}`.",
            POLICY_MODES.join(", ")
        ))
        .into());
    }
    if let Some(Some(cap)) = body.auto_approve_under_usd
        && (!cap.is_finite() || cap < 0.0)
    {
        return Err(refusal("`autoApproveUnderUsd` must be a non-negative number.").into());
    }
    if let Some(Some(hours)) = body.approval_ttl_hours
        && !(1..=8_760).contains(&hours)
    {
        return Err(refusal("`approvalTtlHours` must be between 1 hour and 1 year.").into());
    }
    if let Some(Some(list)) = &body.always_approve {
        if list.len() > MAX_ALWAYS_APPROVE_ENTRIES {
            return Err(refusal(&format!(
                "`alwaysApprove` may hold at most {MAX_ALWAYS_APPROVE_ENTRIES} entries — you \
                 sent {}.",
                list.len()
            ))
            .into());
        }
        if let Some((index, entry)) = list
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.len() > MAX_ALWAYS_APPROVE_ENTRY_LEN)
        {
            return Err(refusal(&format!(
                "`alwaysApprove[{index}]` is {} characters, over the \
                 {MAX_ALWAYS_APPROVE_ENTRY_LEN} limit.",
                entry.len()
            ))
            .into());
        }
    }

    let write_lock = company_write_lock(company.id());
    let _lock = write_lock.lock().await;

    let mut record = load_record(&company).await?;

    // Merge onto whatever is already stored, so setting the tier does not
    // silently discard an always-ask list a previous call set (and vice versa).
    // The outer `Option` is "did the caller mention this field"; the inner one is
    // the value or an explicit "stop overriding it".
    let held = record.overlay_policy.clone();
    let mode = match body.mode {
        Some(value) => value,
        None => held.as_ref().and_then(|o| o.mode.clone()),
    };
    let always_approve = match body.always_approve {
        Some(value) => value,
        None => held.as_ref().and_then(|o| o.always_approve.clone()),
    };
    let auto_approve_under_usd = match body.auto_approve_under_usd {
        Some(value) => Some(value),
        None => held.as_ref().and_then(|o| o.auto_approve_under_usd),
    };
    let approval_ttl_hours = match body.approval_ttl_hours {
        Some(Some(value)) => Some(value),
        // An explicit `null` clears this one override — restoring the
        // manifest/default deadline — while an omitted field keeps the held
        // value. `mode: null` and `alwaysApprove` already work this way.
        Some(None) => None,
        None => held.as_ref().and_then(|o| o.approval_ttl_hours),
    };

    let entry = PolicyOverride {
        mode,
        always_approve,
        auto_approve_under_usd,
        approval_ttl_hours,
        set_by: Actor {
            kind: ActorKind::User,
            id: admin.user_id,
        },
        at_millis: now_millis(),
    };
    // A merge that cleared both fields is a reset, and storing an empty row
    // would leave the console showing "overridden" over the manifest's own
    // values. `DELETE`'s outcome is the honest one.
    record.overlay_policy = (!entry.is_empty()).then_some(entry);

    save(&company, &record).await?;
    // The deadline is immediate, not next-turn: a parked card stays the same
    // request, but its deadline is re-evaluated against the current TTL each
    // time it is displayed, swept or resolved, so waiting for the next cycle
    // would let approvals parked under the old (longer) TTL outlive the deadline
    // the console just reported. The tier/cap/always-ask half is the safe-turn-
    // boundary one and is applied by `run_locked` at the start of the next
    // cycle — an in-flight turn must finish under the policy snapshot it started
    // with (issue #1455). A test-injected gate is exempt.
    if !company.runtime.gate_injected {
        company
            .runtime
            .approval_gate
            .apply_effective_ttl(&record.effective_policy());
    }
    Ok(Json(PolicyDto::build(
        &record,
        company.runtime.approval_gate.policy_hitl_enabled(),
    )))
}

/// `DELETE {scope}/policy` — drop the override so the manifest's `[policy]`
/// applies again.
///
/// Not expressible as a `PUT`: writing the manifest's current values would pin
/// them, and a later `company.toml` edit would then be silently overridden by
/// the values it replaced. Deleting when nothing is stored is a no-op rather
/// than a `404` — the caller's intent ("this company should follow the
/// manifest") is already satisfied.
async fn clear_policy(
    company: ScopedCompany,
    State(state): State<AppState>,
    headers: HeaderMap,
    crate::server::graphql::auth::MaybePeer(peer): crate::server::graphql::auth::MaybePeer,
) -> Result<Json<PolicyDto>, crate::server::Rejection> {
    require_admin(&headers, &state, &company.runtime, peer).await?;

    let write_lock = company_write_lock(company.id());
    let _lock = write_lock.lock().await;

    let mut record = load_record(&company).await?;
    record.overlay_policy = None;
    save(&company, &record).await?;
    // Same split as `set_policy` (issue #1455): the manifest/default deadline
    // applies immediately — a parked card's deadline is re-evaluated against the
    // current TTL — while the manifest tier/cap/always-ask returns at the start
    // of the next cycle via `run_locked`.
    if !company.runtime.gate_injected {
        company
            .runtime
            .approval_gate
            .apply_effective_ttl(&record.effective_policy());
    }
    Ok(Json(PolicyDto::build(
        &record,
        company.runtime.approval_gate.policy_hitl_enabled(),
    )))
}

fn refusal(message: &str) -> Response {
    (StatusCode::UNPROCESSABLE_ENTITY, message.to_string()).into_response()
}

async fn load_record(company: &ScopedCompany) -> Result<CompanyRecord, crate::server::Rejection> {
    company
        .runtime
        .store()
        .load(company.id())
        .await?
        .ok_or_else(|| {
            ApiError(OpenCompanyError::CompanyNotFound(company.id().to_string()))
                .into_response()
                .into()
        })
}

async fn save(
    company: &ScopedCompany,
    record: &CompanyRecord,
) -> Result<(), crate::server::Rejection> {
    company
        .runtime
        .store()
        .save(record)
        .await
        .map_err(|e| ApiError(e).into_response().into())
}

#[cfg(test)]
#[path = "policy_bounds_tests.rs"]
mod tests_bounds;
#[cfg(test)]
#[path = "policy_core_tests.rs"]
mod tests_core;
