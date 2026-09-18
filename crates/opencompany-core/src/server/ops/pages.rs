//! Serves agent-authored internal dashboard pages (`pages/<slug>/` in the
//! company workspace) to the operator console.
//!
//! ```text
//! GET …/pages                    every page's manifest, for the nav
//! GET …/pages/{slug}             a fixed HTML shell that mounts the page
//! GET …/pages/{slug}/bootstrap.mjs  the fixed mounting module (capability-gated)
//! GET …/pages/{slug}/bundle.mjs  the page's compiled JS, streamed (capability-gated)
//! ```
//!
//! See `docs/spec/runtime/pages.md` for the full design. The load-bearing
//! difference from `…/workspace/blob/{id}` ([`super::workspace::read_blob`]),
//! which *refuses* to render anything inline for exactly this class of risk
//! (issue #667): the bytes served here are never a raw upload. They are the
//! output of [`crate::harness::pages_tools::compile_page`] — a TSX source
//! parsed, import-checked against an allow-list, and re-rendered by `swc_core`
//! — so serving them as `application/javascript` with `Content-Disposition:
//! inline` is serving *validated compiled output*, not an arbitrary payload a
//! caller uploaded. The isolation boundary a browser actually needs — this is
//! still third-party-authored code running in the browser — is the sandboxed
//! iframe (`sandbox="allow-scripts"`, no `allow-same-origin`) the frontend
//! embeds it in, and the CSP headers below are defense in depth on top of
//! that, not a substitute for it.
//!
//! `harness::pages_tools` is compiled only under the `openhuman` feature (all
//! of `src/harness/` is); this module is always compiled, because the routes
//! it serves must 404 rather than fall through to the console SPA shell even
//! in a build without the harness. So it does not import from
//! `harness::pages_tools` — it re-derives the same `pages/<slug>/` layout from
//! the always-compiled constants in
//! [`crate::company::workspace_scaffold`], the same way `harness::pages_tools`
//! does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{FromRequestParts, Path, Query, RawPathParams};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::company::runtime::CompanyRuntime;
use crate::company::workspace_scaffold::{
    PAGE_COMPILED_MIME, PAGE_COMPILED_NAME, PAGE_MANIFEST_NAME, PAGES_ROOT,
};
use crate::error::OpenCompanyError;
use crate::ports::types::CompanyId;
use crate::ports::workspace::{NodeKind, WorkspaceNode, WorkspaceStore};
use crate::server::error::ApiError;
use crate::server::ops::{ScopedCompany, scoped};

/// The Content-Security-Policy every route in this module sets (plan §5).
///
/// `script-src 'self'` plus `'unsafe-inline'` covers the fixed HTML shell's
/// inline `<script type="module">`; `connect-src 'none'` means the shell
/// itself cannot open its own network requests — the page's real data access
/// is the frontend's postMessage bridge to the parent console tab, which this
/// header does not need to permit because it never leaves the frame as a
/// request this origin makes. `frame-ancestors 'self'` keeps the shell from
/// being embedded anywhere but this console.
const PAGES_CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
     font-src 'self' data:; connect-src 'none'; frame-ancestors 'self'";

/// Builds the pages route fragment.
pub fn router() -> Router<AppState> {
    scoped("/pages", get(list_pages))
        .merge(scoped("/pages/{slug}", get(page_shell)))
        .merge(scoped("/pages/{slug}/bootstrap.mjs", get(page_bootstrap)))
        .merge(scoped("/pages/{slug}/bundle.mjs", get(bundle)))
}

#[derive(Debug, serde::Deserialize)]
struct SlugPath {
    slug: String,
}

/// The capability query parameter the shell embeds in its module URLs.
///
/// Only the two module routes parse it; everything else stays session-
/// authenticated exactly as before.
#[derive(Debug, serde::Deserialize)]
struct ModuleCapQuery {
    /// The short-lived capability minted by the shell ([`mint_module_cap`]).
    oc_cap: String,
}

/// How long a minted page-module capability stays valid.
///
/// The shell mints one per load and the module graph (bootstrap + bundle) is
/// fetched within the first moments of that load; a minute of slack covers a
/// slow network without leaving a replayable token lying around.
const MODULE_CAP_TTL: Duration = Duration::from_secs(60);

