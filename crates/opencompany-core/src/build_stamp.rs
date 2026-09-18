// How the build commit stamp is chosen.
//
// This file is compiled twice on purpose. `build.rs` pulls it in with
// `include!` and supplies the impure inputs — environment, `git` — while
// `src/lib.rs` compiles it as a module under `cfg(test)` so `cargo test`
// executes the same code. Two implementations, one built and one tested,
// would be worse than none: the fallbacks below are the entire risk of
// stamping a commit at build time, and a test that exercises a second copy of
// them proves nothing about the binary that ships.
//
// Regular `//` comments rather than `//!`: an inner doc comment is not
// permitted where `build.rs` performs the `include!`.
//
// Nothing here may reach for `crate::`, `std::process` or the environment.
// Keeping it pure is what makes "there is no git" a case a unit test can
// state, instead of one that needs a container without git in it.

/// The stamp used when no source can name a commit at all.
const UNKNOWN_COMMIT: &str = "unknown";

/// The length a full object id is shortened to. Twelve hex digits is what
/// `git` itself considers unambiguous well past this repository's size, and
/// normalizing to it means an injected `GITHUB_SHA` reads the same as a
/// locally-derived one instead of being forty characters wide in analytics.
const SHORT_COMMIT_LEN: usize = 12;

/// The longest stamp accepted from any source.
///
/// The value is interpolated into a `cargo:rustc-env` line and then served on
/// an HTTP surface, so it is bounded and filtered rather than trusted: a
/// newline in it would forge a second build-script directive.
const MAX_COMMIT_LEN: usize = 64;

/// Normalizes a candidate commit string, or `None` when it names nothing.
///
/// Everything outside `[A-Za-z0-9._-]` is dropped rather than rejected, so a
/// value with a stray quote or trailing newline still yields a usable stamp
/// instead of silently degrading the build to `"unknown"`.
fn sanitize_commit(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(MAX_COMMIT_LEN)
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    // ASCII throughout by construction, so slicing on a byte index is safe.
    if cleaned.len() > SHORT_COMMIT_LEN && cleaned.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(cleaned[..SHORT_COMMIT_LEN].to_string());
    }
    Some(cleaned)
}

/// Chooses the commit stamp from whichever sources can answer.
///
/// The order is a claim about which source knows the most, and each step is
/// reached only because the one before it could not answer:
///
/// 1. `OPENCOMPANY_BUILD_COMMIT` — someone deliberately said what this build
///    is. A builder that bothers to set it knows more than this script can
///    work out, and it is the only escape hatch for a build environment
///    nothing below covers.
/// 2. `git` — ground truth about the tree actually being compiled, and the
///    only source that can also say whether it was clean. Preferred over
///    `GITHUB_SHA` because `GITHUB_SHA` describes what CI *intended* to check
///    out; a workflow that checks out a different ref would otherwise stamp a
///    commit that was never built.
/// 3. `GITHUB_SHA` — what CI believes, used exactly when there is no usable
///    repository to ask: a source tarball, a vendored crate, or a container
///    build whose context omits `.git`. These are the cases that make an
///    environment source necessary rather than merely convenient.
/// 4. `"unknown"` — an honest absence. A missing stamp must never fail a
///    build; a build that cannot say which commit it is remains a build.
///
/// `git_dirty` is consulted only when the stamp came from `git`, because it is
/// the only branch where the answer describes the same thing the stamp names.
/// An injected value is the injector's to be right about, and appending a
/// locally-measured suffix to it would mix two claims into one string.
fn resolve_build_commit(
    explicit: Option<String>,
    github_sha: Option<String>,
    git_head: impl FnOnce() -> Option<String>,
    git_dirty: impl FnOnce() -> bool,
) -> String {
    if let Some(commit) = explicit.as_deref().and_then(sanitize_commit) {
        return commit;
    }
    if let Some(commit) = git_head().as_deref().and_then(sanitize_commit) {
        return if git_dirty() {
            format!("{commit}-dirty")
        } else {
            commit
        };
    }
    if let Some(commit) = github_sha.as_deref().and_then(sanitize_commit) {
        return commit;
    }
    UNKNOWN_COMMIT.to_string()
}

#[cfg(test)]
#[path = "build_stamp_tests.rs"]
mod tests;
