//! Hosted-memory driver selection.

use std::path::PathBuf;
use std::sync::Arc;

use tinymemory::registry::{
    COGNEE_DRIVER_ID, ConfigLabels, DriverClass, DriverEntry, DriverRegistry, MEM0_DRIVER_ID,
    NULL_DRIVER_ID, SUPERMEMORY_DRIVER_ID, TRUSTED,
};
use tinymemory_api::null::NullMemoryProvider;
use tinymemory_api::provider::MemoryProvider;

use super::cortexdb::{CORTEXDB_DRIVER_ID, CortexdbMemory};
use crate::Result;
use crate::error::OpenCompanyError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryMode {
    Remote,
    Null,
}

pub use crate::store::select::RemoteDeployment;

#[derive(Clone)]
pub struct MemoryDriverConfig {
    pub mode: MemoryMode,
    pub driver_id: Option<String>,
    pub url: Option<String>,
    pub api_key: Option<String>,
    /// Retained for the migration command's common configuration shape.
    pub data_dir: Option<PathBuf>,
    /// Which hosted deployment's protocol and authentication to use.
    pub deployment: RemoteDeployment,
}

impl std::fmt::Debug for MemoryDriverConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryDriverConfig")
            .field("mode", &self.mode)
            .field("driver_id", &self.driver_id)
            .field("url", &self.url.as_ref().map(|_| "<set>"))
            .field("api_key", &self.api_key.as_ref().map(|_| "<set>"))
            .field("deployment", &self.deployment)
            .finish()
    }
}

#[derive(Debug)]
pub struct MemoryDriverError(String);
impl std::fmt::Display for MemoryDriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl From<MemoryDriverError> for OpenCompanyError {
    fn from(error: MemoryDriverError) -> Self {
        Self::Config(error.0)
    }
}

fn labels() -> ConfigLabels<'static> {
    ConfigLabels {
        section: "OPENCOMPANY_MEMORY",
        drivers: "OPENCOMPANY_MEMORY_DRIVER",
        driver_entry: "OPENCOMPANY_MEMORY_DRIVER",
    }
}

/// Opens the configured hosted or null provider.
pub fn open_driver(
    config: &MemoryDriverConfig,
) -> Result<Option<(Arc<dyn MemoryProvider>, DriverClass)>> {
    let bound: (Arc<dyn MemoryProvider>, DriverClass) = match config.mode {
        MemoryMode::Null => (
            Arc::new(NullMemoryProvider::new()),
            admit(NULL_DRIVER_ID, DriverClass::Null)?,
        ),
        MemoryMode::Remote => {
            let driver_id = require(
                config.driver_id.as_deref(),
                "OPENCOMPANY_MEMORY=remote requires OPENCOMPANY_MEMORY_DRIVER naming a hosted engine",
            )?;
            let url = require(
                config.url.as_deref(),
                "OPENCOMPANY_MEMORY=remote requires OPENCOMPANY_MEMORY_URL naming the hosted engine's endpoint",
            )?;
            let key = require(
                config.api_key.as_deref(),
                "OPENCOMPANY_MEMORY=remote requires OPENCOMPANY_MEMORY_API_KEY",
            )?;
            (
                remote_provider(driver_id, url, key, config.deployment)?,
                admit(driver_id, DriverClass::External)?,
            )
        }
    };
    tinymemory_api::provider::audit_provider(bound.0.as_ref()).map_err(|audit| {
        OpenCompanyError::Config(format!(
            "memory driver `{}` failed its capability audit: {audit}",
            bound.0.driver_id()
        ))
    })?;
    Ok(Some(bound))
}

