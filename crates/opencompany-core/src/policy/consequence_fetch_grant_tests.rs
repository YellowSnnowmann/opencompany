use super::*;
use crate::ports::types::Verdict;
use serde_json::json;

// -----------------------------------------------------------------------
// Issue #673: a host-scoped fetch grant, and the `auto` line it must not
// cross
// -----------------------------------------------------------------------

/// A `web_fetch` call carrying a real URL, as the policy layer sees it.
pub(super) fn fetching(url: &str) -> serde_json::Value {
    json!({ WEB_FETCH_URL_KEY: url })
}

/// **The entire reason [`Standing::ScopedGrantable`] exists, as a rule.**
///
/// The naive fix for #673 — declaring `web_fetch` [`Standing::Grantable`] to
/// obtain a scoped grant — was tried and rejected: because
/// [`Consequence::parks_under_auto`] read `is_grantable`, it also stopped the
/// tool parking under `auto`, for every agent, with no card and therefore no
/// scope ever consulted. `the_auto_tier_line_is_pinned_tool_by_tool` catches
/// that, and this states the invariant that must hold for the *repair* not to
/// re-open the same hole from the other side.
///
/// Exhaustive over [`Reach`] rather than sampled, because the variant is
/// argument-classified and so appears nowhere in [`DECLARED`] for a table
/// walk to find.
#[test]
pub(super) fn a_scoped_grantable_call_is_delegable_but_never_unattended_under_auto() {
    assert!(
        Standing::ScopedGrantable.is_grantable(),
        "the point of the variant is that an operator CAN delegate it"
    );
    assert!(
        !Standing::ScopedGrantable.runs_unattended_under_auto(),
        "and that it still parks under auto — collapsing these two answers \
         back together is exactly the bug issue #673 fixed"
    );

    for reach in [
        Reach::Nothing,
        Reach::Money,
        Reach::ExternalRead,
        Reach::Consequence,
    ] {
        let verdict = Consequence {
            group: EffectGroup::Other,
            reach,
            standing: Standing::ScopedGrantable,
        };
        assert_eq!(
            verdict.parks_under_auto(),
            reach.parks_under_supervision(),
            "a scoped-grantable tool must park under `auto` wherever it parks \
             under `supervised` — {reach:?} disagreed"
        );
    }
}

/// The declaration table, rendered and pinned (issue #2148).
///
/// `Standing` decides what an operator may hand a teammate for a week, and
/// a one-word edit from `PerCall` to `Grantable` widens that silently — the
/// diff reads as a typo-sized change and the review question it should
/// raise ("should this run unattended for a week?") never gets asked.
/// Rendering the whole table makes every such edit a reviewable line.
///
/// Deliberately the **static** table only, not `consequence_of`: the
/// argument-classified tools answer differently depending on whether the
/// curated Composio catalogue is compiled in, so a snapshot of their live
/// verdicts would pass on one CI lane and fail on the next.
///
/// Re-bless with `BLESS_TOOL_STANDING=1`, then read the diff. A blessed
/// snapshot nobody read is not a pin.
#[test]
pub(super) fn the_declared_grantability_table_is_pinned() {
    let mut rows: Vec<String> = DECLARED
        .iter()
        .map(|d| {
            format!(
                "{} group={:?} reach={:?} standing={:?}",
                d.tool, d.group, d.reach, d.standing
            )
        })
        .collect();
    rows.sort_unstable();
    let rendered = format!("{}\n", rows.join("\n"));

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/tool-standing.txt");
    if std::env::var_os("BLESS_TOOL_STANDING").is_some() {
        std::fs::write(&path, &rendered).expect("write the grantability snapshot");
        return;
    }
    let committed = std::fs::read_to_string(&path).expect("read the grantability snapshot");
    assert_eq!(
        rendered, committed,
        "the declared grantability table moved. If that is intended, re-bless with \
         BLESS_TOOL_STANDING=1 and say in the PR why the tool's standing changed — a tool \
         becoming Grantable is an operator being newly able to hand it over for a week"
    );
}

