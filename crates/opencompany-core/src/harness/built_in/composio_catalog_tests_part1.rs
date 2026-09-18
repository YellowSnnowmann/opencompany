use super::*;

/// The headline invariant: a listing too big to render says so, says how
/// much it dropped, and names the argument that makes the next call
/// smaller. Without this the agent has no reason to change its request.
#[test]
fn an_oversized_listing_says_it_was_cut_and_how_to_narrow_it() {
    let actions = catalogue("github", 300);
    let out = render(&actions, &request("", Detail::Names, &["github"]));

    assert!(out.contains("300 available"), "{out}");
    assert!(
        out.contains("TRUNCATED"),
        "the cut must be announced: {out}"
    );
    assert!(
        out.contains("100 more matching actions were not shown"),
        "the notice must count what it dropped: {out}"
    );
    assert!(
        out.contains("`search`") && out.contains("`toolkits`") && out.contains("`limit`"),
        "the notice must name the arguments that narrow it: {out}"
    );
    assert!(
        out.contains("Do NOT repeat this call unchanged"),
        "the notice must break the retry loop explicitly: {out}"
    );
}

/// Names mode prefers a *complete* list of slugs over a partial list with
/// prose. A 150-action toolkit does not fit with descriptions (they alone
/// run past the byte budget) but fits comfortably without them — so the
/// agent gets every slug it might need, plus the pointer to `detail:
/// "schemas"` for the one it picks.
#[test]
fn names_mode_drops_descriptions_rather_than_actions() {
    let actions = catalogue("github", 150);
    let out = render(&actions, &request("", Detail::Names, &["github"]));

    assert!(out.contains("showing 150"), "{out}");
    assert!(
        !out.contains("TRUNCATED"),
        "nothing had to be dropped: {out}"
    );
    assert!(out.contains("Descriptions omitted"), "{out}");
    assert!(
        out.contains("GITHUB_ACTION_149"),
        "the last slug must be present: {out}"
    );
    assert!(
        !out.contains("Long upstream prose"),
        "the dense fallback must not carry descriptions: {out}"
    );
    assert!(out.len() < 16 * 1024, "{} bytes", out.len());
}

/// The bound is real, not advisory — and it is a whole-entry bound, so the
/// dropped count in the notice is exact rather than approximate.
#[test]
fn rendering_stays_inside_its_byte_budget_and_never_splits_an_entry() {
    // 300 schema blocks at ~500 bytes each is far past the budget.
    let actions = catalogue("gmail", 300);
    let mut request = request("", Detail::Schemas, &["gmail"]);
    request.limit = SCHEMAS_MAX_LIMIT;
    let out = render(&actions, &request);

    assert!(
        out.len() < 16 * 1024,
        "the rendered listing must stay under the harness tool-result cap: {} bytes",
        out.len()
    );
    // Every block that IS present is complete: its trailing newline pair and
    // its `parameters:` line both survived.
    let blocks = out.matches("## GMAIL_ACTION_").count();
    assert_eq!(
        out.matches("parameters: {").count(),
        blocks,
        "an entry was cut in half: {out}"
    );
    assert!(blocks >= 1, "at least one schema must be delivered: {out}");
}

/// A listing that fits is not decorated with a truncation notice — the
/// notice has to mean something.
#[test]
fn a_complete_listing_carries_no_truncation_notice() {
    let actions = catalogue("linear", 12);
    let out = render(&actions, &request("", Detail::Names, &["linear"]));
    assert!(out.contains("showing 12"), "{out}");
    assert!(!out.contains("TRUNCATED"), "{out}");
}

