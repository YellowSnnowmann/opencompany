//! Shared fixtures for the `inference` test split: the bearer-resolution
//! helper, a bare `Inference` builder, and the in-memory secret store
//! (split out of `inference_tests.rs`).

use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

/// The resolved bearer for a decl, for the assertions below.
pub(super) async fn bearer(decl: &InferenceDecl) -> Option<String> {
    decl.bearer().await.expect("credential resolves")
}

pub(super) fn inference(provider: &str) -> Inference {
    Inference {
        provider: Some(provider.to_string()),
        base_url: None,
        api_key_secret: None,
        models: BTreeMap::new(),
    }
}

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
