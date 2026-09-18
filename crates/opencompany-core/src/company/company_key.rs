//! The company's own TinyHumans credential — the one key its admin sets on the
//! tenant, and the identity every **brokered** surface presents (issue #586).
//!
//! ## What this is, and what it is not
//!
//! A company belongs to one owner. The admin sets one TinyHumans key here, and
//! everything the platform backend brokers on the company's behalf rides it:
//! Composio today, whatever else the backend fronts tomorrow. Membership in the
//! company is what grants access to it.
//!
//! It is deliberately **not** [`inference::KEY_KEY`](crate::company::inference)
//! . That slot holds whatever credential the company's *declared provider*
//! wants — an OpenRouter `sk-or-…`, a raw BYOK key, an `openai_compatible`
//! token. It is provider-scoped, not an identity. Handing it to the TinyHumans
//! backend would present one vendor's credential to another. The two coincide
//! only when the declared provider is `managed`, and even then they are the same
//! *value* for different reasons. One slot per meaning.
//!
//! ## Precedence
//!
//! [`resolve`] is the single seam. Most specific first:
//!
//! 1. **The company's own key**, stored here — set, rotated and cleared by an
//!    admin through the console, write-only, never read back.
//! 2. **This instance's platform identity** —
//!    [`TinyhumansTokenSource`](crate::company::credentials::TinyhumansTokenSource),
//!    the projected, audience-bound token a hosted pod carries. Unchanged
//!    behaviour for a tenant whose admin has set nothing.
//! 3. **Nothing** ⇒ [`Credential::None`], and the caller fails closed.
//!
//! Every brokered surface resolving through *this one function* is what makes
//! the rotation guarantee true by construction: there is no second resolution to
//! drift, so a key set in the console cannot reach one surface and miss another.
//! A surface may prepend its own escape-hatch tier ahead of this (Composio keeps
//! its BYO `composio/tinyhumans/key` — see
//! [`harness::composio`](crate::harness::composio)), but nothing may resolve a
//! *company* identity any other way.
//!
//! ## Write-only
//!
//! The value is never returned by any read route. [`key_configured`] answers the
//! only question the console needs, and [`Credential`]'s `Debug` redacts what it
//! holds.
//!
//! ## Freshness
//!
//! Read live from the [`SecretStore`] on every resolution, so a console set /
//! rotate / clear takes effect on the next cycle with no restart. The resolved
//! credential contributes its **value** to the roster fingerprint
//! ([`Credential::hash_identity`]), so a rotation rebuilds the tool roster
//! rather than leaving agents on the previous identity.

use crate::Result;
use crate::company::credentials::{Credential, TinyhumansTokenSource};
use crate::ports::SecretStore;
use crate::ports::types::{CompanyId, SecretValue};

mod fan_out;
mod types;

pub use fan_out::{
    CopyDecision, InferenceProber, LiveProber, SlotFacts, account_key_used_by,
    copy_account_key_to_composio, decide_copy, fan_out, fan_out_note, slot_facts, slot_guard,
};
pub use types::{
    FanOutKey, FanOutReport, FanOutRequest, SkipReason, Slot, SlotOutcome, SlotReport,
};

/// The canonical per-company TinyHumans credential key. Write-only via the
/// console; the value is the raw key string.
///
/// Namespaced `tinyhumans/` rather than under any one consumer, because it is
/// not any one consumer's key — see the module docs on why this is not
/// `inference/key`.
pub const KEY_KEY: &str = "tinyhumans/key";

/// Store (or rotate / clear) the company's TinyHumans credential. A non-empty
/// value rotates it; an empty string clears it. Write-only — never read back
/// over the API.
pub async fn store_key(company: &CompanyId, secrets: &dyn SecretStore, key: &str) -> Result<()> {
    secrets
        .set(company, KEY_KEY, SecretValue(key.trim().to_string()))
        .await
}

/// Whether a non-empty company key is stored — never the key itself. The only
/// thing the read planes ever surface about it.
pub async fn key_configured(company: &CompanyId, secrets: &dyn SecretStore) -> Result<bool> {
    Ok(matches!(
        load(company, secrets).await?,
        Credential::Company(_)
    ))
}

/// The company's own key as a [`Credential`], or [`Credential::None`] when unset
/// or blank.
///
/// A store read error **propagates**. It is tempting to map it to `None` —
/// "degrade rather than brick a roster build" — but `None` here does not mean
/// "no credential", it means *fall through to the instance identity*, and those
/// are different answers to different questions. See [`resolve`].
pub async fn load(company: &CompanyId, secrets: &dyn SecretStore) -> Result<Credential> {
    Ok(match secrets.get(company, KEY_KEY).await? {
        Some(SecretValue(key)) => Credential::from_company_key(key),
        None => Credential::None,
    })
}

/// The credential a brokered call presents for this company — **the** seam.
///
/// The company's own key wins; failing that this instance's platform identity;
/// failing that [`Credential::None`], which every caller treats as fail-closed.
/// See the module docs for why there is exactly one of these.
///
/// ## Why a store error is not a fallthrough
///
/// An unreadable store means *we do not know who this company is*. Treating that
/// as "no key set" would silently resolve a company that **has** a key to the
/// instance's identity — and because a provider connection lives on the backend
/// keyed by the account the bearer resolves to, a connection established in that
/// window would belong to the instance's account rather than the company's. When
/// the store recovered, resolution would return the company key, the bearer would
/// resolve to a different entity, and the connection made under the fallback
/// would no longer be the one the company sees. The same "connect Gmail" click
/// would produce a different owner depending on store health at that instant,
/// with no signal in either direction.
///
/// Availability-degrade and identity-degrade are different decisions, and only
/// the first is safe to make silently. So the error propagates, and each caller
/// answers it in the way its own surface can afford: the roster build withholds
/// the tools for that cycle (fail closed — an unknown credential must never mean
/// a borrowed identity, exactly as an absent one must not), while the console
/// planes surface the failure instead of reporting a confident "not configured".
pub async fn resolve(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    token_source: Option<std::sync::Arc<TinyhumansTokenSource>>,
) -> Result<Credential> {
    Ok(match (load(company, secrets).await?, token_source) {
        (company_key @ Credential::Company(_), _) => company_key,
        (_, Some(source)) => Credential::from_source(source),
        (_, None) => Credential::None,
    })
}

#[cfg(test)]
#[path = "company_key_tests.rs"]
mod tests;
