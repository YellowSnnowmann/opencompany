use super::*;

/// PR #1780 review (round 6): a non-empty allowlist that excludes GitHub
/// (e.g. `["slack"]`) must not tell the agent it can reach GitHub through
/// Composio — the live tools enforce the allowlist and would reject the
/// authorization/execution, routing a GitHub task toward a capability the
/// agent does not hold.
#[test]
fn the_composio_brief_does_not_advertise_github_outside_a_restricting_allowlist() {
    let brief = composio_brief(&["slack".to_string()], &[]);
    assert!(
        !brief.to_lowercase().contains("github"),
        "an allowlist that excludes GitHub must not name it as reachable: {brief}"
    );
    assert!(
        brief.contains("Connected toolkits for this company: slack"),
        "{brief}"
    );
    // The routing rule and grounding still hold generically.
    assert!(brief.contains("composio_execute"), "{brief}");
    assert!(brief.contains("http_request"), "{brief}");
}

/// Open mode (an empty allowlist) must NOT invent a provider the company may
/// not have connected — it points the agent at `composio_list_connections`
/// to discover the real set instead. This is requirement #3: never advertise
/// a toolkit that is not known-connected.
#[test]
fn the_composio_brief_open_mode_points_at_discovery_without_naming_a_provider() {
    let brief = composio_brief(&[], &[]);
    assert!(
        brief.contains("not fixed here"),
        "open mode must defer to discovery: {brief}"
    );
    assert!(
        !brief.contains("Connected toolkits for this company:"),
        "open mode must not claim a specific connected set: {brief}"
    );
    // The routing rule and grounding still hold with no allowlist.
    assert!(brief.contains("composio_execute"), "{brief}");
    assert!(brief.contains("http_request"), "{brief}");
}

/// PR #1780 review (round 7): an empty allowlist means the CLIENT applies
/// no restriction — the backend's own server-enforced allowlist still
/// decides what is actually connected (see `TenantComposio::toolkits`'s
/// doc comment). It is not proof that GitHub, specifically, is connected.
/// Naming GitHub in the "GitHub and other SaaS" heading before
/// `composio_list_connections` has run promises a capability the agent may
/// not hold — the same class of bug requirement #3 (the discovery-pointer
/// test above) already guards against for the "Connected toolkits for
/// this company:" line.
#[test]
fn the_composio_brief_open_mode_does_not_name_github_before_discovery() {
    let brief = composio_brief(&[], &[]);
    assert!(
        !brief.contains("Connected integrations (GitHub and other SaaS)")
            && !brief.contains("You reach GitHub"),
        "open mode must not claim GitHub is reachable before discovery: {brief}"
    );
    // The routing rule and grounding still hold generically.
    assert!(brief.contains("composio_execute"), "{brief}");
    assert!(brief.contains("http_request"), "{brief}");
    // ...and the guardrail keeps its concrete example. Naming
    // `api.github.com` as something NOT to hand-roll is the opposite of
    // claiming GitHub is reachable, and the turn test
    // `the_composio_routing_brief_reaches_the_model_system_prompt`
    // asserts the model actually sees it. An earlier revision of this
    // test forbade the substring "github" outright, which took the
    // example down with the claim and broke that guarantee.
    assert!(brief.contains("api.github.com"), "{brief}");
}

/// A native capability the agent already holds a built-in tool for is named
/// as such, and told to be used directly rather than routed through Composio.
#[test]
fn the_composio_brief_names_native_capabilities_as_built_in_not_composio() {
    let brief = composio_brief(&["github".to_string()], &["search"]);
    assert!(
        brief.contains("built-in tools of your own: search"),
        "the native capability must be named: {brief}"
    );
    assert!(
        brief.contains("do not route them through Composio"),
        "the precedence must tell the agent to use the built-in tool directly: {brief}"
    );
}

/// Empty native caps render no precedence line, leaving the brief as it was
/// before native-first routing — and the S2 raw-HTTP deflection warning is
/// untouched either way.
#[test]
fn the_composio_brief_with_no_native_caps_is_unchanged_and_keeps_the_deflection_warning() {
    let brief = composio_brief(&["github".to_string()], &[]);
    assert!(
        !brief.contains("built-in tools of your own"),
        "no native caps must add no precedence line: {brief}"
    );
    // The S2 raw-HTTP deflection warning is verbatim regardless.
    assert!(brief.contains("http_request"), "{brief}");
    assert!(brief.contains("api.github.com"), "{brief}");
    assert!(brief.contains("401") || brief.contains("403"), "{brief}");
}

