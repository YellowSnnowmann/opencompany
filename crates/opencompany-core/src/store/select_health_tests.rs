use super::*;

/// A stub whose every mandatory read fails, so the probe has something to
/// find. `NullMemoryProvider` answers everything, which is the right
/// subject for the empty-instance case and useless for the failure one.
#[cfg(feature = "tinymemory")]
#[derive(Debug)]
struct FailingProvider {
    fail_core: bool,
    fail_recall: bool,
}

#[cfg(feature = "tinymemory")]
#[async_trait]
impl tinymemory_api::traits::Memory for FailingProvider {
    fn name(&self) -> &str {
        "failing"
    }
    async fn store(
        &self,
        _namespace: &str,
        _key: &str,
        _content: &str,
        _category: tinymemory_api::types::MemoryCategory,
        _session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn get(
        &self,
        _namespace: &str,
        _key: &str,
    ) -> anyhow::Result<Option<tinymemory_api::types::MemoryEntry>> {
        if self.fail_core {
            anyhow::bail!("core is unreachable");
        }
        Ok(None)
    }
    async fn forget(&self, _namespace: &str, _key: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn list(
        &self,
        _namespace: Option<&str>,
        _category: Option<&tinymemory_api::types::MemoryCategory>,
        _session_id: Option<&str>,
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
        _query: &str,
        _limit: usize,
        _opts: tinymemory_api::types::RecallOpts<'_>,
    ) -> anyhow::Result<Vec<tinymemory_api::types::MemoryEntry>> {
        if self.fail_recall {
            anyhow::bail!("recall is unreachable");
        }
        Ok(Vec::new())
    }
}

#[cfg(feature = "tinymemory")]
fn failing(fail_core: bool, fail_recall: bool) -> tinymemory_api::mandatory::MemoryTraitProvider {
    tinymemory_api::mandatory::MemoryTraitProvider::new(
        Arc::new(FailingProvider {
            fail_core,
            fail_recall,
        }),
        "failing",
    )
}

/// A working engine that holds nothing must not be reported as broken.
///
/// On a freshly provisioned per-tenant instance every family is legitimately
/// empty, so a probe reading "returned no rows" as "not implemented" would
/// refuse every family on day one. `NullMemoryProvider` is exactly that
/// shape — every read succeeds and returns nothing — so it must probe clean.
#[cfg(feature = "tinymemory")]
#[tokio::test]
async fn an_empty_engine_probes_clean() {
    let provider = tinymemory_api::null::NullMemoryProvider::new();
    let outcome = probe_engine(&provider, std::time::Duration::from_secs(5)).await;
    assert!(
        outcome.unreachable.is_empty() && outcome.slow.is_empty(),
        "an engine that answers every read but holds nothing must not be reported \
             unreachable or slow; got {outcome:?}"
    );
}

/// The direction the clean-probe test cannot pin: an engine that fails must
/// actually be reported. Without this, replacing the probe body with
/// `Vec::new()` still passes the suite.
#[cfg(feature = "tinymemory")]
#[tokio::test]
async fn a_dead_engine_reports_every_family() {
    let provider = failing(true, true);
    let outcome = probe_engine(&provider, std::time::Duration::from_secs(5)).await;
    assert_eq!(
        outcome.unreachable,
        vec!["core".to_string(), "recall".to_string()]
    );
    assert!(outcome.slow.is_empty(), "an error is not a timeout");
}

/// Attribution: one broken family must not condemn the others, and — the
/// case that caught a real bug — a recall that fails must be *seen* to
/// fail. An empty probe query short-circuits inside `RemoteMemory::recall`
/// before the network, which made this leg unfalsifiable.
#[cfg(feature = "tinymemory")]
#[tokio::test]
async fn one_broken_family_is_named_alone() {
    let outcome = probe_engine(&failing(false, true), std::time::Duration::from_secs(5)).await;
    assert_eq!(outcome.unreachable, vec!["recall".to_string()]);

    let outcome = probe_engine(&failing(true, false), std::time::Duration::from_secs(5)).await;
    assert_eq!(outcome.unreachable, vec!["core".to_string()]);
}

/// A blackholed endpoint must surface as *slow*, not as clean.
///
/// This is the branch `refresh_health`'s own doc names — packets going
/// nowhere — and it is the one a weakened guard would silently pass.
/// Relaxing `Ok(Ok(_))` to `!matches!(.., Ok(Err(_)))` makes a timeout look
/// healthy; this fails if that happens.
#[cfg(feature = "tinymemory")]
#[tokio::test]
async fn a_blackholed_engine_reports_slow_not_clean() {
    #[derive(Debug, Default)]
    struct Sleeper;

    #[async_trait]
    impl tinymemory_api::traits::Memory for Sleeper {
        fn name(&self) -> &str {
            "sleeper"
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
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
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
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok(Vec::new())
        }
    }

    let provider =
        tinymemory_api::mandatory::MemoryTraitProvider::new(Arc::new(Sleeper), "sleeper");
    let outcome = probe_engine(&provider, std::time::Duration::from_millis(50)).await;
    assert_eq!(outcome.slow, vec!["core".to_string(), "recall".to_string()]);
    assert!(
        outcome.unreachable.is_empty(),
        "a timeout is not a refusal: the engine never said no, it just did not answer"
    );
}

/// The probe must send a **non-empty** recall query.
///
/// `RemoteMemory::recall` returns `Ok(vec![])` without reaching the network
/// when the query trims to empty, so an empty probe query makes the recall
/// leg unfalsifiable on every hosted engine: a revoked credential reports
/// healthy. A stub cannot reproduce that short-circuit — it lives in the
/// remote adapter, not the contract — so this asserts the precondition
/// directly instead.
#[cfg(feature = "tinymemory")]
#[tokio::test]
async fn the_recall_probe_query_is_never_empty() {
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct Recorder(Mutex<Option<String>>);

    #[async_trait]
    impl tinymemory_api::traits::Memory for Recorder {
        fn name(&self) -> &str {
            "recorder"
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
            query: &str,
            _limit: usize,
            _opts: tinymemory_api::types::RecallOpts<'_>,
        ) -> anyhow::Result<Vec<tinymemory_api::types::MemoryEntry>> {
            *self.0.lock().expect("probe query lock") = Some(query.to_string());
            Ok(Vec::new())
        }
    }

    let recorder = Arc::new(Recorder::default());
    let provider = tinymemory_api::mandatory::MemoryTraitProvider::new(
        Arc::clone(&recorder) as Arc<dyn tinymemory_api::traits::Memory>,
        "recorder",
    );
    let _ = probe_engine(&provider, std::time::Duration::from_secs(5)).await;

    let seen = recorder.0.lock().expect("probe query lock").clone();
    let seen = seen.expect("the probe never called recall at all");
    assert!(
        !seen.trim().is_empty(),
        "the recall probe sent `{seen}`, which RemoteMemory short-circuits before the \
             network — the leg would pass against a dead engine"
    );
}

/// `refresh_health` must record what it probed, not just log it — the
/// engine route reads the descriptor.
#[cfg(feature = "tinymemory")]
#[tokio::test]
async fn refresh_health_records_unreachable_families() {
    let bound = crate::store::memory::BoundMemory::bind(
        Arc::new(failing(true, true)),
        tinymemory::registry::DriverClass::External,
    )
    .expect("bind");
    let mut overlay = MemoryOverlay {
        memory: bound.memory(),
        context: bound.context(),
        facts: Some(bound.facts()),
        inbound_context: Some(bound.inbound_context()),
        scratch: Some(bound.scratch()),
        scopes: Some(Arc::new(bound.clone())),
        descriptor: MemoryDescriptor {
            backend: MemoryBackend::Remote,
            driver_id: "failing".into(),
            capabilities: Vec::new(),
            healthy: None,
            unreachable_families: None,
            slow_families: None,
        },
        probe: Some(Arc::new(failing(true, true))),
    };
    overlay
        .refresh_health(std::time::Duration::from_secs(5))
        .await;
    assert_eq!(
        overlay.descriptor.unreachable_families,
        Some(vec!["core".to_string(), "recall".to_string()])
    );
}
