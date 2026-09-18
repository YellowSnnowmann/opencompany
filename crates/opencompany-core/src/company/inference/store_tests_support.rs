//! Shared fixtures for the `inference::store` test split: the in-memory
//! and failing secret stores, and the company/draft/entry-zero helpers
//! (split out of `store_tests.rs`).

use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

#[derive(Default)]
pub(super) struct MemSecrets {
    pub(super) map: Mutex<HashMap<String, String>>,
}

#[async_trait]
impl SecretStore for MemSecrets {
    async fn get(&self, _c: &CompanyId, key: &str) -> Result<Option<SecretValue>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(key)
            .map(|v| SecretValue(v.clone())))
    }
    async fn set(&self, _c: &CompanyId, key: &str, value: SecretValue) -> Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value.0);
        Ok(())
    }
}

/// A store whose writes to one key always fail — for the rollback rules.
pub(super) struct FailsWriting {
    pub(super) inner: MemSecrets,
    pub(super) failing_key: String,
}

#[async_trait]
impl SecretStore for FailsWriting {
    async fn get(&self, c: &CompanyId, key: &str) -> Result<Option<SecretValue>> {
        self.inner.get(c, key).await
    }
    async fn set(&self, c: &CompanyId, key: &str, value: SecretValue) -> Result<()> {
        if key == self.failing_key {
            return Err(OpenCompanyError::Store("disk is on fire".into()));
        }
        self.inner.set(c, key, value).await
    }
}

pub(super) fn company() -> CompanyId {
    CompanyId::new("acme")
}

pub(super) fn draft(slug: &str) -> ProviderDraft {
    ProviderDraft {
        slug: slug.to_string(),
        label: slug.to_string(),
        kind: "openai_compatible".to_string(),
        base_url: format!("https://{slug}.example/v1"),
        models: BTreeMap::new(),
        enabled: true,
    }
}

pub(super) async fn write_entry_zero(secrets: &dyn SecretStore, provider: &str) {
    let config = RuntimeInference {
        provider: provider.to_string(),
        base_url: None,
        models: BTreeMap::new(),
    };
    super::super::save_runtime_config(&company(), secrets, &config)
        .await
        .unwrap();
}
