use super::*;

#[test]
fn every_account_provider_has_an_endpoint_a_key_source_and_an_auth_style() {
    for info in CATALOGUE.iter().filter(|i| i.category == Category::Account) {
        assert!(info.auth.is_some(), "{} has no auth style", info.slug);
        assert!(
            info.endpoint.starts_with("https://"),
            "{} endpoint is not https",
            info.slug
        );
        assert!(info.key_source.is_some(), "{} has no key source", info.slug);
        assert!(info.needs_key(), "{}", info.slug);
        assert!(!info.needs_endpoint(), "{}", info.slug);
    }
}

#[test]
fn searxng_is_an_address_rather_than_an_account() {
    let info = entry("searxng").expect("searxng");
    assert!(info.auth.is_none());
    assert!(!info.needs_key());
    assert!(info.needs_endpoint());
    assert!(info.endpoint.is_empty());
}

#[test]
fn managed_is_not_a_catalogue_entry() {
    // It has no endpoint the company owns and no key it pastes, so there is
    // nothing to add. The row is rendered from resolution instead.
    assert!(entry(super::super::MANAGED_PROVIDER).is_none());
}

/// The Rust table and its TypeScript mirror must not drift.
///
/// The inference rework's fourth known defect is a provider table duplicated
/// by hand across the language boundary with nothing noticing when the
/// copies disagree — a provider added to one and not the other half-works.
/// This catalogue is four rows, so there is no excuse for repeating it.
///
/// Reads the mirror as text rather than parsing TypeScript: the property
/// this holds is that the same four slugs, labels and categories appear on
/// both sides, and a regex over a `const` array is enough to fail loudly
/// when one is added to only one of them.
/// The console mirror, found by walking up from this crate's manifest.
///
/// **Not `CARGO_MANIFEST_DIR/frontend`.** That only resolves when the
/// manifest directory *is* the repo root, which it is for a plain `cargo
/// test` at the top level and is not in CI, where the crate is built from
/// `crates/opencompany-core` — so the test passed locally and panicked on
/// the `Rust` lane with "cannot read …".
fn console_mirror() -> Option<std::path::PathBuf> {
    let mut dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("frontend/src/search-providers/catalogue.ts");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = dir.parent()?;
    }
}

#[test]
fn the_console_mirror_lists_the_same_providers() {
    let mirror = console_mirror()
        .expect("the console mirror must exist somewhere above this crate's manifest");
    let source = std::fs::read_to_string(&mirror)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", mirror.display()));

    for info in CATALOGUE {
        assert!(
            source.contains(&format!("slug: \"{}\"", info.slug)),
            "`{}` is in the Rust catalogue and not in the console mirror",
            info.slug
        );
        assert!(
            source.contains(&format!("label: \"{}\"", info.label)),
            "`{}` has a different label in the console mirror",
            info.slug
        );
        assert!(
            source.contains(&format!("category: \"{}\"", info.category.as_str())),
            "`{}`'s category is missing from the console mirror",
            info.slug
        );
    }

    let mirrored = source.matches("slug: \"").count();
    assert_eq!(
        mirrored,
        CATALOGUE.len(),
        "the console mirror lists {mirrored} providers and the Rust catalogue lists {}",
        CATALOGUE.len()
    );
}

#[test]
fn every_catalogue_slug_is_a_supported_provider() {
    for info in CATALOGUE {
        assert!(
            super::super::provider_supported(info.slug),
            "{} is in the catalogue but not in SUPPORTED_PROVIDERS",
            info.slug
        );
    }
}
