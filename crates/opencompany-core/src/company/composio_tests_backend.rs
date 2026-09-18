//! Composio backend-URL resolution tests (split out of `composio_tests.rs`).

use super::*;

#[test]
fn backend_url_follows_api_url_then_default() {
    // Neither set → prod default.
    assert_eq!(backend_url_or_default(None), DEFAULT_BACKEND_URL);

    // api_url set → follow the tenant API base (the staging case).
    assert_eq!(
        backend_url_or_default(Some("https://staging-api.tinyhumans.ai".into())),
        "https://staging-api.tinyhumans.ai"
    );

    // Whitespace/empty api_url falls through to the prod default.
    assert_eq!(
        backend_url_or_default(Some("   ".into())),
        DEFAULT_BACKEND_URL
    );

    // api_url is trimmed before use.
    assert_eq!(
        backend_url_or_default(Some("  https://staging-api.tinyhumans.ai  ".into())),
        "https://staging-api.tinyhumans.ai"
    );
}

/// `backend_url_or_default` takes only its one argument now — the explicit
/// per-surface override (`OPENCOMPANY_COMPOSIO_BACKEND_URL`) is gone
/// (issue #2306, phase 6a) and nothing replaced it with another env read.
/// Setting that removed variable, and the one the function's argument is
/// normally sourced from, in the *process* environment must have zero
/// effect: the function has no `EnvSource` to read them through.
#[test]
fn backend_url_or_default_reads_no_environment_variable() {
    let _env = crate::test_support::EnvVarGuard::capture(&[
        "OPENCOMPANY_COMPOSIO_BACKEND_URL",
        TINYHUMANS_API_URL_ENV,
    ]);
    _env.set(
        "OPENCOMPANY_COMPOSIO_BACKEND_URL",
        "https://should-be-ignored.example",
    );
    _env.set(
        TINYHUMANS_API_URL_ENV,
        "https://also-should-be-ignored.example",
    );

    assert_eq!(
        backend_url_or_default(Some("https://explicit-arg.example".into())),
        "https://explicit-arg.example",
        "the argument is the only input"
    );
    assert_eq!(
        backend_url_or_default(None),
        DEFAULT_BACKEND_URL,
        "with no argument, the process environment must not fill in a value"
    );
}