/// The two levers coexist on one brief: a connected toolkit is still routed
/// through Composio while a native capability is called out as built-in.
#[test]
fn the_composio_brief_routes_composio_toolkits_and_native_caps_separately() {
    let brief = composio_brief(&["gmail".to_string()], &["search"]);
    assert!(
        brief.contains("Connected toolkits for this company: gmail"),
        "the connected toolkit is still routed through Composio: {brief}"
    );
    assert!(
        brief.contains("built-in tools of your own: search"),
        "the native capability is called out as built-in: {brief}"
    );
}

/// The headline case the guardrail exists for: a raw call to
/// `api.github.com` when `github` is connected is deflected, and the refusal
/// names the Composio route the agent should have taken.
#[test]
fn a_connected_provider_host_is_deflected_with_the_composio_route() {
    let connected = vec!["github".to_string()];
    let reason = web_call_deflection(&connected, "https://api.github.com/repos/o/r/issues")
        .expect("a connected provider host must be deflected");
    assert!(reason.contains("api.github.com"), "{reason}");
    assert!(reason.contains("composio_execute"), "{reason}");
    assert!(reason.contains("composio_list_tools"), "{reason}");
    assert!(
        reason.contains("401") || reason.contains("403"),
        "the refusal must explain the unauthenticated failure: {reason}"
    );
}

/// Requirement #2: the SAME host passes through untouched when its toolkit is
/// NOT connected — the company may legitimately hit a public endpoint of a
/// provider it has not wired.
#[test]
fn the_same_host_passes_through_when_its_toolkit_is_not_connected() {
    // Some other toolkit is connected, but not github.
    let connected = vec!["slack".to_string()];
    assert!(
        web_call_deflection(&connected, "https://api.github.com/repos/o/r").is_none(),
        "an unconnected provider host must pass through"
    );
    // And with nothing connected at all.
    assert!(
        web_call_deflection(&[], "https://api.github.com/repos/o/r").is_none(),
        "no connected toolkits means no deflection"
    );
}

/// A non-provider host is never deflected, whatever is connected.
#[test]
fn a_non_provider_host_always_passes_through() {
    let connected = vec!["github".to_string(), "gmail".to_string()];
    assert!(web_call_deflection(&connected, "https://example.com/data.json").is_none());
    assert!(web_call_deflection(&connected, "https://raw.githubusercontent.com/x").is_none());
}

/// Sub-domains of a connected provider's API host are caught; the connected
/// list is normalised (trim + lowercase) the same way [`composio_brief`]
/// normalises it, so a manifest `"GitHub"` still matches.
#[test]
fn subdomains_are_caught_and_the_connected_list_is_normalised() {
    let connected = vec![" GitHub ".to_string()];
    assert!(
        web_call_deflection(&connected, "https://uploads.api.github.com/x").is_some(),
        "a sub-domain of the API host must be deflected"
    );
}

/// PR #1780 review (round 12): `https://api.github.com./repos/o/r` is a
/// valid absolute-FQDN spelling — the trailing dot is the DNS root label
/// and resolves to the exact same host — but `Url::host_str` keeps that
/// dot verbatim, so before this fix neither the equality nor the
/// subdomain-suffix arm of `host_is` matched it and the request bypassed
/// deflection entirely.
#[test]
fn a_trailing_dot_fqdn_still_matches_the_provider_host() {
    let connected = vec!["github".to_string()];
    assert!(
        web_call_deflection(&connected, "https://api.github.com./repos/o/r").is_some(),
        "the root-label trailing dot must not defeat the host match"
    );
}

/// A URL that does not parse to a host is not a provider call — it passes
/// through rather than panicking or denying.
#[test]
fn an_unparseable_url_passes_through() {
    let connected = vec!["github".to_string()];
    assert!(web_call_deflection(&connected, "not a url").is_none());
    assert!(web_call_deflection(&connected, "file:///etc/hosts").is_none());
}

