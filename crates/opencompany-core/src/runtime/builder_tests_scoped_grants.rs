use super::tests_core::*;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

/// A catch-all company grant must not satisfy opt-in namespaces that
/// carry billing, tenant credentials, third-party source access, or
/// workspace writes. Workspace writes protect operator-owned
/// guidance and therefore require an explicit `workspace` or
/// `workspace.write` grant, just like the special namespaces below.
#[test]
fn wildcard_does_not_cover_special_namespaces() {
    let allow = strings(&["*"]);
    for grant in [
        "media",
        "media.*",
        "media.image",
        "composio",
        "composio.*",
        "composio.gmail",
        "chargebee",
        "chargebee.*",
        "chargebee.read",
        "hosting",
        "hosting.*",
        "hosting.deploy",
        "paypal",
        "paypal.*",
        "paypal.wallet",
        "search",
        "search.*",
        "search.web",
        "mcp:*",
        "mcp*",
        "mcp_registry",
        "mcp_registry.*",
        "mcp_registry.notion",
    ] {
        assert!(
            !allow_covers(&allow, grant),
            "catch-all must not cover opt-in grant `{grant}`"
        );
    }
    assert!(!allow_covers(&allow, "workspace.write"));
    assert!(
        !allow_covers(&allow, "workspace"),
        "the bare `workspace` grant is a write grant to the wiring predicate, \
         so a catch-all must not cover it"
    );
    assert!(allow_covers(&allow, "workspace.read"));
    assert!(allow_covers(&allow, "docs.read"));
}

/// Explicit special grants still cover the corresponding setup belt —
/// bare namespaces and sub-grant requests alike, matching the `_explicit`
/// wiring predicates that accept both shapes.
#[test]
fn explicit_special_grants_cover_their_namespaces() {
    let allow = strings(&[
        "media",
        "composio",
        "chargebee",
        "hosting",
        "paypal",
        "search",
        "mcp:*",
        "mcp_registry",
        "workspace",
    ]);
    for grant in [
        "media",
        "media.*",
        "media.image",
        "composio",
        "composio.*",
        "composio.gmail",
        "chargebee",
        "chargebee.*",
        "chargebee.read",
        "hosting",
        "hosting.*",
        "hosting.deploy",
        "paypal",
        "paypal.*",
        "paypal.wallet",
        "search",
        "search.*",
        "search.web",
        "mcp:*",
        "mcp_registry",
        "mcp_registry.*",
        "mcp_registry.notion",
        "workspace",
        "workspace.write",
    ] {
        assert!(
            allow_covers(&allow, grant),
            "explicit grant must cover `{grant}`"
        );
    }
}

/// The workspace write grant does not cover a read-glob request, in
/// either direction of the asymmetry the manifest pair documents.
///
/// `workspace` is a *write* grant to the wiring predicate
/// ([`grants_workspace_write_explicit`]), while a `workspace.*` request
/// strips to `workspace.` and falls to the generic matcher, where an
/// unstarred grant matches only itself — so `allow_covers` answers
/// false, and `agent_effective_grants` drops the request from the
/// belt. The console's `companyCovers` mirror pins the same pair.
#[test]
fn a_write_grant_does_not_cover_a_read_glob_request() {
    assert!(!allow_covers(&strings(&["workspace"]), "workspace.*"));
    assert!(allow_covers(
        &strings(&["workspace", "workspace.*"]),
        "workspace.*"
    ));
    assert!(!allow_covers(&strings(&["*"]), "workspace.write"));
}

