//! [`OpenHumanToolProvider`]: a [`ToolProvider`] backed by openhuman-core.
//!
//! The catalog is fetched over JSON-RPC and **filtered by the company's tool
//! grants** (`[tools].allow` intersected with per-agent `tools`, precomputed by
//! the builder). `invoke` re-checks the grant *before* issuing any RPC, so an
//! ungranted call is side-effect-free. When openhuman-core is unreachable the
//! provider degrades to a built-in `fallback` rather than failing.

use std::sync::Arc;

use async_trait::async_trait;

use crate::Result;
use crate::error::OpenCompanyError;
use crate::openhuman::rpc::{OpenHumanRpc, rpc_method};
use crate::ports::tools::ToolProvider;
use crate::ports::types::{CompanyId, ToolCall, ToolResult, ToolSpec};
use crate::runtime::tools::grant_matches;

/// A [`ToolProvider`] that delegates to openhuman-core over JSON-RPC.
pub struct OpenHumanToolProvider {
    rpc: Arc<dyn OpenHumanRpc>,
    grants: Vec<String>,
    fallback: Arc<dyn ToolProvider>,
}

impl OpenHumanToolProvider {
    /// Wires a provider over `rpc`, restricting the catalog/invocations to
    /// `grants` and degrading to `fallback` when openhuman-core fails.
    pub fn new(
        rpc: Arc<dyn OpenHumanRpc>,
        grants: Vec<String>,
        fallback: Arc<dyn ToolProvider>,
    ) -> Self {
        Self {
            rpc,
            grants,
            fallback,
        }
    }

    /// Whether `tool` is covered by any of the company's grant globs.
    fn is_granted(&self, tool: &str) -> bool {
        self.grants.iter().any(|grant| grant_matches(grant, tool))
    }
}

#[async_trait]
impl ToolProvider for OpenHumanToolProvider {
    async fn catalog(&self, company: &CompanyId) -> Result<Vec<ToolSpec>> {
        // On any RPC failure, degrade to the built-in catalog rather than error.
        let value = match self
            .rpc
            .call(&rpc_method("tools", "list"), serde_json::json!({}))
            .await
        {
            Ok(value) => value,
            Err(_) => return self.fallback.catalog(company).await,
        };
        let specs: Vec<ToolSpec> = serde_json::from_value(value)?;
        Ok(specs
            .into_iter()
            .filter(|spec| self.is_granted(&spec.name))
            .collect())
    }

    async fn invoke(&self, _company: &CompanyId, call: ToolCall) -> Result<ToolResult> {
        // Enforce the grant *before* any RPC/side effect.
        if !self.is_granted(&call.tool) {
            return Err(OpenCompanyError::ToolNotGranted(call.tool));
        }
        let params = serde_json::json!({ "tool": call.tool, "args": call.args });
        match self.rpc.call(&rpc_method("tools", "invoke"), params).await {
            // The wire result is a `ToolResult`; fall back to a well-formed
            // failure if openhuman returns a shape we cannot decode.
            Ok(value) => Ok(serde_json::from_value(value.clone()).unwrap_or(ToolResult {
                ok: false,
                output: value,
            })),
            // Granted but the RPC failed: report a failed-but-well-formed result
            // so a grant misconfiguration and a runtime failure stay distinct.
            Err(err) => Ok(ToolResult {
                ok: false,
                output: serde_json::json!({
                    "error": "openhuman rpc failed",
                    "tool": call.tool,
                    "detail": err.to_string(),
                }),
            }),
        }
    }
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
