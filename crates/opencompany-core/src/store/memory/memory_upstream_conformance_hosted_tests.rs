use std::sync::Arc;

use tinymemory::registry::DriverClass;
use tinymemory_api::provider::MemoryProvider;

use super::driver::{MemoryDriverConfig, MemoryMode, RemoteDeployment, open_driver};
use super::tests_upstream_conformance::{assert_retains_then_conforms, facade_round_trip};

mod live_hosted {
    use super::*;

    /// Whether a missing endpoint must FAIL rather than skip.
    ///
    /// Same shape and same reasoning as the MongoDB suite's
    /// `OPENCOMPANY_TEST_MONGODB_REQUIRED` (issue #555): the skip is right for
    /// a laptop with no vendor keys and wrong for a lane whose entire purpose
    /// is running these. There, an unset pair is a misconfigured job, and the
    /// skip would report it as a pass — the whole suite silently absent behind
    /// a green tick.
    ///
    /// `0` and the empty string read as unset, so it can be threaded through a
    /// workflow matrix that always defines it.
    fn required() -> bool {
        matches!(
            std::env::var("OPENCOMPANY_TEST_HOSTED_REQUIRED").as_deref(),
            Ok(value) if !value.is_empty() && value != "0"
        )
    }

    /// The `(url, key)` pair for one engine, or `None` to skip.
    fn credentials(engine: &str) -> Option<(String, String)> {
        let upper = engine.to_uppercase();
        let url = std::env::var(format!("OPENCOMPANY_TEST_{upper}_URL")).ok();
        let key = std::env::var(format!("OPENCOMPANY_TEST_{upper}_KEY")).ok();
        match (url, key) {
            (Some(url), Some(key)) if !url.is_empty() && !key.is_empty() => Some((url, key)),
            _ => {
                assert!(
                    !required(),
                    "OPENCOMPANY_TEST_HOSTED_REQUIRED is set but \
                     OPENCOMPANY_TEST_{upper}_URL / _KEY are not. This lane exists to run \
                     the port suite against the real {engine} service, so a skip here is a \
                     misconfigured job rather than a pass."
                );
                eprintln!("skipping {engine}: OPENCOMPANY_TEST_{upper}_URL / _KEY are not set");
                None
            }
        }
    }

    /// Binds the live engine named by the environment, or `None` to skip.
    fn live_driver(engine: &str) -> Option<(Arc<dyn MemoryProvider>, DriverClass)> {
        let (url, key) = credentials(engine)?;
        let config = MemoryDriverConfig {
            mode: MemoryMode::Remote,
            driver_id: Some(engine.into()),
            url: Some(url),
            api_key: Some(key),
            data_dir: None,
            // Every engine reachable this way is the vendor's managed
            // platform; a self-hosted instance is not something this lane can
            // reach from CI.
            deployment: RemoteDeployment::Managed,
        };
        Some(
            open_driver(&config)
                .expect("the live driver binds")
                .expect("remote with a driver named yields a provider"),
        )
    }

    /// The **provider contract** against the live service: what the engine
    /// promises about raw content it is handed.
    ///
    /// Kept separate from the port suite below, because the two can disagree
    /// and the difference decides who has to act. A vendor that alters raw
    /// content fails here; whether that reaches this host depends on what this
    /// host actually sends, which is the next test's question.
    async fn live_provider_contract(engine: &str) {
        let Some((provider, _)) = live_driver(engine) else {
            return;
        };
        assert_retains_then_conforms(provider).await;
    }

    /// The **host's ports** against the live service: what OpenCompany itself
    /// depends on, through the facades it really uses.
    ///
    /// This is the claim that decides whether a tenant can run on this engine.
    /// It is deliberately not the same as the provider contract: the facades
    /// JSON-encode every record into `content`, so a byte that an engine
    /// mangles in raw text may never reach it as raw text at all.
    async fn live_host_ports(engine: &str) {
        let Some((provider, class)) = live_driver(engine) else {
            return;
        };
        facade_round_trip(provider, class).await;
    }

    /// CortexDB, which unlike the others is one *we* host — so this lane can
    /// reach a self-hosted instance, and `RemoteDeployment::Managed` above is
    /// still right because its `self_hosted` constructor is an alias for `api`.
    ///
    /// ```sh
    /// OPENCOMPANY_TEST_CORTEX_URL=http://127.0.0.1:3141 \
    /// OPENCOMPANY_TEST_CORTEX_KEY=... \
    ///   cargo test --lib live_hosted
    /// ```
    #[tokio::test]
    async fn the_live_cortex_service_upholds_the_provider_contract() {
        live_provider_contract("cortex").await;
    }

    #[tokio::test]
    async fn the_live_cortex_service_upholds_the_host_ports() {
        live_host_ports("cortex").await;
    }

    #[tokio::test]
    async fn the_live_supermemory_service_upholds_the_provider_contract() {
        live_provider_contract("supermemory").await;
    }

    #[tokio::test]
    async fn the_live_supermemory_service_upholds_the_host_ports() {
        live_host_ports("supermemory").await;
    }

    #[tokio::test]
    async fn the_live_mem0_service_upholds_the_provider_contract() {
        live_provider_contract("mem0").await;
    }

    #[tokio::test]
    async fn the_live_mem0_service_upholds_the_host_ports() {
        live_host_ports("mem0").await;
    }

    #[tokio::test]
    async fn the_live_cognee_service_upholds_the_provider_contract() {
        live_provider_contract("cognee").await;
    }

    #[tokio::test]
    async fn the_live_cognee_service_upholds_the_host_ports() {
        live_host_ports("cognee").await;
    }
}
