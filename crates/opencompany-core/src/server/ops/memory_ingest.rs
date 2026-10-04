//! Dropping files, folders and links into a company's memory.
//!
//! ```text
//! POST …/memory/ingest        multipart: one or many files (folder drops carry paths)
//! POST …/memory/ingest/links  JSON: URLs to fetch and remember
//! ```
//!
//! ## What a drop becomes
//!
//! Text, filed as one document in the company's **brain** — OpenHuman's
//! memory under `team:<company>/source:<kind>` ([`crate::memory`]) — which
//! every teammate's recall reaches, so a dropped document reaches a teammate on
//! its next turn with no further step. The extraction rules live in
//! [`crate::ingest`]; this module is the transport and the reporting.
//!
//! The original bytes are **not** kept. See [`crate::ingest`] for why that is
//! the design and not an omission — in short, the workspace tree is where
//! files live, and a second copy of every upload there would make this page a
//! silently diverging file manager.
//!
//! ## One answer per file, never one for the batch
//!
//! A folder drop is dozens of files, and some of them are always going to be
//! `.DS_Store`, a PNG, or a scanned PDF with no text layer. Failing the whole
//! request over one of them would make the feature unusable on any real
//! folder, and silently skipping them would leave an operator believing their
//! folder is in memory when a third of it is not. So every file gets a row in
//! the response saying what happened to it, and the request itself succeeds as
//! long as it was well-formed.
//!
//! ## Links are fetched by the host, so the guard is the host's too
//!
//! `POST …/memory/ingest/links` makes this server issue an outbound request to
//! an operator-supplied URL — the shape of a server-side request forgery. The
//! request is therefore restricted to `http`/`https` and refused for any host
//! that is, or resolves to, a loopback, link-local, or private address, which
//! is what stops "remember this page" from being a read primitive against the
//! deployment's own network. The enforcing guard is TinyMemory's
//! `sources::fetch::fetch_url` (`tinymemory-integrations`), which connects only to the addresses it vetted
//! (closing DNS rebinding) and re-checks every redirect hop; the check in this
//! module is an early, readable refusal in front of it.

use axum::extract::Path;
use axum::extract::multipart::MultipartError;
use axum::extract::{DefaultBodyLimit, Multipart};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::OpenCompanyError;
use crate::ingest::{Extracted, MAX_DOCUMENT_BYTES, extract};
use crate::server::error::ApiError;
use crate::server::ops::scope::{ScopedCompany, scoped};

/// How much one ingest request may carry.
///
/// A folder drop is many files in one request, so this is a multiple of the
/// per-document cap rather than equal to it. The console still batches — this
/// is the ceiling, not the expected size.
const INGEST_BODY_LIMIT: usize = 8 * MAX_DOCUMENT_BYTES;

/// The largest page body a link ingest will read.
#[cfg(feature = "documents")]
const MAX_LINK_BYTES: usize = 4 * 1024 * 1024;

/// Builds the ingest route fragment.
pub fn router() -> Router<AppState> {
    scoped("/memory/ingest", post(ingest))
        .layer(DefaultBodyLimit::max(INGEST_BODY_LIMIT))
        .merge(scoped("/memory/ingest/links", post(ingest_links)))
        .merge(scoped("/memory/document/{source}", delete(forget_document)))
}

/// What happened to one dropped file or link.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IngestedItem {
    /// The name the operator will recognise — a file name, a relative path
    /// inside a dropped folder, or a URL.
    source: String,
    /// `stored`, `empty`, `unsupported`, or `failed`.
    status: &'static str,
    /// How many brain documents it became: `1` when stored, else `0`.
    chunks: usize,
    /// The brain source it was filed under (`pdf`, `markdown`, `web`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    brain_source: Option<String>,
    /// Why it is not `stored`, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

impl IngestedItem {
    fn failed(source: String, detail: String) -> Self {
        Self {
            source,
            status: "failed",
            chunks: 0,
            brain_source: None,
            detail: Some(detail),
        }
    }
}

/// The whole batch's outcome.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IngestedDto {
    /// One row per file or link, in the order they were sent.
    items: Vec<IngestedItem>,
    /// Documents filed across the batch — what the Brain counter will move by.
    chunks: usize,
    /// How many sources actually landed in memory.
    stored: usize,
}

impl IngestedDto {
    fn of(items: Vec<IngestedItem>) -> Self {
        Self {
            chunks: items.iter().map(|i| i.chunks).sum(),
            stored: items.iter().filter(|i| i.status == "stored").count(),
            items,
        }
    }
}

