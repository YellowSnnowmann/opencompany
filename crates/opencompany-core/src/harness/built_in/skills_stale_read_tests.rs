use serde_json::json;

use super::*;

fn bundle(source_dir: &Path, slug: &str, resource: &str) {
    let dir = source_dir.join("skills").join(slug);
    std::fs::create_dir_all(dir.join("references")).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {slug}\ndescription: {slug} probe\n---\n# {slug}\n"),
    )
    .unwrap();
    std::fs::write(dir.join("references").join("spec.md"), resource).unwrap();
}

fn materialize(ws: &Path, src: &Path) -> EffectiveSkills {
    EffectiveSkills::materialize(ws.to_path_buf(), Some(src), &[], &[], "prober", None).unwrap()
}

async fn read(eff: &EffectiveSkills, slug: &str) -> String {
    let tools = eff.read_tools();
    let tool = tools
        .iter()
        .find(|tool| tool.name() == READ_SKILL_RESOURCE_TOOL)
        .expect("read tool");
    match tool
        .execute(json!({ "skill_id": slug, "relative_path": "references/spec.md" }))
        .await
    {
        Ok(result) => result.output_for_llm(false),
        Err(err) => format!("ERR {err}"),
    }
}

async fn describe(eff: &EffectiveSkills, slug: &str) -> String {
    let tools = eff.read_tools();
    let tool = tools
        .iter()
        .find(|tool| tool.name() == DESCRIBE_SKILL_TOOL)
        .expect("describe tool");
    match tool.execute(json!({ "skill_id": slug })).await {
        Ok(result) => result.output_for_llm(false),
        Err(err) => format!("ERR {err}"),
    }
}

async fn listed(eff: &EffectiveSkills) -> String {
    let tools = eff.read_tools();
    let tool = tools
        .iter()
        .find(|tool| tool.name() == LIST_SKILLS_TOOL)
        .expect("list tool");
    tool.execute(json!({})).await.unwrap().output_for_llm(false)
}

fn lists(listing: &str, slug: &str) -> bool {
    listing.contains(&format!("\"dir_name\":\"{slug}\""))
}

/// What the three read tools answer after the harness rebuilds an agent's
/// skill tree in place, as a roster rebuild does.
///
/// `list_skills` and `describe_skill` rescan the tree, and a resource file's
/// content is read off disk, so an edit to a skill already present is served
/// fresh. `read_skill_resource` resolves the skill through OpenHuman's
/// process-wide metadata cache, keyed by workspace and never invalidated for
/// this tree, so a skill added by the rebuild is listed and described but its
/// resources are "not found", and a removed one fails on the missing
/// directory rather than as unknown.
#[tokio::test]
async fn read_skill_resource_resolves_skills_from_the_first_scan_of_a_tree() {
    let ws = tempfile::tempdir().unwrap();
    let first_src = tempfile::tempdir().unwrap();
    bundle(first_src.path(), "probe", "VERSION-ONE");
    let first = materialize(ws.path(), first_src.path());
    assert!(read(&first, "probe").await.contains("VERSION-ONE"));

    let second_src = tempfile::tempdir().unwrap();
    bundle(second_src.path(), "probe", "VERSION-TWO");
    bundle(second_src.path(), "added", "ADDED-ONE");
    let second = materialize(ws.path(), second_src.path());
    assert!(
        read(&second, "probe").await.contains("VERSION-TWO"),
        "an edited resource of a skill already scanned is read fresh"
    );
    assert!(lists(&listed(&second).await, "added"));
    assert!(describe(&second, "added").await.contains("added probe"));
    let added = read(&second, "added").await;
    assert!(
        added.contains("skill 'added' not found"),
        "a skill added after the first scan is not resolvable by read_skill_resource: {added}"
    );

    let third_src = tempfile::tempdir().unwrap();
    bundle(third_src.path(), "added", "ADDED-ONE");
    let third = materialize(ws.path(), third_src.path());
    assert!(!lists(&listed(&third).await, "probe"));
    let removed = read(&third, "probe").await;
    assert!(
        removed.contains("failed to canonicalize skill root"),
        "a removed skill still resolves from the cache and fails on its directory: {removed}"
    );
}
