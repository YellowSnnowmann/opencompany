use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(outside.join("secrets.txt"), "sshhh").unwrap();
    Fixture {
        _dir: dir,
        root: root.canonicalize().unwrap(),
        outside: outside.canonicalize().unwrap(),
    }
}

fn confine(f: &Fixture) -> Confinement {
    Confinement::new(&f.root).unwrap()
}

#[test]
fn a_file_inside_the_root_resolves() {
    let f = fixture();
    let c = confine(&f);
    assert_eq!(
        c.resolve_read(&f.root.join("src/main.rs")).unwrap(),
        f.root.join("src/main.rs")
    );
}

#[test]
fn a_traversal_out_of_the_root_is_refused() {
    // The reason `starts_with` on an unresolved path is the wrong check.
    let f = fixture();
    let c = confine(&f);
    let escape = f.root.join("../outside/secrets.txt");
    assert_eq!(c.resolve_read(&escape), Err(ConfineError::Escapes));
}

#[test]
fn a_symlink_leaving_the_root_is_refused() {
    // Not a hostile construction: working trees are full of links. A check
    // that compared before resolving would follow this one straight out.
    let f = fixture();
    let link = f.root.join("escape");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&f.outside, &link).unwrap();
    #[cfg(not(unix))]
    return;

    let c = confine(&f);
    assert_eq!(
        c.resolve_read(&link.join("secrets.txt")),
        Err(ConfineError::Escapes)
    );
}

#[test]
fn a_write_through_a_symlinked_parent_is_refused() {
    // The write path resolves the PARENT, so this is the case that would
    // slip through if it resolved nothing.
    let f = fixture();
    let link = f.root.join("out");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&f.outside, &link).unwrap();
    #[cfg(not(unix))]
    return;

    let c = confine(&f);
    assert_eq!(
        c.resolve_write(&link.join("planted.txt")),
        Err(ConfineError::Escapes)
    );
}

#[test]
fn a_write_through_a_dangling_symlink_is_refused() {
    // The case the parent-resolution branch does NOT catch on its own. A
    // link whose target does not exist yet fails `canonicalize`, so the
    // path looks exactly like an ordinary new file — and its parent really
    // is the root. Without the `symlink_metadata` check this returns
    // `Ok(<root>/planted.txt)` and the caller's `fs::write` follows the
    // link out of the tree and creates the file there.
    let f = fixture();
    let link = f.root.join("planted.txt");
    let escapee = f.outside.join("planted.txt");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&escapee, &link).unwrap();
    #[cfg(not(unix))]
    return;

    assert!(
        !escapee.exists(),
        "the link must dangle for this to be the case"
    );
    let c = confine(&f);
    assert_eq!(c.resolve_write(&link), Err(ConfineError::Escapes));

    // And the same link pointing back INSIDE the root is still refused
    // rather than silently rewritten to its target: this layer resolves
    // paths, and quietly retargeting a write is a decision, not a
    // resolution.
    let inward = f.root.join("inward.txt");
    #[cfg(unix)]
    std::os::unix::fs::symlink(f.root.join("src/fresh.rs"), &inward).unwrap();
    assert_eq!(c.resolve_write(&inward), Err(ConfineError::Escapes));
}

#[test]
fn a_new_file_inside_the_root_is_allowed_to_be_written() {
    // The common case: the file does not exist yet, so there is nothing to
    // canonicalise and the parent decides.
    let f = fixture();
    let c = confine(&f);
    let fresh = f.root.join("src/new.rs");
    assert_eq!(c.resolve_write(&fresh).unwrap(), fresh);
}

#[test]
fn a_write_into_a_directory_that_does_not_exist_is_refused() {
    // Refused rather than created: creating intermediate directories on a
    // model's say-so is a decision, and it is not this layer's to make.
    let f = fixture();
    let c = confine(&f);
    assert_eq!(
        c.resolve_write(&f.root.join("nope/deeper/file.txt")),
        Err(ConfineError::NoParent)
    );
}

#[test]
fn a_sibling_whose_name_merely_starts_with_the_root_is_refused() {
    // `/tmp/x/repo-secrets` has `/tmp/x/repo` as a *string* prefix. Path
    // comparison is component-wise, and this asserts it stays that way.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    let sibling = dir.path().join("repo-secrets");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("k.txt"), "x").unwrap();

    let c = Confinement::new(&root).unwrap();
    assert_eq!(
        c.resolve_read(&sibling.join("k.txt")),
        Err(ConfineError::Escapes)
    );
}

#[test]
fn a_relative_path_is_refused_rather_than_resolved_against_the_process() {
    // ACP mandates absolute paths. Resolving a relative one here would use
    // *this process's* working directory, which has nothing to do with the
    // session — and would differ depending on how the app was launched.
    let f = fixture();
    let c = confine(&f);
    assert_eq!(
        c.resolve_read(Path::new("src/main.rs")),
        Err(ConfineError::NotAbsolute)
    );
    assert_eq!(
        c.resolve_write(Path::new("src/new.rs")),
        Err(ConfineError::NotAbsolute)
    );
}

#[test]
fn the_root_itself_resolves_even_when_reached_through_a_link() {
    // A root behind a symlink (macOS `/tmp` is one) must not refuse
    // everything under it — a boundary that rejects all is easy to mistake
    // for a boundary that works.
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    std::fs::create_dir_all(real.join("sub")).unwrap();
    std::fs::write(real.join("sub/f.txt"), "x").unwrap();
    let linked = dir.path().join("linked");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    #[cfg(not(unix))]
    return;

    let c = Confinement::new(&linked).unwrap();
    assert!(c.resolve_read(&linked.join("sub/f.txt")).is_ok());
}

#[test]
fn a_trailing_parent_component_resolves_to_a_directory_and_is_refused() {
    // `<root>/src/..` is the root — genuinely *inside* the confinement, so
    // this is not an escape. It is a directory, and a write to a directory
    // is never what an agent meant.
    let f = fixture();
    let c = confine(&f);
    assert_eq!(
        c.resolve_write(&f.root.join("src/..")),
        Err(ConfineError::IsDirectory)
    );
    // The escape it is easy to confuse this with: one level higher does
    // leave the root, and that is refused as an escape.
    assert_eq!(
        c.resolve_write(&f.root.join("..")),
        Err(ConfineError::Escapes)
    );
}

#[test]
fn an_existing_directory_is_never_a_write_target() {
    let f = fixture();
    let c = confine(&f);
    assert_eq!(
        c.resolve_write(&f.root.join("src")),
        Err(ConfineError::IsDirectory)
    );
}
