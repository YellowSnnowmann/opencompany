//! What an MCP server says about itself: the `serverInfo` block of its
//! `initialize` reply, kept per server beside its health record.
//!
//! Icons never become a request from the operator's browser: the host fetches
//! the icon during the probe it already performs and stores the bytes inline as
//! a `data:` URI. [`load`] refuses a stored `icon_data_url` that is not a
//! `data:` image, so a tampered store cannot turn the field back into a remote
//! URL.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Result;
use crate::error::OpenCompanyError;
use crate::ports::SecretStore;
use crate::ports::types::{CompanyId, SecretValue};

/// The longest stored title.
const MAX_TITLE_CHARS: usize = 120;

/// The longest stored description.
const MAX_DESCRIPTION_CHARS: usize = 400;

/// The largest icon the host will fetch and store.
pub const MAX_ICON_BYTES: usize = 64 * 1024;

/// How long the host waits for an icon before giving up on it.
#[cfg(feature = "mcp")]
const ICON_FETCH_TIMEOUT_SECS: u64 = 5;

/// The [`SecretStore`] key holding a declared server's [`McpServerInfo`].
pub fn server_info_key(name: &str) -> String {
    format!("mcp/{name}/server_info")
}

/// What a server said about itself on its last successful handshake.
///
/// Every field is absent-by-default, and absent means the server did not say.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    /// The display name the server prefers for itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The server's own description of what it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The server's home page, as an `http(s)` URL. Rendered as a link, never
    /// fetched by this host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    /// The server's icon, inlined as a `data:` URI by [`fetch_icon`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_data_url: Option<String>,
}

impl McpServerInfo {
    /// Whether the server said anything worth storing.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.website_url.is_none()
            && self.icon_data_url.is_none()
    }
}

/// Reads the `title`, `description` and `websiteUrl` a server reported.
///
/// The icon is not filled here: [`fetch_icon`] fetches the URL [`icon_source`]
/// picks out.
pub fn from_server_info(server_info: &Value) -> McpServerInfo {
    McpServerInfo {
        title: text(server_info, "title", MAX_TITLE_CHARS),
        description: text(server_info, "description", MAX_DESCRIPTION_CHARS),
        website_url: server_info
            .get("websiteUrl")
            .and_then(Value::as_str)
            .and_then(http_url),
        icon_data_url: None,
    }
}

/// The first `http(s)` icon source a server advertised, if any.
pub fn icon_source(server_info: &Value) -> Option<String> {
    server_info
        .get("icons")?
        .as_array()?
        .iter()
        .filter_map(|icon| icon.get("src").and_then(Value::as_str))
        .find_map(http_url)
}

/// One field of the remote block, bounded and stripped of control characters.
fn text(server_info: &Value, field: &str, max_chars: usize) -> Option<String> {
    let raw = server_info.get(field)?.as_str()?;
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// A URL the console may render as a link: `http(s)` and nothing else.
fn http_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    (trimmed.starts_with("https://") || trimmed.starts_with("http://")).then(|| trimmed.to_string())
}

/// Whether a stored `icon_data_url` is what this module writes.
fn is_inline_image(value: &str) -> bool {
    value.starts_with("data:image/") && value.contains(";base64,")
}

/// Reads a server's stored self-description, or the empty one when it has never
/// been probed.
///
/// A malformed record degrades to the empty description rather than erroring. A
/// stored `icon_data_url` that is not an inline image is dropped on the way out.
pub async fn load(company: &CompanyId, name: &str, secrets: &dyn SecretStore) -> McpServerInfo {
    let raw = match secrets.get(company, &server_info_key(name)).await {
        Ok(Some(SecretValue(raw))) => raw,
        Ok(None) => return McpServerInfo::default(),
        Err(err) => {
            tracing::warn!(
                company = %company,
                server = %name,
                error = %err,
                "reading MCP server info failed; this server describes itself with nothing"
            );
            return McpServerInfo::default();
        }
    };
    if raw.trim().is_empty() {
        return McpServerInfo::default();
    }
    let mut info: McpServerInfo = serde_json::from_str(&raw).unwrap_or_default();
    if !info.icon_data_url.as_deref().is_none_or(is_inline_image) {
        info.icon_data_url = None;
    }
    info
}

/// Persists a server's self-description.
pub async fn save(
    company: &CompanyId,
    name: &str,
    info: &McpServerInfo,
    secrets: &dyn SecretStore,
) -> Result<()> {
    let raw = serde_json::to_string(info)
        .map_err(|e| OpenCompanyError::Store(format!("serializing mcp server info: {e}")))?;
    secrets
        .set(company, &server_info_key(name), SecretValue(raw))
        .await
}

/// Fetches `url` and returns it as an inline `data:` image, or `None`.
///
/// Bounded by the outbound SSRF guard, no redirect following, a byte ceiling
/// read off the body rather than the declared length, and a media type sniffed
/// from the bytes rather than the `Content-Type` header. Anything that fails one
/// of them yields `None`.
#[cfg(feature = "mcp")]
pub async fn fetch_icon(url: &str) -> Option<String> {
    use futures::StreamExt;

    if openhuman_core::tools::validate_url(url, &[]).is_err() {
        return None;
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(ICON_FETCH_TIMEOUT_SECS))
        .build()
        .ok()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut bytes: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk.ok()?);
        if bytes.len() > MAX_ICON_BYTES {
            return None;
        }
    }
    inline_image(&bytes)
}

/// Fetched bytes as the inline image they will be stored and served as, or
/// `None` when they are not an image this host will re-serve.
///
/// The media type comes from the bytes' own signature, so SVG — which has none
/// — is always `None` whatever it was labelled. The decoded size is held to the
/// avatar decompression-bomb check.
#[cfg(feature = "mcp")]
fn inline_image(bytes: &[u8]) -> Option<String> {
    use base64::Engine;

    if bytes.len() > MAX_ICON_BYTES {
        return None;
    }
    let media_type = super::avatar::sniff_image(bytes)?;
    super::avatar::check_image_dimensions(bytes).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:{media_type};base64,{encoded}"))
}

/// Always `None` without the `mcp` feature.
#[cfg(not(feature = "mcp"))]
pub async fn fetch_icon(_url: &str) -> Option<String> {
    None
}

#[cfg(test)]
#[path = "mcp_server_info_tests.rs"]
mod tests;