/// A scope-required approval with no scope to give is refused, not minted
/// wide (issue #2148).
///
/// Driven through the inner rule because the shipped table cannot currently
/// produce that combination — `web_fetch_consequence` answers
/// `ScopedGrantable` only where the scope was already read. That is exactly
/// why the case is worth a test: the rule has to hold for the next entry
/// that derives its scope by another route, and a rule whose failing case
/// can only be described in prose is one nobody has run.
#[test]
pub(super) fn a_scope_required_approval_without_a_scope_is_refused() {
    let refused = decide_standing_mint_scope(Standing::ScopedGrantable, None, Verdict::Approve);
    let StandingMintScope::Refused(why) = refused else {
        panic!("a scope-required approval with no scope must not mint: {refused:?}");
    };
    assert!(
        why.contains("approve it once instead"),
        "the refusal has to leave the operator somewhere to go: {why}"
    );
}

/// The asymmetry, stated: an unscoped denial refuses everything, which is
/// broad but safe, and refusing to mint it would take away the operator's
/// only way to decline a tool for a period (issue #1458).
#[test]
pub(super) fn a_scope_required_denial_may_be_unscoped() {
    assert_eq!(
        decide_standing_mint_scope(Standing::ScopedGrantable, None, Verdict::Deny),
        StandingMintScope::Unscoped
    );
}

#[test]
pub(super) fn a_derived_scope_is_carried_whatever_the_declaration_says() {
    for standing in [
        Standing::Grantable,
        Standing::ScopedGrantable,
        Standing::PerCall,
    ] {
        for verdict in [Verdict::Approve, Verdict::Deny] {
            assert_eq!(
                decide_standing_mint_scope(standing, Some("github".to_string()), verdict),
                StandingMintScope::Scoped("github".to_string()),
                "{standing:?}/{verdict:?}: a scope that was derived is never dropped"
            );
        }
    }
}

/// No declared tool can reach the mint claiming a scope it cannot produce.
///
/// This is the walk that makes the invariant enforced rather than merely
/// true today: it fails the moment a classification answers
/// `ScopedGrantable` down a path where `standing_scope_of` returns `None`.
/// Both probes matter — the rich one reaches the argument-classified
/// branches, the empty one is what a mint sees when a call carried nothing
/// the classifier could read.
#[test]
pub(super) fn no_scope_required_tool_can_mint_unscoped() {
    let rich = json!({
        WEB_FETCH_URL_KEY: "https://docs.rs/serde",
        COMPOSIO_ACTION_KEY: "GITHUB_GET_A_REPOSITORY",
    });
    for probe in [&rich, &json!({})] {
        for tool in declared_tools() {
            if consequence_of(tool, probe).standing != Standing::ScopedGrantable {
                continue;
            }
            assert!(
                !matches!(
                    standing_mint_scope(tool, probe, Verdict::Approve),
                    StandingMintScope::Unscoped
                ),
                "`{tool}` is scope-required but would mint unscoped, and an unscoped grant \
                 admits every host and every toolkit"
            );
        }
    }
}

/// The same rule walked over the declaration table, so a tool that becomes
/// scoped-grantable later is covered without editing this test.
///
/// The `seen` counter is the point: every tool here is probed with arguments
/// rich enough to reach the argument-classified branches, and a walk that
/// found no scoped-grantable verdict at all would pass while asserting
/// nothing.
#[test]
pub(super) fn every_scoped_grantable_tool_in_the_table_follows_its_reach() {
    let probe = json!({
        WEB_FETCH_URL_KEY: "https://docs.rs/serde",
        COMPOSIO_ACTION_KEY: "GITHUB_GET_A_REPOSITORY",
    });
    let mut seen = 0;
    for tool in declared_tools() {
        let verdict = consequence_of(tool, &probe);
        if verdict.standing == Standing::ScopedGrantable {
            seen += 1;
            assert_eq!(
                verdict.parks_under_auto(),
                verdict.reach.parks_under_supervision(),
                "`{tool}` is scoped-grantable, so its reach alone decides whether it parks"
            );
        }
    }
    assert!(
        seen > 0,
        "the walk reached no scoped-grantable tool, so it proved nothing"
    );
}

