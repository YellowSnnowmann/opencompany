//! Which provider serves which workload. Pure: no IO, no async, no fixtures
//! beyond structs.
//!
//! Everything here is a decision with a branch worth a test, which is exactly
//! why none of it lives in a handler or a component. Given a list of providers
//! and a routing map, these functions answer *which one*, *which model*, and
//! *what happened when the answer is none* — and they answer it the same way for
//! the console's status view and for the turn that is about to be sent.
//!
//! ## Four rules, each of which is a bug somebody already shipped
//!
//! **No workload inherits another's route.** Upstream shipped the opposite:
//! setting only the coding route moved chat and reasoning onto that key too, so
//! ordinary conversations were silently billed to the user's own account with no
//! settings field saying so. An unset workload resolves through the *primary*,
//! never through a sibling's configured provider. [`Resolution::Primary`] is how
//! that is said out loud.
//!
//! **Fail closed on a route naming a provider that is gone.** A route pointing
//! at a slug nobody holds is an error that names the workload and the slug —
//! [`Resolution::Missing`] — not a silent demotion to the primary. Demoting
//! quietly would attribute that workload's spend to whatever the fallback
//! happened to be, which is the same defect as resolving an unknown provider
//! kind rather than rejecting it.
//!
//! **A disabled provider is not a routing target, and a route naming one is
//! reported rather than demoted** — [`Resolution::Disabled`]. Same reasoning:
//! "stop billing this account this week" must not become "quietly bill a
//! different one".
//!
//! **Removing a provider scrubs the routes pointing at it, by three different
//! rules**, because only one of the three kinds of ref carries a slug. See
//! [`scrub_removed`]; all three cases are bugs upstream had to fix.
//!
//! ## An alias is not an inherited route
//!
//! [`Workload::Coding`] resolves through the agentic route. That is an **alias**
//! — two names for one configured route — and it is a different thing from an
//! unset route borrowing a set one. The distinction matters because the
//! inheritance bug above looked exactly like an alias from the outside.
//!
//! It is an alias rather than a fifth row for a concrete reason: OpenCompany has
//! four abstract tiers and no distinct coding tier, so a separate editable
//! coding row would write the agentic tier's route under a second name. Setting
//! one would silently change the other, which is the inheritance bug wearing a
//! different hat. The row appears when a coding tier does.

use std::collections::BTreeMap;

use super::catalogue::{self, Category};
use super::store::Provider;

/// What a request is for.
///
/// The labels, descriptions and recommendation hints that go with these live in
/// the console; this is the routing key alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Workload {
    /// Direct conversational back-and-forth.
    Chat,
    /// Deep thinking: the main chat agent and heavier answer synthesis.
    Reasoning,
    /// Sub-agent runners and tool loops.
    Agentic,
    /// Code generation and refactor passes. An **alias** of [`Workload::Agentic`].
    Coding,
    /// Image understanding.
    Vision,
}

/// Every workload that has a row of its own — one per abstract tier.
///
/// [`Workload::Coding`] is deliberately absent: it is an alias, and a row for it
/// would write the agentic tier's route under a second name.
pub const ROUTABLE_WORKLOADS: &[Workload] = &[
    Workload::Chat,
    Workload::Reasoning,
    Workload::Agentic,
    Workload::Vision,
];

impl Workload {
    /// The abstract tier this workload routes through.
    pub fn tier(self) -> &'static str {
        match self {
            Self::Chat => "chat-v1",
            Self::Reasoning => "reasoning-v1",
            // Coding shares the agentic tier — see the module header on why
            // that is an alias rather than a fifth row.
            Self::Agentic | Self::Coding => "agentic-v1",
            Self::Vision => "vision-v1",
        }
    }

    /// Whether this workload reads another's route rather than owning one.
    pub fn is_alias(self) -> bool {
        matches!(self, Self::Coding)
    }

    /// The workload for a tier name, if it is one of ours.
    pub fn from_tier(tier: &str) -> Option<Self> {
        ROUTABLE_WORKLOADS
            .iter()
            .copied()
            .find(|w| w.tier() == tier.trim())
    }

    /// The stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Reasoning => "reasoning",
            Self::Agentic => "agentic",
            Self::Coding => "coding",
            Self::Vision => "vision",
        }
    }

    /// The name this workload has on screen.
    ///
    /// A tier id is an internal name and it leaked into two operator-facing
    /// sentences — the disable note ("agentic-v1, vision-v1 are parked") and the
    /// orphan banner. Every other sentence on both tabs says "Agentic", so an
    /// operator had to learn a second name for the same row in order to read a
    /// warning about it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Reasoning => "Reasoning",
            Self::Agentic => "Agentic",
            Self::Coding => "Coding",
            Self::Vision => "Vision",
        }
    }
}