/// One minted capability's scope: which page it authorizes and when it stops.
struct ModuleCap {
    company: CompanyId,
    slug: String,
    expires_at: Instant,
}

/// The live set of minted capabilities, keyed by the token itself.
///
/// The two module routes consume these instead of a session. They must: the
/// opaque-origin page iframe cannot attach the operator's session cookie to
/// its module imports (docs/spec/runtime/pages.md §5), so the shell — which
/// *can* authenticate, because the iframe loads it by navigation — mints an
/// unguessable token here and embeds it in the module URLs. The map is bounded
/// by real page loads and lazily swept on every access, so it stays small;
/// entries also die with the process, which is fine because each is minted
/// fresh per shell load and lives at most [`MODULE_CAP_TTL`].
static MODULE_CAPS: OnceLock<Mutex<HashMap<String, ModuleCap>>> = OnceLock::new();

fn module_caps() -> &'static Mutex<HashMap<String, ModuleCap>> {
    MODULE_CAPS.get_or_init(Default::default)
}

/// Mints an unguessable, short-lived capability for one page's module graph.
fn mint_module_cap(company: &CompanyId, slug: &str) -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .expect("the OS CSPRNG is unavailable; cannot mint a page-module capability");
    let cap = hex_encode(&bytes);
    let mut caps = module_caps()
        .lock()
        .expect("page-module capability map lock");
    // Lazy sweep: drop everything that has expired since the last access.
    caps.retain(|_, c| c.expires_at > Instant::now());
    caps.insert(
        cap.clone(),
        ModuleCap {
            company: company.clone(),
            slug: slug.to_string(),
            expires_at: Instant::now() + MODULE_CAP_TTL,
        },
    );
    cap
}

/// Whether `cap` currently authorizes this company's `slug` module graph.
///
/// Bound to the company *and* the slug, so a capability minted for one page
/// cannot open another page's bundle, and a capability minted for one company
/// cannot be replayed against another.
fn validate_module_cap(cap: &str, company: &CompanyId, slug: &str) -> bool {
    let mut caps = module_caps()
        .lock()
        .expect("page-module capability map lock");
    caps.retain(|_, c| c.expires_at > Instant::now());
    caps.get(cap)
        .is_some_and(|c| c.company == *company && c.slug == slug && c.expires_at > Instant::now())
}

/// Lowercase-hex encoding for a capability token: URL-safe and unguessable.
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Infallible: writing to a String never fails.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Resolves the addressed company exactly as [`ScopedCompany`] does, but
/// admits the request without a session.
///
/// The two module routes are the sole consumers. An opaque-origin sandboxed
/// iframe cannot attach the operator's session cookie to its module imports,
/// so the shell — an authenticated navigation — mints a short-lived capability
/// and embeds it in the module URLs; these routes validate that capability
/// ([`validate_module_cap`]) in the handler, against the company *and* the
/// slug, instead of a session. Nothing else is ever registered behind this
/// extractor, so the capability's reach is exactly the module graph.
struct ModuleScopedCompany {
    runtime: Arc<CompanyRuntime>,
}

impl FromRequestParts<AppState> for ModuleScopedCompany {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let id = RawPathParams::from_request_parts(parts, state)
            .await
            .ok()
            .and_then(|params| {
                params
                    .iter()
                    .find(|(key, _)| *key == "id")
                    .map(|(_, value)| value.to_string())
            });
        let runtime = match &id {
            Some(id) => state.registry().get(&CompanyId::new(id)).ok_or_else(|| {
                ApiError(OpenCompanyError::CompanyNotFound(id.clone())).into_response()
            })?,
            None => state.registry().sole().ok_or_else(|| {
                ApiError(OpenCompanyError::CompanyNotFound(
                    "single-company".to_string(),
                ))
                .into_response()
            })?,
        };
        Ok(ModuleScopedCompany { runtime })
    }
}

/// One page's manifest, as the console nav consumes it.
///
/// Field names match what `PagesView.tsx` (the frontend nav, built alongside
/// this route) reads: `slug`, `title`, `description`, `icon`, `navVisible`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PageListing {
    slug: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    nav_visible: bool,
}