/// A fetch of a named host is grantable and scoped to that host; the same
/// call with an unreadable URL is neither.
///
/// The second half is not tidiness. A grant is minted with whatever
/// `standing_scope_of` returned, and an unscoped grant admits *everything*
/// (`StandingGrant::admits_scope`), so a URL-less call that stayed grantable
/// would let one approval mint a grant over every host on earth. The two
/// answers come from one function precisely so that cannot be represented.
#[test]
pub(super) fn a_fetch_is_grantable_only_when_its_host_can_be_read() {
    let verdict = consequence_of(WEB_FETCH, &fetching("https://docs.rs/serde/latest"));
    assert_eq!(verdict.standing, Standing::ScopedGrantable);
    assert_eq!(
        standing_scope_of(WEB_FETCH, &fetching("https://docs.rs/serde/latest")),
        Some("https://docs.rs".to_string())
    );

    for unreadable in [
        json!({}),                         // no url at all
        fetching("not-a-url"),             // no scheme
        fetching("file:///etc/passwd"),    // not http(s)
        fetching("ftp://example.com/x"),   // not http(s)
        fetching("https://"),              // no host
        fetching("https://:8080/"),        // a port naming no host
        fetching("https://exa mple.com/"), // outside the host alphabet
    ] {
        let verdict = consequence_of(WEB_FETCH, &unreadable);
        assert_eq!(
            verdict.standing,
            Standing::PerCall,
            "an unreadable URL must not be grantable: {unreadable}"
        );
        assert_eq!(
            verdict.reach,
            Reach::Consequence,
            "an unreadable URL must stay gated, not free: {unreadable}"
        );
        assert!(verdict.reach.parks_under_supervision(), "{unreadable}");
        assert!(verdict.parks_under_auto(), "{unreadable}");
        assert_eq!(
            standing_scope_of(WEB_FETCH, &unreadable),
            None,
            "{unreadable}"
        );
    }
}

#[test]
pub(super) fn read_shaped_http_is_free_but_readonly_and_spend_stay_closed() {
    for method in ["GET", "get", "HEAD", "OPTIONS"] {
        let verdict = consequence_of(
            "http_request",
            &json!({ "method": method, "url": "https://api.github.com/repos/o/r" }),
        );
        assert_eq!(verdict.reach, Reach::ExternalRead, "{method}");
        assert_eq!(verdict.standing, Standing::ScopedGrantable, "{method}");
        assert!(!verdict.reach.parks_under_supervision(), "{method}");
        assert!(!verdict.parks_under_auto(), "{method}");
        assert!(verdict.reach.denied_under_readonly(), "{method}");
        assert!(!verdict.reach.costs_money(), "{method}");
    }
}

/// A read-shaped method whose URL is a runtime expression — `=item.endpoint`
/// is tinyflows' own unresolved-value prefix — names no host yet. Free
/// classification is earned by a destination the operator can see; an
/// upstream node choosing the destination at run time is the case the
/// workflow gate's own
/// `an_unresolved_url_gates_and_says_the_destination_is_not_known_yet` pins.
#[test]
pub(super) fn a_runtime_resolved_get_target_stays_gated() {
    for method in ["GET", "HEAD", "OPTIONS"] {
        let verdict = consequence_of(
            "http_request",
            &json!({ "method": method, "url": "=item.endpoint" }),
        );
        assert_eq!(verdict.reach, Reach::Consequence, "{method}");
        assert_eq!(verdict.standing, Standing::PerCall, "{method}");
        assert!(verdict.reach.parks_under_supervision(), "{method}");
        assert!(verdict.parks_under_auto(), "{method}");
    }
}

