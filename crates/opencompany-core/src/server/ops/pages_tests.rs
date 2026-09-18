use super::*;

#[test]
fn slug_validation_matches_the_tool_side() {
    assert!(valid_slug("revenue"));
    assert!(valid_slug("revenue-2"));
    assert!(!valid_slug(""));
    assert!(!valid_slug("Revenue"));
    assert!(!valid_slug("../secrets"));
    assert!(!valid_slug("rev enue"));
}

#[test]
fn shell_embeds_the_minted_capability_in_the_bootstrap_url() {
    let html = page_shell_html("revenue", "cap-abc123");
    // The module graph is fetched by the opaque-origin iframe without a
    // session cookie, so the shell hands the capability it minted over in
    // the bootstrap module's URL — the only authenticated party in the
    // load chain is the shell route itself.
    assert!(
        html.contains(
            "<script type=\"module\" src=\"./revenue/bootstrap.mjs?oc_cap=cap-abc123\"></script>"
        ),
        "the bootstrap module URL must carry the shell-minted capability"
    );
}

#[test]
fn shell_bundle_path_is_relative_to_the_shells_own_url() {
    let html = page_shell_html("revenue", "cap");
    // Bug this guards (PR #985): the shell imported `./bundle.mjs`, which
    // resolves against `…/pages/{slug}` (no trailing slash) to
    // `…/pages/bundle.mjs` — the shell route with slug "bundle.mjs", which
    // fails `valid_slug` and 404s. `./{slug}/bootstrap.mjs` resolves to the
    // registered bootstrap route.
    assert!(
        html.contains("src=\"./revenue/bootstrap.mjs"),
        "the bootstrap module must be relative to the shell URL"
    );
}

#[test]
fn shell_links_the_sdk_css_and_maps_react_jsx_runtime_to_the_sdk_bundle() {
    let html = page_shell_html("revenue", "cap");
    // Bug this guards (PR #985): the SDK's `index.css` was built and
    // shipped but never linked, so every page rendered unstyled.
    assert!(
        html.contains("<link rel=\"stylesheet\" href=\"/pages-sdk/index.css\">"),
        "the SDK stylesheet must be linked in the shell"
    );
    // The import map is what lets the compiler's automatic-jsx output
    // (`import { jsx } from "react/jsx-runtime"`) link at all.
    assert!(
        html.contains("\"react/jsx-runtime\": \"/pages-sdk/react.mjs\""),
        "react/jsx-runtime must resolve to the SDK's React bundle"
    );
}

#[test]
fn bootstrap_threads_the_capability_to_the_bundle_import() {
    let body = page_bootstrap_body("cap-def456");
    // A static import's URL is resolved against the importing module's own
    // URL, so the `?oc_cap` query on the bootstrap URL does NOT propagate
    // to its `./bundle.mjs` import — the bootstrap must pass the validated
    // capability along explicitly or the bundle request would 404.
    assert!(
        body.contains("from \"./bundle.mjs?oc_cap=cap-def456\""),
        "the bootstrap must thread the capability into the bundle import"
    );
}

#[test]
fn capability_is_bound_to_its_company_and_slug() {
    let company = CompanyId::new("acme");
    let other_company = CompanyId::new("globex");
    let cap = mint_module_cap(&company, "revenue");

    assert!(validate_module_cap(&cap, &company, "revenue"));
    // A capability minted for one company cannot open another company's
    // module graph…
    assert!(!validate_module_cap(&cap, &other_company, "revenue"));
    // …nor another page in the same company.
    assert!(!validate_module_cap(&cap, &company, "finance"));
    assert!(!validate_module_cap(&cap, &company, "revenue-2"));
    // An unknown token is never valid.
    assert!(!validate_module_cap("deadbeef", &company, "revenue"));
}

#[test]
fn capability_tokens_are_url_safe() {
    let cap = mint_module_cap(&CompanyId::new("acme"), "revenue");
    assert!(
        cap.bytes().all(|b| b.is_ascii_hexdigit()),
        "capability must be hex-encoded to ride a URL: {cap}"
    );
}

#[test]
fn bundle_cors_headers_allow_the_opaque_origin_with_credentials() {
    let mut headers = axum::http::HeaderMap::new();
    apply_page_module_cors_headers(&mut headers);

    assert_eq!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
        "null"
    );
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .unwrap(),
        "true"
    );
    assert_eq!(headers.get(header::VARY).unwrap(), "Origin");
}

#[test]
fn shell_loads_the_page_sdk_even_for_a_page_that_does_not_import_it() {
    let html = page_shell_html("revenue", "cap");
    // The toast click relay (`toast-click-through.ts`) forwards a click on
    // a toast over this frame to the page SDK's own listener
    // (`pages-sdk/client.ts`), so the SDK must be present in every frame —
    // including a static page whose own bundle never imports it. Without
    // the shell import, a relayed click into such a page would reach no
    // listener and the control beneath the toast would stay blocked.
    assert!(
        html.contains("import \"@opencompany/site\";"),
        "the shell must load the page SDK itself: {html}"
    );
}