/// A bare opt-in namespace grant covers its sub-grant requests, again
/// matching the wiring predicate: `search.web` in the effective grants
/// satisfies `grants_search_explicit` exactly as `search` does, so the
/// request must not be dropped at the allow-list. The ordinary namespaces
/// keep the exact-match rule, which is why this test sits beside the two
/// opt-in ones rather than being folded into the generic matcher.
#[test]
fn a_bare_opt_in_grant_covers_its_sub_grants() {
    assert!(allow_covers(&strings(&["search"]), "search.*"));
    assert!(allow_covers(&strings(&["search"]), "search.web"));
    assert!(allow_covers(&strings(&["media"]), "media.image"));
    assert!(allow_covers(&strings(&["chargebee"]), "chargebee.read"));
    assert!(allow_covers(
        &strings(&["mcp_registry"]),
        "mcp_registry.notion"
    ));
    assert!(
        !allow_covers(&strings(&["docs"]), "docs.read"),
        "ordinary namespaces keep the unstarred-grant exact-match rule"
    );
}

/// A request glob whose `*` is glued to an explicit opt-in namespace
/// (`search*`, `workspace.write*`) is stored *verbatim* by the write
/// path, and the wiring predicates reject the glued spelling —
/// `grants_search_explicit` wants `search` or a `search.`-descendant,
/// `grants_workspace_write_explicit` wants the two exact tokens. So even
/// a company that holds the namespace must not have `allow_covers`
/// promise a grant that will silently fail to wire; the console's
/// `companyCovers` mirror pins the same rule.
#[test]
fn a_glued_star_opt_in_request_is_not_covered() {
    let allow = strings(&[
        "search",
        "workspace",
        "media",
        "composio",
        "chargebee",
        "hosting",
        "paypal",
        "mcp:*",
        "mcp_registry",
    ]);
    for grant in [
        "search*",
        "workspace*",
        "workspace.write*",
        "media*",
        "composio*",
        "chargebee*",
        "hosting*",
        "paypal*",
        "mcp*",
        "mcp_registry*",
    ] {
        assert!(
            !allow_covers(&allow, grant),
            "glued-star `{grant}` must not be covered"
        );
    }
}

/// The separator-broken opt-in spellings — the ones the wiring
/// predicates actually accept — stay covered even when they end in a
/// `*`: `search.web*` strips to a `search.`-descendant that
/// `grants_search_explicit` accepts verbatim, `workspace.write` is an
/// exact write token, and `mcp:notion*` is a colon-scoped prefix.
#[test]
fn a_separator_broken_opt_in_request_stays_covered() {
    let allow = strings(&["search", "workspace", "media", "mcp:*", "mcp_registry"]);
    assert!(allow_covers(&allow, "search.*"));
    assert!(allow_covers(&allow, "search.web*"));
    assert!(allow_covers(&allow, "workspace.write"));
    assert!(allow_covers(&allow, "media.*"));
    assert!(allow_covers(&allow, "media.image*"));
    assert!(allow_covers(&allow, "mcp:notion*"));
    assert!(allow_covers(&allow, "mcp_registry.notion*"));
}

/// Runs the three-level narrowing over `&str` slices, so each case below
/// reads as the table row it is.
fn scope(company: &[&str], desks: &[&[&str]], agent: &[&str]) -> Vec<String> {
    let company = strings(company);
    let desk_owned: Vec<Vec<String>> = desks.iter().map(|d| strings(d)).collect();
    let desk_refs: Vec<&[String]> = desk_owned.iter().map(Vec::as_slice).collect();
    // An empty per-agent slice is "no line of its own" → `None` (inherit)
    // since #1804, NOT `Some(&[])` (which is a deny-all). A test that
    // wants the deny-all case calls `agent_scoped_grants` directly.
    let agent = strings(agent);
    let agent_tools = (!agent.is_empty()).then_some(agent.as_slice());
    agent_scoped_grants(&company, &desk_refs, agent_tools)
}

/// No desk and no per-agent list: the company grant passes through
/// untouched. This is the shape every pre-existing manifest has, so it
/// is the case that must be byte-identical to the old behaviour.
#[test]
fn empty_levels_pass_through() {
    assert_eq!(scope(&["*", "search"], &[], &[]), ["*", "search"]);
    assert_eq!(scope(&["*", "search"], &[&[]], &[]), ["*", "search"]);
    assert_eq!(scope(&["*", "search"], &[&[], &[]], &[]), ["*", "search"]);
}

