//! Cutting extracted text into the pieces memory holds.
//!
//! A [`ContextStore`](crate::ports::ContextStore) chunk is what recall
//! retrieves and what an agent is shown, so the unit matters: a whole
//! forty-page contract as one chunk floods a turn's context with thirty-nine
//! irrelevant pages, and a chunk per sentence loses the context that made the
//! sentence mean anything.

/// Label prefix for every chunk a dropped document or link produced.
///
/// Its own prefix, beside `operator-fact/` and `task-outcome/`, because the
/// console renders these as their own origin: an operator who dropped a folder
/// must be able to see what it became, and a document chunk showing up as
/// "teammate memory" would read as something an agent learned.
pub const DOCUMENT_LABEL_PREFIX: &str = "document";

/// Target chunk size in characters.
///
/// Roughly 500 tokens: large enough that a paragraph keeps its neighbours,
/// small enough that a recall hit costs a fraction of a turn's budget.
const TARGET_CHARS: usize = 2_000;

/// The largest a chunk may get before it is cut mid-paragraph.
///
/// Paragraph boundaries are preferred, but a document with no blank lines —
/// extracted spreadsheet rows, minified prose — must not produce one chunk the
/// size of the file.
const MAX_CHARS: usize = 3_000;

/// One chunk, ready for [`ContextStore::put`](crate::ports::ContextStore::put).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentChunk {
    /// `document/{source}/{index}` — the address recall and the console both
    /// key off.
    pub label: String,
    /// The chunk text, with the source named on its first line.
    pub body: String,
}

/// The label prefix for one source's chunks: `document/{slug}`.
///
/// Slugged because the label is an addressing key on every backend, and a raw
/// file name carries slashes (a folder drop sends relative paths) that would
/// silently nest one document's chunks under another's prefix.
pub fn label_for(source: &str) -> String {
    let slug: String = source
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    let slug = if slug.is_empty() {
        "document".to_string()
    } else {
        slug
    };
    // Bounded: a deeply nested folder path can be longer than some backends
    // index comfortably, and the tail of a path is the identifying part.
    let slug = if slug.chars().count() > 96 {
        slug.chars().skip(slug.chars().count() - 96).collect()
    } else {
        slug
    };
    format!("{DOCUMENT_LABEL_PREFIX}/{slug}")
}

/// Splits `text` into labelled chunks for a document named `source`.
///
/// Every chunk repeats the source name on its first line. That costs a line
/// per chunk and buys the thing recall cannot otherwise recover: a chunk
/// surfaces on its own, with no path back to the document it came from, and an
/// agent shown a paragraph of a contract needs to know which contract.
pub fn chunk_document(source: &str, text: &str) -> Vec<DocumentChunk> {
    let prefix = label_for(source);
    let mut chunks = Vec::new();
    for (index, body) in split(text).into_iter().enumerate() {
        chunks.push(DocumentChunk {
            label: format!("{prefix}/{index:04}"),
            body: format!("{source}\n\n{body}"),
        });
    }
    chunks
}

/// Accumulates paragraphs up to [`TARGET_CHARS`], hard-splitting anything that
/// would cross [`MAX_CHARS`] on its own.
fn split(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for paragraph in text.split("\n\n") {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        if paragraph.chars().count() > MAX_CHARS {
            if !current.trim().is_empty() {
                out.push(std::mem::take(&mut current).trim().to_string());
            }
            out.extend(hard_split(paragraph));
            continue;
        }
        if current.chars().count() + paragraph.chars().count() > TARGET_CHARS
            && !current.trim().is_empty()
        {
            out.push(std::mem::take(&mut current).trim().to_string());
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph);
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// Cuts an over-long paragraph on the last sentence end before the limit,
/// falling back to the limit itself.
fn hard_split(paragraph: &str) -> Vec<String> {
    let chars: Vec<char> = paragraph.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + TARGET_CHARS).min(chars.len());
        // Only look for a boundary when there is more text after this cut —
        // the final piece ends where the paragraph does.
        let cut = if end == chars.len() {
            end
        } else {
            chars[start..end]
                .iter()
                .rposition(|c| matches!(c, '.' | '!' | '?' | '\n'))
                .map(|offset| start + offset + 1)
                .filter(|cut| *cut > start)
                .unwrap_or(end)
        };
        let piece: String = chars[start..cut].iter().collect();
        if !piece.trim().is_empty() {
            out.push(piece.trim().to_string());
        }
        start = cut;
    }
    out
}
