//! Single-use marker tests: first consume wins, every later one loses.

use super::*;
use crate::ports::types::CompanyId;

fn temp_home() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oc-sso-jti-{}-{}",
        std::process::id(),
        crate::ports::generate_id()
    ));
    std::fs::create_dir_all(&dir).expect("temp home");
    dir
}

#[tokio::test]
async fn first_consume_wins_and_replay_loses() {
    // Arrange
    let home = temp_home();
    let company = CompanyId::new("acme");
    let markers = ConsumedJtis::new(&home, &company);

    // Act
    let first = markers
        .consume("jti-1", 9_999_999_999)
        .await
        .expect("first");
    let second = markers
        .consume("jti-1", 9_999_999_999)
        .await
        .expect("second");

    // Assert
    assert!(first, "the first consume of a jti must win");
    assert!(!second, "a replay of the same jti must lose");
}

#[tokio::test]
async fn distinct_jtis_are_independent() {
    let home = temp_home();
    let company = CompanyId::new("acme");
    let markers = ConsumedJtis::new(&home, &company);

    assert!(markers.consume("a", 1).await.expect("a"));
    assert!(markers.consume("b", 1).await.expect("b"));
    // And each is still single-use.
    assert!(!markers.consume("a", 1).await.expect("a again"));
}

#[tokio::test]
async fn companies_do_not_share_consumed_jtis() {
    // One process serving two companies (local development) must not let a token
    // consumed in one appear consumed in the other.
    let home = temp_home();
    let acme = ConsumedJtis::new(&home, &CompanyId::new("acme"));
    let other = ConsumedJtis::new(&home, &CompanyId::new("other"));

    assert!(acme.consume("shared-jti", 1).await.expect("acme"));
    assert!(
        other.consume("shared-jti", 1).await.expect("other"),
        "a jti consumed in acme must still be redeemable in a different company"
    );
}

#[tokio::test]
async fn a_traversal_jti_stays_inside_the_marker_dir() {
    // A jti is attacker-influenced; it must never escape the marker directory.
    // Hashing it into the filename makes the path `[0-9a-f]{64}` by construction.
    let home = temp_home();
    let company = CompanyId::new("acme");
    let markers = ConsumedJtis::new(&home, &company);

    assert!(
        markers
            .consume("../../etc/passwd", 1)
            .await
            .expect("traversal jti")
    );

    // Nothing was written outside the per-company marker directory.
    let outside = home.join("sso").join("acme").join("etc");
    assert!(
        !outside.exists(),
        "a traversal jti must not write outside its dir"
    );
    let escaped = home.join("etc").join("passwd");
    assert!(
        !escaped.exists(),
        "a traversal jti must not escape the data root"
    );
}