/// A page's small manifest, as stored in `page.toml`.
///
/// Mirrors `harness::pages_tools::PageManifest` field-for-field; kept as a
/// separate type rather than shared because that one lives behind the
/// `openhuman` feature and this route must parse the same TOML without it.
#[derive(Debug, Deserialize)]
struct StoredManifest {
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default = "default_nav_visible")]
    nav_visible: bool,
}

fn default_nav_visible() -> bool {
    true
}

/// A slug's resolved bundle: whichever of its three nodes exist.
struct PageBundle {
    manifest: Option<WorkspaceNode>,
    compiled: Option<WorkspaceNode>,
}

/// Whether `slug` is a safe path segment to build a workspace lookup and a
/// URL path from.
///
/// The HTML this route serves is a fixed Rust format string — not agent
/// content — so there is no injection risk from the slug reaching the
/// response body; this check exists so a malformed slug resolves to a clean
/// 404 instead of an ambiguous or surprising tree lookup. Mirrors
/// `harness::pages_tools::valid_slug`.
fn valid_slug(slug: &str) -> bool {
    let mut chars = slug.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Resolves every `pages/<slug>/` bundle from one company-scoped tree read.
async fn all_pages(
    store: &dyn WorkspaceStore,
    company: &CompanyId,
) -> crate::Result<Vec<(String, PageBundle)>> {
    let nodes = store.tree(company).await?;
    // Case-insensitive, matching `harness::pages_tools`: the root and the
    // compiled node were `Pages/` and `Page.compiled.mjs` before the workspace's
    // lowercase-dashed rule, and this route serves exactly what those tools
    // wrote — including in a company created under the old spelling.
    let Some(pages_root) = nodes.iter().find(|n| {
        n.parent_id.is_none()
            && n.kind == NodeKind::Folder
            && n.name.eq_ignore_ascii_case(PAGES_ROOT)
    }) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for folder in nodes.iter().filter(|n| {
        n.kind == NodeKind::Folder && n.parent_id.as_deref() == Some(pages_root.id.as_str())
    }) {
        let mut bundle = PageBundle {
            manifest: None,
            compiled: None,
        };
        for child in nodes
            .iter()
            .filter(|n| n.parent_id.as_deref() == Some(folder.id.as_str()))
        {
            if child.name.eq_ignore_ascii_case(PAGE_MANIFEST_NAME) {
                bundle.manifest = Some(child.clone());
            } else if child.name.eq_ignore_ascii_case(PAGE_COMPILED_NAME) {
                bundle.compiled = Some(child.clone());
            }
        }
        out.push((folder.name.clone(), bundle));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

async fn read_manifest(
    store: &dyn WorkspaceStore,
    company: &CompanyId,
    node: &WorkspaceNode,
    fallback_title: &str,
) -> StoredManifest {
    let body = match store.read(company, &node.id).await {
        Ok(Some((_, body))) => body,
        _ => String::new(),
    };
    toml::from_str(&body).unwrap_or_else(|_| StoredManifest {
        title: fallback_title.to_string(),
        description: None,
        icon: None,
        nav_visible: true,
    })
}

/// `GET {scope}/pages` — every page's manifest, for the console nav.
async fn list_pages(company: ScopedCompany) -> Result<Response, ApiError> {
    let pages = all_pages(company.runtime.workspace().as_ref(), company.id()).await?;
    let mut listings = Vec::with_capacity(pages.len());
    for (slug, bundle) in &pages {
        let manifest = match &bundle.manifest {
            Some(node) => {
                read_manifest(
                    company.runtime.workspace().as_ref(),
                    company.id(),
                    node,
                    slug,
                )
                .await
            }
            None => StoredManifest {
                title: slug.clone(),
                description: None,
                icon: None,
                nav_visible: true,
            },
        };
        listings.push(PageListing {
            slug: slug.clone(),
            title: manifest.title,
            description: manifest.description,
            icon: manifest.icon,
            nav_visible: manifest.nav_visible,
        });
    }
    let mut response = Json(listings).into_response();
    apply_pages_headers(response.headers_mut());
    Ok(response)
}

/// The fixed HTML shell that mounts a page's compiled module (not agent
/// content). Extracted from the route so the shell's load-bearing invariants
/// — the React namespace import, the slug-relative bootstrap path, the
/// shell-minted capability, the unconditional SDK load, the import map — are
/// unit-testable instead of living only in a route that needs a full workspace
/// to exercise.
///
/// `cap` is the short-lived capability minted by the authenticated shell route
/// ([`mint_module_cap`]); it rides the bootstrap module's URL so the module
/// graph, which the opaque-origin iframe fetches without a session cookie, can
/// still be authorized server-side.
fn page_shell_html(slug: &str, cap: &str) -> String {
    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{slug}</title>
<link rel="stylesheet" href="/pages-sdk/index.css">
<script type="importmap">
{{
  "imports": {{
    "react": "/pages-sdk/react.mjs",
    "react-dom/client": "/pages-sdk/react.mjs",
    "react/jsx-runtime": "/pages-sdk/react.mjs",
    "@opencompany/site": "/pages-sdk/index.mjs"
  }}
}}
</script>
</head>
<body>
<div id="root"></div>
<script type="module">
  import "@opencompany/site";
</script>
<script type="module" src="./{slug}/bootstrap.mjs?oc_cap={cap}"></script>
</body>
</html>
"#,
        slug = slug,
        cap = cap,
    )
}

/// The fixed external module that mounts one page bundle.
///
/// Served from the [`page_bootstrap`] route with the shell-minted capability
/// interpolated into the bundle's own import URL. The shell is the only
/// authenticated party in the load chain; the capability it mints is what lets
/// the opaque-origin module graph fetch the authenticated bundle — a static
/// import's URL is resolved against the importing module's own URL, so the
/// `?oc_cap` query has to be threaded here explicitly or the bundle request
/// would drop it.
fn page_bootstrap_body(cap: &str) -> String {
    format!(
        r#"import * as React from "react";
import * as ReactDOM from "react-dom/client";
import Page from "./bundle.mjs?oc_cap={cap}";
const root = ReactDOM.createRoot(document.getElementById("root"));
root.render(React.createElement(Page));
"#,
        cap = cap
    )
}

async fn page_bootstrap(
    ModuleScopedCompany { runtime }: ModuleScopedCompany,
    Query(query): Query<ModuleCapQuery>,
    Path(SlugPath { slug }): Path<SlugPath>,
) -> Result<Response, ApiError> {
    if !valid_slug(&slug) {
        return Err(ApiError(OpenCompanyError::NotFound(format!("page {slug}"))));
    }
    let company = runtime.id();
    if !validate_module_cap(&query.oc_cap, company, &slug) {
        return Err(ApiError(OpenCompanyError::NotFound(format!("page {slug}"))));
    }
    let pages = all_pages(runtime.workspace().as_ref(), company).await?;
    if !pages.iter().any(|(name, _)| name == &slug) {
        return Err(ApiError(OpenCompanyError::NotFound(format!("page {slug}"))));
    }
    let body = page_bootstrap_body(&query.oc_cap);
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, PAGE_COMPILED_MIME)
        .body(Body::from(body))
        .map_err(|e| {
            ApiError(OpenCompanyError::Store(format!(
                "page bootstrap failed: {e}"
            )))
        })?;
    apply_pages_headers(response.headers_mut());
    apply_page_module_cors_headers(response.headers_mut());
    Ok(response)
}

/// `GET {scope}/pages/{slug}` — a fixed HTML shell that mounts the page.
///
/// Not agent content: the slug is validated and interpolated into a literal
/// Rust format string, so nothing the page's own source contains ever reaches
/// this response.
async fn page_shell(
    company: ScopedCompany,
    Path(SlugPath { slug }): Path<SlugPath>,
) -> Result<Response, ApiError> {
    if !valid_slug(&slug) {
        return Err(ApiError(OpenCompanyError::NotFound(format!("page {slug}"))));
    }
    let pages = all_pages(company.runtime.workspace().as_ref(), company.id()).await?;
    if !pages.iter().any(|(name, _)| name == &slug) {
        return Err(ApiError(OpenCompanyError::NotFound(format!("page {slug}"))));
    }

    // This route is session-authenticated — the iframe loads the shell by
    // navigation, which does attach the cookie — but the module graph it points
    // at does not get that cookie. Mint the capability the graph needs here,
    // bound to this company and page, and hand it out in the module URLs.
    let cap = mint_module_cap(company.id(), &slug);
    let html = page_shell_html(&slug, &cap);

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(html))
        .map_err(|e| ApiError(OpenCompanyError::Store(format!("page shell failed: {e}"))))?;
    apply_pages_headers(response.headers_mut());
    Ok(response)
}