/// Narrowing is generic: the same words find the one action in a
/// hundred-slug catalogue for any toolkit, with no per-provider table.
#[test]
fn search_narrows_to_one_action_on_any_toolkit() {
    let mut actions = catalogue("github", 120);
    actions.push(CatalogAction {
        slug: "GITHUB_LIST_ISSUES".to_string(),
        toolkit: "github".to_string(),
        description: "List issues in a repository.".to_string(),
        parameters: Some(json!({"type": "object", "properties": {"repo": {"type": "string"}}})),
    });
    let mut other = catalogue("notion", 130);
    other.push(CatalogAction {
        slug: "NOTION_SEARCH_PAGES".to_string(),
        toolkit: "notion".to_string(),
        description: "Search pages in the workspace.".to_string(),
        parameters: Some(json!({"type": "object", "properties": {"query": {"type": "string"}}})),
    });

    let github = render(
        &actions,
        &request("list issues", Detail::Schemas, &["github"]),
    );
    assert!(github.contains("GITHUB_LIST_ISSUES"), "{github}");
    assert!(
        github.contains("\"repo\""),
        "the schema must be present: {github}"
    );
    assert!(
        !github.contains("TRUNCATED"),
        "one match is not a cut: {github}"
    );

    let notion = render(
        &other,
        &request("search pages", Detail::Schemas, &["notion"]),
    );
    assert!(notion.contains("NOTION_SEARCH_PAGES"), "{notion}");
    assert!(notion.contains("\"query\""), "{notion}");
}

/// An exact slug read back from the names view resolves to that one schema
/// — the second half of the two-step, and the reason `_` is a search
/// separator.
#[test]
fn an_exact_slug_pasted_into_search_returns_that_schema() {
    let mut actions = catalogue("slack", 90);
    actions.push(CatalogAction {
        slug: "SLACK_POST_MESSAGE".to_string(),
        toolkit: "slack".to_string(),
        description: "Post a message to a channel.".to_string(),
        parameters: Some(json!({"type": "object", "properties": {"channel": {"type": "string"}}})),
    });
    let out = render(
        &actions,
        &request("SLACK_POST_MESSAGE", Detail::Schemas, &["slack"]),
    );
    assert!(out.contains("SLACK_POST_MESSAGE"), "{out}");
    assert!(out.contains("\"channel\""), "{out}");
    assert!(out.contains("showing 1"), "{out}");
}

/// No match is reported as a fact, with the words that produced it, and
/// with an explicit instruction not to invent a slug.
#[test]
fn no_match_is_stated_plainly_rather_than_returned_empty() {
    let actions = catalogue("gmail", 40);
    let out = render(
        &actions,
        &request("quantum teleport", Detail::Names, &["gmail"]),
    );
    assert!(out.contains("nothing matched `quantum teleport`"), "{out}");
    assert!(
        out.contains("40 returned"),
        "the real total is stated: {out}"
    );
    assert!(out.contains("Do NOT guess a slug"), "{out}");
    assert!(!out.contains("TRUNCATED"), "nothing was cut: {out}");
}

/// A curated listing must not present itself as the full catalogue.
///
/// An unnarrowed BYOK browse asks Composio for featured actions only, so
/// the count that comes back is a preview. Calling it "available" is what
/// told an agent taking an inventory of GitHub that ~50 rows were
/// everything it could do (codex on tinyhumansai/opencompany#2153).
#[test]
fn a_curated_listing_says_it_is_a_preview() {
    let actions = catalogue("github", 50);
    let mut curated = request("", Detail::Names, &["github"]);
    curated.curated = true;
    let out = render(&actions, &curated);

    assert!(out.contains("featured"), "curation must be named: {out}");
    assert!(
        out.contains("not the full catalogue"),
        "the preview must say what it is not: {out}"
    );
    assert!(
        out.contains("search"),
        "the way to reach the rest must be given: {out}"
    );
    assert!(
        !out.contains("50 available"),
        "a curated count is not what is available: {out}"
    );

    // An unnarrowed listing that was NOT curated still reports plainly.
    let plain = render(&actions, &request("", Detail::Names, &["github"]));
    assert!(plain.contains("50 available"), "{plain}");
    assert!(!plain.contains("featured"), "{plain}");
}

