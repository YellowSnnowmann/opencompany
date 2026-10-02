//! `list_ledgers`: names every ledger the agent may see, with its statuses and
//! open/closed counts.

use async_trait::async_trait;
use serde_json::{Value, json};

use tinytools::{PermissionLevel, Tool, ToolResult};

use super::{LIST_LEDGERS_TOOL, ledger_access};
use crate::company::LedgerGrant;
use crate::company::ledgers::{self, Ledgers};
use crate::ledger::{LedgerSource, LedgerSpec};

pub(super) struct ListLedgers {
    pub(super) ctx: Ledgers,
    pub(super) ledger_grants: Option<Vec<LedgerGrant>>,
}

#[async_trait]
impl Tool for ListLedgers {
    fn name(&self) -> &str {
        LIST_LEDGERS_TOOL
    }

    fn description(&self) -> &str {
        "Name every ledger this company keeps, with what each one holds, its statuses and how many \
         rows are open. USE FOR finding where something belongs before recording it, and for \
         checking whether an axis already exists before declaring a new one. Read a ledger's rows \
         with `read_ledger`."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    async fn execute(&self, _arguments: Value) -> anyhow::Result<ToolResult> {
        let registry = match ledgers::registry(&self.ctx).await {
            Ok(registry) => registry,
            Err(error) => {
                return Ok(ToolResult::error(format!(
                    "Could not read this company's ledgers: {error}."
                )));
            }
        };
        let mut out = String::new();
        for spec in registry
            .specs()
            .iter()
            .filter(|spec| ledger_access(&self.ledger_grants, &spec.slug).is_some())
        {
            out.push_str(&format!(
                "- `{}` — {}\n  statuses: {}\n  {}\n",
                spec.slug,
                crate::ledger::budget::truncate(&spec.purpose, 300),
                spec.statuses
                    .iter()
                    .map(|status| status.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                counts_line(&self.ctx, spec).await,
            ));
            if spec.source == LedgerSource::Native {
                out.push_str(&format!("  read-only here: {}\n", spec.written_by));
            }
        }
        // Surfaced rather than swallowed: a company whose ledger silently
        // stopped appearing has no way to find out why.
        for fault in registry.faults() {
            out.push_str(&format!("- (not loaded) {fault}\n"));
        }
        Ok(ToolResult::success(out))
    }
}

/// The open/closed count line for one ledger in `list_ledgers`.
async fn counts_line(ctx: &Ledgers, spec: &LedgerSpec) -> String {
    match ledgers::entries(ctx, spec).await {
        Ok(entries) => format!(
            "{} open, {} closed",
            entries.open_count(spec),
            entries.closed_count(spec)
        ),
        Err(error) => {
            tracing::warn!(ledger = %spec.slug, %error, "list_ledgers: rows could not be read");
            format!(
                "counts unavailable — rows could not be read ({error}); do not treat this ledger \
                 as empty"
            )
        }
    }
}
