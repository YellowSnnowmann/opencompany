use super::*;

fn headers(pairs: &[&str]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for p in pairs {
        h.append(COOKIE, p.parse().unwrap());
    }
    h
}

#[test]
fn cookie_name_carries_the_company() {
    let name = session_cookie_name(&CompanyId::new("acme")).unwrap();
    assert_eq!(name, "oc_session_acme");
    assert_eq!(company_from_cookie_name(&name), Some("acme"));
    assert_eq!(company_from_cookie_name("unrelated"), None);
    assert_eq!(company_from_cookie_name("oc_session_"), None);
}

#[test]
fn minted_company_ids_can_name_a_cookie() {
    // The runtime's own id shape must never be rejected.
    let id = CompanyId::new(crate::ports::generate_id());
    assert!(session_cookie_name(&id).is_some(), "{id:?} was rejected");
}

#[test]
fn a_company_id_that_could_forge_a_header_gets_no_cookie() {
    // CompanyId::new validates nothing, so this is reachable. Emitting
    // `oc_session_evil; Path=/; HttpOnly=x=y` would let the id choose the
    // cookie's attributes.
    for hostile in [
        "evil;Path=/",
        "evil=x",
        "evil name",
        "evil\nSet-Cookie: a=b",
        "",
        "evil;Secure",
    ] {
        assert!(
            session_cookie_name(&CompanyId::new(hostile)).is_none(),
            "{hostile:?} must not be able to name a cookie"
        );
    }
}

#[test]
fn parses_multiple_cookies_from_one_header() {
    let h = headers(&["a=1; oc_session_acme=tok; b=2"]);
    assert_eq!(cookie(&h, "oc_session_acme").as_deref(), Some("tok"));
    assert_eq!(cookie(&h, "a").as_deref(), Some("1"));
    assert_eq!(cookie(&h, "missing"), None);
}

#[test]
fn parses_across_separate_cookie_headers() {
    // A client may split cookies across headers; missing one would drop a
    // session that was actually presented.
    let h = headers(&["a=1", "oc_session_acme=tok"]);
    assert_eq!(cookie(&h, "oc_session_acme").as_deref(), Some("tok"));
}

#[test]
fn tolerates_whitespace_and_odd_pairs() {
    let h = headers(&["  a = 1 ;;  oc_session_acme =  tok  ; junk ; =novalue"]);
    assert_eq!(cookie(&h, "oc_session_acme").as_deref(), Some("tok"));
    assert_eq!(cookie(&h, "a").as_deref(), Some("1"));
    // A pair with no '=' and one with an empty name are skipped, not fatal.
    assert_eq!(cookie(&h, "junk"), None);
}

#[test]
fn keeps_equals_signs_inside_a_value() {
    let h = headers(&["t=aa==bb"]);
    assert_eq!(cookie(&h, "t").as_deref(), Some("aa==bb"));
}

#[test]
fn later_duplicate_wins() {
    let h = headers(&["t=first; t=second"]);
    assert_eq!(cookie(&h, "t").as_deref(), Some("second"));
}

#[test]
fn no_cookie_header_is_not_an_error() {
    assert!(parse_cookies(&HeaderMap::new()).is_empty());
    assert_eq!(cookie(&HeaderMap::new(), "t"), None);
}

fn carrier(value: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(SESSION_CARRIER_HEADER, value.parse().unwrap());
    h
}

#[test]
fn the_header_carrier_is_opt_in() {
    assert!(wants_header_carrier(&carrier("header")));
    // Case and surrounding space are the client's business, not a refusal.
    assert!(wants_header_carrier(&carrier("Header")));
    assert!(wants_header_carrier(&carrier("  header ")));
}

#[test]
fn anything_else_means_the_cookie() {
    // The default must be the HttpOnly carrier, so an absent, empty or
    // unrecognised value degrades to the safer one rather than to none.
    assert!(!wants_header_carrier(&HeaderMap::new()));
    for value in ["", "cookie", "bearer", "headers", "header-ish", "1"] {
        assert!(
            !wants_header_carrier(&carrier(value)),
            "{value:?} must not select the header carrier"
        );
    }
}

#[test]
fn the_session_header_value_is_what_the_parser_reads_back() {
    // The two are each other's inverse, and a round trip is the only
    // assertion that stays true if either side's format changes.
    let rendered = session_header_value(&CompanyId::new("acme"), "tok").unwrap();
    assert_eq!(rendered, "acme.tok");
    let mut h = HeaderMap::new();
    h.insert(SESSION_HEADER, rendered.parse().unwrap());
    let (company, token) = session_from_header(&h).unwrap();
    assert_eq!(company.as_ref(), "acme");
    assert_eq!(token, "tok");
}

#[test]
fn a_company_that_cannot_name_a_cookie_cannot_name_a_header_either() {
    // Both carriers gate on `may_carry_session`, so an id is either
    // addressable by both or by neither. An id expressible in one carrier
    // only would be a company reachable by desktop and not by browser.
    for id in ["", "evil;Secure", "a.b", "with space"] {
        let company = CompanyId::new(id);
        assert_eq!(session_cookie_name(&company), None, "{id:?}");
        assert_eq!(session_header_value(&company, "tok"), None, "{id:?}");
    }
}

#[test]
fn an_empty_token_never_renders_a_header() {
    // `session_from_header` refuses an empty token, so rendering one would
    // hand back a value that cannot authenticate anything.
    assert_eq!(session_header_value(&CompanyId::new("acme"), ""), None);
}

#[test]
fn set_cookie_carries_the_defensive_attributes() {
    let rendered = set_cookie("oc_session_acme", "tok", 3600, false);
    assert!(rendered.starts_with("oc_session_acme=tok;"));
    assert!(rendered.contains("HttpOnly"), "{rendered}");
    assert!(rendered.contains("SameSite=Lax"), "{rendered}");
    assert!(rendered.contains("Path=/"), "{rendered}");
    assert!(rendered.contains("Max-Age=3600"), "{rendered}");
    assert!(rendered.contains("Secure"), "{rendered}");
}

#[test]
fn secure_is_dropped_only_for_insecure_dev() {
    let rendered = set_cookie("t", "v", 60, true);
    assert!(
        !rendered.contains("Secure"),
        "http loopback dev cannot set Secure: {rendered}"
    );
    // Everything else still applies.
    assert!(rendered.contains("HttpOnly"));
}

#[test]
fn clear_cookie_expires_immediately_and_matches_set_attributes() {
    let rendered = clear_cookie("oc_session_acme", false);
    assert!(rendered.contains("Max-Age=0"), "{rendered}");
    // A browser only replaces a cookie when name/path match.
    assert!(rendered.contains("Path=/"), "{rendered}");
    assert!(rendered.contains("HttpOnly"), "{rendered}");
    assert!(rendered.contains("Secure"), "{rendered}");
}

#[test]
fn a_rendered_cookie_parses_back() {
    let token = super::super::token::mint_session_token(&super::super::token::OsTokens);
    let rendered = set_cookie("oc_session_acme", &token, 60, false);
    // Simulate the browser echoing just the name=value pair back.
    let pair = rendered.split(';').next().unwrap();
    let h = headers(&[pair]);
    assert_eq!(
        cookie(&h, "oc_session_acme").as_deref(),
        Some(token.as_str())
    );
}
