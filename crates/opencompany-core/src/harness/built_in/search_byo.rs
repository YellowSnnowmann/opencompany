//! A company's **own** search connection: the BYO half of the search surface
//! (`issue #238` deferred it explicitly — "wiring it belongs with the console
//! credential surface that `composio` already uses").
//!
//! # What this inherits, and what it does not
//!
//! OpenHuman owns the search domain (`oh::search`): six engines, a canonical
//! `web_search_tool` slot, `managed` backend-proxied by default. The BYO engines
//! there — Brave, Exa, Querit, plus the standalone SearXNG tool — are ordinary
//! `Tool` implementations with public constructors that take a key and nothing
//! else. This module **calls those constructors** with the company's own stored
//! key. No provider trait, no HTTP client, no result parsing of its own: the
//! whole point is that a Brave result rendered for an OpenCompany agent is the
//! same text OpenHuman renders.
//!
//! What it does *not* do is call [`oh::search::build_search_tools`], for the
//! reason [`search`](crate::harness::search) already gives: that entry point
//! takes OpenHuman's global `Config`, and the harness assembles per-company
//! state instead of a process-wide config — two companies on one host search
//! through two different accounts, so a global is not merely awkward here, it is
//! wrong.
//!
//! # One name, whichever provider
//!
//! Every provider's canonical "search the web" tool is presented to the model as
//! **`web_search`** — the same name the managed surface uses — through
//! [`AliasedTool`]. A company that switches from managed to Brave changes what
//! the tool costs and who bills it; it does not change what the agent is told it
//! can do. The shipped research skills name `web_search` in their instructions,
//! and a belt where that name appears and disappears with a settings change is
//! how an agent comes to invent URLs instead of searching for them.
//!
//! Provider extras keep their upstream names (`exa_find_similar`,
//! `exa_get_contents`, `brave_news_search`, `brave_image_search`,
//! `brave_video_search`) — they are genuinely different affordances, and a name
//! borrowed from upstream is one an operator can look up.
//!
//! # Fail open to managed, never to nothing
//!
//! Resolution answers `None` when the company configured nothing, or configured
//! a provider whose credential is missing. The caller then wires the metered
//! managed surface, which is exactly what OpenHuman does ("a BYO engine with no
//! key falls back to the managed surface"). A half-configured settings page
//! therefore degrades to a working, capped search rather than to an agent with
//! no way to find a source.
//!
//! # Money, and why the daily cap does not follow
//!
//! The managed tool is metered and daily-capped because every call spends the
//! *platform's* money ([`search`](crate::harness::search) explains the ledger).
//! A BYO call spends the *company's* own account, billed by Brave or Exa
//! directly, under rate limits that company chose. Applying the platform's cap
//! to it would be this host throttling a bill it does not pay, so it does not:
//! the cap travels with the managed credential, and a company that wants a
//! ceiling on its own key sets one where the key is issued.

use std::sync::Arc;

use crate::company::search::{configuration_complete, provider_is_byo};
use crate::ports::SecretStore;
use crate::ports::types::CompanyId;

/// Results a BYO provider is asked for when the caller does not say. Matches the
/// managed tool's default so switching providers does not change how much
/// context one search costs.
const DEFAULT_MAX_RESULTS: usize = 5;

/// Seconds a BYO provider call may take before it is abandoned. Deliberately
/// shorter than a turn: a search that has not answered in half a minute has
/// already cost the agent more than the answer is worth.
const TIMEOUT_SECS: u64 = 30;

/// Language a SearXNG instance is queried in when the company sets none.
const SEARXNG_LANGUAGE: &str = "all";

/// One company's resolved BYO search connection.
///
/// Only ever constructed for a provider that is both BYO and complete — see
/// [`TenantSearch::resolve`]. `managed`, and every half-configured provider,
/// resolve to `None` rather than to a `TenantSearch` that would wire a tool with
/// no credential behind it.
#[derive(Clone)]
pub struct TenantSearch {
    provider: String,
    api_key: Option<String>,
    endpoint: Option<String>,
}

/// Prints the provider and endpoint, never the key.
impl std::fmt::Debug for TenantSearch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TenantSearch")
            .field("provider", &self.provider)
            .field("endpoint", &self.endpoint)
            .field(
                "api_key",
                &if self.api_key.is_some() {
                    "<redacted>"
                } else {
                    "<unset>"
                },
            )
            .finish()
    }
}