/// What a tier is called on screen, or the raw id when it is not one of ours.
pub fn tier_label(tier: &str) -> String {
    Workload::from_tier(tier).map_or_else(|| tier.to_string(), |w| w.label().to_string())
}

/// What one routing row points at.
///
/// [`ProviderRef::Managed`] and [`ProviderRef::Default`] are different states on
/// purpose: one is a choice, the other is an absence. Collapsing them loses the
/// ability to say "this row is deliberately managed" as distinct from "this row
/// was never set", and those two want different copy and different behaviour
/// when a provider is removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderRef {
    /// Explicitly the managed brain.
    Managed,
    /// Unset. Falls through to the primary, never to a sibling.
    Default,
    /// A cloud or custom provider, addressed by slug.
    Cloud {
        /// The provider's slug.
        provider_slug: String,
        /// The model id to send, if pinned.
        model: Option<String>,
    },
    /// A local runtime. Carries **no slug**, which is why its scrub rule
    /// differs.
    Local {
        /// The model id to send, if pinned.
        model: Option<String>,
    },
    /// A CLI login. Carries no slug either.
    ClaudeCode {
        /// The model id to send, if pinned.
        model: Option<String>,
    },
}

impl ProviderRef {
    /// Parses the hand-editable string grammar.
    ///
    /// An operator reads and edits routes as `reasoning-v1 -> acme:gpt-5`, so
    /// the grammar has to survive a round trip through a human. An empty string
    /// is [`ProviderRef::Default`] — an absence, not a parse failure, because
    /// "nothing set here" is a legitimate value an operator writes by deleting.
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if raw.is_empty() || raw == "default" {
            return Self::Default;
        }
        if raw == "managed" {
            return Self::Managed;
        }
        let (slug, model) = match raw.split_once(':') {
            Some((slug, model)) => (
                slug.trim(),
                Some(model.trim().to_string()).filter(|m| !m.is_empty()),
            ),
            None => (raw, None),
        };
        match slug {
            "claude-code" => Self::ClaudeCode { model },
            "local" => Self::Local { model },
            _ => Self::Cloud {
                provider_slug: slug.to_string(),
                model,
            },
        }
    }

    /// The string form, in the same grammar [`ProviderRef::parse`] reads.
    ///
    /// This is the **persisted** shape, deliberately: a route is stored as the
    /// text an operator would type, so the stored value and the value they
    /// hand-edit are the same value. Storing a tagged enum instead would make
    /// the console's grammar a presentation layer over a second representation,
    /// and the two would have to be kept in step forever.
    ///
    /// [`ProviderRef::Default`] renders as the empty string — an absence, which
    /// is why the writer drops those rather than storing `""`.
    pub fn to_route_string(&self) -> String {
        match self {
            Self::Default => String::new(),
            Self::Managed => "managed".to_string(),
            Self::Cloud {
                provider_slug,
                model,
            } => match model {
                Some(model) => format!("{provider_slug}:{model}"),
                None => provider_slug.clone(),
            },
            Self::Local { model } => match model {
                Some(model) => format!("local:{model}"),
                None => "local".to_string(),
            },
            Self::ClaudeCode { model } => match model {
                Some(model) => format!("claude-code:{model}"),
                None => "claude-code".to_string(),
            },
        }
    }

    /// The slug this ref names, when it names one at all.
    ///
    /// `None` for local and CLI refs is not an oversight — it is the fact the
    /// three scrub rules exist to work around.
    pub fn slug(&self) -> Option<&str> {
        match self {
            Self::Cloud { provider_slug, .. } => Some(provider_slug),
            _ => None,
        }
    }

    /// The pinned model id, if any.
    pub fn model(&self) -> Option<&str> {
        match self {
            Self::Cloud { model, .. } | Self::Local { model } | Self::ClaudeCode { model } => {
                model.as_deref()
            }
            Self::Managed | Self::Default => None,
        }
    }
}

