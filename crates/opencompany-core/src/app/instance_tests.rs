use super::*;

struct Fixed(u8);
impl TokenSource for Fixed {
    fn fill(&self, out: &mut [u8]) {
        out.fill(self.0);
    }
}

/// `peek` reads, and — the point of it existing — never writes.
#[test]
fn peeking_reports_an_id_without_creating_one() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(peek(dir.path()), None, "an empty root has no id to report");
    assert!(
        !dir.path().join(INSTANCE_ID_FILE).exists(),
        "peeking must not mint: the desktop peeks at roots it is not running"
    );

    let id = load_or_create(dir.path());
    assert_eq!(peek(dir.path()).as_deref(), Some(id.as_str()));
}

/// A hand-edited file is not an id, and `peek` says so rather than
/// replacing it — replacing is `load_or_create`'s job, on the boot path.
#[test]
fn peeking_refuses_a_malformed_id() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(INSTANCE_ID_FILE), "not-an-id").unwrap();

    assert_eq!(peek(dir.path()), None);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(INSTANCE_ID_FILE)).unwrap(),
        "not-an-id",
        "peeking leaves the file alone"
    );
}

#[test]
fn a_minted_id_is_hex_and_well_formed() {
    let id = mint(&Fixed(0xab));
    assert_eq!(id, "ab".repeat(INSTANCE_ID_BYTES));
    assert!(is_well_formed(&id));
}

#[test]
fn an_id_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let first = load_or_create(dir.path());
    let second = load_or_create(dir.path());
    assert_eq!(first, second, "the id must be stable across boots");
    assert!(is_well_formed(&first));
}

#[test]
fn two_homes_get_different_ids() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    assert_ne!(load_or_create(a.path()), load_or_create(b.path()));
}

#[test]
fn a_corrupt_file_is_replaced_rather_than_served() {
    let dir = tempfile::tempdir().unwrap();
    for junk in ["", "   ", "not-an-id", "zz", &"ab".repeat(64)] {
        std::fs::write(dir.path().join(INSTANCE_ID_FILE), junk).unwrap();
        let id = load_or_create(dir.path());
        assert!(is_well_formed(&id), "{junk:?} produced {id:?}");
        assert_ne!(id.trim(), junk.trim());
    }
}

#[test]
fn an_unwritable_home_still_yields_an_id() {
    // A read-only or missing home must not take the host down over a name.
    let id = load_or_create(Path::new("/nonexistent-oc-instance-home"));
    assert!(is_well_formed(&id));
}
