use super::*;

#[test]
fn a_missing_file_means_the_user_never_chose() {
    let dir = tempfile::tempdir().unwrap();
    let preferences = Preferences::load(dir.path());
    assert_eq!(preferences, Preferences::default());
    assert!(preferences.analytics_enabled(), "the default is on");
}

#[test]
fn a_choice_round_trips_atomically() {
    let dir = tempfile::tempdir().unwrap();
    Preferences {
        analytics: Some(false),
    }
    .save(dir.path())
    .unwrap();

    let body = std::fs::read_to_string(dir.path().join(PREFERENCES_FILE)).unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json, serde_json::json!({ "analytics": false }));

    let loaded = Preferences::load(dir.path());
    assert_eq!(loaded.analytics, Some(false));
    assert!(!loaded.analytics_enabled());

    // No temporary file is left behind by the rename.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".preferences")
        })
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn an_unreadable_file_falls_back_to_the_default() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(PREFERENCES_FILE), b"{not json").unwrap();
    assert_eq!(Preferences::load(dir.path()), Preferences::default());

    // A directory where the file should be is unreadable too.
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir(other.path().join(PREFERENCES_FILE)).unwrap();
    assert!(Preferences::load(other.path()).analytics_enabled());
}

#[test]
fn unknown_keys_are_ignored_and_a_key_less_file_is_the_default() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(PREFERENCES_FILE), br#"{"theme":"dark"}"#).unwrap();
    assert_eq!(Preferences::load(dir.path()).analytics, None);
}