/// Tier name → what that tier routes through.
pub type Routes = BTreeMap<String, ProviderRef>;

/// What resolving a workload produced.
///
/// An enum rather than an `Option` because three of these outcomes are not
/// "nothing" — they are three different things to say to an operator, and
/// collapsing any pair of them tells someone to do something that will not help.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution<'a> {
    /// A provider was named and found.
    Resolved {
        /// The provider serving this workload.
        provider: &'a Provider,
        /// The model id the route pinned, if any.
        model: Option<String>,
    },
    /// Deliberately the managed brain.
    Managed,
    /// Unset. Falls through to the primary — never to a sibling's provider.
    Primary,
    /// The route names a provider this company does not hold. Fail closed.
    Missing {
        /// The workload whose route is broken.
        workload: Workload,
        /// The slug that resolved to nothing.
        slug: String,
    },
    /// The route names a provider that is switched off.
    Disabled {
        /// The workload whose route is parked.
        workload: Workload,
        /// The provider that is off.
        slug: String,
    },
}

/// The providers a company can actually route to — enabled only, in list order.
pub fn routing_targets(providers: &[Provider]) -> Vec<&Provider> {
    providers.iter().filter(|p| p.enabled).collect()
}

/// The provider an unset workload falls through to.
///
/// **The marked default, and only then list order.** `marked` is the slug the
/// company has said is its default
/// ([`load_default_slug`](super::store::load_default_slug)); `None` is every
/// company that has never said, which is every company that existed before the
/// marker did.
///
/// The fallback is first-enabled, which is what this did unconditionally — and
/// entry zero sorts first in
/// [`list_providers`](super::store::list_providers), so a company that had one
/// provider before any of this existed keeps sending its unset workloads exactly
/// where it always did. No migration, no backfill.
///
/// ## Why the marker exists at all
///
/// First-enabled answers "which provider is my default" by **list order**. Add
/// three providers, delete the first, and the default silently becomes the
/// second — with nothing on screen having changed to say so, and the company's
/// unrouted spend moving to a different account. An explicit marker makes that a
/// thing the operator said rather than a thing that happened.
///
/// ## Two ways the marker can be stale, and one answer to both
///
/// A marked provider may be **disabled** or **gone**. Both are handled the same
/// way — fall back to first-enabled — rather than by refusing to resolve, and
/// deliberately: the routes the operator *did* set fail closed when they name a
/// provider that is missing or off ([`Resolution::Missing`],
/// [`Resolution::Disabled`]), because those are choices with a workload attached.
/// An unset workload has no such choice behind it, and the alternative to
/// falling back is a company that cannot think at all because of a marker it
/// forgot about. The write paths keep this rare rather than relying on it: both
/// disabling and deleting clear the marker in the same operation.
///
/// `None` means nothing enabled resolves, and every caller reads that as the
/// managed brain — which is always available and is the right fallback.
pub fn primary<'a>(providers: &'a [Provider], marked: Option<&str>) -> Option<&'a Provider> {
    if let Some(marked) = marked.map(str::trim).filter(|s| !s.is_empty())
        && let Some(provider) = providers.iter().find(|p| p.slug == marked && p.enabled)
    {
        return Some(provider);
    }
    providers.iter().find(|p| p.enabled)
}