/// PR #1780 review: Slack's Web API is served from `slack.com/api/*`, but
/// `slack.com` also hosts Slack's public marketing/help pages. A
/// `web_fetch` of one of those pages needs no Composio connection and has
/// no equivalent Composio action, so it must pass through untouched even
/// when `slack` is connected — only the `/api/` path is deflected.
#[test]
fn slack_deflection_is_scoped_to_the_api_path_not_the_whole_domain() {
    let connected = vec!["slack".to_string()];
    assert!(
        web_call_deflection(&connected, "https://slack.com/api/chat.postMessage").is_some(),
        "a real Slack Web API call must still be deflected"
    );
    assert!(
        web_call_deflection(&connected, "https://slack.com/help/some-public-article").is_none(),
        "a public slack.com page outside /api/ must pass through"
    );
    assert!(
        web_call_deflection(&connected, "https://slack.com/").is_none(),
        "the bare domain root must pass through"
    );
}

/// Same shape as Slack: Discord's REST API is served from
/// `discord.com/api/*`, but `discord.com` also hosts the main web client
/// and public invite/marketing pages.
#[test]
fn discord_deflection_is_scoped_to_the_api_path_not_the_whole_domain() {
    let connected = vec!["discord".to_string()];
    assert!(
        web_call_deflection(&connected, "https://discord.com/api/v10/users/@me").is_some(),
        "a real Discord API call must still be deflected"
    );
    assert!(
        web_call_deflection(&connected, "https://discord.com/invite/somepublicserver").is_none(),
        "a public discord.com page outside /api/ must pass through"
    );
}

/// `www.googleapis.com` is Google's shared legacy gateway for many APIs,
/// not just Drive — unlike Slack/Discord this is one host fronting several
/// unrelated *products*, not a product mixing API and public-page traffic.
/// Before the fix this entry had no path prefix, so a `googledrive`
/// connection deflected `www.googleapis.com/youtube/v3/...` to the Drive
/// toolkit even though Drive cannot serve it (PR #1780 review).
///
/// PR #1780 review (round 8): like the `/upload/drive/` sibling prefix
/// above, batching several Drive calls into one request goes to
/// `www.googleapis.com/batch/drive/v3` — a third sibling prefix under the
/// same shared gateway host, not a deeper path under `/drive/`. Before
/// this fix `googledrive`'s prefix set had no `/batch/drive/` entry, so a
/// batch request passed straight through instead of being deflected.
#[test]
fn drive_deflection_on_the_legacy_host_is_scoped_to_drive_paths() {
    let connected = vec!["googledrive".to_string()];
    assert!(
        web_call_deflection(&connected, "https://www.googleapis.com/drive/v3/files").is_some(),
        "a real Drive call on the legacy gateway host must still be deflected"
    );
    assert!(
        web_call_deflection(
            &connected,
            "https://www.googleapis.com/youtube/v3/search?q=rust"
        )
        .is_none(),
        "an unrelated Google API sharing the legacy gateway host must pass through"
    );
    assert!(
        web_call_deflection(&connected, "https://drive.googleapis.com/drive/v3/files").is_some(),
        "the dedicated Drive host stays deflected unscoped"
    );
    assert!(
        web_call_deflection(
            &connected,
            "https://www.googleapis.com/upload/drive/v3/files"
        )
        .is_some(),
        "the resumable/media upload route on the legacy gateway host must also be deflected"
    );
    assert!(
        web_call_deflection(&connected, "https://www.googleapis.com/batch/drive/v3").is_some(),
        "the batch endpoint on the legacy gateway host must also be deflected, the same as \
         Drive's sibling /upload/drive/ prefix"
    );
}

/// PR #1780 review: `api.atlassian.com` only covers the OAuth-3LO gateway.
/// The Jira REST API an agent plausibly curls by hand lives on the
/// tenant's own domain, `<site>.atlassian.net/rest/api/...` — before the
/// fix that host was not in the table at all, so this request passed
/// straight through with no credential instead of being deflected to
/// Composio. The same tenant host also serves the ordinary Jira web UI,
/// so the match must stay scoped to `/rest/api/`.
#[test]
fn jira_deflection_covers_the_tenant_specific_atlassian_net_host() {
    let connected = vec!["jira".to_string()];
    assert!(
        web_call_deflection(
            &connected,
            "https://my-company.atlassian.net/rest/api/3/issue/PROJ-1"
        )
        .is_some(),
        "a tenant's Jira Cloud REST API call must be deflected"
    );
    assert!(
        web_call_deflection(
            &connected,
            "https://my-company.atlassian.net/jira/software/projects/PROJ/boards/1"
        )
        .is_none(),
        "the tenant's public Jira web UI outside /rest/api/ must pass through"
    );
    assert!(
        web_call_deflection(
            &connected,
            "https://api.atlassian.com/ex/jira/some-cloud-id/rest/api/3/issue/PROJ-1"
        )
        .is_some(),
        "a Jira call through the OAuth-3LO gateway stays deflected"
    );
    assert!(
        web_call_deflection(
            &connected,
            "https://api.atlassian.com/ex/confluence/some-cloud-id/rest/api/content"
        )
        .is_none(),
        "another Atlassian product on the shared gateway must pass through — \
         a jira connection is not a Confluence one"
    );
    assert!(
        web_call_deflection(
            &connected,
            "https://my-company.atlassian.net/rest/agile/1.0/board"
        )
        .is_some(),
        "Jira Software's Agile REST family on the tenant host must also be deflected"
    );
}

