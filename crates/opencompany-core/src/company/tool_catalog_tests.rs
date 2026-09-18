use super::*;
use crate::company::GATEABLE_NAMESPACES;
use crate::company::{grants_composio_explicit, grants_media_explicit, grants_search_explicit};

fn manifest(toml: &str) -> CompanyManifest {
    toml::from_str(toml).expect("valid toml")
}

fn base() -> CompanyManifest {
    manifest("[company]\nname = \"X\"\n")
}

#[test]
fn the_catalog_lists_every_gateable_namespace() {
    let entries = catalog(&base());
    for namespace in GATEABLE_NAMESPACES {
        assert!(
            entries.iter().any(|e| e.grant == namespace),
            "`{namespace}` is gateable but absent from the catalog"
        );
    }
}

/// The invariant that keeps this a projection: every grant the catalog
/// advertises must be one the real matcher understands. A catalog entry
/// naming a token nothing resolves would be a permission this file invented.
#[test]
fn every_advertised_grant_resolves_through_the_real_matcher() {
    let m = manifest(
        "[company]\nname = \"X\"\n\n[[mcp_server]]\nname = \"notion\"\nendpoint = \"https://example.com\"\n\n[tools.composio]\ntoolkits = [\"gmail\"]\n",
    );
    for entry in catalog(&m) {
        // Granting exactly this token at the company level must make the
        // catalog report the entry as granted.
        let allow = vec![entry.grant.clone()];
        let mut granting = m.clone();
        granting.tools.allow = allow;
        let resolved = catalog(&granting);
        let same = resolved
            .iter()
            .find(|e| e.key == entry.key)
            .expect("entry survives");
        assert!(
            same.granted,
            "granting `{}` did not confer catalog entry `{}`",
            entry.grant, entry.key
        );
    }
}

/// The wildcard set here must match the explicit-grant predicates exactly,
/// or the console would tell an operator `*` granted something it did not.
#[test]
fn the_wildcard_set_matches_the_explicit_grant_predicates() {
    let wildcard = vec!["*".to_string()];
    assert!(!grants_media_explicit(&wildcard));
    assert!(!grants_composio_explicit(&wildcard));
    assert!(!grants_search_explicit(&wildcard));

    for (namespace, _) in BUILTIN_DESCRIPTIONS {
        let expected = !matches!(*namespace, "media" | "composio" | "search");
        assert_eq!(
            wildcard_covers(namespace),
            expected,
            "`{namespace}` wildcard coverage disagrees with the predicates"
        );
    }
}

#[test]
fn a_wildcard_company_grants_the_ordinary_families_but_not_the_opt_in_ones() {
    let mut m = base();
    m.tools.allow = vec!["*".to_string()];
    let entries = catalog(&m);
    let granted = |grant: &str| {
        entries
            .iter()
            .find(|e| e.grant == grant)
            .map(|e| e.granted)
            .expect("entry")
    };
    assert!(granted("shell"));
    assert!(granted("web"));
    assert!(!granted("media"), "`*` must never confer real-money media");
    assert!(!granted("search"), "`*` must never confer billed search");
}

#[test]
fn an_mcp_server_appears_with_its_own_grant_token() {
    let m = manifest(
        "[company]\nname = \"X\"\n\n[[mcp_server]]\nname = \"notion\"\nendpoint = \"https://example.com\"\ndescription = \"Notion pages.\"\n",
    );
    let entry = catalog(&m)
        .into_iter()
        .find(|e| matches!(&e.kind, ToolKind::Mcp { server } if server == "notion"))
        .expect("server listed");
    assert_eq!(entry.grant, "mcp:notion");
    assert_eq!(entry.description, "Notion pages.");
}

/// A disabled server is listed and not granted. Omitting the row would leave
/// an operator unable to see that the server exists at all.
#[test]
fn a_disabled_mcp_server_is_listed_but_not_granted() {
    let m = manifest(
        "[company]\nname = \"X\"\ntools = { allow = [\"mcp:notion\"] }\n\n[[mcp_server]]\nname = \"notion\"\nendpoint = \"https://example.com\"\nenabled = false\n",
    );
    let entry = catalog(&m)
        .into_iter()
        .find(|e| e.key == "mcp:notion")
        .expect("listed");
    assert!(!entry.granted);
}

/// Composio toolkits ride the single `composio` grant — the catalog must not
/// advertise a per-toolkit permission that nothing enforces.
#[test]
fn composio_toolkits_share_the_one_composio_grant() {
    let m = manifest(
        "[company]\nname = \"X\"\n\n[tools]\nallow = [\"composio\"]\n\n[tools.composio]\ntoolkits = [\"gmail\", \"slack\"]\n",
    );
    let toolkits: Vec<CatalogEntry> = catalog(&m)
        .into_iter()
        .filter(|e| matches!(e.kind, ToolKind::Composio { .. }))
        .collect();
    assert_eq!(toolkits.len(), 2);
    for entry in toolkits {
        assert_eq!(entry.grant, "composio");
        assert!(entry.granted);
    }
}

#[test]
fn catalog_keys_are_unique() {
    let m = manifest(
        "[company]\nname = \"X\"\n\n[[mcp_server]]\nname = \"notion\"\nendpoint = \"https://e.com\"\n\n[tools.composio]\ntoolkits = [\"gmail\"]\n",
    );
    let entries = catalog(&m);
    let mut keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
    keys.sort_unstable();
    let before = keys.len();
    keys.dedup();
    assert_eq!(before, keys.len(), "duplicate catalog keys");
}
