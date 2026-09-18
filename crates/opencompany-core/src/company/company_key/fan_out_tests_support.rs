//! Shared fixtures for the `company_key::fan_out` test split: fake stores,
//! a fake prober, and the small helpers every matrix/isolation/concurrency
//! test builds on. Kept separate (issue tracked in `fan_out.rs`'s own module
//! doc) because every split test file needs it.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use super::*;
use crate::company::inference::store as inference_store;
use crate::error::OpenCompanyError;
use crate::ports::SecretStore;
use crate::ports::types::{CompanyId, SecretValue};

pub(super) const OLD: &str = "th-not-a-real-key";
pub(super) const NEW: &str = "th-not-a-real-key-2";
pub(super) const CUSTOM: &str = "th-not-a-real-key-custom";
pub(super) const MODEL: &str = "acme/test-model";

pub(super) const ACCOUNT_KEY_KEY: &str = crate::company::company_key::KEY_KEY;
pub(super) const COMPOSIO_KEY_KEY: &str = crate::company::composio::TINYHUMANS_KEY_KEY;
pub(super) const COMPOSIO_LEGACY_KEY: &str = crate::company::composio::LEGACY_TOKEN_KEY;
pub(super) const LEGACY_INFERENCE_KEY_KEY: &str = crate::company::inference::KEY_KEY;

pub(super) fn company(tag: &str) -> CompanyId {
    CompanyId::new(format!("fan-out-{tag}"))
}

pub(super) fn llm_key_key() -> String {
    inference_store::provider_key_key(inference::MANAGED_SLUG)
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

/// A store whose writes to one key always fail — for the rollback and
/// failure-isolation rules.
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

/// A store whose every write sleeps briefly — for
/// [`concurrent_saves_leave_every_copy_equal_to_the_account_key`], which needs
/// two `fan_out` calls to actually interleave rather than one finishing
/// before the other starts.
pub(super) struct SlowSecrets {
    pub(super) inner: MemSecrets,
}

#[async_trait]
impl SecretStore for SlowSecrets {
    async fn get(&self, c: &CompanyId, key: &str) -> Result<Option<SecretValue>> {
        self.inner.get(c, key).await
    }
    async fn set(&self, c: &CompanyId, key: &str, value: SecretValue) -> Result<()> {
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        self.inner.set(c, key, value).await
    }
}

/// A canned prober answer, with a call counter so a test can assert the
/// probe never ran at all (e.g. on a `CustomKey` skip).
pub(super) struct FakeProber {
    answer: std::result::Result<Vec<String>, probe::ProbeClass>,
    pub(super) calls: AtomicUsize,
    last_base_url: Mutex<Option<String>>,
}

impl FakeProber {
    pub(super) fn ok(ids: &[&str]) -> Self {
        Self {
            answer: Ok(ids.iter().map(|s| s.to_string()).collect()),
            calls: AtomicUsize::new(0),
            last_base_url: Mutex::new(None),
        }
    }

    pub(super) fn failing(class: probe::ProbeClass) -> Self {
        Self {
            answer: Err(class),
            calls: AtomicUsize::new(0),
            last_base_url: Mutex::new(None),
        }
    }

    /// The `base_url` the most recent [`InferenceProber::probe`] call
    /// received, so a test can assert *what* was probed, not merely that
    /// something was.
    pub(super) fn last_base_url(&self) -> Option<String> {
        self.last_base_url.lock().unwrap().clone()
    }
}

#[async_trait]
impl InferenceProber for FakeProber {
    async fn probe(
        &self,
        base_url: &str,
        _key: &str,
    ) -> std::result::Result<Vec<String>, probe::ProbeFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_base_url.lock().unwrap() = Some(base_url.to_string());
        match &self.answer {
            Ok(ids) => Ok(ids.clone()),
            Err(class) => Err(probe::ProbeFailure {
                class: *class,
                raw: "fake".to_string(),
                truncated: false,
            }),
        }
    }
}

pub(super) async fn raw_set(
    secrets: &dyn SecretStore,
    company: &CompanyId,
    key: &str,
    value: &str,
) {
    secrets
        .set(company, key, SecretValue(value.to_string()))
        .await
        .unwrap();
}

pub(super) async fn raw_get(secrets: &dyn SecretStore, company: &CompanyId, key: &str) -> String {
    secrets
        .get(company, key)
        .await
        .unwrap()
        .map(|SecretValue(v)| v)
        .unwrap_or_default()
}

pub(super) async fn seed_row(secrets: &dyn SecretStore, company: &CompanyId, model: &str) {
    inference_store::put_provider(
        company,
        secrets,
        inference_store::ProviderDraft {
            slug: inference::MANAGED_SLUG.to_string(),
            label: "TinyHumans".to_string(),
            kind: inference::MANAGED_SLUG.to_string(),
            base_url: catalogue::cloud_provider(inference::MANAGED_SLUG)
                .unwrap()
                .endpoint
                .to_string(),
            models: tier_overrides(model),
            enabled: true,
        },
    )
    .await
    .unwrap();
}

pub(super) fn outcome(report: &FanOutReport, slot: Slot) -> SlotOutcome {
    report
        .slots
        .iter()
        .find(|r| r.slot == slot)
        .unwrap_or_else(|| panic!("no {slot:?} slot in {report:?}"))
        .outcome
}

pub(super) fn full(provider: &str, model: &str) -> inference_store::DefaultChoice {
    inference_store::DefaultChoice::Full(inference_store::ModelChoice {
        provider: provider.to_string(),
        model: model.to_string(),
    })
}