/// A server-side filter that matches nothing says so about the *filter*.
///
/// Once `search` and `tags` travel to Composio, a narrowed query that
/// matches nothing comes back with zero rows — and the old message read
/// that as "this toolkit has no callable actions (it may not be
/// connected)". That is a lie about a connected toolkit, and the expensive
/// kind: an agent told a capability does not exist stops looking for it,
/// which is the failure this whole listing was rewritten to end (codex on
/// tinyhumansai/opencompany#2153).
#[test]
fn an_empty_server_filtered_response_does_not_blame_the_connection() {
    let mut narrowed = request("quantum teleport", Detail::Names, &["gmail"]);
    narrowed.tags = vec!["important".to_string()];
    // Zero rows back, because the server did the filtering.
    let out = render(&[], &narrowed);

    assert!(
        !out.contains("may not be connected"),
        "an empty filter result says nothing about the connection: {out}"
    );
    assert!(
        !out.contains("no callable actions"),
        "the toolkit was not shown to be empty: {out}"
    );
    assert!(
        out.contains("quantum teleport") && out.contains("important"),
        "both halves of the filter are named: {out}"
    );
    assert!(
        out.contains("not what the toolkit has"),
        "the count must be disclosed as the filter's, not the toolkit's: {out}"
    );

    // The unnarrowed empty case still points at the connection, which is
    // the one time that is the right thing to say.
    let bare = render(&[], &request("", Detail::Names, &["github"]));
    assert!(bare.contains("may not be connected"), "{bare}");
}

/// An empty catalogue is a different fact from an empty search, and points
/// at the connection rather than at the search words.
#[test]
fn an_empty_catalogue_points_at_the_connection() {
    let out = render(&[], &request("", Detail::Names, &["github"]));
    assert!(out.contains("none"), "{out}");
    assert!(out.contains("composio_list_connections"), "{out}");
}

/// A single schema larger than the whole budget is still delivered — a
/// correctly-described way of being useless is still useless.
#[test]
fn a_single_oversized_schema_is_still_delivered_whole() {
    let actions = vec![CatalogAction {
        slug: "GIANT_ACTION".to_string(),
        toolkit: "giant".to_string(),
        description: "One enormous schema.".to_string(),
        parameters: Some(json!({"type": "object", "blob": "z".repeat(MAX_RENDER_BYTES * 2)})),
    }];
    let out = render(&actions, &request("", Detail::Schemas, &["giant"]));
    assert!(out.contains("GIANT_ACTION"), "{out}");
    assert!(
        out.contains(&"z".repeat(1000)),
        "the only matching schema must survive whole"
    );
    assert!(!out.contains("TRUNCATED"), "nothing was dropped: {out}");
}

/// Argument parsing: the defaults are the cheap ones, an over-large `limit`
/// is clamped rather than rejected, and an unknown `detail` degrades to the
/// names view instead of dumping 200 schemas.
#[test]
fn arguments_default_and_clamp_rather_than_failing() {
    let names = ListRequest::parse(&json!({}), vec!["github".into()]);
    assert_eq!(names.detail, Detail::Names);
    assert_eq!(names.limit, NAMES_DEFAULT_LIMIT);
    assert!(names.search.is_empty());

    let schemas = ListRequest::parse(&json!({"detail": "schemas"}), Vec::new());
    assert_eq!(schemas.limit, SCHEMAS_DEFAULT_LIMIT);

    let clamped = ListRequest::parse(&json!({"detail": "schemas", "limit": 9999}), Vec::new());
    assert_eq!(clamped.limit, SCHEMAS_MAX_LIMIT);

    let typo = ListRequest::parse(&json!({"detail": "everything"}), Vec::new());
    assert_eq!(
        typo.detail,
        Detail::Names,
        "an unknown detail must not dump schemas"
    );

    let terms = ListRequest::parse(&json!({"search": "  List   Issues "}), Vec::new());
    assert_eq!(terms.search, vec!["list".to_string(), "issues".to_string()]);
}

/// The advertised argument names and the ones the parser reads are the same
/// literals — a filter the model is told about but the tool ignores would
/// be the same bug wearing a different hat.
#[test]
fn the_advertised_schema_matches_the_arguments_the_parser_reads() {
    let schema = list_tools_parameters_schema();
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .expect("properties object");
    for key in ["toolkits", "search", "detail", "limit"] {
        assert!(properties.contains_key(key), "`{key}` is not advertised");
    }
    let description = list_tools_description();
    assert!(description.contains("search"), "{description}");
    assert!(description.contains("schemas"), "{description}");
    assert!(
        description.contains("truncated"),
        "the model must be told results can be cut: {description}"
    );

    let toolkits = list_toolkits_parameters_schema();
    let properties = toolkits
        .get("properties")
        .and_then(Value::as_object)
        .expect("properties object");
    for key in ["search", "limit"] {
        assert!(properties.contains_key(key), "`{key}` is not advertised");
    }
    assert!(list_toolkits_description().contains("search"));
}

