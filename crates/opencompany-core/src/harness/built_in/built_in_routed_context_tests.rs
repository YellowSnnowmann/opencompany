//! `built_in::routed_context` fingerprint/resolution tests, split out of
//! `built_in_test_fixtures` to keep that file under the 750-line limit and to
//! stop mislabeling tests as fixtures.

use super::built_in_test_fixtures::*;
use super::*;

/// Context routing: the resolution that feeds a persona, and the fingerprint
/// that decides whether an edit reaches the next turn.
mod routed_context {
    use super::*;
    use crate::ports::workspace::{NodeKind, WorkspaceNode, WorkspaceOrigin};

    fn docs(entries: &[(&str, &[(&str, &str)])]) -> HashMap<String, Vec<(String, String)>> {
        entries
            .iter()
            .map(|(agent, documents)| {
                (
                    (*agent).to_string(),
                    documents
                        .iter()
                        .map(|(p, b)| ((*p).to_string(), (*b).to_string()))
                        .collect(),
                )
            })
            .collect()
    }

    /// The property the whole axis exists for. The routing table is manifest
    /// data and does not move when an operator edits a note, so a
    /// name-only hash would leave the edit invisible until a restart.
    #[test]
    fn the_fingerprint_moves_when_a_documents_body_changes() {
        let before = routed_context_fingerprint(&docs(&[("ceo", &[("brief.md", "old")])]));
        let after = routed_context_fingerprint(&docs(&[("ceo", &[("brief.md", "new")])]));
        assert_ne!(
            before, after,
            "an edited routed note must rebuild the roster"
        );
    }

    /// A `HashMap` has no order, so an order-sensitive hash would drop every
    /// live agent session on a rebuild that changed nothing.
    #[test]
    fn the_fingerprint_is_stable_across_map_iteration_order() {
        let one = docs(&[
            ("ceo", &[("brief.md", "b")]),
            ("engineer", &[("claims.md", "c")]),
        ]);
        let two = docs(&[
            ("engineer", &[("claims.md", "c")]),
            ("ceo", &[("brief.md", "b")]),
        ]);
        assert_eq!(
            routed_context_fingerprint(&one),
            routed_context_fingerprint(&two)
        );
    }

    /// Renaming a document is a real change even when its text is identical:
    /// the persona quotes the path as the section heading.
    #[test]
    fn the_fingerprint_moves_when_a_document_is_renamed() {
        let before = routed_context_fingerprint(&docs(&[("ceo", &[("brief.md", "same")])]));
        let after = routed_context_fingerprint(&docs(&[("ceo", &[("GOAL.md", "same")])]));
        assert_ne!(before, after);
    }

    /// A company with no workspace store keeps a stable fingerprint and never
    /// rebuilds on this axis — the pre-routing behaviour exactly.
    #[tokio::test]
    async fn no_workspace_store_resolves_to_nothing() {
        let fx = fixture();
        assert!(fx.deps.workspace.is_none(), "fixture has no store wired");

        let pool = HarnessPool::new();
        let routed = pool.resolve_routed_context(&record(), &fx.deps, &[]).await;
        assert!(routed.is_empty(), "{routed:?}");
        assert_eq!(
            routed_context_fingerprint(&routed),
            routed_context_fingerprint(&HashMap::new()),
            "a company that routes nothing must not rebuild on this axis"
        );
    }

    /// The real path: a routed document that exists in the tree is read and
    /// keyed to the agent whose manifest asked for it.
    #[tokio::test]
    async fn a_routed_document_is_resolved_per_agent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws: Arc<dyn crate::ports::WorkspaceStore> =
            Arc::new(crate::store::FsOps::new(dir.path()));
        let company = CompanyId::new("acme");
        ws.create(
            &company,
            &WorkspaceNode {
                id: "n-brief".to_string(),
                name: "brief.md".to_string(),
                kind: NodeKind::File,
                parent_id: None,
                updated_at_millis: 1,
                created_by: WorkspaceOrigin::Operator,
                updated_by: WorkspaceOrigin::Operator,
                mime: None,
                size: None,
                sha256: None,
                adopted: false,
            },
            Some("What the company established."),
        )
        .await
        .expect("create");

        let mut fx = fixture();
        fx.deps.workspace = Some(ws);

        let pool = HarnessPool::new();
        let routed = pool.resolve_routed_context(&record(), &fx.deps, &[]).await;

        // Both fixture agents default to the `reasoning` row, which routes
        // BRIEF — so both resolve it, and neither invents the notes that do
        // not exist in the tree.
        for agent in ["ceo", "engineer"] {
            let documents = routed
                .get(agent)
                .unwrap_or_else(|| panic!("no routed documents for {agent}: {routed:?}"));
            assert_eq!(
                documents,
                &vec![(
                    "brief.md".to_string(),
                    "What the company established.".to_string()
                )],
                "{agent}"
            );
        }
    }
}