#[test]
pub(super) fn mutating_and_unknown_http_methods_stay_per_call() {
    for args in [
        json!({ "method": "POST", "url": "https://api.example.com/items" }),
        json!({ "method": "DELETE", "url": "https://api.example.com/items/1" }),
        json!({ "method": "BREW", "url": "https://api.example.com/coffee" }),
        json!({ "method": 7, "url": "https://api.example.com/items" }),
    ] {
        let verdict = consequence_of("http_request", &args);
        assert_eq!(verdict.reach, Reach::Consequence, "{args}");
        assert_eq!(verdict.standing, Standing::PerCall, "{args}");
        assert!(verdict.reach.parks_under_supervision(), "{args}");
        assert!(verdict.parks_under_auto(), "{args}");
        assert!(!verdict.reach.costs_money(), "{args}");
    }
}

/// An authored `http_request` node with only a `url` omits `method`
/// entirely, and both the execution path
/// ([`crate::workflows::caps::http::to_tool_args`] forwards the omission,
/// then `HttpRequestTool` defaults it to GET) and the approvals-card path
/// ([`crate::workflows::gate::http_target`]) treat that omission as GET.
/// The classifier must agree, or a plain "fetch this URL" node parks under
/// `supervised`/`auto` while it actually runs a free read.
#[test]
pub(super) fn an_omitted_http_method_defaults_to_get_and_is_free() {
    let args = json!({ "url": "https://api.example.com/items" });
    let verdict = consequence_of("http_request", &args);
    assert_eq!(verdict.reach, Reach::ExternalRead, "{args}");
    assert_eq!(verdict.standing, Standing::ScopedGrantable, "{args}");
    assert!(!verdict.reach.parks_under_supervision(), "{args}");
    assert!(!verdict.parks_under_auto(), "{args}");
}

/// A `GET` with a `body` is not a read: [`to_tool_args`] forwards the body
/// verbatim and `HttpRequestTool::execute_request` attaches it to the
/// outgoing request regardless of method, so this is an outbound data
/// transmission wearing a read-shaped method (PR #1989 review 3905098660).
/// Proven red pre-fix: before this change `http_request_consequence`
/// looked at `method` alone, so this call classified `ExternalRead` /
/// `ScopedGrantable` and ran unattended under `supervised`/`auto`.
#[test]
pub(super) fn a_get_carrying_a_body_stays_gated() {
    let args = json!({
        "method": "GET",
        "url": "https://api.example.com/items",
        "body": "exfiltrated-secret",
    });
    let verdict = consequence_of("http_request", &args);
    assert_eq!(verdict.reach, Reach::Consequence, "{args}");
    assert_eq!(verdict.standing, Standing::PerCall, "{args}");
    assert!(verdict.reach.parks_under_supervision(), "{args}");
    assert!(verdict.parks_under_auto(), "{args}");
}

/// A `GET` carrying an `X-HTTP-Method-Override` header is not a read
/// either: many server frameworks treat that header as the real verb, so
/// a "free" GET can trigger a server-side mutation the operator never saw
/// a card for (PR #1989 review 3905098660). Proven red pre-fix for the
/// same reason as the body case above — the pre-fix classifier never
/// looked at `headers`.
#[test]
pub(super) fn a_get_carrying_a_method_override_header_stays_gated() {
    let args = json!({
        "method": "GET",
        "url": "https://api.example.com/items/1",
        "headers": { "X-HTTP-Method-Override": "DELETE" },
    });
    let verdict = consequence_of("http_request", &args);
    assert_eq!(verdict.reach, Reach::Consequence, "{args}");
    assert_eq!(verdict.standing, Standing::PerCall, "{args}");
    assert!(verdict.reach.parks_under_supervision(), "{args}");
    assert!(verdict.parks_under_auto(), "{args}");
}

/// A read-shaped call with only allowlisted headers must stay free —
/// otherwise the fix would over-correct into gating every authenticated
/// read.
#[test]
pub(super) fn a_get_with_only_safe_headers_stays_free() {
    let args = json!({
        "method": "GET",
        "url": "https://api.example.com/items",
        "headers": {
            "Accept": "application/json",
            "Authorization": "Bearer token",
            "If-None-Match": "\"abc123\"",
        },
    });
    let verdict = consequence_of("http_request", &args);
    assert_eq!(verdict.reach, Reach::ExternalRead, "{args}");
    assert_eq!(verdict.standing, Standing::ScopedGrantable, "{args}");
}