/// The regression guard for #410's actual root cause.
///
/// A tool body must never be sized by
/// [`SCRUB_MAX_BYTES`](crate::harness::mcp_probe::SCRUB_MAX_BYTES). That
/// constant is a 300-byte cap on a one-line operator message, and routing a
/// successful Composio result through it is what turned a 260-action
/// catalogue into "the first action and half of its schema, ending in `…`".
/// This pins the two halves apart: redaction never shortens, the body bound
/// is orders of magnitude larger than the message bound, and a bounded body
/// says so.
#[test]
fn a_tool_body_is_bounded_as_a_body_not_as_a_message() {
    use crate::harness::mcp_probe::{SCRUB_MAX_BYTES, redact, scrub};

    let body = format!("secret-token {}", "payload ".repeat(4_000));
    let secrets = vec!["secret-token".to_string()];

    // Redaction still redacts — that half was never the bug.
    let redacted = redact(&body, &secrets);
    assert!(
        !redacted.contains("secret-token"),
        "the token survived redaction"
    );
    assert!(redacted.contains("•••"));
    // …and it does NOT shorten. The old path lost 99% of the payload here.
    assert!(
        redacted.len() > SCRUB_MAX_BYTES * 10,
        "redact must not apply the message cap: {} bytes",
        redacted.len()
    );
    assert!(
        scrub(&body, &secrets).len() <= SCRUB_MAX_BYTES + 3,
        "scrub keeps the message cap for the messages it was built for"
    );

    // The body bound is a body bound, and it announces itself.
    const {
        assert!(
            MAX_BODY_BYTES > SCRUB_MAX_BYTES * 20,
            "a body budget sized like a message budget is the bug"
        )
    };
    let bounded = bound_body(redacted.clone(), "`GITHUB_LIST_ISSUES` output");
    assert!(
        bounded.contains("TRUNCATED"),
        "an oversized body must say so"
    );
    assert!(
        bounded.contains("bytes longer than this"),
        "the notice must quantify what was lost: {bounded}"
    );
    assert!(bounded.contains("`GITHUB_LIST_ISSUES` output"), "{bounded}");

    // A body that fits is returned untouched — no decoration, no marker.
    let small = "a short provider response".to_string();
    assert_eq!(bound_body(small.clone(), "output"), small);
}

/// The toolkit catalogue is the same bug one level up, so it gets the same
/// self-describing cut.
#[test]
fn an_oversized_toolkit_listing_says_it_was_cut_and_how_to_narrow_it() {
    let toolkits = toolkit_catalogue(400);
    let out = render_toolkits(&toolkits, &ToolkitListRequest::parse(&json!({})));
    assert!(out.contains("400 available"), "{out}");
    assert!(out.contains("TRUNCATED"), "{out}");
    assert!(
        out.contains("more matching toolkits were not shown"),
        "{out}"
    );
    assert!(
        out.contains("`search`") || out.contains("\"search\""),
        "{out}"
    );
    assert!(
        out.len() < 16 * 1024,
        "the listing must stay under the harness cap: {} bytes",
        out.len()
    );
}

/// Narrowing and the connected marker, which is what an agent actually
/// needs before it reaches for `composio_authorize`.
#[test]
fn toolkit_search_narrows_and_marks_connection_state() {
    let mut toolkits = toolkit_catalogue(50);
    toolkits.push(CatalogToolkit {
        slug: "googlecalendar".to_string(),
        name: "Google Calendar".to_string(),
        description: "Read and write calendar events.".to_string(),
        connected: Some(true),
    });
    let out = render_toolkits(
        &toolkits,
        &ToolkitListRequest::parse(&json!({"search": "calendar"})),
    );
    assert!(
        out.contains("googlecalendar (Google Calendar) [connected]"),
        "{out}"
    );
    assert!(out.contains("showing 1"), "{out}");
    assert!(!out.contains("TRUNCATED"), "{out}");
}

