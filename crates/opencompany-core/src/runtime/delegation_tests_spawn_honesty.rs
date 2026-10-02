use super::tests_core2::*;
use super::*;

struct RefusesTitle {
    inner: Arc<dyn TaskStore>,
    refused: &'static str,
}

#[async_trait]
impl TaskStore for RefusesTitle {
    async fn list(&self, company: &CompanyId) -> Result<Vec<TaskRecord>> {
        self.inner.list(company).await
    }
    async fn upsert(&self, company: &CompanyId, task: &TaskRecord) -> Result<()> {
        if task.title == self.refused {
            return Err(crate::OpenCompanyError::Harness("disk full".to_string()));
        }
        self.inner.upsert(company, task).await
    }
    async fn update_if_column(
        &self,
        company: &CompanyId,
        task: &TaskRecord,
        observed: &TaskRecord,
        expected_column: &str,
    ) -> Result<bool> {
        self.inner
            .update_if_column(company, task, observed, expected_column)
            .await
    }
    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool> {
        self.inner.delete(company, id).await
    }
}

fn spawn(title: &str) -> Delegation {
    Delegation::SpawnTask {
        title: title.to_string(),
        note: None,
        assignee: None,
    }
}

#[tokio::test]
async fn a_spawn_with_no_board_is_reported_rather_than_silently_dropped() {
    let record = record();
    let queue = DelegationQueue::default();
    let steer = InflightRegistry::default();
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(&fx, vec![]);
    let runner = DelegationRunner::new(
        &turns,
        &record,
        None,
        &steer,
        &record.id,
        &queue,
        orchestrator::MAX_DELEGATIONS_PER_TURN,
    );
    queue.push(spawn("Draft the plan"));
    queue.push(spawn("Book the venue"));
    let drained = runner
        .drain_and_execute(None, MessageContext::default(), HandOffs::Run)
        .await
        .expect("a missing board does not fail the drain");
    assert_eq!(drained.spawned_task, None);
    let refused: Vec<(&str, &str)> = drained
        .refused_cards
        .iter()
        .map(|refused| (refused.tool, refused.card.as_str()))
        .collect();
    assert_eq!(
        refused,
        vec![
            ("spawn_task", "Draft the plan"),
            ("spawn_task", "Book the venue")
        ]
    );
}

#[tokio::test]
async fn one_card_the_board_refuses_does_not_drop_the_rest_of_the_drain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backing: Arc<dyn TaskStore> = Arc::new(FsOps::new(dir.path()));
    let tasks: Arc<dyn TaskStore> = Arc::new(RefusesTitle {
        inner: backing.clone(),
        refused: "Broken card",
    });
    let record = record();
    let queue = DelegationQueue::default();
    let steer = InflightRegistry::default();
    let fx = Fixture::new();
    let turns = ScriptedTurns::new(&fx, vec![]);
    let runner = DelegationRunner::new(
        &turns,
        &record,
        Some(&tasks),
        &steer,
        &record.id,
        &queue,
        orchestrator::MAX_DELEGATIONS_PER_TURN,
    );
    queue.push(spawn("Broken card"));
    queue.push(spawn("Working card"));
    let drained = runner
        .drain_and_execute(None, MessageContext::default(), HandOffs::Run)
        .await
        .expect("a refused write does not fail the drain");
    assert_eq!(drained.refused_cards.len(), 1);
    assert_eq!(drained.refused_cards[0].card, "Broken card");
    let cards = backing.list(&record.id).await.unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].title, "Working card");
    assert_eq!(drained.spawned_task.as_deref(), Some(cards[0].id.as_str()));
}