/// PR #1780 review (finding 6): `www.googleapis.com` is the same shared
/// legacy gateway for Calendar as it is for Drive. Before the fix
/// `googlecalendar` only recognised `calendar.googleapis.com`, so the
/// standard REST URL most examples and agents actually curl,
/// `www.googleapis.com/calendar/v3/...`, passed straight through with no
/// credential instead of being deflected to Composio.
///
/// PR #1780 review (round 9): like Drive's `/batch/drive/` sibling prefix
/// (round 8), Calendar's batch endpoint on the shared gateway,
/// `www.googleapis.com/batch/calendar/v3`, is a sibling of `/calendar/`,
/// not a deeper path under it. Before this fix `googlecalendar`'s prefix
/// set had no `/batch/calendar/` entry, so a batch request passed
/// straight through instead of being deflected.
#[test]
fn calendar_deflection_on_the_legacy_host_is_scoped_to_calendar_paths() {
    let connected = vec!["googlecalendar".to_string()];
    assert!(
        web_call_deflection(
            &connected,
            "https://www.googleapis.com/calendar/v3/calendars/primary/events"
        )
        .is_some(),
        "a real Calendar call on the legacy gateway host must be deflected"
    );
    assert!(
        web_call_deflection(
            &connected,
            "https://www.googleapis.com/youtube/v3/search?q=rust"
        )
        .is_none(),
        "an unrelated Google API sharing the legacy gateway host must pass through"
    );
    assert!(
        web_call_deflection(
            &connected,
            "https://calendar.googleapis.com/calendar/v3/calendars/primary/events"
        )
        .is_some(),
        "the dedicated Calendar host stays deflected unscoped"
    );
    assert!(
        web_call_deflection(&connected, "https://www.googleapis.com/batch/calendar/v3").is_some(),
        "the batch endpoint on the legacy gateway host must also be deflected, the same as \
         Drive's sibling /batch/drive/ prefix"
    );
}

/// Same shape as Calendar and Drive: `gmail.googleapis.com` is the
/// dedicated host, but `www.googleapis.com/gmail/v1/...` is the same
/// shared legacy gateway an agent plausibly curls by hand. Before the fix
/// the table only had the dedicated host, so this call passed through
/// unscoped.
///
/// PR #1780 review: like Drive's `/upload/drive/` sibling prefix
/// (finding 7), Gmail's legacy-gateway media/resumable upload route for
/// sending or importing a message with an attachment is
/// `/upload/gmail/v1/users/me/messages/send`, not `/gmail/v1/...` — a
/// sibling path prefix under the same host, not a deeper path under
/// `/gmail/`. Before the fix `gmail`'s prefix set only had `/gmail/`, so
/// this raw unauthenticated request passed straight through instead of
/// being deflected to Composio.
///
/// PR #1780 review (round 10): like Drive's and Calendar's `/batch/`
/// sibling prefixes (rounds 8 and 9), batching several Gmail calls into
/// one request goes to `www.googleapis.com/batch/gmail/v1` — another
/// sibling of `/gmail/`, not a deeper path under it. Before this fix
/// Gmail's prefix set had no `/batch/gmail/` entry, so a batch request
/// passed straight through instead of being deflected.
#[test]
fn gmail_deflection_on_the_legacy_host_is_scoped_to_gmail_paths() {
    let connected = vec!["gmail".to_string()];
    assert!(
        web_call_deflection(
            &connected,
            "https://www.googleapis.com/gmail/v1/users/me/messages"
        )
        .is_some(),
        "a real Gmail call on the legacy gateway host must be deflected"
    );
    assert!(
        web_call_deflection(&connected, "https://www.googleapis.com/drive/v3/files").is_none(),
        "an unrelated Google API sharing the legacy gateway host must pass through"
    );
    assert!(
        web_call_deflection(
            &connected,
            "https://gmail.googleapis.com/gmail/v1/users/me/messages"
        )
        .is_some(),
        "the dedicated Gmail host stays deflected unscoped"
    );
    assert!(
        web_call_deflection(
            &connected,
            "https://www.googleapis.com/upload/gmail/v1/users/me/messages/send"
        )
        .is_some(),
        "the legacy media/resumable upload route on the shared gateway host must also be \
         deflected, the same as Drive's sibling /upload/drive/ prefix"
    );
    assert!(
        web_call_deflection(&connected, "https://www.googleapis.com/batch/gmail/v1").is_some(),
        "the batch endpoint on the legacy gateway host must also be deflected, the same as \
         Drive's sibling /batch/drive/ and Calendar's sibling /batch/calendar/ prefixes"
    );
}

