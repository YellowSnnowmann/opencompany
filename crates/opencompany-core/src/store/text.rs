//! Char-boundary-safe slicing shared by every
//! [`ContextStore`](crate::ports::ContextStore) backend.
//!
//! The ranged `peek` and the search-snippet window both compute **byte**
//! offsets — a caller-supplied range, or ±24 bytes around a match — and a byte
//! offset lands mid-codepoint on any non-ASCII body. A raw `body[start..end]`
//! there panics, and `memory_recall` routes agent queries straight into
//! `search`, so the panic is reachable from ordinary chunk content. These
//! helpers widen outward to the nearest boundary instead: slightly more text
//! than asked, never a failed read.

use std::ops::Range;

/// Clamps `range` to the string's length and to char boundaries.
///
/// Widening is outward on both ends (floor the start, ceil the end), so the
/// requested bytes are always contained in the answer. An inverted or empty
/// range yields the empty string rather than a panic — including a zero-length
/// range landing mid-codepoint, which the widening alone would have grown into
/// a whole character (asking for no bytes must never answer with one).
pub(crate) fn slice_on_char_boundaries(body: &str, range: Range<usize>) -> String {
    if range.start >= range.end {
        return String::new();
    }
    let start = floor_boundary(body, range.start.min(body.len()));
    let end = ceil_boundary(body, range.end.min(body.len()));
    if start >= end {
        return String::new();
    }
    body[start..end].to_string()
}

/// The nearest char boundary at or below `at`.
pub(crate) fn floor_boundary(s: &str, mut at: usize) -> usize {
    while at > 0 && !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The nearest char boundary at or above `at` (callers clamp to `s.len()`).
pub(crate) fn ceil_boundary(s: &str, mut at: usize) -> usize {
    while at < s.len() && !s.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
