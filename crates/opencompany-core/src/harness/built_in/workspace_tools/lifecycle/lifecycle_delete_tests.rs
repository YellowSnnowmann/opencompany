use std::sync::Arc;

use super::lifecycle_fixtures_tests::*;
use super::*;
use crate::harness::workspace_tools::tests::{TEST_AGENT, file, text, ws};
use crate::ports::artifacts::ArtifactStore;
use crate::ports::types::CompanyId;

// ---------------------------------------------------------------------------
// workspace_delete — the happy paths
// ---------------------------------------------------------------------------

/// The whole point of the issue: an agent can clear a superseded draft out of
/// its own folder, and is told plainly that nothing will bring it back.
#[tokio::test]
async fn deleting_your_own_note_removes_it_and_names_the_loss_as_permanent() {
    let home = own_home("acme").await;

    let out = home
        .deleter()
        .execute(json!({ "path": "agents/ceo/Draft.md", "expected_updated_at": NOTE_REV }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    let message = text(&out);
    assert!(message.contains("agents/ceo/Draft.md"), "{message}");
    assert!(
        message.contains("permanent"),
        "a directly-created note's deletion is final and must say so: {message}"
    );
    assert!(!home.has("n-draft").await, "the note survived the delete");
}

/// An empty folder of your own is deletable — which is what makes the
/// non-empty refusal below an instruction rather than a dead end.
#[tokio::test]
async fn an_empty_folder_of_your_own_can_be_deleted() {
    let home = own_home("acme").await;

    let out = home
        .deleter()
        .execute(json!({ "path": "agents/ceo/archive", "expected_updated_at": FOLDER_REV }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    assert!(text(&out).contains("folder"), "{}", text(&out));
    assert!(!home.has("f-archive").await);
}

/// A published note's history lives on its artifact chain, so its deletion is
/// the **recoverable** case — and the message has to say the opposite of what
/// it says for an ordinary note, or the agent will treat the two identically.
#[tokio::test]
async fn deleting_a_published_note_says_its_history_survives_in_artifacts() {
    let home = own_home("acme").await;
    home.publish("art-1", "n-draft", "# Draft").await;

    let out = home
        .deleter()
        .execute(json!({ "id": "n-draft", "expected_updated_at": NOTE_REV }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    let message = text(&out);
    assert!(message.contains("Artifacts"), "{message}");
    assert!(
        !message.contains("permanent"),
        "a published note is the recoverable case: {message}"
    );
    assert!(!home.has("n-draft").await);

    // The chain is untouched, dangling node id and all — the same state the
    // operator's own DELETE route leaves behind today, and deliberately not
    // "repaired" here.
    let stored = home
        .artifacts
        .get(&home.company, "art-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.workspace_node_id(), Some("n-draft"));
    assert_eq!(stored.versions.len(), 1);
}

/// When the artifact store cannot answer whether a node was published, the
/// delete still proceeds (documented at the `history_of` call site: "the
/// delete proceeds and the agent is told nothing either way") — but it must
/// fall back to the honest [`History::Unknown`] note, never fabricate
/// "permanent" or "survives in Artifacts" from a call that never completed.
#[tokio::test]
async fn a_publish_lookup_failure_still_deletes_and_claims_no_history_either_way() {
    let home = own_home("acme").await;
    let deleter = WorkspaceDeleteTool::new(
        ws(home.store.clone(), home.company.clone())
            .with_artifacts(Some(Arc::new(FailingArtifacts) as Arc<dyn ArtifactStore>)),
    );

    let out = deleter
        .execute(json!({ "path": "agents/ceo/Draft.md", "expected_updated_at": NOTE_REV }))
        .await
        .unwrap();
    assert!(
        !out.is_error,
        "an unreadable publish history must not block tidying your own folder: {}",
        text(&out)
    );
    let message = text(&out);
    assert!(!home.has("n-draft").await, "the note survived the delete");
    assert!(
        !message.contains("permanent") && !message.contains("Artifacts"),
        "an unresolved lookup must not fabricate either history claim: {message}"
    );
}

/// The port promises a binary node's payload goes with it (issue #553). This
/// pins that the agent-facing path reaches that promise rather than refusing
/// bytes the way `workspace_write` does.
#[tokio::test]
async fn a_binary_node_in_your_own_folder_deletes_with_its_payload() {
    let home = own_home("acme").await;
    home.add_own_binary("n-own-img", "chart.png", &[0x89, b'P', b'N', b'G'])
        .await;

    let out = home
        .deleter()
        .execute(json!({ "path": "agents/ceo/chart.png", "expected_updated_at": NOTE_REV }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    assert!(
        home.store
            .read_bytes(&home.company, "n-own-img")
            .await
            .unwrap()
            .is_none(),
        "the payload outlived its node"
    );
}

// ---------------------------------------------------------------------------
// workspace_delete — the scope gate
// ---------------------------------------------------------------------------

/// Shared guidance is not the agent's to remove, and a refusal must leave it
/// byte-identical rather than merely un-deleted.
#[tokio::test]
async fn a_note_outside_your_own_folder_is_refused_and_left_alone() {
    let home = own_home("acme").await;

    let out = home
        .deleter()
        .execute(json!({
            "path": "standards/engineering-standards.md",
            "expected_updated_at": NOTE_REV,
        }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(
        text(&out).contains("outside your own folder"),
        "{}",
        text(&out)
    );
    assert_eq!(
        home.read("n-eng").await.1,
        "# Engineering\nReview every PR."
    );
}

/// The gate is checked on the **resolved** node, not on the argument — so the
/// `id` form cannot walk around a `path` form's refusal. This is the bypass the
/// whole scope argument rests on.
#[tokio::test]
async fn an_id_that_names_a_note_outside_your_folder_refuses_like_its_path_would() {
    let home = own_home("acme").await;
    let tool = home.deleter();

    for node in ["n-eng", "n-readme", "n-mate"] {
        let out = tool
            .execute(json!({ "id": node, "expected_updated_at": NOTE_REV }))
            .await
            .unwrap();
        assert!(out.is_error, "`{node}` was allowed: {}", text(&out));
        assert!(
            text(&out).contains("outside your own folder"),
            "`{node}`: {}",
            text(&out)
        );
        assert!(home.has(node).await, "`{node}` was deleted");
    }
}

/// A teammate's folder is a teammate's, whichever end of it is named — and the
/// `Agents` root itself belongs to nobody.
#[tokio::test]
async fn a_teammates_home_and_its_contents_are_refused() {
    let home = own_home("acme").await;
    let tool = home.deleter();

    for path in ["agents/cmo", "agents/cmo/Plan.md", "Agents"] {
        let out = tool
            .execute(json!({ "path": path, "expected_updated_at": NOTE_REV }))
            .await
            .unwrap();
        assert!(out.is_error, "`{path}` was allowed: {}", text(&out));
        assert!(
            text(&out).contains("outside your own folder"),
            "`{path}`: {}",
            text(&out)
        );
    }
    assert!(home.has("n-mate").await);
}

/// The home folder gets a refusal of its own, and it has to explain the actual
/// consequence: `ensure_agent_folder` resolves the home **by name**, so
/// deleting it does not merely remove a folder — it makes the next publish mint
/// a second, empty one and forks the company's view of where this agent's work
/// lives.
#[tokio::test]
async fn your_own_home_folder_gets_its_own_refusal() {
    let home = own_home("acme").await;

    let out = home
        .deleter()
        .execute(json!({ "path": "agents/ceo", "expected_updated_at": FOLDER_REV }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    let message = text(&out);
    assert!(message.contains("your own folder itself"), "{message}");
    assert!(
        !message.contains("outside your own folder"),
        "the home must not get the generic outside-scope message: {message}"
    );
    assert!(
        message.contains("Act on what is inside it"),
        "the refusal must name the next useful action: {message}"
    );
    assert!(home.tree().await.iter().any(|n| n.name == TEST_AGENT));
}

/// One node per call, in the destructive direction. A recursive delete would be
/// one approval card naming one path that takes an unbounded amount of work
/// with it — so a non-empty folder is refused, having changed nothing at all.
#[tokio::test]
async fn a_folder_that_still_holds_anything_is_refused_and_deletes_nothing() {
    let home = own_home("acme").await;
    home.store
        .create(
            &home.company,
            &file("n-in-archive", "old.md", Some("f-archive")),
            Some("old"),
        )
        .await
        .unwrap();
    let before = home.tree().await.len();

    let out = home
        .deleter()
        .execute(json!({ "path": "agents/ceo/archive", "expected_updated_at": FOLDER_REV }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    let message = text(&out);
    assert!(message.contains("still holds 1 node(s)"), "{message}");
    assert!(
        message.contains("one call each"),
        "the refusal must say how to proceed: {message}"
    );
    assert_eq!(
        home.tree().await.len(),
        before,
        "a refused folder delete removed something"
    );
}

// ---------------------------------------------------------------------------
// workspace_delete — the revision guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_without_a_revision_is_refused() {
    let home = own_home("acme").await;

    let out = home
        .deleter()
        .execute(json!({ "path": "agents/ceo/Draft.md" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(text(&out).contains("expected_updated_at"), "{}", text(&out));
    assert!(home.has("n-draft").await);
}

/// A note edited in the console since the agent last read it is not deleted on
/// the strength of a view that predates the change.
#[tokio::test]
async fn a_stale_revision_is_refused_and_names_the_current_one() {
    let home = own_home("acme").await;

    let out = home
        .deleter()
        .execute(json!({ "path": "agents/ceo/Draft.md", "expected_updated_at": 1 }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    let message = text(&out);
    assert!(message.contains(&NOTE_REV.to_string()), "{message}");
    assert!(
        message.contains("do NOT retry with the same expected_updated_at"),
        "{message}"
    );
    assert!(home.has("n-draft").await);
}

/// Models stringify numbers constantly. `workspace_write` learned this the
/// expensive way; the same leniency is inherited rather than re-litigated.
#[tokio::test]
async fn a_revision_is_accepted_as_a_number_or_a_string() {
    for revision in [json!(NOTE_REV), json!(NOTE_REV.to_string())] {
        let home = own_home("acme").await;
        let out = home
            .deleter()
            .execute(json!({
                "path": "agents/ceo/Draft.md",
                "expected_updated_at": revision,
            }))
            .await
            .unwrap();
        assert!(!out.is_error, "{revision}: {}", text(&out));
        assert!(!home.has("n-draft").await, "{revision}");
    }

    // A string that is not a number is still not a revision.
    let home = own_home("acme").await;
    let out = home
        .deleter()
        .execute(json!({ "path": "agents/ceo/Draft.md", "expected_updated_at": "latest" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(home.has("n-draft").await);
}

/// The tenancy argument the module rests on, applied to the destructive tool: a
/// node id borrowed from another company is simply absent from this company's
/// index, so the store is never asked about it.
#[tokio::test]
async fn tenancy_a_borrowed_node_id_cannot_be_deleted_by_another_company() {
    let home = own_home("acme").await;
    let intruder = WorkspaceDeleteTool::new(
        ws(home.store.clone(), CompanyId::new("other"))
            .with_artifacts(Some(home.artifacts.clone())),
    );

    let out = intruder
        .execute(json!({ "id": "n-draft", "expected_updated_at": NOTE_REV }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(
        text(&out).contains("No workspace note matches"),
        "{}",
        text(&out)
    );
    assert!(home.has("n-draft").await);
}
