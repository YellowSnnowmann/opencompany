use super::*;

#[test]
fn a_remembered_session_comes_back() {
    remember_device("conn-a", "acme.token-a").unwrap();
    assert_eq!(device_session("conn-a").as_deref(), Some("acme.token-a"));
}

#[test]
fn connections_do_not_share_an_entry() {
    // The namespacing property. Two hosts in one keychain must not collide,
    // and a desktop holding several is the entire point.
    remember_device("conn-b", "acme.token-b").unwrap();
    remember_device("conn-c", "other.token-c").unwrap();
    assert_eq!(device_session("conn-b").as_deref(), Some("acme.token-b"));
    assert_eq!(device_session("conn-c").as_deref(), Some("other.token-c"));
}

#[test]
fn an_unpaired_connection_has_no_session() {
    assert_eq!(device_session("conn-never-paired"), None);
}

#[test]
fn forgetting_is_idempotent() {
    // Removing a connection twice, or removing one that never paired, must
    // not surface an error to a console that is only tidying up.
    remember_device("conn-d", "acme.token-d").unwrap();
    forget_device("conn-d").unwrap();
    assert_eq!(device_session("conn-d"), None);
    forget_device("conn-d").expect("deleting an absent entry is not an error");
}

#[test]
fn tests_never_reach_the_operators_keychain() {
    // The property that makes this module testable at all. If this ever
    // reported `os`, every test above would be writing to a real keychain —
    // a modal prompt on macOS, and nothing at all on a headless runner.
    assert_eq!(store().name(), "memory");
}

/// A store that is reachable for reads but refuses every write.
///
/// What a locked keychain looks like from here, and the shape the old probe
/// could not see: `get` answers `Ok(None)` either way, so a probe built on
/// the read path called this backend healthy and then failed on the first
/// `remember_device`.
struct LockedStore;

impl SecretStore for LockedStore {
    fn get(&self, _key: &str) -> Result<Option<String>, KeychainError> {
        Ok(None)
    }
    fn set(&self, _key: &str, _value: &str) -> Result<(), KeychainError> {
        Err(KeychainError::Backend("the keychain is locked".into()))
    }
    fn delete(&self, _key: &str) -> Result<(), KeychainError> {
        Err(KeychainError::Backend("the keychain is locked".into()))
    }
    fn name(&self) -> &'static str {
        "locked"
    }
}

#[test]
fn a_read_cannot_tell_a_locked_store_from_an_empty_one() {
    // The property that makes the probe's own error inspection necessary:
    // no amount of reading distinguishes these, so a probe built on `get`
    // is a probe that always says yes.
    assert_eq!(LockedStore.get("device-session:x").unwrap(), None);
    assert_eq!(
        MemoryStore::default().get("device-session:x").unwrap(),
        None
    );
    // And the difference only shows on write, which is too late to choose a
    // backend by.
    assert!(LockedStore.set("device-session:x", "acme.tok").is_err());
    assert!(
        MemoryStore::default()
            .set("device-session:x", "acme.tok")
            .is_ok()
    );
}

#[test]
fn a_key_names_its_purpose_and_its_connection() {
    // Someone reading their own keychain should be able to tell what an
    // entry is for and delete the right one.
    assert_eq!(device_key("abc123"), "device-session:abc123");
}