/// Which provider serves `workload`.
///
/// Reads the route for the workload's **tier**, so an alias
/// ([`Workload::Coding`]) resolves through the route its tier owns rather than
/// through one of its own.
pub fn provider_for_workload<'a>(
    workload: Workload,
    routes: &Routes,
    providers: &'a [Provider],
) -> Resolution<'a> {
    let route = routes
        .get(workload.tier())
        .cloned()
        .unwrap_or(ProviderRef::Default);
    match route {
        ProviderRef::Default => Resolution::Primary,
        ProviderRef::Managed => Resolution::Managed,
        ProviderRef::Cloud {
            ref provider_slug,
            ref model,
        } => match providers.iter().find(|p| &p.slug == provider_slug) {
            None => Resolution::Missing {
                workload,
                slug: provider_slug.clone(),
            },
            Some(p) if !p.enabled => Resolution::Disabled {
                workload,
                slug: provider_slug.clone(),
            },
            Some(provider) => Resolution::Resolved {
                provider,
                model: model.clone(),
            },
        },
        // A slug-less ref names a category, not a record. It resolves to the
        // first enabled provider of that category — and to `Missing` when there
        // is none, rather than falling through to the primary, because the
        // operator did choose something here.
        ProviderRef::Local { ref model } => {
            resolve_by_category(workload, providers, Category::Local, model, "local")
        }
        ProviderRef::ClaudeCode { ref model } => {
            resolve_by_category(workload, providers, Category::Cli, model, "claude-code")
        }
    }
}

fn resolve_by_category<'a>(
    workload: Workload,
    providers: &'a [Provider],
    category: Category,
    model: &Option<String>,
    name: &str,
) -> Resolution<'a> {
    let of_category: Vec<&Provider> = providers
        .iter()
        .filter(|p| catalogue::category_of(&p.kind) == category)
        .collect();
    match of_category.iter().find(|p| p.enabled) {
        Some(provider) => Resolution::Resolved {
            provider,
            model: model.clone(),
        },
        None if of_category.is_empty() => Resolution::Missing {
            workload,
            slug: name.to_string(),
        },
        None => Resolution::Disabled {
            workload,
            slug: name.to_string(),
        },
    }
}

/// The routing modes, inferred from the routes.
///
/// **Never stored.** A mode field would be a fifth thing that can disagree with
/// the four routes, and the routes are the truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutingMode {
    /// Every row is managed or unset, **and managed can answer**.
    Managed,
    /// Every row names the same provider and model.
    Own,
    /// Anything else.
    Advanced,
    /// Every row is managed or unset and **managed resolves to nothing**.
    ///
    /// The company has no mode it can use. This is not a fourth thing the
    /// operator can pick — it is the absence of a usable choice, and it exists
    /// so the console can render that absence rather than a selected row the
    /// same card calls Not set up.
    Unset,
}

/// Whether any routable workload is **explicitly** pointed at the managed tier.
///
/// The one question boot-time brain selection had no way to ask. A company that
/// routes its tiers to `managed` has configured its inference — but it has
/// configured it in a place neither half of
/// [`resolve_effective_scoped`](super::resolve_effective_scoped)'s original two
/// branches looks: not in the provider list (managed has no record there), and
/// not in the legacy runtime blob or the manifest. So `RuntimeBuilder::build`
/// saw "nothing configured", handed the company the offline echo brain, and a
/// restart changed nothing because a fresh boot ran the identical computation.
///
/// **[`ProviderRef::Default`] deliberately does not count.** An unset row means
/// the operator chose nothing, and [`provider_for_workload`] maps it to
/// [`Resolution::Primary`] — the provider list, then the legacy chain, both of
/// which the caller has already tried by the time it asks this. Counting an
/// absence as a choice here would report every company configured, which is the
/// mirror image of the bug and strictly worse: it would take companies off the
/// echo brain that genuinely have nothing to think with.
///
/// This is deliberately weaker than [`infer_routing_mode`]'s Managed rule, which
/// also accepts all-unset. That function answers "which mode is this table
/// describing"; this one answers "would a turn actually reach managed", and only
/// an explicit row does that.
pub fn any_route_is_managed(routes: &Routes) -> bool {
    ROUTABLE_WORKLOADS
        .iter()
        .any(|w| matches!(routes.get(w.tier()), Some(ProviderRef::Managed)))
}

