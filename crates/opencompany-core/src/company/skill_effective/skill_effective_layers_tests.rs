//! The fold over injected layers: nothing it resolves comes from ambient state.

use super::*;

fn doc(slug: &str, name: &str) -> SkillDoc {
    SkillDoc {
        slug: slug.to_string(),
        name: name.to_string(),
        description: format!("{slug} does things"),
        category: None,
        version: None,
        body: format!("# {name}\n"),
        extra_frontmatter: Vec::new(),
    }
}

fn seed_bundle(root: &Path, slug: &str, name: &str) {
    let dir = root.join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {slug} does things\n---\n# {name}\n"),
    )
    .unwrap();
}

#[test]
fn the_baseline_is_whatever_the_caller_passes() {
    let baseline = [doc("injected", "Injected")];
    let layers = SkillLayers {
        baseline: &baseline,
        bundle_root: None,
        library: &[],
    };

    let set = resolve(&layers, &[]).unwrap();

    let slugs: Vec<&str> = set.iter().map(|skill| skill.slug.as_str()).collect();
    assert_eq!(slugs, ["injected"], "no global leaks into an injected fold");
    assert!(set[0].enabled);
    assert_eq!(set[0].source, SkillSource::Company);
    assert_eq!(
        set[0].content.as_ref().map(|content| &content.body),
        Some(&SkillBody::Inline(render_skill_md(&baseline[0])))
    );
}

#[test]
fn an_empty_baseline_resolves_nothing_without_deltas() {
    let layers = SkillLayers {
        baseline: &[],
        bundle_root: None,
        library: &[],
    };
    assert!(resolve(&layers, &[]).unwrap().is_empty());
}

#[test]
fn the_bundle_root_is_read_as_given_and_supersedes_the_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    seed_bundle(tmp.path(), "shared", "From Bundle");
    let baseline = [doc("shared", "From Baseline")];
    let layers = SkillLayers {
        baseline: &baseline,
        bundle_root: Some(tmp.path()),
        library: &[],
    };

    let set = resolve(&layers, &[]).unwrap();

    assert_eq!(set.len(), 1);
    assert_eq!(set[0].doc().map(|d| d.name.as_str()), Some("From Bundle"));
    assert_eq!(
        set[0].content.as_ref().map(|content| &content.body),
        Some(&SkillBody::Bundle(tmp.path().join("shared")))
    );
}

#[test]
fn a_disabling_delta_beats_an_injected_baseline() {
    let baseline = [doc("injected", "Injected")];
    let layers = SkillLayers {
        baseline: &baseline,
        bundle_root: None,
        library: &[],
    };

    let set = resolve(
        &layers,
        &globals_skill_disables(&["skill:injected".to_string()]),
    )
    .unwrap();

    assert_eq!(set.len(), 1);
    assert!(!set[0].enabled);
}

#[test]
fn the_library_heals_a_stub_from_the_injected_layer_only() {
    let live = doc("lib", "Live");
    let stub = "---\nname: Lib\ndescription: one line\n---\none line\n";
    let deltas = [SkillState {
        slug: "lib".to_string(),
        enabled: true,
        source: SkillSource::Registry,
        custom_doc: Some(stub.to_string()),
        install: None,
        updated_at_millis: None,
    }];

    let without = SkillLayers {
        baseline: &[],
        bundle_root: None,
        library: &[],
    };
    let unhealed = resolve(&without, &deltas).unwrap();
    assert_eq!(unhealed[0].doc().map(|d| d.name.as_str()), Some("Lib"));

    let library = [live];
    let with = SkillLayers {
        library: &library,
        ..without
    };
    let healed = resolve(&with, &deltas).unwrap();
    assert_eq!(healed[0].doc().map(|d| d.name.as_str()), Some("Live"));
}

#[test]
fn the_agent_slice_narrows_the_injected_fold() {
    let baseline = [doc("alpha", "Alpha"), doc("beta", "Beta")];
    let layers = SkillLayers {
        baseline: &baseline,
        bundle_root: None,
        library: &[],
    };
    let scope = vec!["beta".to_string()];

    let set = resolve_for_agent(&layers, &[], "writer", Some(&scope)).unwrap();

    let slugs: Vec<&str> = set.iter().map(|skill| skill.slug.as_str()).collect();
    assert_eq!(slugs, ["beta"]);
}
