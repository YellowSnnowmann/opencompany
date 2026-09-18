//! Turning dropped files and links into memory.
//!
//! One question, asked of arbitrary operator-supplied bytes: *what text is in
//! here, and how should memory hold it?* The answer has three parts, and they
//! are deliberately separate:
//!
//! - [`extract`] — bytes to text, per format ([`documents`] for the ones that
//!   need a parser).
//! - [`chunk`] — text to the sized, labelled pieces a
//!   [`ContextStore`](crate::ports::ContextStore) actually holds.
//! - `crate::server::ops::memory_ingest` — the route that puts them there.
//!
//! ## What is stored, and what is not
//!
//! **The text, not the file.** A dropped document is extracted, chunked, and
//! written to memory; the original bytes are not retained anywhere. That is a
//! deliberate answer to "where should this live", not an oversight: the
//! workspace tree is the place for files an operator wants back
//! (`ops::workspace`), and duplicating every upload into it would make the
//! Brain page a second, silently diverging file manager. The consequence is
//! stated plainly in the console — memory keeps what the document *said*, and
//! re-uploading is how you correct it.
//!
//! ## Extraction never guesses
//!
//! A format this build cannot read produces [`Extracted::Unsupported`], which
//! the route reports per file. It does **not** fall back to storing the raw
//! bytes as if they were text: a chunk of decoded PDF operators would recall
//! as noise, count as a memory, and be indistinguishable from a real one.

pub mod chunk;
pub mod documents;
#[cfg(test)]
#[path = "ingest_tests.rs"]
mod tests;
pub mod text;

pub use chunk::{DOCUMENT_LABEL_PREFIX, DocumentChunk, chunk_document, label_for};

/// What extraction made of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extracted {
    /// Text was recovered. Never empty and never whitespace-only — an empty
    /// extraction is [`Self::Empty`], because "we read it and it said nothing"
    /// and "we stored nothing" must not be the same answer to the operator.
    Text(String),
    /// The format was understood and carried no text: a scanned PDF with no
    /// text layer, an empty spreadsheet, a blank note.
    Empty,
    /// Nothing here can read this format. Carries what to say about it.
    Unsupported(String),
}

impl Extracted {
    /// The text, when there is any.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }
}

/// Extracts the text of `bytes`, dispatching on the file name's extension and
/// the declared content type.
///
/// The extension is consulted first and the declared type second, because a
/// browser's `FormData` reports `application/octet-stream` for anything the OS
/// has no mapping for — a `.md` file dropped from a folder tree routinely
/// arrives that way, and trusting the declared type would send it to the
/// unsupported branch.
pub fn extract(name: &str, declared: Option<&str>, bytes: &[u8]) -> Extracted {
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    let declared = declared.unwrap_or("").to_ascii_lowercase();

    match extension.as_str() {
        "pdf" => documents::pdf(bytes),
        "docx" => documents::docx(bytes),
        "pptx" => documents::pptx(bytes),
        "xlsx" | "xlsm" => documents::xlsx(bytes),
        "html" | "htm" => text::from_html_bytes(bytes),
        // Word's pre-2007 binary formats and their friends. Named rather than
        // left to the catch-all so the refusal can say what to do about it.
        "doc" | "ppt" | "xls" | "rtf" | "pages" | "key" | "numbers" => {
            Extracted::Unsupported(format!(
                "`{extension}` is a legacy binary format this build does not read — export it as \
                 PDF, .docx, .xlsx, .pptx or plain text and drop it again"
            ))
        }
        _ if declared.starts_with("application/pdf") => documents::pdf(bytes),
        _ => text::from_bytes(name, &declared, bytes),
    }
}

/// The largest single file this route will read.
///
/// Deliberately below the workspace upload's own cap: that route stores what
/// it is given, while this one *parses* it, and every parser here holds the
/// whole document plus its extraction in memory at once. Twenty-five mebibytes
/// covers the documents people actually drop on a memory page and keeps the
/// peak per in-flight upload bounded to something a tenant container survives.
pub const MAX_DOCUMENT_BYTES: usize = 25 * 1024 * 1024;