/// `GET {scope}/pages/{slug}/bundle.mjs` — the page's compiled JS, streamed.
async fn bundle(
    ModuleScopedCompany { runtime }: ModuleScopedCompany,
    Query(query): Query<ModuleCapQuery>,
    Path(SlugPath { slug }): Path<SlugPath>,
) -> Result<Response, ApiError> {
    if !valid_slug(&slug) {
        return Err(ApiError(OpenCompanyError::NotFound(format!("page {slug}"))));
    }
    let company = runtime.id();
    if !validate_module_cap(&query.oc_cap, company, &slug) {
        return Err(ApiError(OpenCompanyError::NotFound(format!("page {slug}"))));
    }
    let pages = all_pages(runtime.workspace().as_ref(), company).await?;
    let Some((_, bundle)) = pages.into_iter().find(|(name, _)| name == &slug) else {
        return Err(ApiError(OpenCompanyError::NotFound(format!("page {slug}"))));
    };
    let Some(compiled) = bundle.compiled else {
        return Err(ApiError(OpenCompanyError::NotFound(format!(
            "page {slug} has not been compiled yet"
        ))));
    };

    // The compiled bundle reaches the tree two ways: `pages_write` stores it as
    // a *binary* node (`mime` set, size and sha256 computed by the store), while
    // an operator creating `page.compiled.mjs` through the console — or a test
    // seeding the tree over the workspace API — stores it as a plain text file.
    // Both hold the same JS bytes, so serve whichever kind the node is; a route
    // that served only one would 404 a legitimate page over a storage detail
    // that has nothing to do with what the module graph needs.
    let (node, body) = if compiled.is_binary() {
        let Some((node, stream)) = runtime
            .workspace()
            .read_bytes(company, &compiled.id)
            .await?
        else {
            return Err(ApiError(OpenCompanyError::NotFound(format!(
                "page {slug} bundle"
            ))));
        };
        (node, Body::from_stream(stream))
    } else {
        let Some((node, content)) = runtime.workspace().read(company, &compiled.id).await? else {
            return Err(ApiError(OpenCompanyError::NotFound(format!(
                "page {slug} bundle"
            ))));
        };
        (node, Body::from(content))
    };

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, PAGE_COMPILED_MIME)
        .header(header::CONTENT_DISPOSITION, "inline")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if let Some(sha) = &node.sha256 {
        response = response.header(header::ETAG, format!("\"{sha}\""));
    }
    if let Some(size) = node.size {
        response = response.header(header::CONTENT_LENGTH, size);
    }
    let mut response = response.body(body).map_err(|e| {
        ApiError(OpenCompanyError::Store(format!(
            "bundle response failed: {e}"
        )))
    })?;
    apply_pages_headers(response.headers_mut());
    apply_page_module_cors_headers(response.headers_mut());
    Ok(response)
}

