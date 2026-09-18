use super::*;
use crate::company::CompanyManifest;
use crate::runtime::RuntimeBuilder;

fn manifest(name: &str) -> CompanyManifest {
    toml::from_str(&format!("[company]\nname = \"{name}\"\n")).unwrap()
}

fn tmp_home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-reg-")
        .tempdir()
        .expect("tempdir")
}

async fn runtime(home: &std::path::Path, id: &str) -> Arc<CompanyRuntime> {
    Arc::new(
        RuntimeBuilder::new(home.to_path_buf(), manifest(id))
            .with_id(CompanyId::new(id))
            .build()
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn sole_returns_the_only_company() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let registry = CompanyRegistry::new();
    assert!(registry.is_empty());
    assert!(registry.sole().is_none());

    registry.insert(CompanyId::new("acme"), runtime(&home, "acme").await);
    assert!(registry.sole().is_some());
    assert_eq!(registry.list(), vec![CompanyId::new("acme")]);

    registry.insert(CompanyId::new("globex"), runtime(&home, "globex").await);
    // Two companies: no sole.
    assert!(registry.sole().is_none());
    assert_eq!(registry.len(), 2);
    assert!(registry.get(&CompanyId::new("globex")).is_some());
}

#[tokio::test]
async fn remove_unregisters_the_company() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let registry = CompanyRegistry::new();
    registry.insert(CompanyId::new("acme"), runtime(&home, "acme").await);
    assert!(registry.get(&CompanyId::new("acme")).is_some());

    let removed = registry.remove(&CompanyId::new("acme"));
    assert!(removed.is_some());
    assert!(registry.get(&CompanyId::new("acme")).is_none());
    assert!(registry.remove(&CompanyId::new("acme")).is_none());
}

/// `remove_if` removes on a match, the same as an unconditional `remove`.
#[tokio::test]
async fn remove_if_removes_on_a_matching_runtime() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let registry = CompanyRegistry::new();
    let rt = runtime(&home, "acme").await;
    registry.insert(CompanyId::new("acme"), rt.clone());

    let removed = registry.remove_if(&CompanyId::new("acme"), &rt);
    assert!(removed.is_some());
    assert!(registry.get(&CompanyId::new("acme")).is_none());
}

/// Codex review on #1943, PR comment 3894439351: a caller that observed
/// one runtime instance and later asks to remove it by id must not evict
/// whatever has since replaced it there — `insert` (the only writer of an
/// already-occupied slot; a rebuild swap is the production case) replaces
/// unconditionally, so a plain `remove(id)` would delete the replacement
/// instead of doing nothing.
#[tokio::test]
async fn remove_if_leaves_a_replacement_untouched() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let registry = CompanyRegistry::new();
    let original = runtime(&home, "acme").await;
    let replacement = runtime(&home, "acme").await;
    assert!(
        !Arc::ptr_eq(&original, &replacement),
        "sanity: distinct instances"
    );

    // The replacement is what's actually registered now.
    registry.insert(CompanyId::new("acme"), replacement.clone());

    let removed = registry.remove_if(&CompanyId::new("acme"), &original);
    assert!(
        removed.is_none(),
        "the registered runtime is not the one `remove_if` was asked to remove"
    );
    let still_there = registry.get(&CompanyId::new("acme"));
    assert!(still_there.is_some(), "the id must still be registered");
    assert!(
        Arc::ptr_eq(still_there.as_ref().unwrap(), &replacement),
        "the replacement must be exactly what remains registered — untouched"
    );
}

/// `remove_if` on an id that was never registered is a no-op, the same
/// shape as "replaced" from the caller's point of view (`None` either
/// way) — nothing to assert differently, just that it doesn't panic and
/// genuinely removes nothing.
#[tokio::test]
async fn remove_if_on_an_unregistered_id_is_a_no_op() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let registry = CompanyRegistry::new();
    let rt = runtime(&home, "acme").await;

    let removed = registry.remove_if(&CompanyId::new("acme"), &rt);
    assert!(removed.is_none());
    assert!(registry.is_empty());
}