/// PR #1780 review (round 5): `api.github.com` is GitHub's dedicated REST
/// host, but release-asset uploads are served from a SIBLING host,
/// `uploads.github.com` — not a sub-domain of `api.github.com`, so
/// `host_is` never caught it before this host was added to the table. An
/// agent uploading a release asset by hand hit this host with no
/// credential and passed straight through instead of being deflected.
#[test]
fn github_upload_host_is_deflected_alongside_the_api_host() {
    let connected = vec!["github".to_string()];
    assert!(
        web_call_deflection(
            &connected,
            "https://uploads.github.com/repos/o/r/releases/1/assets?name=out.zip"
        )
        .is_some(),
        "the release-asset upload host must be deflected alongside api.github.com"
    );
    assert!(
        web_call_deflection(&connected, "https://api.github.com/repos/o/r/issues").is_some(),
        "the dedicated API host stays deflected"
    );
}

/// PR #1780 review (round 6): `api.stripe.com` is Stripe's general REST
/// host, but file uploads (dispute evidence, identity documents, ...) are
/// served from a SIBLING host, `files.stripe.com` — not a subdomain of
/// `api.stripe.com`, so `host_is` never caught it before this host was
/// added to the table, and a raw file-upload request passed through
/// unauthenticated instead of being deflected.
#[test]
fn stripe_file_upload_host_is_deflected_alongside_the_api_host() {
    let connected = vec!["stripe".to_string()];
    assert!(
        web_call_deflection(&connected, "https://files.stripe.com/v1/files").is_some(),
        "the file-upload host must be deflected alongside api.stripe.com"
    );
    assert!(
        web_call_deflection(&connected, "https://api.stripe.com/v1/charges").is_some(),
        "the dedicated API host stays deflected"
    );
}

/// PR #1780 review (round 11): `dropbox` is a first-class toolkit in the
/// operator console's connection catalogue
/// (`frontend/src/lib/connections.ts`), but `toolkit_api_hosts` had no
/// match arm for it and fell through to the `_ => &[]` default, so a
/// company that connected Dropbox got no deflection at all — a raw call
/// to either of Dropbox's two API hosts (the RPC host and the
/// content-transfer host used for upload/download) passed straight
/// through unauthenticated instead of being deflected to Composio.
#[test]
fn dropbox_is_deflected_across_its_api_and_content_hosts() {
    let connected = vec!["dropbox".to_string()];
    assert!(
        web_call_deflection(&connected, "https://api.dropboxapi.com/2/files/list_folder").is_some(),
        "the RPC API host must be deflected"
    );
    assert!(
        web_call_deflection(&connected, "https://content.dropboxapi.com/2/files/upload").is_some(),
        "the content-transfer host (upload/download) must also be deflected"
    );
}

/// PR #1780 review (round 13): same gap as Dropbox (round 11) —
/// `twitter` (surfaced as "X" in the console) and `linkedin` are
/// first-class toolkits in `frontend/src/lib/connections.ts`, but
/// neither had a match arm here, so both fell through to the `_ => &[]`
/// default and got no deflection at all.
#[test]
fn x_and_linkedin_are_deflected_across_their_api_hosts() {
    let connected = vec!["twitter".to_string(), "linkedin".to_string()];
    assert!(
        web_call_deflection(&connected, "https://api.twitter.com/2/tweets").is_some(),
        "the legacy twitter.com API host must be deflected"
    );
    assert!(
        web_call_deflection(&connected, "https://api.x.com/2/tweets").is_some(),
        "the current x.com API host must also be deflected"
    );
    assert!(
        web_call_deflection(&connected, "https://api.linkedin.com/rest/posts").is_some(),
        "the LinkedIn API host must be deflected"
    );
}
