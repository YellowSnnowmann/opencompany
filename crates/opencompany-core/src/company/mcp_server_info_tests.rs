use super::*;

use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct MemSecrets {
    map: Mutex<HashMap<String, String>>,
}

#[async_trait]
impl SecretStore for MemSecrets {
    async fn get(&self, _company: &CompanyId, key: &str) -> Result<Option<SecretValue>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(key)
            .map(|raw| SecretValue(raw.clone())))
    }
    async fn set(&self, _company: &CompanyId, key: &str, value: SecretValue) -> Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value.0);
        Ok(())
    }
}

fn company() -> CompanyId {
    CompanyId::new("acme")
}

/// A server that reports everything is read in full.
#[test]
fn a_full_server_info_block_is_read() {
    let info = from_server_info(&json!({
        "name": "context7",
        "title": "Context7",
        "description": "Up-to-date documentation for any library.",
        "websiteUrl": "https://context7.com",
    }));
    assert_eq!(info.title.as_deref(), Some("Context7"));
    assert_eq!(
        info.description.as_deref(),
        Some("Up-to-date documentation for any library.")
    );
    assert_eq!(info.website_url.as_deref(), Some("https://context7.com"));
    assert!(info.icon_data_url.is_none(), "the icon needs a fetch");
}

/// A server that reports only its name says nothing this surfaces, and absent
/// must stay absent rather than resolving to a placeholder.
#[test]
fn a_bare_server_info_block_reports_nothing() {
    let info = from_server_info(&json!({ "name": "deepwiki", "version": "0.1.0" }));
    assert!(info.is_empty(), "{info:?}");
}

/// Remote text is bounded and stripped of control characters before it can
/// reach an operator's screen.
#[test]
fn remote_text_is_bounded_and_stripped() {
    let info = from_server_info(&json!({
        "title": "A\u{0007}B\u{001b}[2J",
        "description": "x".repeat(MAX_DESCRIPTION_CHARS + 50),
    }));
    assert_eq!(info.title.as_deref(), Some("AB[2J"));
    assert_eq!(
        info.description.as_deref().map(str::len),
        Some(MAX_DESCRIPTION_CHARS)
    );
}

/// A blank field is not a value.
#[test]
fn a_blank_field_is_absent() {
    let info = from_server_info(&json!({ "title": "   ", "description": "" }));
    assert!(info.title.is_none());
    assert!(info.description.is_none());
}

/// The website is a scheme allow-list, so a server cannot hand the console a
/// link that runs script or inlines a document.
#[test]
fn only_http_websites_are_kept() {
    for hostile in [
        "javascript:alert(1)",
        "data:text/html,<script>x</script>",
        "file:///etc/passwd",
        "//context7.com",
        "https://context7.com /x",
    ] {
        let info = from_server_info(&json!({ "websiteUrl": hostile }));
        assert!(
            info.website_url.is_none(),
            "{hostile} must not survive as a link"
        );
    }
    let info = from_server_info(&json!({ "websiteUrl": " http://context7.com " }));
    assert_eq!(info.website_url.as_deref(), Some("http://context7.com"));
}

/// The icon source is the first `http(s)` entry, and a non-`http` one is not a
/// fetch this host will attempt.
#[test]
fn the_icon_source_is_the_first_http_entry() {
    assert_eq!(
        icon_source(&json!({
            "icons": [
                { "src": "javascript:alert(1)" },
                { "sizes": "48x48" },
                { "src": "https://context7.com/icon.png", "sizes": "48x48" },
                { "src": "https://context7.com/other.png" },
            ]
        })),
        Some("https://context7.com/icon.png".to_string())
    );
    assert!(icon_source(&json!({ "icons": [] })).is_none());
    assert!(icon_source(&json!({})).is_none());
}

/// What was stored is what comes back.
#[tokio::test]
async fn a_stored_description_round_trips() {
    let secrets = MemSecrets::default();
    let stored = McpServerInfo {
        title: Some("Context7".to_string()),
        description: Some("Docs.".to_string()),
        website_url: Some("https://context7.com".to_string()),
        icon_data_url: Some("data:image/png;base64,AAAA".to_string()),
    };
    save(&company(), "context7", &stored, &secrets)
        .await
        .expect("saved");
    assert_eq!(load(&company(), "context7", &secrets).await, stored);
}

/// A server nobody has probed describes itself with nothing.
#[tokio::test]
async fn an_unprobed_server_loads_empty() {
    let secrets = MemSecrets::default();
    assert!(load(&company(), "context7", &secrets).await.is_empty());
}

/// The icon field is read back only as an inline image. A stored remote URL —
/// which this module never writes — is dropped rather than handed to the
/// console for an `src=`, so the field cannot become a request from an
/// operator's browser even if the store is tampered with.
#[tokio::test]
async fn a_stored_remote_icon_url_is_refused_on_read() {
    let secrets = MemSecrets::default();
    for hostile in [
        "https://tracker.example/beacon.gif",
        "javascript:alert(1)",
        "data:text/html,<script>x</script>",
        "data:image/png,notbase64",
    ] {
        secrets
            .set(
                &company(),
                &server_info_key("context7"),
                SecretValue(json!({ "title": "Context7", "iconDataUrl": hostile }).to_string()),
            )
            .await
            .expect("stored");
        let info = load(&company(), "context7", &secrets).await;
        assert!(info.icon_data_url.is_none(), "{hostile} must not survive");
        assert_eq!(
            info.title.as_deref(),
            Some("Context7"),
            "the rest of the record still reads"
        );
    }
}

/// An unreadable record degrades to the empty description.
#[tokio::test]
async fn a_malformed_record_loads_empty() {
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company(),
            &server_info_key("context7"),
            SecretValue("{ not json".to_string()),
        )
        .await
        .expect("stored");
    assert!(load(&company(), "context7", &secrets).await.is_empty());
}

/// A PNG header announcing the given size — enough for the signature sniff and
/// the decoded-size read, which is all the inline form depends on.
#[cfg(feature = "mcp")]
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&w.to_be_bytes());
    bytes.extend_from_slice(&h.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes
}

/// Image bytes become the `data:` URI the console renders, typed from the
/// signature rather than from anything the server claimed.
#[cfg(feature = "mcp")]
#[test]
fn image_bytes_become_an_inline_data_uri() {
    let inlined = inline_image(&png(48, 48)).expect("a PNG is servable");
    assert!(inlined.starts_with("data:image/png;base64,"), "{inlined}");
    assert!(is_inline_image(&inlined));
}

/// Everything an icon URL could return that is not an image this host will
/// re-serve is refused, so nothing hostile reaches an `src=`.
#[cfg(feature = "mcp")]
#[test]
fn a_non_image_body_is_refused() {
    for body in [
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script/></svg>".to_vec(),
        b"<!doctype html><html><body>hi</body></html>".to_vec(),
        b"%PDF-1.7".to_vec(),
        Vec::new(),
    ] {
        assert!(
            inline_image(&body).is_none(),
            "{:?} must be refused",
            &body[..body.len().min(16)]
        );
    }
}

/// An oversized body is refused, and so is a small one whose header promises a
/// decode nobody should be asked to perform.
#[cfg(feature = "mcp")]
#[test]
fn an_oversized_or_bomb_icon_is_refused() {
    let mut huge = png(48, 48);
    huge.resize(MAX_ICON_BYTES + 1, 0);
    assert!(inline_image(&huge).is_none(), "over the byte ceiling");
    assert!(
        inline_image(&png(65535, 65535)).is_none(),
        "a decompression bomb must not be stored"
    );
}
