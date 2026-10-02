use super::*;

/// A driver that advertises **every** optional family, delegating each
/// accessor to the null driver — which implements them all and advertises
/// none, so it is exactly the body a wrapper needs and nothing more.
///
/// The subject for the question the per-family table exists to answer: given a
/// driver that claims everything, which families does this host actually read?
#[cfg(feature = "tinymemory")]
#[derive(Debug, Default)]
struct AdvertisesEverything(tinymemory_api::null::NullMemoryProvider);

#[cfg(feature = "tinymemory")]
#[async_trait]
impl tinymemory_api::provider::MemoryCore for AdvertisesEverything {
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
impl tinymemory_api::provider::MemoryRecall for AdvertisesEverything {
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
impl tinymemory_api::provider::MemoryPortability for AdvertisesEverything {
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

/// The null driver implements every optional family except `episodic`, so that
/// one is answered here. Every method succeeds and returns nothing: the point
/// of this double is what gets *asked*, not what comes back.
#[cfg(feature = "tinymemory")]
#[async_trait]
impl tinymemory_api::provider::MemoryEpisodic for AdvertisesEverything {
    async fn insert_turn(
        &self,
        _turn: &tinymemory_api::provider::EpisodicTurn,
    ) -> std::result::Result<i64, tinymemory_api::error::MemoryError> {
        Ok(0)
    }
    async fn session_turns(
        &self,
        _session_id: &str,
    ) -> std::result::Result<
        Vec<tinymemory_api::provider::EpisodicTurn>,
        tinymemory_api::error::MemoryError,
    > {
        Ok(Vec::new())
    }
    async fn open_segment(
        &self,
        _session_id: &str,
    ) -> std::result::Result<
        Option<tinymemory_api::provider::ConversationSegment>,
        tinymemory_api::error::MemoryError,
    > {
        Ok(None)
    }
    async fn create_segment(
        &self,
        _segment_id: &str,
        _session_id: &str,
        _namespace: &str,
        _start_episodic_id: i64,
        _start_seq: Option<u32>,
        _start_timestamp: f64,
        _now: f64,
    ) -> std::result::Result<(), tinymemory_api::error::MemoryError> {
        Ok(())
    }
    async fn append_turn(
        &self,
        _segment_id: &str,
        _episodic_id: i64,
        _seq: Option<u32>,
        _timestamp: f64,
        _now: f64,
    ) -> std::result::Result<(), tinymemory_api::error::MemoryError> {
        Ok(())
    }
    async fn close_segment(
        &self,
        _segment_id: &str,
        _now: f64,
    ) -> std::result::Result<(), tinymemory_api::error::MemoryError> {
        Ok(())
    }
    async fn set_segment_summary(
        &self,
        _segment_id: &str,
        _summary: &str,
        _now: f64,
    ) -> std::result::Result<(), tinymemory_api::error::MemoryError> {
        Ok(())
    }
    async fn insert_event(
        &self,
        _event: &tinymemory_api::provider::EpisodicEvent,
    ) -> std::result::Result<(), tinymemory_api::error::MemoryError> {
        Ok(())
    }
    async fn upsert_segment_embedding(
        &self,
        _segment_id: &str,
        _model_signature: &str,
        _embedding: &[f32],
        _created_at: f64,
    ) -> std::result::Result<(), tinymemory_api::error::MemoryError> {
        Ok(())
    }
}

#[cfg(feature = "tinymemory")]
#[async_trait]
impl tinymemory_api::provider::MemoryProvider for AdvertisesEverything {
    fn driver_id(&self) -> &str {
        "advertises-everything"
    }
    fn capabilities(&self) -> tinymemory_api::capabilities::Capabilities {
        tinymemory_api::capabilities::Capabilities::all()
    }
    async fn health(&self) -> tinymemory_api::health::MemoryHealth {
        tinymemory_api::health::MemoryHealth::Ready
    }
    fn as_ingest(&self) -> Option<&dyn tinymemory_api::provider::MemoryIngest> {
        Some(&self.0)
    }
    fn as_documents(&self) -> Option<&dyn tinymemory_api::provider::MemoryDocuments> {
        Some(&self.0)
    }
    fn as_tree(&self) -> Option<&dyn tinymemory_api::provider::MemoryTree> {
        Some(&self.0)
    }
    fn as_entities(&self) -> Option<&dyn tinymemory_api::provider::MemoryEntities> {
        Some(&self.0)
    }
    fn as_graph(&self) -> Option<&dyn tinymemory_api::provider::MemoryGraph> {
        Some(&self.0)
    }
    fn as_diff(&self) -> Option<&dyn tinymemory_api::provider::MemoryDiff> {
        Some(&self.0)
    }
    fn as_goals(&self) -> Option<&dyn tinymemory_api::provider::MemoryGoals> {
        Some(&self.0)
    }
    fn as_tool_memory(&self) -> Option<&dyn tinymemory_api::provider::MemoryToolMemory> {
        Some(&self.0)
    }
    fn as_sources(&self) -> Option<&dyn tinymemory_api::provider::MemorySourceSink> {
        Some(&self.0)
    }
    fn as_maintenance(&self) -> Option<&dyn tinymemory_api::provider::MemoryMaintenance> {
        Some(&self.0)
    }
    fn as_people(&self) -> Option<&dyn tinymemory_api::provider::MemoryPeople> {
        Some(&self.0)
    }
    fn as_chunks(&self) -> Option<&dyn tinymemory_api::provider::MemoryChunks> {
        Some(&self.0)
    }
    fn as_retrieval(&self) -> Option<&dyn tinymemory_api::provider::MemoryRetrieval> {
        Some(&self.0)
    }
    fn as_profile(&self) -> Option<&dyn tinymemory_api::provider::MemoryProfile> {
        Some(&self.0)
    }
    fn as_episodic(&self) -> Option<&dyn tinymemory_api::provider::MemoryEpisodic> {
        Some(self)
    }
    fn as_source_sync(&self) -> Option<&dyn tinymemory_api::provider::MemorySourceSync> {
        Some(&self.0)
    }
    fn as_coding_sessions(&self) -> Option<&dyn tinymemory_api::provider::MemoryCodingSessions> {
        Some(&self.0)
    }
    fn as_scoring(&self) -> Option<&dyn tinymemory_api::provider::MemoryScoring> {
        Some(&self.0)
    }
    fn as_document_ingest(&self) -> Option<&dyn tinymemory_api::provider::MemoryDocumentIngest> {
        Some(&self.0)
    }
    fn as_conversation_ingest(
        &self,
    ) -> Option<&dyn tinymemory_api::provider::MemoryConversationIngest> {
        Some(&self.0)
    }
    fn as_learning_ingest(&self) -> Option<&dyn tinymemory_api::provider::MemoryLearningIngest> {
        Some(&self.0)
    }
    fn as_event_ingest(&self) -> Option<&dyn tinymemory_api::provider::MemoryEventIngest> {
        Some(&self.0)
    }
    fn as_answer(&self) -> Option<&dyn tinymemory_api::provider::MemoryAnswer> {
        Some(&self.0)
    }
}

/// The per-family table, pinned.
///
/// `family_leg`'s exhaustive match makes a *new* family a compile error, but it
/// says nothing about an existing arm quietly becoming `None` — which would
/// narrow what this host checks while every other test still passed. This is
/// the list, written out, for a driver that advertises all twenty-six families.
///
/// Changing it means changing `docs/spec/runtime/memory-engine.md` too: the
/// nine absences are documented there with a reason each, and an absence with no
/// reason is the defect issue #1968 is about.
#[cfg(feature = "tinymemory")]
#[test]
fn the_probed_families_are_the_documented_table() {
    let probed = probed_families(&AdvertisesEverything::default());
    assert_eq!(
        probed,
        vec![
            "core",
            "recall",
            // No `ingest`: every required method writes.
            "documents",
            "tree",
            "entities",
            "graph",
            "diff",
            "goals",
            "tool_memory",
            // No `sources`: writes, and `forget_source` deletes.
            // No `maintenance`: whole-store jobs, and the cheap reads are defaulted.
            // No `portability`: `export_page` walks the whole corpus.
            "people",
            "chunks",
            "retrieval",
            "profile",
            "episodic",
            "source_sync",
            "coding_sessions",
            "scoring",
            // No `*_ingest`: single write methods.
            // No `answer`: a metered inference call.
        ],
        "the set of families this host reads at bind time changed; update \
         docs/spec/runtime/memory-engine.md with the reason, or put the arm back"
    );
}

/// A family the driver does not advertise is not read at all.
///
/// Absence is a legitimate answer — a minimal driver implements two families
/// and nothing else — so probing an unadvertised family would report every one
/// of them broken. `NullMemoryProvider` advertises only the mandatory three and
/// implements almost every optional family behind `None` accessors, which is
/// exactly the shape that would break if the probe consulted implementations
/// rather than the advertisement.
#[cfg(feature = "tinymemory")]
#[test]
fn an_unadvertised_family_is_never_probed() {
    let probed = probed_families(&tinymemory_api::null::NullMemoryProvider::new());
    assert_eq!(
        probed,
        vec!["core", "recall"],
        "portability is mandatory and deliberately unprobed; nothing optional is advertised"
    );
}

/// The issue's harm at full width: a driver advertising every family over a
/// body that serves none of them.
///
/// `AdvertisesEverything` is exactly that — the null driver refuses every
/// optional call precisely *because* it advertises none of them, and this
/// wrapper advertises all of them anyway. `audit_provider` passes it without a
/// word: the accessors return objects, so `provides()` is true for all
/// twenty-six.
///
/// Every probed family must therefore come back refused, and none of them may
/// come back `unreachable` — the mandatory legs answered, and an optional
/// refusal is not something the apply route turns an engine away for.
/// `episodic` is the exception and the control: this wrapper answers it
/// itself, so its absence here is the probe's "empty is success" rule holding
/// for an optional family.
#[cfg(feature = "tinymemory")]
#[tokio::test]
async fn an_engine_refusing_everything_it_advertises_reports_every_probed_family() {
    let outcome = probe_engine(
        &AdvertisesEverything::default(),
        std::time::Duration::from_secs(5),
    )
    .await;
    assert!(outcome.healthy, "the engine answers its health check");
    assert_eq!(
        outcome.degraded,
        vec![
            "documents",
            "tree",
            "entities",
            "graph",
            "diff",
            "goals",
            "tool_memory",
            "people",
            "chunks",
            "retrieval",
            "profile",
            "source_sync",
            "coding_sessions",
            "scoring",
        ],
        "every probed optional family must be read and reported; `episodic` answers Ok here \
         and must not appear"
    );
    assert!(
        outcome.unreachable.is_empty(),
        "core and recall answered; nothing optional may reach the list apply refuses on"
    );
    assert!(outcome.slow.is_empty(), "an error is not a timeout");
}