/// A company with no integrations at all is a different fact from a search
/// that matched nothing, and both are stated rather than returned empty.
#[test]
fn an_empty_toolkit_catalogue_and_an_empty_search_read_differently() {
    let none = render_toolkits(&[], &ToolkitListRequest::parse(&json!({})));
    assert!(none.contains("none available"), "{none}");

    let no_match = render_toolkits(
        &toolkit_catalogue(5),
        &ToolkitListRequest::parse(&json!({"search": "zzz"})),
    );
    assert!(no_match.contains("0 matching `zzz`"), "{no_match}");
    assert!(
        no_match.contains("Do NOT guess a toolkit slug"),
        "{no_match}"
    );
}

/// The brief must name the concrete Composio tools an agent holds and the
/// two-step it reasons in, or it re-creates the unmentioned-tool failure the
/// sandbox brief exists to stop, one surface over.
#[test]
fn the_composio_brief_names_the_tools_and_the_two_step() {
    let brief = composio_brief(&["github".to_string()], &[]);
    for tool in [
        "composio_list_toolkits",
        "composio_list_connections",
        "composio_list_tools",
        "composio_execute",
        "composio_authorize",
    ] {
        assert!(brief.contains(tool), "brief never names `{tool}`: {brief}");
    }
}

/// The routing rule is the whole point: GitHub / connected SaaS go through
/// Composio, and the raw web tools are named as the wrong door (they answer
/// 401/403 unauthenticated). The observed failure — `api.github.com` via
/// `http_request` — must be called out by name.
#[test]
fn the_composio_brief_routes_provider_apis_through_composio_not_the_web_tools() {
    let brief = composio_brief(&["github".to_string()], &[]);
    for web_tool in ["http_request", "curl", "web_fetch"] {
        assert!(
            brief.contains(web_tool),
            "the brief must warn off `{web_tool}`: {brief}"
        );
    }
    assert!(brief.contains("api.github.com"), "{brief}");
    assert!(brief.contains("401") || brief.contains("403"), "{brief}");
}

/// The grounding half: the agent is told not to promise an action it has no
/// tool for, with the exact browser overreach the issue observed named.
#[test]
fn the_composio_brief_forbids_promising_actions_it_has_no_tool_for() {
    let brief = composio_brief(&["github".to_string()], &[]);
    let lower = brief.to_lowercase();
    assert!(lower.contains("no browser"), "{brief}");
    assert!(lower.contains("do not promise"), "{brief}");
}

/// PR #1780 review (round 5): `composio_brief` is rendered whenever the
/// Composio tools are wired, with no view of whether the agent was
/// SEPARATELY granted an MCP browser tool (Browserbase and friends —
/// `build_agent`'s MCP bridge is wholly independent of the Composio grant,
/// see `build.rs`). The old wording claimed "you have no browser" as an
/// unconditional fact, which is false on that combination and could make
/// the agent refuse a browser action it actually holds a tool for. The
/// claim must be qualified on "unless separately granted a browser tool"
/// (or equivalent), not stated as an absolute.
#[test]
fn the_composio_brief_does_not_unconditionally_deny_holding_a_browser_tool() {
    let brief = composio_brief(&["github".to_string()], &[]);
    let lower = brief.to_lowercase();
    assert!(
        lower.contains("unless you were separately granted a browser tool")
            || lower.contains("unless separately granted a browser tool"),
        "the no-browser claim must be qualified, not absolute, since MCP can grant one \
         independently of Composio: {brief}"
    );
}

/// A non-empty allowlist names exactly those toolkits, lowercased, so the
/// agent is grounded in what THIS company connected rather than a generic
/// list.
#[test]
fn the_composio_brief_names_the_connected_toolkits_lowercased() {
    let brief = composio_brief(&["GitHub".to_string(), " Gmail ".to_string()], &[]);
    assert!(
        brief.contains("Connected toolkits for this company: github, gmail"),
        "{brief}"
    );
}
