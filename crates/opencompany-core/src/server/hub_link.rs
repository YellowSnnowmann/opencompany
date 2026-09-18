//! The console side of the TinyHumans key grant: the PKCE secret, and the
//! flows waiting on one.
//!
//! An admin clicks "Connect TinyHumans"; the browser goes to the hub, signs in,
//! approves; and the console comes back holding a one-time `code`. This module
//! owns the half of that exchange the browser never sees.
//!
//! ## Why the verifier lives here and not in the browser
//!
//! PKCE would work either way — the console could mint its own verifier, keep
//! it in `sessionStorage`, and send it up with the code. It does not, for one
//! reason: whoever holds the verifier can redeem the code, and whatever redeems
//! the code receives the **key**. Putting the verifier in the browser would mean
//! the browser redeems, which means a TinyHumans key with `connections` on it
//! passes through a tab, a `fetch` response, and whatever a devtools panel or an
//! extension makes of both — on its way to being sent straight back down to the
//! host that needed it.
//!
//! So the host mints the verifier, keeps it, and hands the browser only an
//! opaque `state`. The browser's whole job is to carry `state` out and `code`
//! back. The key is minted, stored, and forgotten without ever being
//! serialized towards a client.
//!
//! ## Why it is in memory
//!
//! A pending link is worth less than a minute of anyone's time: if the process
//! restarts mid-flow the button simply has to be clicked again. Writing the
//! verifier to the secret store would mean a durable record of a live secret
//! whose entire purpose is to stop existing, and a sweep to delete it. The
//! `oauth_pending` map for MCP (issue #90) parks its `code_verifier` the same
//! way and for the same reason.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::server::platform_auth::b64url_encode;
use crate::server::users::token::TokenSource;

/// How long a started link may sit unfinished.
///
/// Comfortably longer than the hub's own 10-minute grant code, so the failure a
/// slow user hits is the hub's — which says "that approval expired, start
/// again" — rather than this host's, which would only be able to say the state
/// is unknown and leave them guessing whether they did something wrong.
pub const PENDING_TTL: Duration = Duration::from_secs(15 * 60);

/// How many random bytes back a `state` and a verifier. 256 bits each.
const SECRET_BYTES: usize = 32;

/// One flow waiting on the browser to come back.
#[derive(Clone, Debug)]
pub struct PendingLink {
    /// The PKCE secret. Never leaves this process until redemption.
    pub verifier: String,
    /// Which company the resulting key belongs to. Checked on return, so a
    /// `state` minted for one company cannot be finished against another.
    pub company: String,
    /// When it was parked, for the sweep.
    parked: Instant,
}

/// The flows this host has started and not yet finished.
///
/// Keyed by the opaque `state` the browser carries. Swept on insert rather than
/// on a timer: the map only grows when somebody starts a link, so that is
/// exactly when it is worth looking for abandoned ones, and it costs no task.
#[derive(Debug, Default)]
pub struct HubLinks {
    pending: Mutex<HashMap<String, PendingLink>>,
}

/// A started link: what to send the browser to, and how to find it again.
pub struct StartedLink {
    /// The opaque handle the browser carries out and back.
    pub state: String,
    /// `base64url(sha256(verifier))` — the only part of the secret that leaves.
    pub challenge: String,
}

impl HubLinks {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mints a verifier and parks it, returning the `state` and challenge.
    pub fn start(&self, src: &dyn TokenSource, company: &str) -> StartedLink {
        let state = mint_secret(src);
        let verifier = mint_secret(src);
        let challenge = challenge_for(&verifier);

        let mut pending = self.pending.lock().expect("hub links poisoned");
        pending.retain(|_, link| link.parked.elapsed() < PENDING_TTL);
        pending.insert(
            state.clone(),
            PendingLink {
                verifier,
                company: company.to_string(),
                parked: Instant::now(),
            },
        );

        StartedLink { state, challenge }
    }

    /// Takes the pending link for `state`, if it is still live and belongs to
    /// `company`.
    ///
    /// Removed whether or not it is used: a `state` is single-use, so a second
    /// attempt with the same handle finds nothing. The company is checked here
    /// rather than by the caller so that no route can forget to.
    pub fn take(&self, state: &str, company: &str) -> Option<PendingLink> {
        let mut pending = self.pending.lock().expect("hub links poisoned");
        let link = pending.remove(state)?;
        if link.parked.elapsed() >= PENDING_TTL {
            return None;
        }
        if link.company != company {
            return None;
        }
        Some(link)
    }

    /// The verifier parked under `state`, without spending it.
    ///
    /// Tests only, and deliberately not part of the trait a route can reach: a
    /// route that could read a verifier without consuming the link would be one
    /// step from redeeming twice. Tests need it to play the part of the hub,
    /// which is the one participant that legitimately knows both halves.
    #[cfg(test)]
    pub fn peek_verifier(&self, state: &str) -> Option<String> {
        self.pending
            .lock()
            .expect("hub links poisoned")
            .get(state)
            .map(|link| link.verifier.clone())
    }
}

/// 256 bits of `src`, unpadded base64url — 43 characters.
///
/// Which is also inside RFC 7636's 43–128 range for a `code_verifier`, and uses
/// only its unreserved alphabet, so the same mint serves both the `state` and
/// the verifier.
fn mint_secret(src: &dyn TokenSource) -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    src.fill(&mut bytes);
    b64url_encode(&bytes)
}

/// The S256 transformation: `base64url(sha256(verifier))`.
pub fn challenge_for(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    b64url_encode(&digest)
}

#[cfg(test)]
#[path = "hub_link_tests.rs"]
mod tests;
