//! What [`EffectiveSkills::materialize`] writes when a bundle on disk is not a
//! plain tree of files.

use super::*;

fn seed(source_dir: &Path, dir_name: &str) -> PathBuf {
    let dir = source_dir.join("skills").join(dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {dir_name}\ndescription: does things\n---\n# Body\n"),
    )
    .unwrap();
    dir
}

fn materialize(ws: &Path, src: &Path) -> crate::Result<EffectiveSkills> {
    EffectiveSkills::materialize(ws.to_path_buf(), Some(src), &[], &[], "agent", None)
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_a_bundle_is_not_copied() {
    let src = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    let bundle = seed(src.path(), "linked");
    std::os::unix::fs::symlink(outside.path().join("secret.txt"), bundle.join("secret.txt"))
        .unwrap();

    materialize(ws.path(), src.path()).unwrap();

    let out = ws.path().join("skills").join("linked");
    assert!(out.join("SKILL.md").is_file());
    assert!(!out.join("secret.txt").exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_skills_root_is_replaced_and_its_target_left_alone() {
    let src = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::write(target.path().join("keep.txt"), "keep").unwrap();
    std::os::unix::fs::symlink(target.path(), ws.path().join("skills")).unwrap();
    seed(src.path(), "plain");

    materialize(ws.path(), src.path()).unwrap();

    let root = ws.path().join("skills");
    assert!(!root.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(root.join("plain").join("SKILL.md").is_file());
    assert_eq!(
        std::fs::read_to_string(target.path().join("keep.txt")).unwrap(),
        "keep"
    );
    assert!(!target.path().join("plain").exists());
}

#[test]
fn a_bundle_nested_past_the_depth_limit_fails_the_materialization() {
    let src = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let mut deep = seed(src.path(), "deep");
    for level in 0..=tinyskills::MAX_MATERIALIZE_DEPTH {
        deep = deep.join(format!("d{level}"));
    }
    std::fs::create_dir_all(&deep).unwrap();

    let Err(error) = materialize(ws.path(), src.path()) else {
        panic!("a bundle nested past the limit materialized");
    };
    let message = error.to_string();
    assert!(
        message.contains(&format!(
            "nests more than {} directories deep",
            tinyskills::MAX_MATERIALIZE_DEPTH
        )),
        "{message}"
    );
}

#[test]
fn a_bundle_at_the_depth_limit_is_copied() {
    let src = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let mut deep = seed(src.path(), "deep");
    let mut relative = PathBuf::new();
    for level in 0..tinyskills::MAX_MATERIALIZE_DEPTH {
        deep = deep.join(format!("d{level}"));
        relative = relative.join(format!("d{level}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("leaf.md"), "leaf").unwrap();

    materialize(ws.path(), src.path()).unwrap();

    let leaf = ws
        .path()
        .join("skills")
        .join("deep")
        .join(relative)
        .join("leaf.md");
    assert_eq!(std::fs::read_to_string(leaf).unwrap(), "leaf");
}

#[test]
fn a_bundle_whose_directory_is_not_a_safe_name_is_left_out_of_tree_and_catalogue() {
    let src = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    seed(src.path(), "My Skill");
    seed(src.path(), "plain");

    let eff = materialize(ws.path(), src.path()).unwrap();

    assert!(!ws.path().join("skills").join("My Skill").exists());
    assert!(ws.path().join("skills").join("plain").is_dir());
    assert!(eff.docs.iter().all(|doc| doc.slug != "My Skill"));
    assert!(!eff.catalogue().contains("My Skill"));
}