/// Links to fetch and remember.
///
/// Deserialized even in a build without the feature — the refusing handler
/// still takes the body, so a console gets the honest "this build cannot" and
/// not a deserialization error about a shape it sent correctly.
#[derive(Debug, Deserialize)]
struct LinksRequest {
    #[cfg_attr(
        not(feature = "documents"),
        allow(dead_code, reason = "the refusing handler reads no field")
    )]
    urls: Vec<String>,
}

/// Extracts one source's bytes and writes its chunks, returning its row.
///
/// Per-source rather than per-batch failure handling: see the module doc.
async fn store_source(
    company: &ScopedCompany,
    source: String,
    declared: Option<&str>,
    bytes: &[u8],
) -> IngestedItem {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return IngestedItem::failed(
            source,
            format!(
                "larger than the {} MiB this page reads from one document",
                MAX_DOCUMENT_BYTES / (1024 * 1024)
            ),
        );
    }
    let text = match extract(&source, declared, bytes) {
        Extracted::Text(text) => text,
        Extracted::Empty => {
            return IngestedItem {
                source,
                status: "empty",
                chunks: 0,
                brain_source: None,
                detail: Some(
                    "no text in it — a scanned document needs OCR before memory can hold it"
                        .to_string(),
                ),
            };
        }
        Extracted::Unsupported(reason) => {
            return IngestedItem {
                source,
                status: "unsupported",
                chunks: 0,
                brain_source: None,
                detail: Some(reason),
            };
        }
    };

    let kind = brain_source_of(&source, declared);
    match company.runtime.memory().brain_file(&source, kind, &text).await {
        Ok(filed) => IngestedItem {
            source,
            status: "stored",
            chunks: 1,
            brain_source: Some(filed.source),
            detail: None,
        },
        Err(error) => {
            tracing::warn!(company = %company.id(), %source, %error, "memory ingest failed");
            IngestedItem::failed(source, format!("memory refused it: {error}"))
        }
    }
}

/// The brain source a dropped file or fetched link is filed under: its
/// format, which is what an operator forgets by.
fn brain_source_of(name: &str, declared: Option<&str>) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    let html = declared.is_some_and(|mime| mime.contains("html"));
    if lower.starts_with("http://") || lower.starts_with("https://") || html {
        return "web";
    }
    match ext {
        "pdf" => "pdf",
        "md" | "markdown" | "txt" | "text" => "markdown",
        "htm" | "html" => "web",
        _ => "document",
    }
}

/// The brain source whose documents are to be forgotten.
#[derive(Debug, Deserialize)]
struct DocumentPath {
    /// The source id (`pdf`, `markdown`, `web`, `document`).
    source: String,
}

/// `DELETE …/memory/document/{source}` — forget every dropped document of one
/// source.
///
/// The counterpart the drop zone needs to be usable: dropping the wrong folder
/// is a mistake an operator makes once. Scoped to the brain alone — nothing
/// here can reach a teammate's logged turns or the shared learnings.
async fn forget_document(
    company: ScopedCompany,
    Path(DocumentPath { source }): Path<DocumentPath>,
) -> Result<Json<ForgottenDto>, ApiError> {
    let forgotten = company.runtime.memory().brain_forget(&source).await?;
    if forgotten == 0 {
        return Err(ApiError(OpenCompanyError::NotFound(format!(
            "no documents in the brain under `{source}`"
        ))));
    }
    super::memory::journal_forget(&company, format!("source:{source}")).await?;
    Ok(Json(ForgottenDto { forgotten }))
}

/// How much of a document was forgotten.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ForgottenDto {
    /// Documents removed.
    forgotten: usize,
}

/// Classifies a multipart failure the way the workspace upload does: a body
/// that overran the limit is a 413, anything else a malformed request. The
/// 413 is raised as [`OpenCompanyError::WorkspaceQuota`] on purpose — the
/// shared "too big" vocabulary `server::ops::workspace`'s own
/// `multipart_error` documents, rather than a second one for this route.
fn multipart_error(error: MultipartError, context: &str) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError(OpenCompanyError::WorkspaceQuota(format!(
            "this drop is larger than the {} MiB one request may carry, so it was cut off before \
             anything could be read. Nothing was stored — drop it in smaller batches.",
            INGEST_BODY_LIMIT / (1024 * 1024)
        )));
    }
    ApiError(OpenCompanyError::InvalidRequest(format!(
        "{context}: {error}"
    )))
}

