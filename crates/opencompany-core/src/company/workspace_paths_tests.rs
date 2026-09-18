use super::*;
use crate::ports::workspace::{NodeKind, WorkspaceOrigin};

fn node(id: &str, name: &str, parent: Option<&str>) -> WorkspaceNode {
    WorkspaceNode {
        id: id.to_string(),
        name: name.to_string(),
        kind: NodeKind::File,
        parent_id: parent.map(str::to_string),
        updated_at_millis: 1,
        created_by: WorkspaceOrigin::Operator,
        updated_by: WorkspaceOrigin::Operator,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    }
}

fn index(nodes: &[WorkspaceNode]) -> HashMap<&str, &WorkspaceNode> {
    nodes.iter().map(|n| (n.id.as_str(), n)).collect()
}

#[test]
fn a_path_is_the_ancestor_chain_joined() {
    let nodes = vec![
        node("a", "standards", None),
        node("b", "engineering-standards.md", Some("a")),
    ];
    let by_id = index(&nodes);
    assert_eq!(
        render_path(&nodes[1], &by_id).as_deref(),
        Some("standards/engineering-standards.md")
    );
    assert_eq!(render_path(&nodes[0], &by_id).as_deref(), Some("standards"));
}

#[test]
fn a_dangling_or_cyclic_chain_has_no_path() {
    let orphan = vec![node("b", "note.md", Some("missing"))];
    assert_eq!(render_path(&orphan[0], &index(&orphan)), None);

    let cycle = vec![node("a", "A", Some("b")), node("b", "B", Some("a"))];
    let by_id = index(&cycle);
    assert_eq!(render_path(&cycle[0], &by_id), None);
    assert_eq!(render_path(&cycle[1], &by_id), None);
}

#[test]
fn an_illegal_segment_anywhere_on_the_chain_has_no_path() {
    for name in ["..", ".", "a/b", "a\\b", "", "nul\0"] {
        let nodes = vec![node("x", name, None)];
        assert_eq!(
            render_path(&nodes[0], &index(&nodes)),
            None,
            "name {name:?} must not render a path"
        );
    }
    // …including when the illegal name is an *ancestor*, not the node.
    let nodes = vec![node("a", "..", None), node("b", "note.md", Some("a"))];
    assert_eq!(render_path(&nodes[1], &index(&nodes)), None);
}

#[test]
fn traversal_shaped_paths_are_rejected() {
    for path in [
        "../secrets.md",
        "standards/../../etc/passwd",
        "./Standards",
        "..",
        "standards/..",
        "C:\\Windows",
        "   ",
    ] {
        assert!(
            split_logical_path(path).is_err(),
            "path {path:?} must be rejected"
        );
    }
}

#[test]
fn redundant_separators_are_tolerated_but_segments_are_not_invented() {
    assert_eq!(
        split_logical_path("/standards/").unwrap(),
        vec!["standards"]
    );
    assert_eq!(
        split_logical_path("standards//eng.md").unwrap(),
        vec!["standards", "eng.md"]
    );
    assert!(split_logical_path("/").unwrap_err().contains("segments"));
}
