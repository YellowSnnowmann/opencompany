//! Stub capabilities for a **dry run** (issue #542).
//!
//! A dry run walks the *real* graph — real compile, real branch selection, real
//! item flow — but over these stubs in place of the effectful capabilities, so
//! it proves a workflow's routing and output shape without any real effect:
//! zero agent inference, zero tool/http execution.
//!
//! # Fail-closed by construction, not by remembering
//!
//! Every effectful slot is stubbed, so there is no path by which a *future*
//! node kind could reach a real effect in a dry run: the engine only ever calls
//! what is on the bundle, and on a dry bundle every effectful entry is one of
//! these. A stub that forgot to exist would be a compile error at
//! [`build_capabilities`](super::build_capabilities), not a silent live effect.
//!
//! # What is NOT stubbed, and why
//!
//! The read-only, effect-free capabilities stay real:
//!
//! * the **resolver** ([`StoreWorkflowResolver`](super::resolver)) — resolving a
//!   `sub_workflow` child is a read, and the child runs under this same dry
//!   bundle, so a dry run propagates into sub-workflows rather than stopping at
//!   the boundary;
//! * **state** is the inert [`NoopState`](super::state::NoopState) — never the
//!   durable [`CompanyStateStore`](super::state::CompanyStateStore), so a dry
//!   run cannot persist run state either;
//! * `llm` / `code` / `memory` are unchanged (already unwired stubs / `None`).
//!
//! # The grant check is kept, deliberately
//!
//! [`DryRunTools`] still runs the fail-closed `[tools].allow` grant check before
//! returning its canned echo. The check is pure — it reads the company's grants
//! and touches nothing outside the process — so keeping it means a `tool_call`
//! the company does not grant is refused in a dry run *exactly* as it is live.
//! A test run is meant to prove routing, and "this node would have been denied"
//! is part of the routing.

use async_trait::async_trait;
use serde_json::{Value, json};
use tinyflows::caps::{AgentRunner, HttpClient, ToolInvoker};
use tinyflows::error::{EngineError, Result as TfResult};

use super::tools::{WorkflowToolWiring, refusal_for};

/// A marker key set on every dry-stub output, so a downstream node (or a test)
/// can tell a stubbed item from a real one.
pub(super) const DRY_RUN_MARKER: &str = "dry_run";

/// The [`AgentRunner`] a dry run wires in place of
/// [`HarnessAgentRunner`](super::HarnessAgentRunner): it returns a structured
/// echo of what the node *would* have asked, with **zero** pool routing and zero
/// inference.
///
/// The reply mirrors the real runner's `{ text, agent_ref }` envelope shape so a
/// downstream `=item.text` binding still resolves — a dry run must exercise the
/// same routing the real one would — plus the [`DRY_RUN_MARKER`] so nothing
/// mistakes the fixture for a real turn.
pub(super) struct DryRunAgent;

#[async_trait]
impl AgentRunner for DryRunAgent {
    async fn run_agent(
        &self,
        agent_ref: &str,
        request: Value,
        _conn: Option<&str>,
    ) -> TfResult<Value> {
        // Same extraction the real runner uses, so the fixture echoes exactly
        // the instruction the live turn would have received.
        let instruction = super::message_from_request(&request);
        tracing::debug!(
            agent = agent_ref,
            "workflow dry run: stubbing agent node (no inference)"
        );
        Ok(json!({
            "text": format!("[dry run] agent `{agent_ref}` would run: {instruction}"),
            "agent_ref": agent_ref,
            "instruction": instruction,
            DRY_RUN_MARKER: true,
        }))
    }
}

/// The [`ToolInvoker`] a dry run wires in place of
/// [`WorkflowToolInvoker`](super::tools::WorkflowToolInvoker): it keeps the
/// exact same fail-closed grant gate — so an ungranted `tool_call` refuses
/// identically in a dry run — but returns a canned echo instead of executing the
/// tool, so nothing touches the workspace, the network, or a priced backend.
pub(super) struct DryRunTools {
    /// The company's `[tools].allow` grant globs — the same gate the live
    /// invoker applies, reused verbatim.
    grants: Vec<String>,
    wiring: WorkflowToolWiring,
}

impl DryRunTools {
    /// Builds a dry invoker gated by the company's `[tools].allow`.
    pub(super) fn new(grants: Vec<String>, wiring: WorkflowToolWiring) -> Self {
        Self { grants, wiring }
    }
}