/// `POST …/memory/ingest` — multipart file drop.
///
/// A folder drop sends each file with its **relative path** as the part's
/// filename (`Contracts/2026/acme.pdf`). That path is kept as the source name
/// rather than reduced to a basename, because it is what the operator sees in
/// their own file manager and the only thing distinguishing the four
/// `README.md`s a repository drop contains.
async fn ingest(
    company: ScopedCompany,
    mut multipart: Multipart,
) -> Result<Json<IngestedDto>, ApiError> {
    let mut items = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| multipart_error(e, "malformed multipart drop"))?
    {
        if field.name() != Some("file") {
            // Ignored rather than refused: a browser's `FormData` carries
            // fields this route has no use for, and failing the drop over one
            // would be a puzzle to debug from the console side.
            continue;
        }
        let name = field
            .file_name()
            .map(str::to_string)
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "untitled".to_string());
        let declared = field.content_type().map(str::to_string);
        // The read is the one place a body-limit overrun surfaces, and it
        // aborts the whole request: a truncated part is indistinguishable from
        // a malformed one, so there is nothing honest to report per file.
        let bytes = field
            .bytes()
            .await
            .map_err(|e| multipart_error(e, "unreadable file part"))?;
        items.push(store_source(&company, name, declared.as_deref(), &bytes).await);
    }

    if items.is_empty() {
        return Err(ApiError(OpenCompanyError::InvalidRequest(
            "the drop carried no files".to_string(),
        )));
    }
    Ok(Json(IngestedDto::of(items)))
}

/// `POST …/memory/ingest/links`.
#[cfg(feature = "documents")]
async fn ingest_links(
    company: ScopedCompany,
    Json(request): Json<LinksRequest>,
) -> Result<Json<IngestedDto>, ApiError> {
    if request.urls.is_empty() {
        return Err(ApiError(OpenCompanyError::InvalidRequest(
            "no links were sent".to_string(),
        )));
    }
    let mut items = Vec::new();
    for url in request.urls {
        let url = url.trim().to_string();
        if let Err(refusal) = link_refusal(&url) {
            items.push(IngestedItem::failed(url, refusal));
            continue;
        }
        items.push(fetch_link(&company, url).await);
    }
    Ok(Json(IngestedDto::of(items)))
}

/// Refuses, by its spelling alone, a URL this host must not fetch: a scheme
/// other than http(s), a literal internal address, or an internal host name.
///
/// A cheap early answer with a readable reason, not the guard itself. The
/// guard is TinyMemory's fetcher (`tinymemory_integrations::sources::fetch::fetch_url`):
/// its client resolves through a public-only resolver and connects to exactly
/// the addresses it vetted, and re-checks every redirect hop. That is what
/// closes DNS rebinding — the host's own resolve-then-connect check it
/// replaces vetted one lookup and let the client make another.
#[cfg(feature = "documents")]
fn link_refusal(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "not a URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("only http:// and https:// links can be fetched".to_string());
    }
    if !tinymemory_integrations::sources::readers::ssrf::is_url_allowed(&parsed) {
        return Err("that host is inside this deployment's own network".to_string());
    }
    Ok(())
}

/// Fetches one link and stores what it said.
#[cfg(feature = "documents")]
async fn fetch_link(company: &ScopedCompany, url: String) -> IngestedItem {
    let document = match tinymemory_integrations::sources::fetch::fetch_url(&url).await {
        Ok(document) => document,
        Err(error) => return IngestedItem::failed(url, format!("could not be fetched: {error}")),
    };
    if document.bytes.len() > MAX_LINK_BYTES {
        return IngestedItem::failed(
            url,
            format!(
                "the page is larger than the {} MiB this reads from one link",
                MAX_LINK_BYTES / (1024 * 1024)
            ),
        );
    }
    let declared = document
        .declared_mime
        .as_deref()
        .map(|v| v.split(';').next().unwrap_or(v).trim().to_string());
    // A URL has no extension to dispatch on, so the declared content type
    // decides — with `text/html` the overwhelming case, and the extractor
    // handling `application/pdf` and friends from the same signal.
    let name = match declared.as_deref() {
        Some("text/html") | Some("application/xhtml+xml") | None => format!("{url}#html"),
        _ => url.clone(),
    };
    let mut item = store_source(company, name, declared.as_deref(), &document.bytes).await;
    // Report the URL the operator typed, not the extension-bearing name the
    // dispatch needed.
    item.source = url;
    item
}

/// Without the feature there is no HTTP client to fetch with, so the route
/// exists and refuses rather than 404ing — a missing route reads to the
/// console as an older host, which is a different thing to tell the operator.
#[cfg(not(feature = "documents"))]
async fn ingest_links(
    _company: ScopedCompany,
    Json(_request): Json<LinksRequest>,
) -> Result<Json<IngestedDto>, ApiError> {
    Err(ApiError(OpenCompanyError::InvalidRequest(
        "remembering a link needs the `documents` feature, which this build was compiled without"
            .to_string(),
    )))
}

#[cfg(test)]
#[path = "memory_ingest_tests.rs"]
mod tests;