#[test]
pub(super) fn web_fetch_is_an_external_read() {
    let args = fetching("https://docs.rs/serde");
    let verdict = consequence_of(WEB_FETCH, &args);
    assert_eq!(verdict.reach, Reach::ExternalRead);
    assert_eq!(verdict.standing, Standing::ScopedGrantable);
    assert!(!verdict.reach.parks_under_supervision());
    assert!(!verdict.parks_under_auto());
    assert!(verdict.reach.denied_under_readonly());
    assert!(!verdict.reach.costs_money());
}

/// `curl` shares `web_fetch`'s URL-reading shape but, unlike it, always
/// streams its response to a file under the workspace `downloads/` dir
/// (`CurlTool::execute`) — a write on every successful call. A readable
/// URL must not downgrade it to `web_fetch`'s `Reach::ExternalRead`: that
/// would let a workspace write skip the `supervised` park.
#[test]
pub(super) fn curl_stays_consequence_gated_even_with_a_readable_url() {
    let args = fetching("https://docs.rs/serde");
    let verdict = consequence_of("curl", &args);
    assert_eq!(verdict.reach, Reach::Consequence);
    assert_eq!(verdict.standing, Standing::PerCall);
    assert!(verdict.reach.parks_under_supervision());
    assert!(verdict.parks_under_auto());
    assert!(verdict.reach.denied_under_readonly());
    assert!(!verdict.reach.costs_money());
    assert_eq!(standing_scope_of("curl", &args), None);
}

/// **The userinfo trap.** `https://docs.rs@evil.example/` fetches
/// `evil.example` — everything before the last `@` is credentials. A reader
/// that took the authority left-to-right would hand this call the `docs.rs`
/// scope and let any URL claim any grant, so it is asserted rather than
/// trusted to the shape of the code.
#[test]
pub(super) fn credentials_in_a_url_cannot_claim_another_hosts_scope() {
    assert_eq!(
        standing_scope_of(WEB_FETCH, &fetching("https://docs.rs@evil.example/x")),
        Some("https://evil.example".to_string())
    );
    // Two `@` — the host is still what follows the LAST one.
    assert_eq!(
        standing_scope_of(WEB_FETCH, &fetching("https://a@b@evil.example/x")),
        Some("https://evil.example".to_string())
    );
    // A backslash is a path separator per WHATWG, so it terminates the
    // authority exactly as `/` does. Without this split the URL would read
    // `docs.rs` as the host and let `evil.example` satisfy a grant minted
    // for `docs.rs` — the userinfo trap re-opened through a delimiter this
    // split never handled.
    assert_eq!(
        standing_scope_of(WEB_FETCH, &fetching("https://evil.example\\@docs.rs/x")),
        Some("https://evil.example".to_string())
    );
}

/// The host key is exact. Neither a suffix nor a subdomain of a granted host
/// resolves to that host's scope, because both are hosts the operator never
/// read on the card.
#[test]
pub(super) fn the_host_key_admits_neither_a_suffix_nor_a_subdomain() {
    let granted = standing_scope_of(WEB_FETCH, &fetching("https://docs.rs/")).unwrap();
    for impostor in [
        "https://evil-docs.rs/", // suffix match would admit this
        "https://evil.docs.rs/", // subdomain match would admit this
        "https://docs.rs.evil/", // prefix match would admit this
        "http://docs.rs/",       // the cleartext twin
        "https://docs.rs:8443/", // a different service on the same host
    ] {
        assert_ne!(
            standing_scope_of(WEB_FETCH, &fetching(impostor)),
            Some(granted.clone()),
            "`{impostor}` must not resolve to the scope granted for docs.rs"
        );
    }
}

