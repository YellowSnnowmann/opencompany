//! Who is here, and who is typing.
//!
//! Two ephemeral facts about the humans in a company, kept in memory and
//! published on the live event bus. Neither is journaled and neither has a
//! store: presence is a *lease* rather than a record, and typing is not even
//! that.
//!
//! # Presence is a TTL, not a connection table
//!
//! A console heartbeats every [`PRESENCE_HEARTBEAT_MILLIS`]; an entry is live
//! for [`PRESENCE_TTL_MILLIS`], which is **three times** the beat. The multiple
//! is the point: one dropped request must not flap somebody offline and back,
//! and a browser that crashes needs no cleanup at all — its lease simply
//! expires. Tracking sockets instead would mean every disconnect path (tab
//! close, sleep, network drop, pod restart) had to be handled correctly, and
//! the failure mode of missing one is a person who looks online forever.
//!
//! This is the same shape [`RunnerRegistry`](crate::runner::registry) already
//! uses, including its rule that a heartbeat is only meaningful for a caller
//! the host has actually authenticated.
//!
//! # The subject is always the caller
//!
//! A presence write names no user; the subject is taken from the session. A
//! body that could name somebody else would let any member mark any colleague
//! online or offline, and nothing downstream could tell the difference.
//!
//! # Replica-local, deliberately
//!
//! Two hosted replicas share a tenant database but not this map, so each knows
//! only about the consoles connected to it. That is exactly as partitioned as
//! the live turn timeline ([`crate::turn_stream`]) already is, and for the same
//! reason: both ride a process-local broadcast bus. A viewer sees everyone on
//! their own replica live, and everyone else through the durable
//! `lastSeenAtMillis` floor. Making presence cross-replica means a shared bus,
//! which is a much larger change than the feature warrants.
//!
//! # This is not read receipts
//!
//! [`read_state`](crate::server::ops::read_state) stays what it is: a private
//! floor for computing *your own* unread badge. Nothing here exposes what
//! another person has read. "Who is here" and "who has read this" look adjacent
//! and are not.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::ports::types::CompanyId;

/// How often a live console announces itself.
pub const PRESENCE_HEARTBEAT_MILLIS: u64 = 60_000;

/// How long an announcement stays good for.
///
/// Three times [`PRESENCE_HEARTBEAT_MILLIS`], so a single missed beat — a
/// slow request, a moment of packet loss, a tab that was throttled while
/// backgrounded — does not flap somebody offline and back.
pub const PRESENCE_TTL_MILLIS: u64 = 3 * PRESENCE_HEARTBEAT_MILLIS;

/// The most consoles one person may hold a lease on at once, per company.
///
/// Bounds a real memory-growth vector: a `consoleId` is client-supplied and
/// otherwise unbounded, so a buggy console minting a fresh one on every
/// reconnect — or a member deliberately hammering the route — would otherwise
/// grow this map forever, since an expired lease is only ever *hidden* from
/// reads (`list`, `aggregate`) rather than removed until [`PresenceRegistry::sweep`]
/// next runs. `beat` enforces this cap directly rather than relying on the
/// sweep's cadence, which bounds the *worst case* between sweeps rather than
/// the sweep interval itself. Comfortably above any real browser's tab count
/// (issue: "Bound client-supplied console leases").
const MAX_CONSOLES_PER_PERSON: usize = 16;

/// Exactly three states.
///
/// Anything a browser cannot honestly distinguish is deliberately not a state.
/// In particular there is no "busy" and no "invisible": the first is a guess,
/// and the second is a promise this design cannot keep, because a peer that
/// stops beating is indistinguishable from one that closed its laptop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresenceStatus {
    /// At the machine.
    Online,
    /// Signed in, but idle — **not** "the window is unfocused". See the console
    /// half: an unfocused window is not an absent human.
    Away,
    /// Explicitly appearing offline, or gone.
    Offline,
}

impl PresenceStatus {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Away => "away",
            Self::Offline => "offline",
        }
    }
}

/// One person's live lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Peer {
    status: PresenceStatus,
    last_beat_millis: u64,
}

/// The most present of two statuses — the aggregation rule for a person with
/// more than one open console.
///
/// `Online` beats `Away` beats nothing at all, so a person reading in one tab
/// while idle in another still shows as present: the honest answer to "is this
/// person here" is "yes, in at least one of their consoles," never the
/// gloomiest tab's guess.
fn most_present(a: PresenceStatus, b: PresenceStatus) -> PresenceStatus {
    use PresenceStatus::*;
    match (a, b) {
        (Online, _) | (_, Online) => Online,
        (Away, _) | (_, Away) => Away,
        _ => Offline,
    }
}