impl TenantSearch {
    /// Resolves a company's BYO search connection from its secret store.
    ///
    /// `Ok(None)` means "search through the managed surface": no provider
    /// stored, `managed` stored explicitly, an unknown slug, or a BYO provider
    /// whose credential half is missing. All four are ordinary states of a
    /// settings page, not errors.
    ///
    /// A store **read failure** is an `Err`, not `Ok(None)`. Collapsing them
    /// would make an unhealthy secret store indistinguishable from "not
    /// configured", and the caller's response differs: absence should fall back
    /// to managed, while a transient read error should keep the connection the
    /// roster already had. See `HarnessPool::resolve_tenant_search`.
    ///
    /// # Errors
    ///
    /// Returns an error when the secret store cannot be read.
    pub async fn resolve(
        secrets: &Arc<dyn SecretStore>,
        company: &CompanyId,
    ) -> crate::error::Result<Option<TenantSearch>> {
        // Asks the same resolver the console's status route and the
        // capabilities panel ask. That is not tidiness: the store's convergence
        // rule moves a credential from `search/api_key` to
        // `search/provider/<slug>/key` on the first save, and a reader still
        // looking at the flat address would see an unconfigured company and
        // silently drop it to managed search — the agents would keep searching,
        // they would just quietly stop using the account the operator pays for.
        // Every reader moves together or none does.
        let candidates = crate::company::search::candidates(company, secrets.as_ref()).await?;
        let marked =
            crate::company::search::store::load_default_slug(company, secrets.as_ref()).await?;
        let Some(active) = crate::company::search::resolve::active(&candidates, marked.as_deref())
        else {
            if !candidates.is_empty() {
                tracing::warn!(
                    company = %company,
                    "[search] a BYO provider is connected but none resolves; falling back to the \
                     managed surface"
                );
            }
            return Ok(None);
        };

        let provider = active.provider.slug.clone();
        // Belt to the resolver's braces: `active` only ever returns a complete,
        // enabled provider, and `managed` is never a record.
        if !provider_is_byo(&provider) {
            return Ok(None);
        }

        let api_key =
            crate::company::search::store::load_provider_key(company, secrets.as_ref(), &provider)
                .await?;
        let endpoint = active.provider.endpoint.clone();
        if !configuration_complete(&provider, api_key.is_some(), endpoint.is_some()) {
            tracing::warn!(
                company = %company,
                provider = %provider,
                "[search] BYO provider is selected but its credential is missing; falling back to \
                 the managed surface"
            );
            return Ok(None);
        }

        Ok(Some(TenantSearch {
            provider,
            api_key,
            endpoint,
        }))
    }

    /// The provider this company searches through. Never the key.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// A connection assembled directly, for tests that need one without a
    /// secret store behind it — the roster-build and `build_agent` tests, which
    /// are about what a *resolved* connection wires rather than about how it
    /// resolved.
    #[cfg(test)]
    pub fn for_test(provider: &str, api_key: Option<&str>, endpoint: Option<&str>) -> Self {
        Self {
            provider: provider.to_string(),
            api_key: api_key.map(str::to_string),
            endpoint: endpoint.map(str::to_string),
        }
    }

    /// A stable hash of the connection, for the roster staleness check.
    ///
    /// Covers the key as well as the provider and endpoint, so rotating a key
    /// with everything else unchanged still rebuilds the roster — otherwise a
    /// rotated credential would keep authenticating with the old one until a
    /// restart.
    pub fn fingerprint(config: &Option<TenantSearch>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        match config {
            None => 0u8.hash(&mut hasher),
            Some(search) => {
                1u8.hash(&mut hasher);
                search.provider.hash(&mut hasher);
                search.api_key.hash(&mut hasher);
                search.endpoint.hash(&mut hasher);
            }
        }
        hasher.finish()
    }
}

mod module;

pub use live::{BYO_SEARCH_TOOLS, byo_search_tools};

mod live {
    use super::{DEFAULT_MAX_RESULTS, TIMEOUT_SECS, TenantSearch};

    use async_trait::async_trait;
    use serde_json::Value;

    use openhuman_core as oh;
    use tinysearch_bus::{ExecuteToolRequest, ToolSpec, provider_tool_specs};
    use tinytools::{
        PermissionLevel, Tool, ToolCallOptions, ToolCategory, ToolResult, ToolScope, ToolTimeout,
    };

