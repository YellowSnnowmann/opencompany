use super::*;

/// A driver advertising `people` over an engine that refuses it.
#[cfg(feature = "tinymemory")]
#[derive(Debug, Default)]
struct RefusesPeople(tinymemory_api::null::NullMemoryProvider);

#[cfg(feature = "tinymemory")]
#[async_trait]
impl tinymemory_api::provider::MemoryPeople for RefusesPeople {
    async fn list_people(
        &self,
        _limit: Option<usize>,
    ) -> std::result::Result<
        Vec<tinymemory_api::provider::RankedPerson>,
        tinymemory_api::error::MemoryError,
    > {
        Err(tinymemory_api::error::MemoryError::Backend(
            "the people index is not enabled on this plan".into(),
        ))
    }
    async fn get_person(
        &self,
        _person_id: &str,
    ) -> std::result::Result<
        Option<tinymemory_api::provider::PersonRecord>,
        tinymemory_api::error::MemoryError,
    > {
        Ok(None)
    }
    async fn resolve_handle(
        &self,
        _handle: &tinymemory_api::provider::PersonHandle,
        _create_if_missing: bool,
    ) -> std::result::Result<
        Option<tinymemory_api::provider::ResolvedPerson>,
        tinymemory_api::error::MemoryError,
    > {
        Ok(None)
    }
    async fn add_handle_alias(
        &self,
        _person_id: &str,
        _handle: &tinymemory_api::provider::PersonHandle,
    ) -> std::result::Result<(), tinymemory_api::error::MemoryError> {
        Ok(())
    }
    async fn score_person(
        &self,
        _person_id: &str,
    ) -> std::result::Result<
        Option<tinymemory_api::provider::PersonScore>,
        tinymemory_api::error::MemoryError,
    > {
        Ok(None)
    }
    async fn record_interaction(
        &self,
        _interaction: &tinymemory_api::provider::PersonInteraction,
    ) -> std::result::Result<(), tinymemory_api::error::MemoryError> {
        Ok(())
    }
    async fn seed_from_address_book(
        &self,
    ) -> std::result::Result<
        tinymemory_api::provider::AddressBookSeedOutcome,
        tinymemory_api::error::MemoryError,
    > {
        Ok(tinymemory_api::provider::AddressBookSeedOutcome::default())
    }
}

/// Answers, and holds nothing. The control beside `list_people`: an empty
/// answer is success, so a driver advertising `goals` over an engine with no
/// goals yet must not be reported as refusing it.
#[cfg(feature = "tinymemory")]
#[async_trait]
impl tinymemory_api::provider::MemoryGoals for RefusesPeople {
    async fn goals(
        &self,
    ) -> std::result::Result<tinymemory_api::goals::GoalsDoc, tinymemory_api::error::MemoryError>
    {
        Ok(tinymemory_api::goals::GoalsDoc::default())
    }
    async fn set_goals(
        &self,
        _goals: tinymemory_api::goals::GoalsDoc,
    ) -> std::result::Result<(), tinymemory_api::error::MemoryError> {
        Ok(())
    }
}

#[cfg(feature = "tinymemory")]
#[async_trait]
impl tinymemory_api::provider::MemoryCore for RefusesPeople {
    async fn store(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: tinymemory_api::types::MemoryCategory,
        session_id: Option<&str>,
        taint: tinymemory_api::types::MemoryTaint,
    ) -> std::result::Result<(), tinymemory_api::error::MemoryError> {
        self.0
            .store(namespace, key, content, category, session_id, taint)
            .await
    }
    async fn get(
        &self,
        namespace: &str,
        key: &str,
    ) -> std::result::Result<
        Option<tinymemory_api::types::MemoryEntry>,
        tinymemory_api::error::MemoryError,
    > {
        self.0.get(namespace, key).await
    }
    async fn forget(
        &self,
        namespace: &str,
        key: &str,
    ) -> std::result::Result<bool, tinymemory_api::error::MemoryError> {
        self.0.forget(namespace, key).await
    }
    async fn list(
        &self,
        namespace: Option<&str>,
        category: Option<&tinymemory_api::types::MemoryCategory>,
        session_id: Option<&str>,
    ) -> std::result::Result<
        Vec<tinymemory_api::types::MemoryEntry>,
        tinymemory_api::error::MemoryError,
    > {
        self.0.list(namespace, category, session_id).await
    }
    async fn namespaces(
        &self,
    ) -> std::result::Result<
        Vec<tinymemory_api::types::NamespaceSummary>,
        tinymemory_api::error::MemoryError,
    > {
        self.0.namespaces().await
    }
}

