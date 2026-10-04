use super::*;
use crate::app::config::MapEnv;

#[test]
fn parses_storage_kinds() {
    assert_eq!("fs".parse::<StorageKind>().unwrap(), StorageKind::Fs);
    assert_eq!(
        "sqlite".parse::<StorageKind>().unwrap(),
        StorageKind::Sqlite
    );
    assert_eq!(
        "MongoDB".parse::<StorageKind>().unwrap(),
        StorageKind::Mongodb
    );
    assert!("postgres".parse::<StorageKind>().is_err());
}

/// Issue #752: only MongoDB keeps secret material off the container's own
/// disk, so only MongoDB clears the repository-credential gates.
#[test]
fn only_mongodb_keeps_secrets_off_the_local_disk() {
    assert!(StorageKind::Fs.secrets_are_plaintext_on_disk());
    assert!(StorageKind::Sqlite.secrets_are_plaintext_on_disk());
    assert!(!StorageKind::Mongodb.secrets_are_plaintext_on_disk());
    // The default is the refusing side: a host that never resolved a
    // backend must not be treated as one that keeps secrets safely.
    assert!(StorageKind::default().secrets_are_plaintext_on_disk());
}

/// The refusal has to be actionable on its own — an operator reading it in
/// a console toast has nothing else to go on.
#[test]
fn the_refusal_names_the_condition_and_both_remedies() {
    let message = plaintext_secret_refusal(StorageKind::Fs);
    assert!(message.contains("OPENCOMPANY_STORAGE=fs"), "{message}");
    assert!(message.contains("OPENCOMPANY_STORAGE=mongodb"), "{message}");
    assert!(message.contains("OPENCOMPANY_MONGODB_URI"), "{message}");
    assert!(message.contains("`repo` grant"), "{message}");
    assert!(message.contains("plaintext"), "{message}");
    // The named kind is the one actually in force, not a hard-coded "fs".
    assert!(plaintext_secret_refusal(StorageKind::Sqlite).contains("OPENCOMPANY_STORAGE=sqlite"),);
}

#[tokio::test]
async fn fs_selection_uses_builder_defaults() {
    let settings = StorageSettings::default();
    let handles = open_storage(&settings, Path::new("/tmp")).await.unwrap();
    assert!(handles.is_none());
}

#[test]
fn settings_debug_never_renders_a_credential() {
    // `StorageSettings` is printed at boot, so a derived `Debug` would put a
    // MongoDB connection string in the startup log of every tenant container.
    let settings = StorageSettings {
        mongodb_uri: Some("mongodb://user:hunter2@cluster.example/db".into()),
        ..StorageSettings::default()
    };
    let rendered = format!("{settings:?}");
    assert!(!rendered.contains("hunter2"), "{rendered}");
    // Still useful: it says the value is configured.
    assert!(rendered.contains("<set>"), "{rendered}");
}

#[test]
fn from_env_reads_tenant_id() {
    let env = MapEnv::new([("OPENCOMPANY_TENANT_ID", "acme")]);
    assert_eq!(
        StorageSettings::from_env_source(&env)
            .unwrap()
            .tenant_id
            .as_deref(),
        Some("acme")
    );

    // An empty value is filtered out, same as the mongodb vars.
    assert_eq!(
        StorageSettings::from_env_source(&MapEnv::new([("OPENCOMPANY_TENANT_ID", "")]))
            .unwrap()
            .tenant_id,
        None
    );

    // Unset leaves it `None` (the id-namespacing no-op).
    assert_eq!(
        StorageSettings::from_env_source(&MapEnv::default())
            .unwrap()
            .tenant_id,
        None
    );
}

#[cfg(feature = "mongodb")]
#[tokio::test]
async fn mongodb_selection_requires_uri() {
    let settings = StorageSettings {
        kind: StorageKind::Mongodb,
        ..Default::default()
    };
    let error = open_storage(&settings, std::path::Path::new("/tmp"))
        .await
        .expect_err("mongodb without a URI must refuse")
        .to_string();
    assert!(error.contains("OPENCOMPANY_MONGODB_URI"), "{error}");
}

/// The bundle-command refusals, executed — the #1279 review neutralised
/// the bin-resident versions with `if false &&` and nothing went red;
/// these are the tests that make that mutation fail.
#[test]
fn bundle_env_refusals_fire_and_the_fs_default_passes() {
    // fs default: both flag spellings pass — no regression.
    let default = StorageSettings::default();
    refuse_bundle_env(&default, false).expect("default env, no flag");
    refuse_bundle_env(&default, true).expect("default env, explicit --home");

    // A live environment refuses an explicit --home (two deployments in
    // one bundle) but proceeds without the flag.
    let live = StorageSettings {
        kind: StorageKind::Mongodb,
        ..StorageSettings::default()
    };
    let err = refuse_bundle_env(&live, true)
        .expect_err("live env + --home must refuse")
        .to_string();
    assert!(err.contains("--home"), "{err}");
    refuse_bundle_env(&live, false).expect("live env without the flag proceeds");

    // Tenant mode refuses on a live env; whitespace does not count as set.
    let tenant = StorageSettings {
        kind: StorageKind::Mongodb,
        tenant_id: Some("acme".into()),
        ..StorageSettings::default()
    };
    let err = refuse_bundle_env(&tenant, false)
        .expect_err("tenant mode must refuse")
        .to_string();
    assert!(err.contains("OPENCOMPANY_TENANT_ID"), "{err}");
    let blank_tenant = StorageSettings {
        kind: StorageKind::Mongodb,
        tenant_id: Some("  ".into()),
        ..StorageSettings::default()
    };
    refuse_bundle_env(&blank_tenant, false).expect("blank tenant id is unset");
}
