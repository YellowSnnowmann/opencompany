//! Ed25519 company identity: the [`LocalSigner`].
//!
//! A company's tiny.place identity is a single Ed25519 keypair. The 32-byte
//! seed persists at `keys/agent.ed25519` in the company bundle, hex-encoded,
//! restricted to `0600` on unix, and excluded from bundle exports (see
//! [`crate::store::Bundle::EXPORT_EXCLUDES`]). The `agentId` surfaced to
//! tiny.place is the base58 (Solana-style) encoding of the 32-byte public key.
//!
//! Everything here is offline: keys are generated from `OsRng`, and signing and
//! verification never touch the network.

use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};

use crate::error::OpenCompanyError;
use crate::store::Bundle;
use crate::store::paths::restrict_file;
use crate::{Result, ports::types::CompanyId};

/// A company's local Ed25519 signer.
///
/// Wraps a [`SigningKey`]; the public key doubles as the tiny.place `agentId`
/// via its base58 encoding.
pub struct LocalSigner {
    keypair: SigningKey,
}

impl LocalSigner {
    /// Builds a signer from a raw 32-byte Ed25519 seed.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            keypair: SigningKey::from_bytes(seed),
        }
    }

    /// Generates a fresh signer from the operating system's CSPRNG.
    pub fn generate() -> Self {
        use rand_core::RngCore as _;

        let mut seed = [0u8; 32];
        rand_core::OsRng.fill_bytes(&mut seed);
        Self::from_seed(&seed)
    }

    /// The base58 (Solana-style) address of the 32-byte public key. This is the
    /// company's tiny.place `agentId`.
    pub fn agent_id(&self) -> String {
        bs58::encode(self.public_key_bytes()).into_string()
    }

    /// The raw 32-byte Ed25519 public key.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.keypair.verifying_key().to_bytes()
    }

    /// The raw 32-byte seed. Kept crate-private so it is never serialized into
    /// an exportable surface.
    pub(crate) fn seed_bytes(&self) -> [u8; 32] {
        self.keypair.to_bytes()
    }

    /// Signs `msg`, returning the 64-byte Ed25519 signature.
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.keypair.sign(msg).to_bytes()
    }

    /// Signs `msg` and returns the signature base58-encoded (the wire form used
    /// in SIWX and x402 authorizations).
    pub fn sign_b58(&self, msg: &[u8]) -> String {
        bs58::encode(self.sign(msg)).into_string()
    }
}

/// Verifies a base58-encoded Ed25519 signature over `msg` by a given base58
/// `agent_id` (the signer's public key).
///
/// Returns `Ok(())` on a valid signature, or [`OpenCompanyError::InvalidRequest`]
/// when the id or signature is malformed or the signature does not verify.
pub fn verify_b58(agent_id: &str, msg: &[u8], signature_b58: &str) -> Result<()> {
    let pubkey_bytes = decode_32(agent_id).ok_or_else(|| {
        OpenCompanyError::InvalidRequest(format!(
            "agentId `{agent_id}` is not a 32-byte base58 key"
        ))
    })?;
    let verifying = VerifyingKey::from_bytes(&pubkey_bytes).map_err(|_| {
        OpenCompanyError::InvalidRequest("agentId is not a valid Ed25519 key".into())
    })?;

    let sig_bytes = decode_64(signature_b58).ok_or_else(|| {
        OpenCompanyError::InvalidRequest("signature is not 64 base58 bytes".into())
    })?;
    let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);

    use ed25519_dalek::Verifier as _;
    verifying
        .verify(msg, &signature)
        .map_err(|_| OpenCompanyError::InvalidRequest("signature does not verify".into()))
}

/// Loads the company's signer from `keys/agent.ed25519`, generating and
/// persisting a fresh key (hex seed, `0600`) if the file is absent.
pub async fn load_or_create_signer(bundle: &Bundle) -> Result<LocalSigner> {
    bundle.ensure_dirs().await?;
    let path = bundle.agent_key();

    match tokio::fs::read_to_string(&path).await {
        Ok(text) => {
            let seed = decode_hex_seed(text.trim()).ok_or_else(|| {
                OpenCompanyError::Store(format!(
                    "identity key {} is corrupt (expected 64 hex chars)",
                    path.display()
                ))
            })?;
            Ok(LocalSigner::from_seed(&seed))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let signer = LocalSigner::generate();
            let hex = encode_hex(&signer.seed_bytes());
            tokio::fs::write(&path, hex.as_bytes())
                .await
                .map_err(|source| OpenCompanyError::StoreIo {
                    path: path.clone(),
                    source,
                })?;
            restrict_file(&path)?;
            Ok(signer)
        }
        Err(source) => Err(OpenCompanyError::StoreIo { path, source }),
    }
}

/// Resolves the signer for a [`CompanyId`] under an OpenCompany home root.
pub async fn signer_for(
    root: impl Into<std::path::PathBuf>,
    id: &CompanyId,
) -> Result<LocalSigner> {
    let bundle = Bundle::new(root, id);
    load_or_create_signer(&bundle).await
}

// ---------------------------------------------------------------------------
// Small dependency-free codecs (avoid pulling a `hex` crate).
// ---------------------------------------------------------------------------

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn decode_hex_seed(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    let bytes = text.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = hex_val(bytes[i * 2])?;
        let lo = hex_val(bytes[i * 2 + 1])?;
        *slot = (hi << 4) | lo;
    }
    Some(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn decode_32(b58: &str) -> Option<[u8; 32]> {
    let v = bs58::decode(b58).into_vec().ok()?;
    v.try_into().ok()
}

fn decode_64(b58: &str) -> Option<[u8; 64]> {
    let v = bs58::decode(b58).into_vec().ok()?;
    v.try_into().ok()
}

#[cfg(test)]
#[path = "signer_tests.rs"]
mod tests;