/// The #1804 contract inversion at the resolver: an **explicit empty**
/// agent grant (`Some(&[])`) is a deny-all — it resolves to nothing,
/// the opposite of `None` (inherit), even under a wide-open company and
/// no desk ceiling.
#[test]
fn an_explicit_empty_agent_grant_is_a_deny_all() {
    let company = strings(&["*", "search"]);
    // `None` inherits the whole company grant…
    assert_eq!(agent_scoped_grants(&company, &[], None), ["*", "search"]);
    // …but an explicit empty list holds nothing.
    assert_eq!(
        agent_scoped_grants(&company, &[], Some(&[])),
        Vec::<String>::new()
    );
}

/// The middle level does the work the feature exists for: a department
/// ceiling narrows every member without touching any member's own line.
#[test]
fn a_desk_ceiling_narrows_its_members() {
    assert_eq!(scope(&["*", "search"], &[&["docs.*"]], &[]), ["docs.*"]);
}

/// And the agent narrows further still.
#[test]
fn an_agent_narrows_below_its_desk() {
    assert_eq!(
        scope(&["*", "search"], &[&["docs.*", "web"]], &["docs.*"]),
        ["docs.*"]
    );
}

/// Desks union, so joining a second desk *adds* capability rather than
/// removing it. Intersecting would make adding someone to a desk break
/// the job they already did.
#[test]
fn desks_combine_by_union() {
    assert_eq!(
        scope(&["*", "search"], &[&["docs.*"], &["web"]], &[]),
        ["docs.*", "web"]
    );
}

/// The documented sharp edge, asserted so it cannot change silently: a
/// desk with no ceiling narrows nothing, so an agent on both a
/// restricted and an unrestricted desk ends up unrestricted.
///
/// Asserted by **coverage** rather than by list equality. The union
/// leaves the restricted desk's `docs.*` in the result beside the open
/// desk's `*`, which is redundant but not wrong — `*` already covers it
/// — and pinning the exact list here would be asserting the shape of the
/// bookkeeping instead of the capability it resolves to.
#[test]
fn an_unceilinged_desk_widens_the_union_back_to_the_company_grant() {
    let company = ["*", "search"];
    let resolved = scope(&company, &[&["docs.*"], &[]], &[]);
    for grant in company {
        assert!(
            allow_covers(&resolved, grant),
            "`{grant}` must survive an open desk: {resolved:?}"
        );
    }
}

/// The invariant that makes this safe to add: no path through the
/// narrowing can yield a grant the company did not already allow. A desk
/// ceiling naming something outside `[tools].allow` cannot widen.
#[test]
fn a_desk_can_never_widen_past_the_company_grant() {
    // `search` is deliberately not in the company allow-list, and `*`
    // never confers it.
    assert_eq!(
        scope(&["docs.*"], &[&["search", "shell"]], &[]),
        Vec::<String>::new()
    );
    // Nor can the agent reach past a desk that did not grant it.
    assert_eq!(
        scope(&["*"], &[&["docs.*"]], &["shell"]),
        Vec::<String>::new()
    );
}

/// Adding the desk level must not disturb the two-level answer for a
/// company whose desks declare nothing — the regression that would hit
/// every shipped company at once.
#[test]
fn matches_the_two_level_resolver_when_no_desk_has_a_ceiling() {
    for (company, agent) in [
        (&["*", "media"][..], &[][..]),
        (&["*", "media"][..], &["docs.*"][..]),
        (&["docs.*", "web"][..], &["web"][..]),
        (&["docs.*"][..], &["shell"][..]),
    ] {
        let agent_owned = strings(agent);
        let agent_tools = (!agent_owned.is_empty()).then_some(agent_owned.as_slice());
        assert_eq!(
            agent_scoped_grants(&strings(company), &[&[], &[]], agent_tools),
            agent_effective_grants(&strings(company), agent_tools),
            "company={company:?} agent={agent:?}"
        );
    }
}
