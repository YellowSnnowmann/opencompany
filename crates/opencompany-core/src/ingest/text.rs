//! Plain-text and HTML extraction — the formats that need no parser.
//!
//! Always compiled, whatever feature set: a build without `documents` still
//! ingests Markdown, notes, CSV exports, JSON and source files, which is the
//! bulk of what a drop zone on a memory page receives.

use super::Extracted;

/// Content types that are text whatever their extension says.
const TEXT_MIME_PREFIXES: [&str; 2] = ["text/", "message/"];

/// Structured content types that are text but do not say `text/`.
const TEXT_MIME_EXACT: [&str; 6] = [
    "application/json",
    "application/xml",
    "application/x-yaml",
    "application/yaml",
    "application/toml",
    "application/javascript",
];

/// Decodes `bytes` as UTF-8 text, refusing anything that is not.
///
/// The refusal is the point. A `.bin` renamed `.txt`, or an image dropped with
/// no extension, decodes into replacement characters — storing that would put
/// a memory in the company's brain that says nothing and can never be
/// recalled usefully, while looking exactly like a real one in the list.
pub fn from_bytes(name: &str, declared: &str, bytes: &[u8]) -> Extracted {
    let looks_textual = TEXT_MIME_PREFIXES.iter().any(|p| declared.starts_with(p))
        || TEXT_MIME_EXACT.contains(&declared)
        || declared.is_empty()
        || declared == "application/octet-stream";
    if !looks_textual {
        return Extracted::Unsupported(format!(
            "`{declared}` is not a format this build can read as text"
        ));
    }
    match std::str::from_utf8(bytes) {
        Ok(text) if text.trim().is_empty() => Extracted::Empty,
        Ok(text) => Extracted::Text(normalize(text)),
        Err(_) => Extracted::Unsupported(format!(
            "`{name}` is not UTF-8 text, and its type ({}) names no format this build can parse",
            if declared.is_empty() {
                "unset"
            } else {
                declared
            }
        )),
    }
}

/// Extracts the visible text of an HTML document.
pub fn from_html_bytes(bytes: &[u8]) -> Extracted {
    match std::str::from_utf8(bytes) {
        Ok(html) => from_html(html),
        Err(_) => Extracted::Unsupported("the HTML is not UTF-8".to_string()),
    }
}

/// Strips tags, `<script>`/`<style>` bodies, and HTML entities.
///
/// A hand-written scanner rather than a parser dependency: what memory needs
/// from a web page is its prose, the failure mode of getting a tag boundary
/// slightly wrong is a stray angle bracket in a chunk, and neither justifies
/// pulling a full HTML tree into the default build.
pub fn from_html(html: &str) -> Extracted {
    let mut out = String::with_capacity(html.len() / 2);
    // Drop the two elements whose *content* is code rather than prose. Left in,
    // a page's inline analytics script becomes the first thing recall finds.
    let mut rest = html.to_string();
    for element in ["script", "style"] {
        rest = strip_element(&rest, element);
    }
    let mut in_tag = false;
    for ch in rest.chars() {
        match ch {
            '<' => {
                in_tag = true;
                // Tags are element boundaries, so they are whitespace to the
                // text — without this `<p>a</p><p>b</p>` reads as `ab`.
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if in_tag => {}
            _ => out.push(ch),
        }
    }
    let decoded = decode_entities(&out);
    if decoded.trim().is_empty() {
        return Extracted::Empty;
    }
    Extracted::Text(normalize(&decoded))
}

/// Removes `<name>…</name>` including its content, case-insensitively.
fn strip_element(html: &str, name: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0usize;
    while let Some(start) = lower[cursor..].find(&open) {
        let start = cursor + start;
        out.push_str(&html[cursor..start]);
        match lower[start..].find(&close) {
            Some(end) => cursor = start + end + close.len(),
            // An unclosed `<script>` runs to the end of the document, which is
            // what a browser does with it too.
            None => return out,
        }
    }
    out.push_str(&html[cursor..]);
    out
}

/// Decodes the handful of entities that actually appear in prose.
fn decode_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&hellip;", "…")
}

/// Collapses runs of blank lines and trailing spaces.
///
/// Extraction output is full of layout artefacts — a PDF page break, a table
/// cell's padding — and every one of them costs chunk budget that could hold
/// another sentence.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
            out.push('\n');
            continue;
        }
        blank_run = 0;
        // Interior runs of spaces collapse too: PDF extraction pads columns
        // with dozens of them.
        let mut last_space = false;
        for ch in trimmed.chars() {
            if ch.is_whitespace() {
                if !last_space {
                    out.push(' ');
                }
                last_space = true;
            } else {
                out.push(ch);
                last_space = false;
            }
        }
        out.push('\n');
    }
    out.trim().to_string()
}
