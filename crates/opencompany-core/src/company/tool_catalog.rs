//! The one list of tools a company can grant.
//!
//! Before this existed, "what can our agents reach?" had three unrelated
//! answers: the built-in grant namespaces
//! ([`GATEABLE_NAMESPACES`](crate::company::GATEABLE_NAMESPACES) and the
//! ungated families beside them), the `[[mcp_server]]` entries reachable through
//! `mcp:<slug>` grants, and the Composio toolkits behind `[tools.composio]`.
//! Each has its own vocabulary, and nothing enumerated them together — so a
//! grant naming a server that does not exist looked exactly like one that
//! worked, and no console screen could list what was on offer.
//!
//! This module is a **projection**, never a source of truth. Every entry's
//! [`grant`](CatalogEntry::grant) is a string the real matcher already
//! understands, and the catalog invents nothing: if an entry's grant does not
//! resolve through the same narrowing the harness applies, that is a bug in this
//! file, not a new kind of permission. A test at the bottom holds that line.

use crate::company::CompanyManifest;

/// What kind of thing a catalog entry names.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolKind {
    /// A built-in tool family, gated by its grant namespace.
    Builtin {
        /// The grant namespace, e.g. `shell`, `web`, `docs`.
        namespace: String,
    },
    /// A per-tenant MCP tool server, reached through an `mcp:<name>` grant.
    Mcp {
        /// The server's manifest `name` slug.
        server: String,
    },
    /// A Composio toolkit reachable under the `composio` grant.
    Composio {
        /// The toolkit slug, e.g. `gmail`, `slack`.
        toolkit: String,
    },
}

/// One thing a company can grant an agent.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    /// Stable identifier for this entry, unique within a catalog.
    pub key: String,
    /// What the entry names.
    #[serde(flatten)]
    pub kind: ToolKind,
    /// The grant string that confers it — the exact token an operator writes in
    /// `[tools].allow`, a desk's `tools`, or an agent's `tools`.
    pub grant: String,
    /// One line an operator can read, in prosumer language.
    pub description: String,
    /// Whether a catch-all `*` confers this entry.
    ///
    /// The three real-money / third-party-credential namespaces (`media`,
    /// `composio`, `search`) must be
    /// named explicitly, so a console that renders `*` as "everything" without
    /// this flag would tell an operator they had granted something they had not.
    pub covered_by_wildcard: bool,
    /// Whether the company currently grants this entry at all, at the
    /// company-wide level (`[tools].allow`). Desks and agents narrow further.
    pub granted: bool,
}

/// The built-in tool families, with the one-line description each gets in the
/// console.
///
/// The gateable namespaces come from [`GATEABLE_NAMESPACES`] so this list cannot
/// drift from what the capability filter knows about; the remaining entries are
/// the ungated families a company can still grant or withhold.
const BUILTIN_DESCRIPTIONS: &[(&str, &str)] = &[
    ("shell", "Run commands in the agent's own sandbox."),
    ("code", "Edit files, apply patches, and use git."),
    ("web", "Fetch a URL the agent already has."),
    ("subagent", "Split work across helper agents."),
    ("media", "Generate images and video. Spends real money."),
    (
        "composio",
        "Reach connected third-party accounts (Gmail, Slack, GitHub).",
    ),
    ("search", "Search the web. Billed per search."),
    ("docs", "Read and write documents."),
    ("files", "Read and write files in the agent's sandbox."),
    ("workspace", "Read the company's shared workspace."),
    (
        "pages",
        "Define and edit the company's internal dashboard pages.",
    ),
];

/// Builds the company's tool catalog: built-ins, then MCP servers, then Composio
/// toolkits, each in a stable order so a console list does not reshuffle between
/// reads.
pub fn catalog(manifest: &CompanyManifest) -> Vec<CatalogEntry> {
    let allow = &manifest.tools.allow;
    let mut entries = Vec::new();

    for (namespace, description) in BUILTIN_DESCRIPTIONS {
        entries.push(CatalogEntry {
            key: format!("builtin:{namespace}"),
            kind: ToolKind::Builtin {
                namespace: (*namespace).to_string(),
            },
            grant: (*namespace).to_string(),
            description: (*description).to_string(),
            covered_by_wildcard: wildcard_covers(namespace),
            granted: namespace_granted(allow, namespace),
        });
    }

    for server in &manifest.mcp_servers {
        let grant = format!("mcp:{}", server.name);
        entries.push(CatalogEntry {
            key: format!("mcp:{}", server.name),
            kind: ToolKind::Mcp {
                server: server.name.clone(),
            },
            description: server
                .description
                .clone()
                .unwrap_or_else(|| format!("Tools from the `{}` server.", server.name)),
            // A disabled server is listed but never granted: an operator needs to
            // see that it exists and is switched off, which an omitted row cannot
            // say.
            granted: server.enabled && crate::runtime::builder::allow_covers(allow, &grant),
            covered_by_wildcard: false,
            grant,
        });
    }

    for toolkit in &manifest.tools.composio.toolkits {
        entries.push(CatalogEntry {
            key: format!("composio:{toolkit}"),
            kind: ToolKind::Composio {
                toolkit: toolkit.clone(),
            },
            // Composio toolkits ride the one `composio` grant; the toolkit list
            // narrows which of them the tool may target. So the grant string
            // here is `composio`, not `composio:<toolkit>` — writing the latter
            // would be inventing a permission the matcher does not know.
            grant: "composio".to_string(),
            description: format!("The `{toolkit}` toolkit, through Composio."),
            covered_by_wildcard: false,
            granted: crate::company::grants_composio_explicit(allow),
        });
    }

    entries
}

/// Whether a company's `allow` list grants a built-in namespace.
///
/// Four namespaces are **not** answerable by the generic glob matcher, and this
/// is the one place that difference has to be honoured rather than assumed. The
/// generic matcher says a catch-all `*` covers everything, but `media`,
/// `composio` and `search` each carry a rule that `*` deliberately does
/// not confer them — they spend real money, reach a tenant's third-party
/// accounts, or materialize a third party's source inside a sandbox, so they
/// must be opted into by name.
///
/// Each of those rules already exists as its own predicate beside the manifest
/// types. Delegating to them, rather than re-deriving the answer from `allow`,
/// is what keeps the catalog a projection: the console cannot tell an operator
/// they have granted something the gate will refuse.
fn namespace_granted(allow: &[String], namespace: &str) -> bool {
    match namespace {
        "media" => crate::company::grants_media_explicit(allow),
        "composio" => crate::company::grants_composio_explicit(allow),
        "search" => crate::company::grants_search_explicit(allow),
        _ => crate::runtime::builder::allow_covers(allow, namespace),
    }
}

/// Whether a catch-all `*` grant confers `namespace`.
///
/// The same four exceptions [`namespace_granted`] routes around, surfaced as
/// data so a console rendering `*` as "everything" can say which families it
/// does not in fact cover. The test below pins this set against the predicates
/// themselves, so a fifth opt-in namespace cannot be added in one place only.
fn wildcard_covers(namespace: &str) -> bool {
    !matches!(namespace, "media" | "composio" | "search")
}

#[cfg(test)]
#[path = "tool_catalog_tests.rs"]
mod tests;
