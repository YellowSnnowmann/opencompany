//! Bounds every rendered ledger section is held to.
//!
//! A ledger's derived file is read by people in the console **and** routed into
//! agent turns, so its size is a bill paid on every read. Riemann's
//! `docs/ledgers.md` records what happens without these: nine derived files came
//! to 51% of every assembled prompt, one of them 86 KB, because the table had a
//! row cap and the prose beneath it had none. Nothing broke and no test failed —
//! the file simply grew.
//!
//! So the bounds live here, once, and every renderer goes through [`listed`]:
//! a row cap **and** a per-field character cap, because either alone is the same
//! file by another route. Forty rows of five-kilobyte prose is not bounded.

use std::fmt::Write as _;

/// Rows one rendered section carries.
///
/// A section is read for *what is on it* — "have we already ruled this out" —
/// so dropping a row costs more than shortening one. The row cap is therefore
/// generous and [`REASON_CHARS`] does the real work.
pub const MAX_LISTED: usize = 40;

/// Characters one prose field is held to inside a rendered section.
///
/// Long enough to carry why something closed — *"the vendor never replied and
/// the quarter closed"* — and short enough that forty of them are not a
/// document. The whole value is always one read away, and every renderer says
/// where.
pub const REASON_CHARS: usize = 600;

/// Entries one `read_ledger`-style read returns by default.
pub const DEFAULT_READ_LIMIT: usize = 20;

/// The most entries any single read returns, whatever the caller asks for.
pub const MAX_READ_LIMIT: usize = 100;

/// Truncates `text` to `max` **codepoints**, marking the cut.
///
/// Counts characters rather than bytes so multi-byte text is cut on a character
/// boundary instead of panicking, and appends an ellipsis so a truncated field
/// never reads as a complete one — a shortened value presented as whole is the
/// failure the bounds exist to prevent, arrived at from the other side.
pub fn truncate(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let kept: String = trimmed.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// Renders at most `max` of `items`, and reports how many were left out.
///
/// The count is the point. A section cut to its bound and rendered as though
/// complete tells its reader there is nothing more, so the caller is handed the
/// number and is expected to print it with where the rest lives. See
/// [`elided`].
pub fn listed<T>(
    items: impl IntoIterator<Item = T>,
    max: usize,
    mut render: impl FnMut(&mut String, T),
) -> (String, usize) {
    let mut out = String::new();
    let mut seen = 0_usize;
    for item in items {
        seen += 1;
        if seen > max {
            continue;
        }
        render(&mut out, item);
    }
    (out, seen.saturating_sub(max))
}

/// The line a caller appends when [`listed`] left rows out.
///
/// Empty when it did not, so callers append it unconditionally rather than each
/// writing the same branch and one of them getting it wrong.
pub fn elided(dropped: usize, ledger: &str) -> String {
    if dropped == 0 {
        return String::new();
    }
    let mut out = String::new();
    let _ = write!(
        out,
        "\n_{dropped} more not shown here. Read them with `read_ledger {{ ledger: \"{ledger}\" }}` \
         — they are not gone._\n"
    );
    out
}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod tests;