    use super::module;
    use crate::harness::search::WEB_SEARCH_TOOL;

    /// Every tool name a BYO provider can put on a belt, across all providers.
    ///
    /// Not "the names one company sees" — that is provider-dependent — but the
    /// closed set the `search` namespace has to account for, so
    /// [`namespace_of`](crate::harness::toolbelt::namespace_of) and the
    /// gateable-coverage invariant can be checked against it rather than against
    /// a list somebody remembered to update.
    pub const BYO_SEARCH_TOOLS: [&str; 6] = [
        "exa_find_similar",
        "exa_get_contents",
        "brave_news_search",
        "brave_image_search",
        "brave_video_search",
        WEB_SEARCH_TOOL,
    ];

    /// The one catalogue tool per provider that answers "search the web", and
    /// so wears OpenCompany's [`WEB_SEARCH_TOOL`] name, with the operator-facing
    /// label for its engine.
    ///
    /// The label is fixed rather than read from the response: a BYO tool's
    /// engine is settled when the company stored its key, unlike the managed
    /// surface where only the response says which engine answered.
    fn canonical(provider: &str) -> Option<(&'static str, &'static str)> {
        match provider {
            "brave" => Some(("brave_web_search", "Brave web search")),
            "exa" => Some(("exa_search", "Exa web search")),
            "querit" => Some(("querit_search", "Querit web search")),
            "searxng" => Some(("searxng_search", "SearXNG web search")),
            _ => None,
        }
    }