#[cfg(feature = "tinymemory")]
#[async_trait]
impl tinymemory_api::provider::MemoryRecall for RefusesPeople {
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        opts: &tinymemory_api::types::OwnedRecallOpts,
        scope: Option<&tinymemory_api::provider::SourceScope>,
    ) -> std::result::Result<
        Vec<tinymemory_api::types::MemoryEntry>,
        tinymemory_api::error::MemoryError,
    > {
        self.0.recall(query, limit, opts, scope).await
    }
}

#[cfg(feature = "tinymemory")]
#[async_trait]
impl tinymemory_api::provider::MemoryPortability for RefusesPeople {
    async fn export_page(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> std::result::Result<tinymemory_api::provider::ExportPage, tinymemory_api::error::MemoryError>
    {
        self.0.export_page(cursor, limit).await
    }
    async fn import_records(
        &self,
        records: Vec<tinymemory_api::provider::ExportRecord>,
    ) -> std::result::Result<
        tinymemory_api::provider::ImportOutcome,
        tinymemory_api::error::MemoryError,
    > {
        self.0.import_records(records).await
    }
}

#[cfg(feature = "tinymemory")]
#[async_trait]
impl tinymemory_api::provider::MemoryProvider for RefusesPeople {
    fn driver_id(&self) -> &str {
        "refuses-people"
    }
    fn capabilities(&self) -> tinymemory_api::capabilities::Capabilities {
        tinymemory_api::capabilities::Capabilities::mandatory()
            .with(tinymemory_api::capabilities::Capability::People)
            .with(tinymemory_api::capabilities::Capability::Goals)
    }
    async fn health(&self) -> tinymemory_api::health::MemoryHealth {
        tinymemory_api::health::MemoryHealth::Ready
    }
    fn as_people(&self) -> Option<&dyn tinymemory_api::provider::MemoryPeople> {
        Some(self)
    }
    fn as_goals(&self) -> Option<&dyn tinymemory_api::provider::MemoryGoals> {
        Some(self)
    }
}

/// The case the whole issue is written about, in its optional form: the driver
/// advertises `people`, the audit passes because the accessor returns an
/// object, and the engine behind it refuses the read.
///
/// It must be *reported* — an agent will be handed that tool and it will fail —
/// and it must not be reported as `unreachable`, which is what the console apply
/// route refuses on. An engine serving every cycle and failing one tool is not
/// an engine to take away from an operator who has no other one.
///
/// The same driver advertises `goals` over a body that answers and holds
/// nothing, so this pins the other direction too: an empty answer from an
/// optional family is success, exactly as it is for the mandatory ones.
#[cfg(feature = "tinymemory")]
#[tokio::test]
async fn a_refused_optional_family_is_degraded_not_unreachable() {
    let outcome = probe_engine(&RefusesPeople::default(), std::time::Duration::from_secs(5)).await;
    assert!(outcome.healthy, "the engine answers its health check");
    assert_eq!(outcome.degraded, vec!["people".to_string()]);
    assert!(
        outcome.unreachable.is_empty(),
        "an optional family must never reach the list the apply route refuses on; got {outcome:?}"
    );
    assert!(outcome.slow.is_empty(), "an error is not a timeout");
}

/// `refresh_health` must record the optional verdict too, not only log it:
/// the engine route and both console panels read the descriptor.
#[cfg(feature = "tinymemory")]
#[tokio::test]
async fn refresh_health_records_degraded_families() {
    let mut overlay = overlay_over(Arc::new(RefusesPeople::default()));
    overlay
        .refresh_health(std::time::Duration::from_secs(5))
        .await;
    assert_eq!(
        overlay.descriptor.degraded_families,
        Some(vec!["people".to_string()])
    );
    assert_eq!(overlay.descriptor.unreachable_families, Some(Vec::new()));
}

/// Builds an overlay over `provider`, so a test can probe through
/// `refresh_health` rather than calling `probe_engine` directly.
#[cfg(feature = "tinymemory")]
fn overlay_over(provider: Arc<dyn tinymemory_api::provider::MemoryProvider>) -> MemoryOverlay {
    let bound = crate::store::memory::BoundMemory::bind(
        Arc::clone(&provider),
        tinymemory::registry::DriverClass::External,
    )
    .expect("bind");
    MemoryOverlay {
        memory: bound.memory(),
        context: bound.context(),
        facts: Some(bound.facts()),
        inbound_context: Some(bound.inbound_context()),
        scratch: Some(bound.scratch()),
        scopes: Some(Arc::new(bound.clone())),
        descriptor: MemoryDescriptor {
            backend: MemoryBackend::Remote,
            driver_id: provider.driver_id().to_string(),
            capabilities: Vec::new(),
            healthy: None,
            unreachable_families: None,
            degraded_families: None,
            slow_families: None,
        },
        probe: Some(provider),
        probe_cache: Arc::default(),
    }
}

/// The console read path must not re-probe on every request.
///
/// The probe is one read per advertised family plus health, so re-running it
/// per `GET` charged a page load — and every re-render and poll behind it — a
/// full round against an engine that may meter each call. The reuse has to
/// survive the clone `AppState::memory_overlay` hands out, which is why the
/// cache is an `Arc` on the overlay rather than a field on the descriptor.
#[cfg(feature = "tinymemory")]
#[tokio::test]
async fn a_fresh_probe_answer_is_reused_instead_of_asked_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Default)]
    struct Counting(AtomicUsize);

    #[async_trait]
    impl tinymemory_api::traits::Memory for Counting {
        fn name(&self) -> &str {
            "counting"
        }
        async fn store(
            &self,
            _n: &str,
            _k: &str,
            _c: &str,
            _cat: tinymemory_api::types::MemoryCategory,
            _s: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn get(
            &self,
            _n: &str,
            _k: &str,
        ) -> anyhow::Result<Option<tinymemory_api::types::MemoryEntry>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
        async fn forget(&self, _n: &str, _k: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
        async fn list(
            &self,
            _n: Option<&str>,
            _c: Option<&tinymemory_api::types::MemoryCategory>,
            _s: Option<&str>,
        ) -> anyhow::Result<Vec<tinymemory_api::types::MemoryEntry>> {
            Ok(Vec::new())
        }
        async fn namespace_summaries(
            &self,
        ) -> anyhow::Result<Vec<tinymemory_api::types::NamespaceSummary>> {
            Ok(Vec::new())
        }
        async fn count(&self) -> anyhow::Result<usize> {
            Ok(0)
        }
        async fn health_check(&self) -> bool {
            true
        }
        async fn recall(
            &self,
            _q: &str,
            _l: usize,
            _o: tinymemory_api::types::RecallOpts<'_>,
        ) -> anyhow::Result<Vec<tinymemory_api::types::MemoryEntry>> {
            Ok(Vec::new())
        }
    }

    let counter = Arc::new(Counting::default());
    let provider = Arc::new(tinymemory_api::mandatory::MemoryTraitProvider::new(
        Arc::clone(&counter) as Arc<dyn tinymemory_api::traits::Memory>,
        "counting",
    ));
    let mut overlay = overlay_over(provider);
    let timeout = std::time::Duration::from_secs(5);
    let max_age = std::time::Duration::from_secs(60);

    overlay.refresh_health_within(timeout, max_age).await;
    assert_eq!(counter.0.load(Ordering::SeqCst), 1, "the first read asks");

    // A clone, because that is what the route holds: `memory_overlay()` hands
    // out one, and a cache that did not survive it would buy nothing.
    let mut clone = overlay.clone();
    clone.descriptor.healthy = None;
    clone.descriptor.degraded_families = None;
    clone.refresh_health_within(timeout, max_age).await;
    assert_eq!(
        counter.0.load(Ordering::SeqCst),
        1,
        "a probe taken moments ago must be reused, not re-run"
    );
    assert_eq!(
        clone.descriptor.healthy,
        Some(true),
        "a reused answer must still be written onto the descriptor — the route reads it"
    );
    assert_eq!(clone.descriptor.degraded_families, Some(Vec::new()));

    // Boot and apply pass ZERO and always ask: an operator who has just fixed a
    // credential must not be shown the verdict from before the fix.
    overlay.refresh_health(timeout).await;
    assert_eq!(counter.0.load(Ordering::SeqCst), 2);
}