#[async_trait]
impl ToolInvoker for DryRunTools {
    async fn invoke(&self, slug: &str, args: Value, _conn: Option<&str>) -> TfResult<Value> {
        // Issue #846: the replay arm, mirrored from the live invoker.
        //
        // A dry run cannot reach here through the host's own path — dry runs are
        // never continuations, park no gate and stub every effect — so this is
        // not load-bearing today. It is here because the alternative is worse
        // than redundant: without it, a graph carrying a replay slug would fall
        // through to `namespace_of` and fail the node with "not a wired workflow
        // tool", which is a dry run reporting a routing failure that the real run
        // does not have. The two invokers agreeing about every slug is the
        // property a test run's answer is worth anything for.
        if let Some(result) = super::super::replay::replayed_result(slug, &args) {
            return Ok(result);
        }
        // FAIL-CLOSED grant check FIRST, identical to the live invoker
        // (`WorkflowToolInvoker::invoke`): a dry run must refuse an ungranted
        // tool exactly as a real one does, because that refusal is part of the
        // routing a test run exists to prove. The check is pure — no effect.
        if let Some(message) = refusal_for(slug, &self.grants, &self.wiring) {
            return Err(EngineError::Capability(message));
        }
        tracing::debug!(slug, "workflow dry run: stubbing tool_call (not executed)");
        Ok(json!({
            "text": format!("[dry run] tool_call `{slug}` was not executed"),
            "slug": slug,
            "args": args,
            DRY_RUN_MARKER: true,
        }))
    }
}

/// The [`HttpClient`] a dry run wires in place of
/// [`GuardedHttpClient`](super::http::GuardedHttpClient): it echoes the request
/// descriptor back without issuing anything, so no outbound request (guarded or
/// not) leaves the process.
pub(super) struct DryRunHttp {
    /// The company's `[tools].web_allowed_domains`, so the target check a dry
    /// run *can* make is made against the same list the live client uses.
    allowed_domains: Vec<String>,
}

impl DryRunHttp {
    pub(super) fn new(allowed_domains: Vec<String>) -> Self {
        Self { allowed_domains }
    }
}

#[async_trait]
impl HttpClient for DryRunHttp {
    /// Refuses a target the real run would refuse; otherwise reports the request
    /// as **not checked**, never as a success.
    ///
    /// Issue #1048: this used to return the same cheerful stub for every URL, so
    /// a node aimed at a host the capability layer blocks reported `ok` on Test
    /// run and failed immediately on the real one. Test run is the single
    /// control an operator has before arming a graph on a schedule, so a green
    /// there followed by a refusal on the real run is worse than no dry run at
    /// all — it converts "I checked it" into a false belief.
    ///
    /// The refusal message and `EngineError::Capability` shape match
    /// [`GuardedHttpClient`](super::http::GuardedHttpClient)'s, so a node fails
    /// under its own `on_error`/retry policy exactly as it would live.
    ///
    /// **Still no request is issued, in either branch.** The verdict below is a
    /// pure function of the URL and the company's config — see
    /// [`preflight_refusal`](super::http::preflight_refusal) for what it decides
    /// and, more importantly, what it deliberately leaves to the real run.
    async fn request(&self, request: Value, _conn: Option<&str>) -> TfResult<Value> {
        if let Some(reason) = super::http::preflight_refusal(&request, &self.allowed_domains) {
            tracing::debug!(%reason, "workflow dry run: refusing http_request target");
            return Err(EngineError::Capability(format!("http_request: {reason}")));
        }
        tracing::debug!("workflow dry run: stubbing http_request (not sent)");
        Ok(json!({
            "status": Value::Null,
            // Deliberately not phrased as a result. A dry run cannot know
            // whether the host is up, the credential is current or the response
            // parses, and saying so is more honest than a green that implies it
            // does — the opposite error to the one #1048 fixed, and the one that
            // erodes trust fastest because an operator cannot tell it is wrong.
            "body": "[dry run] http_request was not sent — target allowed, delivery not checked",
            "checked": "target only: a real run may still fail to reach this host",
            "request": request,
            DRY_RUN_MARKER: true,
        }))
    }
}

#[cfg(test)]
#[path = "dry_run_tests.rs"]
mod tests;