    /// The provider's extra affordances, kept under their upstream names
    /// because they are genuinely different tools and a borrowed name is one an
    /// operator can look up.
    ///
    /// Deliberately a fixed list rather than "whatever the catalogue holds":
    /// [`BYO_SEARCH_TOOLS`] is the closed set the capability gate is checked
    /// against, so a tool upstream adds must be added here *and* there in one
    /// change. `exa_answer` is the one such addition pending — see the test that
    /// pins it.
    fn extras(provider: &str) -> &'static [&'static str] {
        match provider {
            "brave" => &[
                "brave_news_search",
                "brave_image_search",
                "brave_video_search",
            ],
            "exa" => &["exa_find_similar", "exa_get_contents"],
            _ => &[],
        }
    }

    /// The search tools for one company's own provider connection.
    ///
    /// An unknown provider slug wires nothing and warns rather than failing the
    /// build: an agent that cannot search is a degraded agent, not a broken
    /// company. In practice the slug was validated by the console write route
    /// and again by [`TenantSearch::resolve`], so reaching the warn arm means
    /// somebody wrote the secret store directly.
    pub fn byo_search_tools(config: &TenantSearch) -> Vec<Box<dyn Tool>> {
        let Some((canonical_name, label)) = canonical(&config.provider) else {
            tracing::warn!(
                provider = %config.provider,
                "[search] unknown BYO search provider stored; no search tools wired"
            );
            return Vec::new();
        };
        let catalogue = provider_tool_specs();
        let Some(specs) = catalogue.get(&config.provider) else {
            // The slug is one this host knows but the vendored catalogue does
            // not publish — an upstream removal. Reported rather than
            // papered over: the company's key is configured and its agents are
            // about to search through the platform's account instead.
            tracing::warn!(
                provider = %config.provider,
                "[search] the vendored TinySearch catalogue publishes no tools for this provider"
            );
            return Vec::new();
        };
        let spec_for = |name: &str| specs.iter().find(|spec| spec.name == name).cloned();

        let mut tools: Vec<Box<dyn Tool>> = Vec::new();
        match spec_for(canonical_name) {
            Some(spec) => tools.push(Box::new(ModuleSearchTool::aliased(
                config.clone(),
                spec,
                WEB_SEARCH_TOOL,
                label,
            ))),
            None => tracing::warn!(
                provider = %config.provider,
                tool = canonical_name,
                "[search] the catalogue no longer publishes this provider's web search tool"
            ),
        }
        for name in extras(&config.provider) {
            match spec_for(name) {
                Some(spec) => {
                    tools.push(Box::new(ModuleSearchTool::upstream(
                        config.clone(),
                        spec,
                        label,
                    )));
                }
                None => tracing::warn!(
                    provider = %config.provider,
                    tool = %name,
                    "[search] the catalogue no longer publishes this provider extra"
                ),
            }
        }
        tools
    }

    /// One catalogue tool, executed by the loaded TinySearch module against the
    /// company's own credential.
    ///
    /// Declaration and rendering both come from upstream — the schema is the
    /// catalogue's [`ToolSpec`], and the result is `oh::search::render::render`
    /// — so a Brave result an OpenCompany agent reads is the same text OpenHuman
    /// renders. What this type adds is the name the belt uses, the company whose
    /// key the call carries, and the policy the harness expects.
    struct ModuleSearchTool {
        tenant: TenantSearch,
        spec: ToolSpec,
        /// The name presented to the model. `None` keeps the catalogue's own.
        alias: Option<&'static str>,
        label: &'static str,
    }

    impl ModuleSearchTool {
        fn aliased(
            tenant: TenantSearch,
            spec: ToolSpec,
            alias: &'static str,
            label: &'static str,
        ) -> Self {
            Self {
                tenant,
                spec,
                alias: Some(alias),
                label,
            }
        }

        fn upstream(tenant: TenantSearch, spec: ToolSpec, label: &'static str) -> Self {
            Self {
                tenant,
                spec,
                alias: None,
                label,
            }
        }

        /// How many results to render. The module is configured with the
        /// company's default; an explicit argument narrows the rendering the
        /// same way upstream's own tool does.
        fn max_results(&self, args: &Value) -> usize {
            args.get("max_results")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_MAX_RESULTS, |count| count as usize)
                .clamp(1, 20)
        }
    }

    #[async_trait]
    impl Tool for ModuleSearchTool {
        fn name(&self) -> &str {
            self.alias.unwrap_or(&self.spec.name)
        }

        fn description(&self) -> &str {
            &self.spec.description
        }

        fn parameters_schema(&self) -> Value {
            self.spec.parameters.clone()
        }

        /// Not the default: it would humanize the alias into a provider-less
        /// "Web search", and which engine the company is paying for is the one
        /// thing an operator reading the step timeline wants to see.
        fn display_label(&self, _args: &Value) -> Option<String> {
            Some(self.label.to_string())
        }

        /// Advisory only, and matched to the managed tool so the two
        /// `web_search` tools present identically. What actually decides whether
        /// a call parks or is denied is the name-based classification in
        /// [`crate::harness::policy`].
        fn permission_level(&self) -> PermissionLevel {
            PermissionLevel::ReadOnly
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Workflow
        }

        fn scope(&self) -> ToolScope {
            ToolScope::All
        }

        fn supports_markdown(&self) -> bool {
            true
        }

        /// A search spends the company's own provider quota and changes nothing
        /// a later call could observe, so concurrent calls are safe — the module
        /// lock serializes them regardless.
        fn is_concurrency_safe(&self, _args: &Value) -> bool {
            true
        }

        fn external_effect(&self) -> bool {
            false
        }

        fn timeout_policy(&self, _args: &Value) -> ToolTimeout {
            // The module is configured with the same ceiling; this is the
            // host-side bound on a module that stopped answering at all.
            ToolTimeout::Millis(TIMEOUT_SECS * 1_000)
        }

        async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
            self.execute_with_options(args, ToolCallOptions::default())
                .await
        }

        async fn execute_with_options(
            &self,
            args: Value,
            options: ToolCallOptions,
        ) -> anyhow::Result<ToolResult> {
            let subject = oh::search::render::subject(&args);
            let max_results = self.max_results(&args);
            let request = ExecuteToolRequest {
                name: self.spec.name.clone(),
                arguments: args,
            };
            match module::execute(&self.tenant, request).await {
                Ok(response) => Ok(oh::search::render::render(
                    &response,
                    &subject,
                    max_results,
                    options.prefer_markdown,
                )),
                Err(error) => {
                    // The query may be echoed in a provider's error detail, so
                    // the classified code is logged and the detail is not.
                    tracing::warn!(
                        tool = %self.spec.name,
                        provider = %self.tenant.provider,
                        code = oh::search::tools::error_code(&error).unwrap_or("unclassified"),
                        "[search] a BYO search failed"
                    );
                    Ok(ToolResult::error(oh::search::tools::user_facing_error(
                        &error,
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "search_byo_tests.rs"]
mod tests;
