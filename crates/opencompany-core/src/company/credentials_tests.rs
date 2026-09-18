use super::*;
use crate::app::config::MapEnv;

/// Builds an unsigned-but-well-shaped JWT carrying `claims` as its payload.
fn jwt(claims: serde_json::Value) -> String {
    let encode = |bytes: &[u8]| {
        const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            let take = chunk.len() + 1;
            for i in 0..take {
                out.push(ALPHA[((n >> (18 - 6 * i)) & 0x3F) as usize] as char);
            }
        }
        out
    };
    format!(
        "{}.{}.{}",
        encode(br#"{"alg":"RS256"}"#),
        encode(claims.to_string().as_bytes()),
        "c2lnbmF0dXJl"
    )
}

fn unix(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// The caller must hold the returned handle: it owns the enclosing
/// directory and removes it on drop.
fn temp_file(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("oc-tokensrc-")
        .tempdir()
        .expect("tempdir");
    let path = dir.path().join(name);
    (dir, path)
}

// ---- tier precedence ---------------------------------------------------

#[test]
fn projected_file_beats_static_key() {
    let (_path_dir, path) = temp_file("token");
    std::fs::write(&path, "projected").unwrap();
    let env = MapEnv::new([
        (TOKEN_FILE_ENV, path.display().to_string()),
        (API_KEY_ENV, "th_static".to_string()),
    ]);
    let source = TinyhumansTokenSource::from_env(&env).expect("configured");
    assert_eq!(source.tier(), TokenTier::ProjectedFile);
    assert_eq!(source.token_file(), Some(path.as_path()));
    assert_eq!(source.credential_source(), CredentialSource::Attested);
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// The docker runtime mounts nothing. A leftover `TINYHUMANS_TOKEN_FILE`
/// pointing at a path that was never projected must degrade to the static
/// tier — not select a tier that can only ever fail.
#[test]
fn a_token_file_that_does_not_exist_degrades_to_the_static_tier() {
    let env = MapEnv::new([
        (TOKEN_FILE_ENV, "/nonexistent/oc/token"),
        (API_KEY_ENV, "th_static"),
    ]);
    let source = TinyhumansTokenSource::from_env(&env).expect("configured");
    assert_eq!(source.tier(), TokenTier::Static);

    // With nothing to fall back to, there is simply no source.
    let alone = MapEnv::new([(TOKEN_FILE_ENV, "/nonexistent/oc/token")]);
    assert!(TinyhumansTokenSource::from_env(&alone).is_none());
}

#[test]
fn static_key_is_the_fallback_tier() {
    let env = MapEnv::new([(API_KEY_ENV, "th_static")]);
    let source = TinyhumansTokenSource::from_env(&env).expect("configured");
    assert_eq!(source.tier(), TokenTier::Static);
    assert!(source.token_file().is_none());
    assert_eq!(source.credential_source(), CredentialSource::Static);
}

#[test]
fn neither_configured_resolves_to_none() {
    assert!(TinyhumansTokenSource::from_env(&MapEnv::default()).is_none());
    // Blank values are not configuration.
    let blank = MapEnv::new([(TOKEN_FILE_ENV, "   "), (API_KEY_ENV, "  ")]);
    assert!(TinyhumansTokenSource::from_env(&blank).is_none());
    // A blank token file falls through to the static key rather than
    // resolving to a source that can never read anything.
    let fallthrough = MapEnv::new([(TOKEN_FILE_ENV, " "), (API_KEY_ENV, "th_static")]);
    assert_eq!(
        TinyhumansTokenSource::from_env(&fallthrough)
            .expect("configured")
            .tier(),
        TokenTier::Static
    );
}

#[tokio::test]
async fn static_tier_returns_its_configured_value() {
    let source = TinyhumansTokenSource::static_key("th_static");
    assert_eq!(source.current().await.unwrap(), "th_static");
}

// ---- projected-file rotation + cache window ----------------------------

/// The kubelet rewrites the projected file **in place**. Once the cache
/// window closes, the next call must present the new token — a permanent
/// cache here is the latent outage this tier exists to avoid.
#[tokio::test]
async fn projected_file_picks_up_an_in_place_rotation() {
    let (_path_dir, path) = temp_file("token");
    // exp 1000s out at t=0 → window = min(0.8 * 1000, 60) = 60s.
    std::fs::write(&path, jwt(serde_json::json!({ "exp": 1000 }))).unwrap();
    let first = std::fs::read_to_string(&path).unwrap();
    let source = TinyhumansTokenSource::projected_file(&path);

    assert_eq!(source.current_at(unix(0)).await.unwrap(), first);

    // Rotation lands. Inside the window the cached copy is still served —
    // that is what keeps the hot path off the filesystem.
    std::fs::write(&path, jwt(serde_json::json!({ "exp": 2000 }))).unwrap();
    let second = std::fs::read_to_string(&path).unwrap();
    assert_ne!(first, second, "the rotation must change the file");
    assert_eq!(
        source.current_at(unix(59)).await.unwrap(),
        first,
        "inside the cache window the previous read is reused"
    );

    // Past the window the file is re-read and the rotated token is served.
    assert_eq!(
        source.current_at(unix(61)).await.unwrap(),
        second,
        "past the cache window the rotated token must be picked up"
    );

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// The window is driven by the token's own expiry: 80% of the remaining TTL,
/// capped at [`MAX_CACHE_WINDOW`], and never for a token we cannot date.
#[test]
fn cache_window_is_eighty_percent_of_remaining_ttl_capped_at_a_minute() {
    // 10s left → 8s window (well under the cap).
    assert_eq!(
        cache_window(unix(0), Some(unix(10))),
        Some(Duration::from_secs(8))
    );
    // 8 minutes left (the projected-token rotation period) → capped at 60s.
    assert_eq!(
        cache_window(unix(0), Some(unix(480))),
        Some(MAX_CACHE_WINDOW)
    );
    // Already expired, or expiring exactly now → never cache.
    assert_eq!(cache_window(unix(100), Some(unix(90))), None);
    assert_eq!(cache_window(unix(100), Some(unix(100))), None);
    // Unknown expiry → never cache.
    assert_eq!(cache_window(unix(0), None), None);
}

/// A rejected bearer (401) must not wait out the cache window: invalidating
/// sends the next call straight back to the file.
#[tokio::test]
async fn invalidate_forces_a_re_read_inside_the_cache_window() {
    let (_path_dir, path) = temp_file("token");
    std::fs::write(&path, jwt(serde_json::json!({ "exp": 1000 }))).unwrap();
    let first = std::fs::read_to_string(&path).unwrap();
    let source = TinyhumansTokenSource::projected_file(&path);
    assert_eq!(source.current_at(unix(0)).await.unwrap(), first);

    std::fs::write(&path, jwt(serde_json::json!({ "exp": 1001 }))).unwrap();
    let second = std::fs::read_to_string(&path).unwrap();
    // Same instant, so the window is wide open — but the cache is gone.
    source.invalidate();
    assert_eq!(source.current_at(unix(0)).await.unwrap(), second);

    // Harmless on the static tier.
    TinyhumansTokenSource::static_key("k").invalidate();
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// A token whose expiry cannot be read is re-read on **every** call. Slower,
/// but it can never serve a credential the platform already rotated away.
#[tokio::test]
async fn an_undatable_token_is_never_cached() {
    let (_path_dir, path) = temp_file("token");
    std::fs::write(&path, "opaque-not-a-jwt").unwrap();
    let source = TinyhumansTokenSource::projected_file(&path);
    assert_eq!(
        source.current_at(unix(0)).await.unwrap(),
        "opaque-not-a-jwt"
    );

    std::fs::write(&path, "opaque-rotated").unwrap();
    assert_eq!(
        source.current_at(unix(0)).await.unwrap(),
        "opaque-rotated",
        "with no readable expiry every call re-reads the file"
    );

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// An expired token is still presented — the backend decides, and refusing
/// locally would turn a recoverable 401 into an outage — but never cached.
#[tokio::test]
async fn an_expired_token_is_served_but_not_cached() {
    let (_path_dir, path) = temp_file("token");
    std::fs::write(&path, jwt(serde_json::json!({ "exp": 50 }))).unwrap();
    let stale = std::fs::read_to_string(&path).unwrap();
    let source = TinyhumansTokenSource::projected_file(&path);
    assert_eq!(source.current_at(unix(100)).await.unwrap(), stale);

    std::fs::write(&path, jwt(serde_json::json!({ "exp": 9000 }))).unwrap();
    let fresh = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        source.current_at(unix(100)).await.unwrap(),
        fresh,
        "an expired token must not have been cached"
    );

    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[tokio::test]
async fn a_missing_or_empty_token_file_is_a_config_error() {
    let missing = TinyhumansTokenSource::projected_file("/nonexistent/oc/token");
    let err = missing.current().await.expect_err("unreadable");
    assert_eq!(err.code(), "config_error");

    let (_path_dir, path) = temp_file("token");
    std::fs::write(&path, "   \n").unwrap();
    let empty = TinyhumansTokenSource::projected_file(&path);
    assert_eq!(
        empty.current().await.expect_err("empty").code(),
        "config_error"
    );
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

// ---- fingerprint identity ---------------------------------------------

fn identity_of(source: &TinyhumansTokenSource) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash_identity(&mut hasher);
    hasher.finish()
}

/// The load-bearing fingerprint contract: a kubelet rotation changes the
/// token but NOT the identity, so the agents' tool roster is not rebuilt
/// every few minutes. A tier change or a static-value change does move it.
#[test]
fn identity_is_stable_across_rotation_and_moves_on_tier_or_value_change() {
    let projected = TinyhumansTokenSource::projected_file("/var/run/token");
    let same_path = TinyhumansTokenSource::projected_file("/var/run/token");
    assert_eq!(
        identity_of(&projected),
        identity_of(&same_path),
        "the same projected path is the same identity whatever the file holds"
    );

    let other_path = TinyhumansTokenSource::projected_file("/var/run/other");
    assert_ne!(identity_of(&projected), identity_of(&other_path));

    let static_a = TinyhumansTokenSource::static_key("key-a");
    let static_b = TinyhumansTokenSource::static_key("key-b");
    assert_ne!(
        identity_of(&static_a),
        identity_of(&static_b),
        "a rotated static key IS a new identity"
    );
    assert_ne!(
        identity_of(&projected),
        identity_of(&static_a),
        "a tier change must move the identity"
    );
}

// ---- redaction ---------------------------------------------------------

#[test]
fn debug_and_describe_never_render_the_token() {
    let source = TinyhumansTokenSource::static_key("th_super_secret_value");
    for rendered in [format!("{source:?}"), source.describe()] {
        assert!(!rendered.contains("th_super_secret_value"), "{rendered}");
    }
    assert!(format!("{source:?}").contains("<redacted>"));

    let projected = TinyhumansTokenSource::projected_file("/var/run/secrets/token");
    assert!(projected.describe().contains("/var/run/secrets/token"));
    assert!(projected.describe().contains("projected_file"));
}

// ---- jwt / base64url ---------------------------------------------------

#[test]
fn unverified_jwt_exp_reads_the_claim_without_verifying() {
    assert_eq!(
        unverified_jwt_exp(&jwt(serde_json::json!({ "exp": 1893456000u64 }))),
        Some(unix(1_893_456_000))
    );
    // No exp, not a JWT, wrong segment count, non-numeric exp → unreadable.
    assert!(unverified_jwt_exp(&jwt(serde_json::json!({ "aud": "x" }))).is_none());
    assert!(unverified_jwt_exp("not-a-jwt").is_none());
    assert!(unverified_jwt_exp("a.b").is_none());
    assert!(unverified_jwt_exp("a.b.c.d").is_none());
    assert!(unverified_jwt_exp(&jwt(serde_json::json!({ "exp": "soon" }))).is_none());
    // A payload that is not base64url at all.
    assert!(unverified_jwt_exp("aaa.!!!!.bbb").is_none());
}

#[test]
fn base64url_decodes_the_unpadded_alphabet() {
    assert_eq!(base64url_decode("aGVsbG8").unwrap(), b"hello");
    assert_eq!(base64url_decode("aGVsbG8=").unwrap(), b"hello");
    assert_eq!(base64url_decode("").unwrap(), Vec::<u8>::new());
    // `-` and `_` are the url-safe substitutions for `+` and `/`.
    assert_eq!(base64url_decode("--__").unwrap(), vec![0xFB, 0xEF, 0xFF]);
    assert!(base64url_decode("a+b/c").is_none());
}

// ---- Credential --------------------------------------------------------

#[tokio::test]
async fn credential_variants_report_status_and_resolve() {
    use std::sync::Arc;

    let none = Credential::None;
    assert!(!none.configured());
    assert_eq!(none.source(), CredentialSource::None);
    assert_eq!(none.current().await.unwrap(), None);

    // Blank is not configuration.
    assert!(!Credential::from_value("   ").configured());

    let value = Credential::from_value("pasted-token");
    assert!(value.configured());
    assert_eq!(value.source(), CredentialSource::Static);
    assert_eq!(
        value.current().await.unwrap().as_deref(),
        Some("pasted-token")
    );

    let (_path_dir, path) = temp_file("token");
    std::fs::write(&path, "projected-token").unwrap();
    let source = Credential::from_source(Arc::new(TinyhumansTokenSource::projected_file(&path)));
    assert!(source.configured());
    assert_eq!(source.source(), CredentialSource::Attested);
    assert_eq!(
        source.current().await.unwrap().as_deref(),
        Some("projected-token")
    );
    // The value it yields follows the file, per call.
    std::fs::write(&path, "rotated-token").unwrap();
    assert_eq!(
        source.current().await.unwrap().as_deref(),
        Some("rotated-token")
    );
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn credential_debug_never_renders_a_value() {
    let rendered = format!("{:?}", Credential::from_value("super-secret"));
    assert!(!rendered.contains("super-secret"), "{rendered}");
    assert!(rendered.contains("<redacted>"));
    assert!(format!("{:?}", Credential::None).contains("<unset>"));
}

/// The tier's DTO spelling is a wire contract the console switches on.
#[test]
fn credential_source_wire_spellings_are_stable() {
    assert_eq!(CredentialSource::Attested.as_str(), "attested");
    assert_eq!(CredentialSource::Static.as_str(), "static");
    assert_eq!(CredentialSource::None.as_str(), "none");
    assert_eq!(
        serde_json::to_string(&CredentialSource::Attested).unwrap(),
        "\"attested\""
    );
    assert_eq!(TokenTier::ProjectedFile.as_str(), "projected_file");
    assert_eq!(TokenTier::Static.as_str(), "static");
}
