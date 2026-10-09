use super::*;

fn repo_companies() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(SKILL_LIBRARY_RESOURCE)
}

async fn registry_slugs(host: &EmbeddedHost) -> Vec<String> {
    let listed: serde_json::Value = reqwest::get(format!(
        "{}/api/v1/company/skills/registry",
        host.base_url()
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    listed
        .as_array()
        .unwrap_or_else(|| panic!("the registry is a list: {listed}"))
        .iter()
        .map(|row| row["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

async fn install(host: &EmbeddedHost, slug: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/company/skills/{slug}/install",
            host.base_url()
        ))
        .json(&serde_json::json!({ "name": "Typed", "description": "from the client" }))
        .send()
        .await
        .unwrap()
}

#[test]
fn the_bundle_resources_land_where_the_host_looks_for_them() {
    let conf: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"))
            .unwrap(),
    )
    .unwrap();
    let resources: Vec<&str> = conf["bundle"]["resources"]
        .as_array()
        .expect("the bundle declares resources")
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert!(
        resources.contains(&format!("{SKILL_LIBRARY_RESOURCE}/*/skills/**/*").as_str()),
        "the bundle ships every company's skills: {resources:?}"
    );

    let resource_dir = std::path::Path::new("/resources");
    let shipped = format!("{SKILL_LIBRARY_RESOURCE}/_globals/skills/web-research/SKILL.md");
    assert_eq!(
        resource_dir.join(tauri::utils::resources::resource_relpath(
            std::path::Path::new(&shipped)
        )),
        packaged_skill_library(resource_dir).join("_globals/skills/web-research/SKILL.md"),
        "a bundled SKILL.md keeps its layout under the packaged library"
    );
}

#[tokio::test]
async fn a_packaged_library_is_served_and_installs_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let host = start_with_library(
        dir.path().to_path_buf(),
        FirstRun::SeedStarterCompany,
        AnalyticsSetup::disabled(),
        Some(repo_companies()),
    )
    .await
    .expect("host starts");

    let slugs = registry_slugs(&host).await;
    assert!(
        slugs.len() > 50,
        "the desktop serves the library: {slugs:?}"
    );
    assert!(slugs.iter().any(|slug| slug == "call-debrief"));

    let installed = install(&host, "call-debrief").await;
    assert_eq!(installed.status(), 200);
    let row: serde_json::Value = installed.json().await.unwrap();
    assert_eq!(row["source"], "registry", "{row}");
    assert_eq!(
        row["name"], "Call Debrief",
        "the library's document, not the client's: {row}"
    );

    let missing = install(&host, "no-such-skill").await;
    assert_eq!(
        missing.status(),
        404,
        "a slug the library does not serve is refused, not stubbed"
    );
}

#[tokio::test]
async fn without_a_packaged_copy_the_desktop_serves_no_library() {
    let dir = tempfile::tempdir().unwrap();
    let host = start_with_library(
        dir.path().to_path_buf(),
        FirstRun::SeedStarterCompany,
        AnalyticsSetup::disabled(),
        Some(dir.path().join("absent")),
    )
    .await
    .expect("host starts");

    assert!(registry_slugs(&host).await.is_empty());
    assert_eq!(install(&host, "anything").await.status(), 200);
}
