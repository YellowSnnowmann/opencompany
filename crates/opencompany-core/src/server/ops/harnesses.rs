//! `GET {scope}/harnesses` (issue #1245's harness-picker follow-up): the
//! company's declared `[[harness]]` set, read-only.
//!
//! Before this route existed, nothing outside the process could see what
//! harnesses a company had declared — `AgentDetailDto`/`EditAgentInput` could
//! carry a `harness` binding (a manifest agent already could; an overlay
//! teammate gained the same field alongside this route), but the console had
//! no way to know *what it could bind to*, or which one is the default a
//! blank binding falls back to. Settings' Harnesses page and the per-agent
//! Harness picker both read this one list, so the two cannot disagree about
//! what the company has declared.
//!
//! Read-only, and stays that way: a harness is declared in the
//! version-controlled `company.toml`, the same blueprint
//! [`team_agent`](super::team_agent)'s own module docs describe the console as
//! never rewriting. Defining a *new* harness from the console is a
//! meaningfully bigger, separate piece of work — it would need an overlay
//! storage mechanism harnesses do not have today, the way
//! [`OverlayAgent`](crate::ports::types::OverlayAgent) gives a teammate one.

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde::Serialize;

use crate::AppState;
use crate::company::{ACP_AGENTS, Harness};
use crate::error::OpenCompanyError;
use crate::server::error::ApiError;
use crate::server::ops::{ScopedCompany, scoped};

/// Builds the harnesses read route fragment.
pub fn router() -> Router<AppState> {
    scoped("/harnesses", get(list_harnesses))
}

/// One declared `[[harness]]`, as the console renders it in a picker.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HarnessDto {
    id: String,
    /// One of `HARNESS_KINDS` — `built_in` (managed) or `acp` (external).
    kind: String,
    /// Whether an agent naming no harness runs here. Exactly one entry in the
    /// list sets this.
    default: bool,
    /// `acp` harnesses only: which CLI (`claude`/`codex`), when
    /// `transport = "local"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    /// `acp` harnesses only: `local` (spawned on this machine) or `runner` (a
    /// registered remote).
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<String>,
    /// Whether this entry is **declared** in `company.toml` or merely
    /// **detected** — a coding CLI this build can drive that is bindable
    /// without any `[[harness]]` naming it
    /// ([`Harness::implicit_local`](crate::company::Harness::implicit_local)).
    ///
    /// The distinction is the whole contract with the console. A declared
    /// harness is a property of the *company* and is the same wherever the
    /// manifest is opened; a detected one is a property of the *machine*, and
    /// whether it can actually run is answered by that machine's own
    /// `acp::discovery` survey — which this host cannot see and deliberately
    /// does not guess at. The console joins the two on `id`.
    detected: bool,
    /// Whether **this host** can spawn this harness's transport.
    ///
    /// The console cannot work this out for itself, and the attempts to infer
    /// it were wrong in a way nobody sees. A desktop connected to a *remote*
    /// company still runs its own `acp::discovery` survey, so a declared
    /// `transport = "local"` harness was probed against the operator's laptop
    /// — reporting `Ready`, and offering to install an adapter — while turns
    /// for that company spawn on the remote host, where the CLI may be absent
    /// and no `AcpAgentFactory` may exist at all.
    ///
    /// So the host answers instead of the client guessing:
    ///
    /// - `built_in` — always true; it is this host's own engine.
    /// - `acp` + `local` — true only where this host wired an
    ///   `AcpAgentFactory`, which is the embedded desktop and nothing else.
    /// - `acp` + `runner` — false; it runs on a registered remote machine.
    ///
    /// False means "do not probe this against the machine you are on", not
    /// "broken": a hosted company's declared local harness is perfectly valid
    /// on the host that serves it.
    runs_here: bool,
}

/// `GET {scope}/harnesses` — every harness an agent here can be bound to.
///
/// Declared `[[harness]]` entries first, then every coding CLI this build
/// knows how to drive that the manifest does not already declare. A declared
/// entry always wins on id collision, so a company that pinned a model on its
/// own `claude` harness is never shadowed by the bare detected one.
///
/// Carries **no readiness**: whether a CLI is installed and signed in is a
/// fact about the operator's machine, not about this company, and this route
/// answers for any client on any machine. See [`HarnessDto::detected`].
async fn list_harnesses(
    State(state): State<AppState>,
    company: ScopedCompany,
) -> Result<Json<Vec<HarnessDto>>, ApiError> {
    let record = company
        .runtime
        .store()
        .load(company.id())
        .await?
        .ok_or_else(|| OpenCompanyError::CompanyNotFound(company.id().to_string()))?;

    let declared = record.manifest.effective_harnesses();

    // A coding CLI is only bindable where **this host** can spawn one, and
    // that is a property of the host rather than of the company: only the
    // embedded desktop wires an `AcpAgentFactory`, and `opencompany serve`
    // builds `AppState` without one. Advertising `claude` and `codex` from a
    // hosted host let an admin bind a teammate to a CLI on somebody else's
    // machine — accepted by `PATCH`, then dead on the next rebuild, because
    // the server has nothing to launch. `lanes.rs` already marks such a lane
    // unavailable; this stops the binding being offered in the first place.
    //
    // Declared entries are untouched: a manifest that names a `[[harness]]`
    // is stating what that deployment has, and this route does not get to
    // second-guess it.
    //
    // Issue #1814: `can_run_local_acp()`, not `acp_agents().is_some()`. The
    // desktop wires a factory on every build, so the bare accessor said yes on
    // a desktop compiled without `acp` — the one host where this route's whole
    // purpose applied and it silently did nothing.
    let can_run_local = state.can_run_local_acp();
    let runs_here = |harness: &Harness| match harness.kind.as_str() {
        "built_in" => true,
        "acp" => {
            can_run_local && harness.acp.as_ref().map(|a| a.transport.as_str()) == Some("local")
        }
        _ => false,
    };
    let detected = ACP_AGENTS
        .iter()
        .filter(|_| can_run_local)
        .filter(|id| !declared.iter().any(|h| h.id == **id))
        .map(|id| Harness::implicit_local(id));

    Ok(Json(
        declared
            .iter()
            .cloned()
            .map(|harness| {
                let here = runs_here(&harness);
                dto(harness, false, here)
            })
            .chain(detected.map(|harness| {
                let here = runs_here(&harness);
                dto(harness, true, here)
            }))
            .collect(),
    ))
}

fn dto(harness: Harness, detected: bool, runs_here: bool) -> HarnessDto {
    HarnessDto {
        id: harness.id,
        kind: harness.kind,
        default: harness.default,
        agent: harness.acp.as_ref().and_then(|acp| acp.agent.clone()),
        transport: harness.acp.map(|acp| acp.transport),
        detected,
        runs_here,
    }
}

#[cfg(test)]
#[path = "harnesses_tests.rs"]
mod tests;