/// Who is currently present, per company.
///
/// Peer state is deliberately thin — a status and a timestamp, nothing else.
/// No cursor, no current room, no last-read: which channel somebody is looking
/// at is not a fact their colleagues need, and carrying it would turn a
/// presence dot into activity tracking.
///
/// # A lease is per console, not per person
///
/// The same signed-in human commonly has more than one tab open. Keying solely
/// on `(company, user)` made closing *any one* of them delete the lease every
/// other tab was still renewing, so the person flapped offline until their next
/// heartbeat healed it (up to a full [`PRESENCE_HEARTBEAT_MILLIS`]). The inner
/// map keys on `(company, user)` as before; the value is now every console that
/// user currently has open, so a departure only ever removes the console that
/// actually left.
#[derive(Debug, Default)]
pub struct PresenceRegistry {
    people: Mutex<HashMap<(CompanyId, String), HashMap<String, Peer>>>,
}

/// One person's presence, as a reader sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceView {
    /// The user id. Already known to every signed-in member through
    /// `GET {scope}/chat/mentionables`, which is why this carries no label:
    /// the console already holds the directory that names them.
    pub user_id: String,
    pub status: PresenceStatus,
    /// When this lease was last renewed, epoch millis.
    pub at_millis: u64,
}

impl PresenceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one console's heartbeat, and says whether it changed anything a
    /// watcher would care about.
    ///
    /// Returning "did this change" is what keeps the bus quiet: a console beats
    /// every minute whether or not anything moved, and republishing an
    /// unchanged `online` to every other console once a minute per person is
    /// pure noise. A frame goes out when the person's *aggregate* status
    /// — [`most_present`] across every console they have open — arrives or
    /// changes, which is exactly when a dot would move.
    ///
    /// An expired lease counts as an arrival, so somebody who was away long
    /// enough to lapse is re-announced rather than silently reappearing on the
    /// next reader's poll.
    pub fn beat(
        &self,
        company: &CompanyId,
        user: &str,
        console: &str,
        status: PresenceStatus,
        now_millis: u64,
    ) -> bool {
        let mut people = self.people.lock().expect("presence registry poisoned");
        let key = (company.clone(), user.to_string());
        let consoles = people.entry(key).or_default();
        let before = aggregate(consoles, now_millis);
        // A genuinely new console (not a renewal of one already tracked) past
        // the cap evicts the stalest entry first — expired ones before live
        // ones, then oldest `last_beat_millis` — so an unbounded stream of
        // fresh `consoleId`s cannot grow this map without limit. See
        // `MAX_CONSOLES_PER_PERSON`.
        if !consoles.contains_key(console)
            && consoles.len() >= MAX_CONSOLES_PER_PERSON
            && let Some(stalest) = consoles
                .iter()
                .min_by_key(|(_, peer)| peer.last_beat_millis)
                .map(|(id, _)| id.clone())
        {
            consoles.remove(&stalest);
        }
        consoles.insert(
            console.to_string(),
            Peer {
                status,
                last_beat_millis: now_millis,
            },
        );
        let after = aggregate(consoles, now_millis);
        before != after
    }

    /// Drops one console's lease immediately — a clean disconnect.
    ///
    /// Worth having even though the TTL would get there eventually: a person
    /// who closes a tab should not linger as online for three minutes, and the
    /// browser can say so on the way out. Only removes the departing console —
    /// a colleague's other open tabs keep their own leases, so closing one does
    /// not drop the others.
    ///
    /// Returns the person's **new aggregate status**, and only when it
    /// actually changed: `None` for a duplicate teardown, or for a departure
    /// that leaves another console whose status already matched the
    /// aggregate (an away tab closing while an online one is still live
    /// changes nothing an observer could see). When the departing console was
    /// carrying the aggregate up — an online tab closing while only an away
    /// one remains — this reports the *downgraded* status (`Away`), not
    /// `Offline`, so the caller publishes what the person now looks like
    /// rather than a false "gone" a moment before their away tab's next
    /// heartbeat corrects it. `Offline` is reported only when nobody is left.
    pub fn detach(
        &self,
        company: &CompanyId,
        user: &str,
        console: &str,
        now_millis: u64,
    ) -> Option<PresenceStatus> {
        let mut people = self.people.lock().expect("presence registry poisoned");
        let key = (company.clone(), user.to_string());
        let consoles = people.get_mut(&key)?;
        let before = aggregate(consoles, now_millis);
        consoles.remove(console)?;
        let after = aggregate(consoles, now_millis);
        if consoles.is_empty() {
            people.remove(&key);
        }
        if after == before {
            None
        } else {
            Some(after.unwrap_or(PresenceStatus::Offline))
        }
    }

    /// Everyone whose lease is still good, newest first.
    ///
    /// Expired entries are filtered here rather than swept on a timer: reads
    /// are the only thing that cares, so a lapsed lease costs a comparison
    /// instead of a background task. [`Self::sweep`] exists for the memory,
    /// not for the correctness. A person with several open consoles is one row
    /// here — [`most_present`] across them, timestamped by whichever renewed
    /// most recently.
    pub fn list(&self, company: &CompanyId, now_millis: u64) -> Vec<PresenceView> {
        let people = self.people.lock().expect("presence registry poisoned");
        let mut out: Vec<PresenceView> = people
            .iter()
            .filter(|((id, _), _)| id == company)
            .filter_map(|((_, user), consoles)| {
                let live: Vec<&Peer> = consoles
                    .values()
                    .filter(|peer| !expired(peer.last_beat_millis, now_millis))
                    .collect();
                if live.is_empty() {
                    return None;
                }
                let status = live
                    .iter()
                    .map(|peer| peer.status)
                    .reduce(most_present)
                    .expect("checked non-empty above");
                let at_millis = live
                    .iter()
                    .map(|peer| peer.last_beat_millis)
                    .max()
                    .expect("checked non-empty above");
                Some(PresenceView {
                    user_id: user.clone(),
                    status,
                    at_millis,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.at_millis
                .cmp(&a.at_millis)
                .then(a.user_id.cmp(&b.user_id))
        });
        out
    }

    /// Forgets every lapsed lease, in every company.
    ///
    /// Purely to bound memory on a long-lived host — [`Self::list`] already
    /// ignores what this removes, so nothing observable changes. Returns how
    /// many console leases went, for a log line.
    pub fn sweep(&self, now_millis: u64) -> usize {
        let mut people = self.people.lock().expect("presence registry poisoned");
        let mut removed = 0;
        people.retain(|_, consoles| {
            let before = consoles.len();
            consoles.retain(|_, peer| !expired(peer.last_beat_millis, now_millis));
            removed += before - consoles.len();
            !consoles.is_empty()
        });
        removed
    }
}

/// The aggregate status across every live console a person currently has open,
/// or `None` when none of them are (an empty map, or every lease expired).
fn aggregate(consoles: &HashMap<String, Peer>, now_millis: u64) -> Option<PresenceStatus> {
    consoles
        .values()
        .filter(|peer| !expired(peer.last_beat_millis, now_millis))
        .map(|peer| peer.status)
        .reduce(most_present)
}

/// Whether a lease taken at `last_beat` has lapsed by `now`.
///
/// Clock-skew tolerant in the one direction that matters: a beat stamped in the
/// future (two hosts disagreeing by a second) is not expired, because
/// `saturating_sub` floors the age at zero rather than wrapping it to
/// eighteen quintillion milliseconds and marking a live peer as gone.
fn expired(last_beat: u64, now: u64) -> bool {
    now.saturating_sub(last_beat) > PRESENCE_TTL_MILLIS
}

/// Periodically forgets lapsed leases across the whole process (issue: "Bound
/// client-supplied console leases").
///
/// [`PresenceRegistry::sweep`] existed from the start but had no production
/// caller — its only caller was a unit test. That was not silently unsafe
/// (`list` already filters an expired lease out of every read, so nothing
/// downstream ever saw a stale one), but the backing map itself only ever
/// grew: a crashed tab that never sent `DELETE`, or a client minting fresh
/// `consoleId`s, both left dead entries nothing ever removed. Deliberately not
/// folded into [`crate::runtime::maintenance::MaintenanceTicker`] — that ticker
/// is scoped to registered *companies* and drives per-company retirement
/// through [`crate::CompanyRuntime`]; this registry is host-global and keyed by
/// neither, so it gets its own always-on task, spawned once at boot the same
/// way. [`MAX_CONSOLES_PER_PERSON`] bounds the worst case *per person* between
/// sweeps; this is what reclaims the memory for everyone once a lease expires.
pub struct PresenceSweeper {
    registry: Arc<PresenceRegistry>,
}

impl PresenceSweeper {
    pub fn new(registry: Arc<PresenceRegistry>) -> Self {
        Self { registry }
    }

    /// Runs until `shutdown` is notified, sweeping once per [`PRESENCE_TTL_MILLIS`]
    /// — lapsed-but-unswept memory is bounded by one TTL's worth of leases, not
    /// by how long the process has been up.
    pub fn spawn(self, shutdown: Arc<Notify>) -> JoinHandle<()> {
        tokio::spawn(async move {
            // Built once and pinned, not rebuilt inside the loop — see
            // `MaintenanceTicker::spawn`'s identical comment for why a
            // freshly-built `Notified` each iteration can miss a shutdown that
            // arrives mid-sweep.
            let notified = shutdown.notified();
            tokio::pin!(notified);
            loop {
                tokio::select! {
                    _ = &mut notified => break,
                    _ = tokio::time::sleep(Duration::from_millis(PRESENCE_TTL_MILLIS)) => {
                        self.registry.sweep(crate::ports::now_millis());
                    }
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "presence_tests.rs"]
mod tests;
