use super::host_toolpack_identity;
use openhuman_core::tools::toolpacks::strip_packed_from_visible;
use std::collections::HashSet;

#[test]
fn coordinator_preserves_real_workflow_and_integration_tools() {
    for composio in [false, true] {
        let mut tools: HashSet<String> =
            ["create_workflow", "run_workflow", "read_workflow", "memory"]
                .into_iter()
                .map(str::to_string)
                .collect();
        if composio {
            tools.insert("composio_execute".into());
        }
        let before = tools.clone();
        strip_packed_from_visible(&mut tools, host_toolpack_identity("cyrill", true, composio));
        assert_eq!(
            tools, before,
            "host-wired coordinator tools must remain callable"
        );
    }
}

#[test]
fn specialist_disclosure_does_not_gain_workflow_tools() {
    // The host's own posture (Composio off, everything else advertised). Set
    // explicitly: the process default is a first-call-wins `OnceLock`, and a
    // filtered run can reach this test before any agent build declares it,
    // leaving every pack withheld.
    crate::harness::built_in::tool_posture::declare();
    let mut tools: HashSet<String> = ["composio_execute", "memory"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let before = tools.clone();
    strip_packed_from_visible(&mut tools, host_toolpack_identity("dirk", false, true));
    assert_eq!(tools, before);
    assert!(!tools.contains("create_workflow"));
    assert_eq!(host_toolpack_identity("henk", false, false), "henk");
}
