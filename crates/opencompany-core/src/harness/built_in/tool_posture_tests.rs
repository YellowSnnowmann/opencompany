use super::*;

#[test]
fn host_advertises_every_pack_except_upstream_composio() {
    let groups = host_tool_groups();
    assert_eq!(
        groups.mode("composio"),
        oh::tools::toolpacks::GroupMode::Off
    );
    for group in oh::tools::toolpacks::ToolGroups::ids().filter(|id| *id != "composio") {
        assert_eq!(
            groups.mode(group),
            oh::tools::toolpacks::GroupMode::Advertised,
            "{group}"
        );
    }
}

#[tokio::test]
async fn legacy_composio_compatibility_tools_are_inert() {
    let tools = retired_composio_tools();
    let names = tools.iter().map(|tool| tool.name()).collect::<Vec<_>>();
    assert_eq!(names, RETIRED_COMPOSIO_TOOL_NAMES);

    for tool in tools {
        let result = tool.execute(serde_json::json!({})).await.unwrap();
        assert!(result.is_error);
    }
}