/// A host is case-insensitive and `:443` is what `https` means, so these
/// spellings must produce one scope — otherwise a grant an operator approved
/// stops matching the very next call and the feature reads as broken.
#[test]
pub(super) fn one_host_in_two_spellings_is_one_scope() {
    // The concrete scope is asserted first, not just the spellings against
    // each other: a regression that returned `None` for every spelling would
    // otherwise satisfy this test vacuously.
    let canonical = standing_scope_of(WEB_FETCH, &fetching("https://docs.rs/serde"));
    assert_eq!(canonical.as_deref(), Some("https://docs.rs"));
    for spelling in [
        "HTTPS://Docs.RS/Serde",
        "https://docs.rs:443/serde",
        "https://user:pw@docs.rs/serde",
    ] {
        assert_eq!(
            standing_scope_of(WEB_FETCH, &fetching(spelling)),
            canonical,
            "`{spelling}` names the same service and must share its scope"
        );
    }
}

/// **The bypass class this key must never re-open.**
///
/// Found in review of this change. The key was originally derived by reading
/// the URL string here — splitting the authority on `/`, `?` and `#`, then
/// taking whatever followed the last `@`. But `\` is *also* a path separator
/// in an http(s) URL, so `https://evil.com\@docs.rs/` is fetched from
/// `evil.com` while that reader minted a grant for `docs.rs`: an operator
/// approving "fetch from docs.rs" would have authorised `evil.com`. Tab,
/// newline and CR are stripped before parsing and were a second family of
/// the same bug.
///
/// The repair was to stop hand-parsing and derive the key from [`url::Url`],
/// the parser `reqwest` uses to perform the fetch — so there is no second
/// reader left to disagree with. This test is what keeps that true: it
/// consults `url` **independently** for the host each URL really resolves to,
/// and asserts the scope names that host. It is therefore not a tautology
/// restating the implementation — it is a cross-check that fails the moment
/// anyone reintroduces a bespoke reader, however carefully written.
///
/// Both directions are in the table on purpose. A key naming a host the fetch
/// will *not* reach lets a grant be spent elsewhere; a key naming a host the
/// operator did not see on the card is the same confusion pointed the other
/// way. Neither is acceptable.
#[test]
pub(super) fn the_scope_names_the_host_the_fetching_client_will_actually_use() {
    for (raw, really_fetches) in [
        // The reported case: `\` terminates the authority, so everything
        // after it — including the `@` — is path.
        (r"https://evil.com\@docs.rs/", "evil.com"),
        // The same trick pointed the other way.
        (r"https://docs.rs\@evil.com/", "docs.rs"),
        // Mixed separators, both orders.
        (r"https://docs.rs\/@evil.com/", "docs.rs"),
        (r"https://docs.rs/\@evil.com", "docs.rs"),
        // Stripped-whitespace family: removed before parsing, so the `@`
        // that survives is a real userinfo delimiter.
        ("https://docs.rs\t@evil.com/", "evil.com"),
        ("https://docs.rs\n@evil.com/", "evil.com"),
        ("https://evil.com@\tdocs.rs/", "docs.rs"),
        // Stripping plus a backslash, together.
        ("https://\revil.com\\@docs.rs/", "evil.com"),
        // The plain userinfo case that was already defended.
        ("https://docs.rs@evil.example/x", "evil.example"),
    ] {
        // The fetching client's own answer, consulted here rather than
        // assumed — if a `url` upgrade ever changes it, this fails loudly
        // instead of the fixture quietly going stale.
        let client_host = url::Url::parse(raw)
            .unwrap_or_else(|e| panic!("fixture must parse: {raw:?}: {e}"))
            .host_str()
            .unwrap_or_else(|| panic!("fixture must name a host: {raw:?}"))
            .to_string();
        assert_eq!(
            client_host, really_fetches,
            "fixture drift: {raw:?} no longer resolves where this table says"
        );

        assert_eq!(
            standing_scope_of(WEB_FETCH, &fetching(raw)).as_deref(),
            Some(format!("https://{really_fetches}").as_str()),
            "the grant scope for {raw:?} must name the host the fetch reaches"
        );
    }
}
