use super::*;

#[tokio::test]
async fn collects_create_and_write_and_dedupes_the_final_target() {
    let collector = TurnOutputCollector::default();
    let claim = collector.claim();
    claim
        .scoped(async {
            collector.workspace_node("n-1", "agents/writer/draft.md");
            collector.workspace_node("n-2", "agents/writer/other.md");
            collector.workspace_node("n-1", "agents/writer/final.md");
        })
        .await;

    let outputs = claim.drain();
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].target_id, "n-1");
    assert_eq!(outputs[0].title, "agents/writer/final.md");
    assert_eq!(outputs[1].target_id, "n-2");
}

#[tokio::test]
async fn two_turns_never_cross_attribute_outputs() {
    let collector = TurnOutputCollector::default();
    let first = collector.claim();
    let second = collector.claim();

    first
        .scoped(collector_write(&collector, "first", "first.md"))
        .await;
    second
        .scoped(collector_write(&collector, "second", "second.md"))
        .await;

    assert_eq!(ids(first.drain()), vec!["first"]);
    assert_eq!(ids(second.drain()), vec!["second"]);
}

#[tokio::test]
async fn collects_the_exact_published_artifact_revision() {
    let collector = TurnOutputCollector::default();
    let claim = collector.claim();
    claim
        .scoped(async {
            collector.artifact("a-1", "t-1", 3, "Launch brief");
        })
        .await;

    let outputs = claim.drain();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].kind, ChatOutputKind::Artifact);
    assert_eq!(outputs[0].target_id, "a-1");
    assert_eq!(outputs[0].task_id.as_deref(), Some("t-1"));
    assert_eq!(outputs[0].version, Some(3));
}

async fn collector_write(collector: &TurnOutputCollector, id: &str, title: &str) {
    collector.workspace_node(id, title);
}

fn ids(outputs: Vec<ChatOutput>) -> Vec<String> {
    outputs.into_iter().map(|output| output.target_id).collect()
}

#[tokio::test]
async fn reads_and_deletes_register_nothing_without_a_write_call() {
    let collector = TurnOutputCollector::default();
    let claim = collector.claim();
    claim.scoped(async {}).await;
    assert!(claim.drain().is_empty());
}

#[tokio::test]
async fn deleting_a_node_removes_its_live_turn_output() {
    let collector = TurnOutputCollector::default();
    let claim = collector.claim();
    claim
        .scoped(async {
            collector.workspace_node("deleted", "draft.md");
            collector.workspace_node("kept", "final.md");
            collector.remove_workspace_node("deleted");
        })
        .await;

    assert_eq!(ids(claim.drain()), vec!["kept"]);
}