fn require<'a>(value: Option<&'a str>, refusal: &str) -> Result<&'a str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| MemoryDriverError(refusal.to_string()).into())
}
fn admit(driver_id: &str, expected: DriverClass) -> Result<DriverClass> {
    let entry = DriverEntry {
        class: Some(expected.as_str()),
        trust_state: TRUSTED,
    };
    let admission = DriverRegistry::builtin()
        .admit(driver_id, Some(entry), labels())
        .map_err(|reason| {
            MemoryDriverError(format!(
                "memory driver `{}` was refused: {}",
                reason.configured_driver, reason.reason
            ))
        })?;
    if admission.class != expected {
        return Err(MemoryDriverError(format!(
            "driver `{driver_id}` is class `{}`, but the selected mode requires `{}`",
            admission.class.as_str(),
            expected.as_str()
        ))
        .into());
    }
    Ok(admission.class)
}
fn remote_provider(
    driver_id: &str,
    url: &str,
    key: &str,
    deployment: RemoteDeployment,
) -> Result<Arc<dyn MemoryProvider>> {
    let managed = deployment == RemoteDeployment::Managed;
    let provider: Arc<dyn MemoryProvider> = match driver_id {
        SUPERMEMORY_DRIVER_ID => Arc::new(tinymemory_remote::supermemory_provider(
            tinymemory_remote::SupermemoryMemory::api(url, key).map_err(open_failed)?,
        )),
        MEM0_DRIVER_ID => Arc::new(tinymemory_remote::mem0_provider(
            if managed {
                tinymemory_remote::Mem0Memory::api(url, key)
            } else {
                tinymemory_remote::Mem0Memory::self_hosted(url, Some(key))
            }
            .map_err(open_failed)?,
        )),
        COGNEE_DRIVER_ID => Arc::new(tinymemory_remote::cognee_provider(
            if managed {
                tinymemory_remote::CogneeMemory::api(url, key)
            } else {
                tinymemory_remote::CogneeMemory::self_hosted(url, Some(key))
            }
            .map_err(open_failed)?,
        )),
        // Two adapters reach the same CortexDB service. `cortexdb` is this
        // repo's own (`store::memory::cortexdb`) and sends `X-Cortex-Actor`;
        // `cortex` is the dialect `tinymemory-remote` ships. Both stay
        // selectable — they differ in what they send, not in what they talk to.
        CORTEXDB_DRIVER_ID => Arc::new(tinymemory::mandatory::MemoryTraitProvider::new(
            Arc::new(CortexdbMemory::api(url, key, cortexdb_actor(key)).map_err(open_failed)?),
            CORTEXDB_DRIVER_ID,
        )),
        // CortexDB takes the same credential either way — `self_hosted` is an
        // alias for `api` upstream — so `managed` only picks the endpoint, and
        // `url` already carries it. One arm covers both deployments.
        tinymemory_remote::CORTEX_DRIVER_ID => Arc::new(tinymemory_remote::cortex_provider(
            tinymemory_remote::CortexMemory::api(url, key).map_err(open_failed)?,
        )),
        other => {
            return Err(MemoryDriverError(format!(
                "no HTTP adapter is compiled for memory driver `{other}`"
            ))
            .into());
        }
    };
    Ok(provider)
}
fn open_failed(error: anyhow::Error) -> OpenCompanyError {
    OpenCompanyError::Config(format!(
        "could not open the configured memory engine: {error}. Check OPENCOMPANY_MEMORY_URL."
    ))
}

/// Resolves the actor CortexDB's `X-Cortex-Actor` header carries.
///
/// `OPENCOMPANY_MEMORY_ACTOR` wins when set. Otherwise, since a CortexDB
/// bearer token is commonly a JWT whose `sub` claim names the actor it was
/// minted for, this decodes that claim (no signature verification — the token
/// is already trusted, it is this host's own configured credential) rather
/// than defaulting straight to a generic name that would mismatch the token
/// and turn every call into a 401.
///
/// Falls back to `"service:opencompany"` when neither source names an actor —
/// verified against a live CortexDB instance, not guessed: the header is
/// `type:id` (type one of `user`, `agent`, `service`, `system`), and a bare
/// name like `"opencompany"` is refused with `invalid X-Cortex-Actor: actor id
/// missing ':' delimiter`.
fn cortexdb_actor(key: &str) -> String {
    const ENV: &str = "OPENCOMPANY_MEMORY_ACTOR";
    if let Ok(actor) = std::env::var(ENV) {
        let actor = actor.trim();
        if !actor.is_empty() {
            return actor.to_string();
        }
    }
    jwt_subject(key).unwrap_or_else(|| "service:opencompany".to_string())
}

/// Reads the `sub` claim out of a JWT's payload segment, without verifying
/// its signature — this token is this host's own configured credential, so
/// there is nothing to verify against; the decode is purely to read a claim
/// already trusted the moment it was configured.
fn jwt_subject(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("sub")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// A minimal, dependency-free base64url (no padding) decoder — just enough to
/// read a JWT payload segment without adding a `base64` dependency to a
/// feature that does not otherwise need one.
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    for byte in input.bytes() {
        if byte == b'=' {
            continue;
        }
        let digit = value(byte)?;
        buffer = (buffer << 6) | u32::from(digit);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

pub const SUPPORTED_REMOTE_DRIVERS: [&str; 5] = [
    SUPERMEMORY_DRIVER_ID,
    MEM0_DRIVER_ID,
    COGNEE_DRIVER_ID,
    CORTEXDB_DRIVER_ID,
    // Neither `cortexdb` nor `cortex` is in `DriverRegistry::builtin()`'s
    // reserved table, which is fine: `admit` takes an unreserved id when the
    // host declares the class, and this host declares every remote driver
    // `External` with `TRUSTED`. The class stays host-decided rather than
    // self-reported, which is the property the reserved table exists to
    // protect.
    tinymemory_remote::CORTEX_DRIVER_ID,
];