/// Which mode the current routes describe, given whether managed can answer.
///
/// ## Why this takes a second argument
///
/// The rule this ports — *every row managed or unset → Managed* — is faithful to
/// openhuman, where it is also true: they run the managed backend, so managed is
/// genuinely always on. **Here managed needs a credential and can resolve to
/// nothing**, and a table of unset rows on such a company does not resolve to
/// managed at all: [`provider_for_workload`] maps an unset row to
/// [`Resolution::Primary`], which is the first enabled provider. So the screen
/// said Managed while the turn went to the operator's own key — and on a company
/// whose only provider had just been added with no per-tier model, that turn was
/// the reported `404 model: agentic-v1`.
///
/// The principle, stated once: **the inferred default must be a mode the company
/// can actually use.** An inferred Managed on a company where managed does not
/// resolve is not a mode, it is a contradiction, and every symptom in the report
/// falls out of it.
///
/// Only the *inference* changes. The mode is still a pure function of the table
/// plus one fact about the company, still never stored, and an operator who has
/// explicitly chosen managed still sees managed — they just see [`Self::Unset`]
/// when that choice has nothing behind it, which is what is true.
pub fn infer_routing_mode(routes: &Routes, managed_resolves: bool) -> RoutingMode {
    let refs: Vec<ProviderRef> = ROUTABLE_WORKLOADS
        .iter()
        .map(|w| {
            routes
                .get(w.tier())
                .cloned()
                .unwrap_or(ProviderRef::Default)
        })
        .collect();
    if refs
        .iter()
        .all(|r| matches!(r, ProviderRef::Managed | ProviderRef::Default))
    {
        return if managed_resolves {
            RoutingMode::Managed
        } else {
            RoutingMode::Unset
        };
    }
    let first = &refs[0];
    if refs.iter().all(|r| r == first) {
        return RoutingMode::Own;
    }
    RoutingMode::Advanced
}

/// Resets every route orphaned by removing `removed`, given what `remaining`
/// holds afterwards. Returns the tiers that were reset, so the console can say
/// which ones moved rather than leaving the operator to notice.
///
/// ## Three rules, because only one kind of ref carries a slug
///
/// * **Cloud and custom** — matched precisely by slug. The easy case.
/// * **CLI logins** — their refs carry no slug. Without special handling,
///   disconnecting one leaves workloads pinned to `claude-code:<model>`, which
///   the resolver still honours, so turns keep using a provider the operator
///   removed.
/// * **Local runtimes** — no slug either, and one more wrinkle: a `local` ref is
///   only definitively orphaned once **no** local runtime remains. Scrubbing on
///   the first removal would unpin a route that a second local runtime still
///   serves. Before this rule existed the local case was silently a no-op.
pub fn scrub_removed(
    routes: &mut Routes,
    removed: &Provider,
    remaining: &[Provider],
) -> Vec<String> {
    let category = catalogue::category_of(&removed.kind);
    // **Enabled, not merely present.** A slugless `local:` route survives only
    // while something in that category can still serve it, and a switched-off
    // runtime cannot: `provider_for_workload` looks for an *enabled* target and
    // fails the workload closed when it finds none. Counting a disabled row as
    // a survivor left the route pinned and the turn hard-failing, instead of
    // resetting to the primary the way the rule above says it should.
    let category_survives = remaining
        .iter()
        .filter(|p| p.enabled)
        .any(|p| catalogue::category_of(&p.kind) == category);

    let mut reset = Vec::new();
    for (tier, route) in routes.iter_mut() {
        // The three rules, in [`route_names`], shared with `routes_served_by`
        // and `orphaned_routes`. Two of those three used to hold a third of this
        // rule each, and both were wrong in the same direction.
        if route_names(route, removed, category, category_survives) {
            *route = ProviderRef::Default;
            reset.push(tier.clone());
        }
    }
    reset
}