/// Sets the CSP and `X-Content-Type-Options` headers every route in this
/// module carries (plan §5), without disturbing whatever content-type header
/// the caller already set.
fn apply_pages_headers(headers: &mut axum::http::HeaderMap) {
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(PAGES_CSP),
    );
    // Authenticated, company-specific content: never let a browser or an
    // intermediary cache reuse another company's (or another session's) page
    // shell, manifest, or bundle.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
}

/// Allows the opaque-origin page iframe to load a module response.
///
/// The shell deliberately omits `allow-same-origin`, which makes module
/// imports send `Origin: null` — every module in the graph is a CORS request
/// from that null origin. Module scripts always fetch with CORS, so the
/// response must admit the origin explicitly. `Access-Control-Allow-Origin:
/// null` matches the opaque origin exactly; `Access-Control-Allow-Credentials:
/// true` is harmless (the frame sends no credentials) and keeps the pair
/// consistent across every module the shell references. This precise response
/// pair is therefore required for the pages module graph; the general CORS
/// middleware cannot provide it because same-origin console deployments leave
/// that middleware disabled.
pub(crate) fn apply_page_module_cors_headers(headers: &mut axum::http::HeaderMap) {
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("null"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
}

#[cfg(test)]
#[path = "pages_tests.rs"]
mod tests;