/// The tiers whose route `provider` serves, by [`scrub_removed`]'s three rules.
///
/// ## Why this exists, rather than a slug comparison at the call site
///
/// `parked_tiers` on the disable path did compare slugs — `route.slug() ==
/// Some(provider.slug)` — and [`ProviderRef::slug`] is `None` for a `local` or
/// `claude-code` ref. So disabling the only Ollama runtime parked every `local:`
/// route while the note said **"Nothing was routed through it."** A false
/// statement in the one sentence whose whole job is to be true.
///
/// `scrub_removed` gets this right with three rules and `orphaned_routes` got it
/// wrong the same way. One matcher now, used by all three, so the next surface
/// that needs the question asked cannot reimplement a third of the answer.
///
/// `alternatives` is what would still serve after `provider` goes: the remaining
/// providers for a removal, the *still-enabled* ones for a disable. A slug-less
/// ref names a category, so it is only orphaned once nothing of that category is
/// left to serve it.
pub fn routes_served_by(
    routes: &Routes,
    provider: &Provider,
    alternatives: &[Provider],
) -> Vec<String> {
    let category = catalogue::category_of(&provider.kind);
    let survives = alternatives
        .iter()
        .any(|p| catalogue::category_of(&p.kind) == category);
    routes
        .iter()
        .filter(|(_, route)| route_names(route, provider, category, survives))
        .map(|(tier, _)| tier.clone())
        .collect()
}

/// Whether `route` is served by `provider` and by nothing else.
///
/// **A slug match is decisive, whatever the category.** This used to also
/// require `category == Cloud`, and the two rules then never met for a local
/// runtime: `ollama:llama3` parses as a `Cloud` ref because it carries a slug,
/// while `category_of("ollama")` is `Local` — so the cloud arm refused it on
/// category and the local arm never saw it, because that arm only matches the
/// slug-less `local` ref. Removing Ollama left every row pointing at it, and the
/// routing table then refused to save at all: `put_routes` fails closed on a
/// route naming a provider nobody holds, so the operator could not re-save their
/// own routing until they had changed every row by hand.
///
/// A slug is unique per company, so naming one that is going away is orphaned by
/// definition. The category never added anything.
fn route_names(
    route: &ProviderRef,
    provider: &Provider,
    category: Category,
    category_survives: bool,
) -> bool {
    match route {
        ProviderRef::Cloud { provider_slug, .. } => provider_slug == &provider.slug,
        ProviderRef::Local { .. } => category == Category::Local && !category_survives,
        ProviderRef::ClaudeCode { .. } => category == Category::Cli && !category_survives,
        ProviderRef::Managed | ProviderRef::Default => false,
    }
}

/// Every route naming a provider this company does not hold.
///
/// The second, independent mechanism behind the same invariant as
/// [`scrub_removed`]. Two mechanisms for one rule because the UI path can be
/// bypassed — by a hand-edited config or an older build — and an unresolvable
/// route must be reported at load rather than discovered mid-turn.
pub fn orphaned_routes(routes: &Routes, providers: &[Provider]) -> Vec<(String, String)> {
    let holds = |category: Category| {
        providers
            .iter()
            .any(|p| catalogue::category_of(&p.kind) == category)
    };
    routes
        .iter()
        .filter_map(|(tier, route)| match route {
            ProviderRef::Cloud { provider_slug, .. } => {
                (!providers.iter().any(|p| &p.slug == provider_slug))
                    .then(|| (tier.clone(), provider_slug.clone()))
            }
            // **Slug-less refs used to be skipped entirely.** `route.slug()?`
            // early-returned on them, so a `local:` route on a company holding
            // no local runtime was reported by nothing at all — while the turn
            // refused it mid-flight with `Resolution::Missing`. The whole point
            // of this second mechanism is that an unresolvable route is reported
            // at load rather than discovered in a turn, and for two of the five
            // ref shapes it never was. Third instance of the same bug shape, and
            // the last one.
            ProviderRef::Local { .. } => {
                (!holds(Category::Local)).then(|| (tier.clone(), "local".to_string()))
            }
            ProviderRef::ClaudeCode { .. } => {
                (!holds(Category::Cli)).then(|| (tier.clone(), "claude-code".to_string()))
            }
            ProviderRef::Managed | ProviderRef::Default => None,
        })
        .collect()
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
