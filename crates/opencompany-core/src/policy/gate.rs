//! The manifest-`[policy]`-driven [`ApprovalGate`] implementation.
//!
//! Evaluation follows the precedence in
//! [`docs/spec/company-brain/approvals.md`](../../docs/spec/company-brain/approvals.md):
//!
//! 1. `never_do` hard-deny (Phase 1: the delegation-rule compiler is stubbed,
//!    so this list is always empty).
//! 2. `[policy].always_approve` effect kinds always park for approval.
//! 3. mode dispatch: `readonly` gates everything, `full` allows everything,
//!    `supervised` applies the checkpoint taxonomy by [`EffectGroup`], and
//!    `auto` applies it too — see [`evaluate_auto`](ManifestApprovalGate::evaluate_auto)
//!    for why those two coincide on this path and differ sharply on the other.
//!
//! There are **four** policy modes, and only three of them
//! (`readonly`, `supervised`, `full`) take their names from OpenHuman's own
//! security tiers. `auto` is opencompany's, added by issue #560, and its
//! addition is what stopped the mapping being 1:1 — see
//! [`docs/spec/company-brain/grants.md`](../../docs/spec/company-brain/grants.md).
//!
//! This header said "three" for long enough that the gate below grew to match
//! it: `auto` fell into the `_` catch-all and parked every native effect,
//! making the tier every provisioned company boots on **stricter** than the one
//! below it on the ladder (issue #1454). The dispatch is now
//! [`mode_decision`](ManifestApprovalGate::mode_decision), which returns `None`
//! only for a word that is not a tier at all, so a test can tell "no arm" from
//! "an arm that decided to park".
//!
//! ## The ladder invariant
//!
//! [`POLICY_MODES`](crate::company::POLICY_MODES) is ordered by increasing
//! autonomy and the console renders it in that order, so the gate owes it one
//! property: **for any given effect, permissiveness must never decrease as you
//! move up the list.** An operator who moves a company one tier up to be
//! interrupted less must not be interrupted more. `the_tier_ladder_is_monotonic`
//! in this module's tests pins that across every tier and every branch of the
//! taxonomy, rather than spot-checking one arm — which is precisely what let
//! #1454 survive.
//!
//! `evaluate` returns a bare [`PolicyDecision`]; the [`ApprovalId`] for a
//! `RequireApproval` outcome is minted separately by [`park`](ManifestApprovalGate::park).
//!
//! Silence is a default-deny: a parked approval left unresolved past its TTL
//! (default [`DEFAULT_TTL_MILLIS`], overridable per company with
//! `[policy].approval_ttl_hours`) resolves to deny, whether swept by
//! [`sweep_expired`](ManifestApprovalGate::sweep_expired) or observed at
//! resolution time by [`resolve_at`](ManifestApprovalGate::resolve_at) /
//! [`resolve_amended`](ManifestApprovalGate::resolve_amended).
//!
//! **The TTL is only half of default-deny-on-silence.** The other half is
//! something actually running the sweep, which until issue #971 only a company
//! with a manifest `[[schedule]]` had — see
//! [`MaintenanceTicker`](crate::runtime::maintenance::MaintenanceTicker), which
//! now drives it for every registered company. A shorter deadline with nothing
//! sweeping is still a queue that never empties.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use async_trait::async_trait;

use crate::Result;
use crate::company::Policy;
use crate::ports::approvals::ApprovalGate;
use crate::ports::now_millis;
use crate::ports::types::{
    Actor, ApprovalId, CompanyId, Effect, EffectGroup, PolicyDecision, Verdict,
};

/// Default time-to-live for a parked approval: 24 hours in milliseconds.
///
/// **Was 7 days until issue #971.** A parked call is refused and re-dispatched
/// rather than suspended (#243/#469), so approving a three-day-old entry does
/// not usefully resume the turn that raised it — the turn is long gone. A
/// week-long deadline therefore did not buy an operator a week of useful
/// decisions; it bought a queue whose oldest entries were unactionable *and*
/// still counted toward the badge, which is how a badge stops describing
/// current state and starts being ignored.
///
/// 24 hours is one working day: an approval raised during a day the operator
/// works is still there when they next look, and one nobody looked at for a
/// full day is one the work behind it has already moved past.
///
/// A company that genuinely wants longer says so explicitly with
/// `[policy].approval_ttl_hours` rather than inheriting it from a constant.
pub const DEFAULT_TTL_MILLIS: u64 = 24 * 60 * 60 * 1000;

/// Replays a company's event log to decide whether it should boot with the
/// emergency stop engaged (issue #86).
///
/// **The event log is the durable state, not a mirror of it.** The kill switch
/// deliberately has no field on
/// [`CompanyRecord`](crate::ports::types::CompanyRecord): a second copy of a
/// safety flag is a second thing that can disagree with the first, and the
/// append-only log already answers "what did the last operator decide" exactly.
/// The last [`EmergencyPauseChanged`](crate::ports::types::CompanyEvent::EmergencyPauseChanged)
/// wins; a log with none was never stopped.
///
/// # Fail-safe
///
/// A read failure returns `Err`, and the caller
/// ([`hydrate_emergency`](crate::runtime::CompanyRuntime::hydrate_emergency))
/// turns that into **stopped**. This deliberately diverges from
/// [`sweep_interrupted_runs`](crate::runtime::sweep_interrupted_runs) beside it,
/// which swallows read failures because record-keeping must never stop a company
/// booting. Here the read *is* the safety decision: a company that cannot prove
/// it was running must not assume it was.
pub async fn replayed_emergency(
    events: &std::sync::Arc<dyn crate::ports::EventLog>,
    company: &CompanyId,
) -> Result<bool> {
    use crate::ports::types::{CompanyEvent, EventSeq};

    // A full scan, on the same terms as the interrupted-run sweep beside it:
    // boot already reads this log end to end, and the port exposes no reverse
    // or filtered read to do better with.
    let stored = events
        .read_from(company, EventSeq::new(0), usize::MAX)
        .await?;
    Ok(stored
        .iter()
        .rev()
        .find_map(|stored| match &stored.event {
            CompanyEvent::EmergencyPauseChanged { engaged, .. } => Some(*engaged),
            _ => None,
        })
        .unwrap_or(false))
}

/// A parked effect awaiting operator resolution.
#[derive(Clone, Debug)]
struct ParkedEffect {
    effect: Effect,
    parked_at_millis: u64,
}

/// What actually happened when a resolve reached the queue (issue #243).
///
/// The [`ApprovalGate`] port's `resolve` returns `Option<Effect>`, which
/// collapses four distinct situations into one `None`: the approval was denied,
/// it expired, it was never parked, or it was already resolved. That is enough
/// for "should I execute the effect?" and nowhere near enough for anything else
/// — most importantly it cannot tell "the operator denied this" from "this
/// approval is already gone", so a double-submit (a double-click, a retried
/// request, two operators on the same queue) looked exactly like a deny and got
/// the full treatment: a second `ApprovalResolved` journal line and a second
/// follow-up cycle, both describing an approval that no longer existed.
///
/// Returned by the concrete gate rather than widened onto the port, following
/// the amend path — [`resolve_amended`](ManifestApprovalGate::resolve_amended) —
/// which already reaches past the `dyn` boundary for the same reason.
#[derive(Clone, Debug, PartialEq)]
pub enum ResolveOutcome {
    /// No such approval is parked: an unknown id, or one already resolved by an
    /// earlier call. The caller must treat this as a no-op, not a deny.
    NotParked,
    /// The approval was parked but is past its TTL, so it resolves to a
    /// default-deny whatever the operator asked for. It IS removed.
    Expired,
    /// The operator denied it. Removed, and here is the effect the card showed,
    /// retained so a standing denial can be minted against the same scoped
    /// arguments (issue #1458) rather than the payload-scrubbed copy the journal
    /// keeps (issue #351), which would read as an unscoped wildcard.
    Denied(Effect),
    /// The operator approved it in time. Removed, and here is the effect.
    Approved(Effect),
}

/// The default [`ApprovalGate`]: evaluates effects against a company's
/// `[policy]` and holds the in-memory approval queue.
pub struct ManifestApprovalGate {
    policy: RwLock<Policy>,
    policy_hitl_enabled: AtomicBool,
    ttl_millis: AtomicU64,
    parked: Mutex<HashMap<ApprovalId, ParkedEffect>>,
    /// Effects removed by TTL expiry, retained only until the runtime completes
    /// their retirement transaction.
    expired_effects: Mutex<HashMap<ApprovalId, Effect>>,
    /// The governance kill switch (issue #86).
    ///
    /// An `AtomicBool` rather than a lock because `evaluate` reads it on every
    /// effect and a kill switch that adds contention to the path it guards is a
    /// poor one. It is a *cache* of the event log, hydrated at boot by
    /// [`replayed_emergency`]; the log is the durable truth.
    ///
    /// Defaults to `false` because a freshly constructed gate has not been told
    /// anything yet, and the one caller that can distinguish "not in emergency"
    /// from "could not find out" — the boot path — engages it explicitly on a
    /// read failure. See
    /// [`hydrate_emergency`](crate::runtime::CompanyRuntime::hydrate_emergency).
    emergency: AtomicBool,
}

impl ManifestApprovalGate {
    /// Builds a gate from a company's manifest `[policy]` block.
    ///
    /// **The TTL default resolves HERE, not at parse** (issue #971).
    /// [`Policy::approval_ttl_hours`] is a plain `Option` with no serde
    /// default, so a manifest that never mentioned the knob deserializes to
    /// `None` — the same bytes and the same value it did before the field
    /// existed. That matters because
    /// [`carry_policy_override`](crate::runtime::builder) compares the previous
    /// boot's seed `[policy]` against this one's *as whole blocks* to decide
    /// whether an operator's console override survives a rebuild. A field that
    /// defaulted to `Some(24)` at parse would make the seed change under any
    /// company whose manifest is silent the moment this constant moves, and the
    /// rebuild would read that as version control having spoken and silently
    /// discard the override. Nobody would have edited anything. See the same
    /// trap spelled out on [`Policy::mode`](crate::company::Policy::mode).
    pub fn new(policy: Policy) -> Self {
        let ttl_millis = policy
            .approval_ttl_hours
            .map(|hours| hours.saturating_mul(60 * 60 * 1000))
            .unwrap_or(DEFAULT_TTL_MILLIS);
        Self {
            policy: RwLock::new(policy),
            policy_hitl_enabled: AtomicBool::new(true),
            ttl_millis: AtomicU64::new(ttl_millis),
            parked: Mutex::new(HashMap::new()),
            expired_effects: Mutex::new(HashMap::new()),
            emergency: AtomicBool::new(false),
        }
    }

    /// Disables policy-generated approval decisions while leaving explicit
    /// `park` calls and hard emergency/read-only denials available.
    pub fn with_policy_hitl_disabled(self) -> Self {
        self.policy_hitl_enabled.store(false, Ordering::Relaxed);
        self
    }

    /// Engages (`true`) or releases (`false`) the emergency stop.
    ///
    /// Returns the **previous** value. The atomic swap is what makes a
    /// transition race-safe: exactly one caller observes the old state, so
    /// [`CompanyRuntime::emergency_pause`] and
    /// [`CompanyRuntime::emergency_resume`] can journal an
    /// [`EmergencyPauseChanged`](crate::ports::types::CompanyEvent::EmergencyPauseChanged)
    /// event only for the caller that actually changed the switch, matching the
    /// event count to the number of real transitions under a double-press.
    ///
    /// `Ordering::SeqCst` on both ends: this is a safety flag read by request
    /// handlers on other threads, and the cost of the strongest ordering on a
    /// single boolean is irrelevant next to being sure a handler that starts
    /// after the switch was pulled observes it pulled.
    pub fn set_emergency(&self, engaged: bool) -> bool {
        self.emergency.swap(engaged, Ordering::SeqCst)
    }

    /// Whether the emergency stop is currently engaged.
    ///
    /// This flag is the switch's single source of truth, but this gate is not
    /// its only enforcer: denying effects leaves the turns that ask for them
    /// running. The halt on work itself is
    /// [`CompanyRuntime::ensure_not_emergency_stopped`](crate::runtime::CompanyRuntime::ensure_not_emergency_stopped),
    /// which reads this same flag.
    pub fn is_emergency(&self) -> bool {
        self.emergency.load(Ordering::SeqCst)
    }

    /// Overrides the parked-approval TTL (default [`DEFAULT_TTL_MILLIS`]).
    pub fn with_ttl_millis(mut self, ttl_millis: u64) -> Self {
        self.ttl_millis = AtomicU64::new(ttl_millis);
        self
    }

    /// How long a parked approval has before it default-denies.
    ///
    /// Read by [`CompanyRuntime::pending_approvals`](crate::CompanyRuntime::pending_approvals)
    /// to project each card's deadline (issue #971). Exposed rather than
    /// recomputed from `[policy]` at the projection because the gate is the one
    /// that resolves the default, and a second resolution of the same rule is a
    /// second thing that can disagree — the console would then show a deadline
    /// the gate does not enforce.
    pub fn ttl_millis(&self) -> u64 {
        self.ttl_millis.load(Ordering::Relaxed)
    }

    /// Whether this gate currently turns policy (the tier, `always_approve`,
    /// the spend cap) into approval requests, as opposed to allowing
    /// everything the hard denials do not already refuse.
    ///
    /// Read by [`PolicyDto`](crate::server::ops::policy::PolicyDto) so the
    /// console states this fact rather than assuming it: every gate built by
    /// [`with_policy_hitl_disabled`](Self::with_policy_hitl_disabled) reports
    /// `false` here, and a copy of that assumption in TypeScript would drift
    /// the moment a gate is built without it.
    pub fn policy_hitl_enabled(&self) -> bool {
        self.policy_hitl_enabled.load(Ordering::Relaxed)
    }

    /// The policy snapshot the gate currently evaluates against.
    ///
    /// What [`apply_effective_policy`](Self::apply_effective_policy) last
    /// installed, or the `[policy]` block [`new`](Self::new) was built from when
    /// nothing has overridden it. The cycle reads this for a test-injected gate
    /// so the harness roster is pinned to the SAME policy the native gate keeps
    /// (issue #1455) — an injected gate carries its own policy on purpose, which
    /// may differ from the persisted record's effective one.
    pub fn policy(&self) -> Policy {
        self.policy.read().expect("policy lock poisoned").clone()
    }

    /// Updates the deadline used for new and already parked approvals.
    ///
    /// The policy overlay is an operator control, so waiting for a process
    /// restart would make the Settings panel report a deadline the live queue
    /// does not use. A parked card remains the same request, but its deadline
    /// is evaluated from the current company policy each time it is displayed
    /// or resolved.
    pub fn set_ttl_millis(&self, ttl_millis: u64) {
        self.ttl_millis.store(ttl_millis, Ordering::Relaxed);
    }

    /// Replaces the policy snapshot the gate evaluates against, keeping the
    /// parked queue and the emergency switch.
    ///
    /// Used at boot/rebuild time and at the start of every cycle (issue #1455):
    /// the gate is constructed from the seed's `[policy]` alone and the
    /// operator's console override resolves only after the persisted record is
    /// read — see
    /// [`CompanyRecord::effective_policy`](crate::ports::types::CompanyRecord::effective_policy).
    /// Applying the effective policy keeps native evaluation (mode,
    /// `always_approve`, spend cap) and the derived deadline enforcing what the
    /// console reports; without it a persisted override would still be returned
    /// by `GET` while the live gate silently reverted to the manifest snapshot,
    /// which is especially unsafe after an operator *shortened* a deadline.
    pub fn apply_effective_policy(&self, policy: Policy) {
        self.apply_effective_ttl(&policy);
        self.policy
            .write()
            .expect("policy lock poisoned")
            .clone_from(&policy);
    }

    /// Moves only the deadline derived from `policy`, leaving the evaluation
    /// snapshot (mode, `always_approve`, spend cap) untouched.
    ///
    /// The TTL is *immediate* by contract while the rest of the policy moves at
    /// the next safe turn boundary: a parked card remains the same request, but
    /// its deadline is re-evaluated against the current TTL each time it is
    /// displayed, swept or resolved, so delaying the deadline until the next
    /// cycle would let approvals parked under a longer TTL outlive the one the
    /// console just reported. The ops handler applies this right after a policy
    /// PUT/DELETE persists, and [`apply_effective_policy`](Self::apply_effective_policy)
    /// applies it alongside the snapshot at boot and per-cycle.
    pub fn apply_effective_ttl(&self, policy: &Policy) {
        let ttl_millis = policy
            .approval_ttl_hours
            .map(|hours| hours.saturating_mul(60 * 60 * 1000))
            .unwrap_or(DEFAULT_TTL_MILLIS);
        self.ttl_millis.store(ttl_millis, Ordering::Relaxed);
    }

    /// The ids of every currently-parked approval.
    pub fn parked_ids(&self) -> Vec<ApprovalId> {
        self.parked
            .lock()
            .expect("parked map poisoned")
            .keys()
            .cloned()
            .collect()
    }

    /// Re-parks an effect under a known id (used by boot replay to rebuild the
    /// queue from the event log).
    pub fn rehydrate(&self, id: ApprovalId, effect: Effect, parked_at_millis: u64) {
        self.parked.lock().expect("parked map poisoned").insert(
            id,
            ParkedEffect {
                effect,
                parked_at_millis,
            },
        );
    }

    /// Re-anchors a parked approval's TTL window to `now`, giving the operator a
    /// fresh full deadline on it (issue #1805). Returns whether an entry was
    /// actually moved — `false` for an id that is not (or no longer) parked, so a
    /// caller can answer 404 rather than pretend it extended something.
    ///
    /// # Why a full fresh window, not "+N hours"
    ///
    /// The parked entry carries a single `parked_at_millis`, and both the sweeper
    /// and the console's deadline are `parked_at + ttl`. Moving that instant to
    /// `now` is therefore the *whole* of an extension: the sweep that would have
    /// retired it no longer sees it expired, and the projected deadline moves in
    /// lockstep, with no second knob that could disagree. An additive "+N" would
    /// need its own stored offset and a second place computing the deadline — the
    /// exact fork [`ttl_millis`](Self::ttl_millis) exists to avoid.
    ///
    /// The durable half lives in the journal (`record_extended`): this moves the
    /// **live** anchor the sweeper reads, and boot replay re-applies the move by
    /// rehydrating from the journal's extended anchor, so an extension survives a
    /// redeploy rather than reverting to the original park instant.
    pub fn extend(&self, id: &ApprovalId, now_millis: u64) -> bool {
        let mut map = self.parked.lock().expect("parked map poisoned");
        match map.get_mut(id) {
            Some(parked) => {
                parked.parked_at_millis = now_millis;
                true
            }
            None => false,
        }
    }

    /// Removes every parked approval older than the TTL relative to `now`,
    /// returning the ids that expired (they resolve to deny).
    pub fn sweep_expired(&self, now_millis: u64) -> Vec<ApprovalId> {
        self.sweep_expired_capped(now_millis, usize::MAX)
    }

    /// [`sweep_expired`](Self::sweep_expired), taking at most `limit` entries,
    /// **oldest first** (issue #971).
    ///
    /// The cap is what keeps a first sweep after a long silence from turning
    /// into one unbounded burst of retirement work. Each retirement is a
    /// journal append, a grant clear, an event append and possibly a released
    /// #469 continuation spawning a whole agent turn; a company that has been
    /// accumulating for days would do all of that for its entire backlog inside
    /// a single minute tick, on the tick shared by every other company in the
    /// process. Capped, the backlog drains over a few minutes and nothing else
    /// waits on it.
    ///
    /// **Oldest first, so the cap is not a lottery.** The map is a `HashMap`
    /// and its iteration order is randomized per process, so an uncapped-order
    /// cap would retire an arbitrary subset and leave an arbitrary one — the
    /// same entry could sit unswept across many ticks while newer ones went
    /// first. Sorting by park instant makes the drain deterministic and makes
    /// "the oldest, most unactionable entries go first" true rather than
    /// incidental.
    pub fn sweep_expired_capped(&self, now_millis: u64, limit: usize) -> Vec<ApprovalId> {
        let mut map = self.parked.lock().expect("parked map poisoned");
        let mut expired: Vec<(u64, ApprovalId)> = map
            .iter()
            .filter(|(_, pe)| now_millis.saturating_sub(pe.parked_at_millis) >= self.ttl_millis())
            .map(|(id, pe)| (pe.parked_at_millis, id.clone()))
            .collect();
        expired.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_ref().cmp(b.1.as_ref())));
        expired.truncate(limit);
        let expired: Vec<ApprovalId> = expired.into_iter().map(|(_, id)| id).collect();
        for id in &expired {
            if let Some(parked) = map.remove(id) {
                self.expired_effects
                    .lock()
                    .expect("expired effects poisoned")
                    .insert(id.clone(), parked.effect);
            }
        }
        expired
    }

    /// Takes the effect removed by an expiry, if its runtime retirement has not
    /// consumed it yet.
    pub fn take_expired_effect(&self, id: &ApprovalId) -> Option<Effect> {
        self.expired_effects
            .lock()
            .expect("expired effects poisoned")
            .remove(id)
    }

    /// A clone of a parked effect without resolving it.
    ///
    /// Used by the amend path to overlay an operator's payload edit onto the
    /// original before re-submitting it for execution.
    pub fn parked_effect(&self, id: &ApprovalId) -> Option<Effect> {
        self.parked
            .lock()
            .expect("parked map poisoned")
            .get(id)
            .map(|pe| pe.effect.clone())
    }

    /// Resolves a parked approval to an operator-amended effect
    /// (approve-with-edit), as of `now`.
    ///
    /// Removes the parked entry and returns the `amended` effect to execute, or
    /// `None` when the approval is unknown or has expired past its TTL — the
    /// same default-deny-on-silence that governs [`resolve_at`](Self::resolve_at).
    ///
    /// Prefer [`resolve_amended_outcome`](Self::resolve_amended_outcome) when
    /// the caller has to *report* what happened: this `None` collapses "there
    /// was nothing parked" into "the deadline had passed", and those are
    /// different things to write into an audit trail (issue #1449).
    pub fn resolve_amended(
        &self,
        id: &ApprovalId,
        amended: Effect,
        by: Actor,
        now_millis: u64,
    ) -> Option<Effect> {
        match self.resolve_amended_outcome(id, amended, by, now_millis) {
            ResolveOutcome::Approved(effect) => Some(effect),
            _ => None,
        }
    }

    /// The amend counterpart to
    /// [`resolve_outcome`](Self::resolve_outcome): resolves a parked approval to
    /// an operator-amended effect and says **which** outcome that was
    /// (issue #1449).
    ///
    /// An amend is an approve, so the outcomes it can produce are the same
    /// three the plain approve can: [`ResolveOutcome::NotParked`],
    /// [`ResolveOutcome::Expired`], and [`ResolveOutcome::Approved`] carrying
    /// the *amended* effect. It never denies — a deny cannot carry an
    /// amendment, and the route refuses the pairing.
    ///
    /// The removal and the outcome decision are one critical section, for the
    /// same reason they are on `resolve_outcome`: two concurrent resolves of
    /// one id must not both win.
    pub fn resolve_amended_outcome(
        &self,
        id: &ApprovalId,
        amended: Effect,
        _by: Actor,
        now_millis: u64,
    ) -> ResolveOutcome {
        let Some(parked) = self.parked.lock().expect("parked map poisoned").remove(id) else {
            return ResolveOutcome::NotParked;
        };
        if now_millis.saturating_sub(parked.parked_at_millis) >= self.ttl_millis() {
            self.expired_effects
                .lock()
                .expect("expired effects poisoned")
                .insert(id.clone(), parked.effect);
            return ResolveOutcome::Expired;
        }
        ResolveOutcome::Approved(amended)
    }

    /// Resolves a parked approval as of `now`, reporting **which** of the four
    /// outcomes occurred rather than collapsing them into an `Option`
    /// (issue #243).
    ///
    /// The `remove` and the outcome decision are one critical section, so two
    /// concurrent resolves of the same id cannot both win: whichever thread
    /// takes the lock first gets `Approved` / `Denied` / `Expired`, and every
    /// other thread finds the map empty and gets [`ResolveOutcome::NotParked`].
    /// That is what makes an approve idempotent at the source instead of
    /// depending on callers to check-then-act, which is racy by construction.
    pub fn resolve_outcome(
        &self,
        id: &ApprovalId,
        verdict: Verdict,
        _by: Actor,
        now_millis: u64,
    ) -> ResolveOutcome {
        let Some(parked) = self.parked.lock().expect("parked map poisoned").remove(id) else {
            return ResolveOutcome::NotParked;
        };
        if now_millis.saturating_sub(parked.parked_at_millis) >= self.ttl_millis() {
            self.expired_effects
                .lock()
                .expect("expired effects poisoned")
                .insert(id.clone(), parked.effect);
            return ResolveOutcome::Expired;
        }
        match verdict {
            Verdict::Approve => ResolveOutcome::Approved(parked.effect),
            // Carry the effect rather than re-reading the journal's scrubbed
            // copy (issue #351): a standing deny minted from that copy would
            // lose the scope the operator actually refused (issue #1458).
            Verdict::Deny => ResolveOutcome::Denied(parked.effect),
        }
    }

    /// Resolves a parked approval as of `now`, so expiry is testable.
    ///
    /// An expired approval resolves to deny (`None`) regardless of `verdict`.
    pub fn resolve_at(
        &self,
        id: &ApprovalId,
        verdict: Verdict,
        _by: Actor,
        now_millis: u64,
    ) -> Option<Effect> {
        let parked = self
            .parked
            .lock()
            .expect("parked map poisoned")
            .remove(id)?;
        if now_millis.saturating_sub(parked.parked_at_millis) >= self.ttl_millis() {
            return None;
        }
        match verdict {
            Verdict::Approve => Some(parked.effect),
            Verdict::Deny => None,
        }
    }

    /// Whether this effect is one a person would want to know had **already
    /// happened** before re-running the work that produced it (issue #351).
    ///
    /// It is the supervised checkpoint taxonomy read as a question about the
    /// past rather than the future: signing, publishing, touching identity,
    /// spending or engaging anything but a known amount under a configured
    /// cap, first contact with a counterparty. Those are the effects
    /// `evaluate_supervised` refuses to wave through, and they are refused
    /// precisely because they cannot be taken back.
    ///
    /// Deliberately **mode-independent**. A `full`-mode company executes every
    /// one of these without ever parking it, which is exactly the case this
    /// warning exists for: the operator was never asked, so the retry dialog is
    /// the first and only place they learn a filing was already submitted.
    /// Asking the same taxonomy the same way for every mode also means a
    /// company that later tightens its policy does not retroactively change
    /// what its history says it did.
    ///
    /// Delegating to [`evaluate_supervised`](Self::evaluate_supervised) rather
    /// than restating the rules is the point: two copies of "which effects are
    /// irreversible" would drift, and the copy in the retry dialog would drift
    /// silently — nobody notices a warning that stopped naming something.
    pub fn is_irreversible(&self, effect: &Effect) -> bool {
        matches!(
            self.evaluate_supervised(effect),
            PolicyDecision::RequireApproval
        )
    }

    /// The tier dispatch, as an `Option` so "no arm for this word" is
    /// distinguishable from "an arm that decided to park" (issue #1454).
    ///
    /// `evaluate` turns `None` into [`PolicyDecision::RequireApproval`], which
    /// is the same fail-safe the `_` catch-all always applied. The difference is
    /// only visible from a test: `auto` sat in that catch-all for two releases
    /// and nothing could see it, because a tier that parks everything and a word
    /// nobody implemented produce identical decisions. Now
    /// `every_policy_mode_has_a_named_arm` walks
    /// [`POLICY_MODES`](crate::company::POLICY_MODES) and fails on the day a
    /// fifth tier is added to that list and forgotten here.
    ///
    /// Takes the evaluated `Policy` snapshot as an argument — not `&self` — so a
    /// caller already holding the policy read guard can dispatch without a
    /// recursive read of the non-reentrant `RwLock`, and `mode` as a separate
    /// argument so the test can ask about a word without building a whole
    /// [`Policy`] whose mode it is, keeping the fail-safe arm reachable.
    fn mode_decision(policy: &Policy, mode: &str, effect: &Effect) -> Option<PolicyDecision> {
        match mode {
            "full" => Some(PolicyDecision::Allow),
            "readonly" => Some(PolicyDecision::RequireApproval),
            "supervised" => Some(Self::evaluate_supervised_with_policy(policy, effect)),
            "auto" => Some(Self::evaluate_auto(policy, effect)),
            _ => None,
        }
    }

    /// The `auto` tier, for **native** effects (issue #1454).
    ///
    /// `auto`'s contract, in the operator's words and in the console's:
    /// *the agents work on their own and stop before anything that leaves the
    /// company or spends money.* On this path that is, exactly and completely,
    /// the supervised checkpoint taxonomy — so this delegates rather than
    /// restating it.
    ///
    /// # Why the two coincide here and not on the tool path
    ///
    /// The tier is real; it just does its work somewhere else. On the **tool**
    /// path `auto` is [`Consequence::parks_under_auto`](crate::policy::Consequence::parks_under_auto),
    /// which waves through the calls declared [`Standing::Grantable`](crate::policy::Standing::Grantable)
    /// — chiefly the agent's own sandbox writes (`file_write`, `edit`,
    /// `apply_patch`, `memory_store`) and reads scoped to one connected account.
    /// The declaration table in [`consequence`](crate::policy::consequence) is
    /// the list; this is a gloss on it, not a copy. Those are what `supervised`
    /// parks and `auto` does not, and they are the whole difference between the
    /// tiers.
    ///
    /// The **native** taxonomy has no such calls to wave through. Every group
    /// [`evaluate_supervised`](Self::evaluate_supervised) parks — a spend not
    /// known to be under the cap, a message to a counterparty nobody has talked
    /// to, a signature, a publish, an identity change, an engagement on those
    /// same cap terms — is by definition something that leaves the company or
    /// spends money, which is the exact line `auto` says it stops at. The only
    /// inside-the-company native bucket is [`EffectGroup::Other`], and
    /// `supervised` already allows it. So there is nothing for `auto` to loosen
    /// here, and the honest
    /// implementation is one that says so.
    ///
    /// # Why not the stricter reading
    ///
    /// The tempting alternative — park `Spend`/`Send`/`Hire` unconditionally,
    /// withholding the cap relief and the established-thread relief as
    /// "`supervised`'s concessions" — inverts the ladder a second time. It would
    /// park a $1 spend and a reply on a running email thread that `supervised`,
    /// the tier *below* it, waves through. A tier cannot be sold as more
    /// autonomy and deliver less; see the ladder invariant on this module.
    ///
    /// # If they ever diverge
    ///
    /// This is a named seam, not an alias, so a future native effect that
    /// genuinely belongs to `auto` and not to `supervised` gets its own arm
    /// here. The invariant that must survive that edit is the one direction:
    /// whatever this parks must stay a **subset** of what
    /// [`evaluate_supervised`](Self::evaluate_supervised) parks.
    fn evaluate_auto(policy: &Policy, effect: &Effect) -> PolicyDecision {
        Self::evaluate_supervised_with_policy(policy, effect)
    }

    /// Evaluates the supervised taxonomy using a captured policy snapshot.
    ///
    /// Keeping the cap as an argument prevents a caller that already holds the
    /// policy read guard from attempting a recursive read of the non-reentrant
    /// `RwLock`.
    fn evaluate_supervised_with_policy(policy: &Policy, effect: &Effect) -> PolicyDecision {
        Self::evaluate_supervised_with_cap(effect, policy.auto_approve_under_usd)
    }

    fn evaluate_supervised(&self, effect: &Effect) -> PolicyDecision {
        let policy = self.policy.read().expect("policy lock poisoned");
        Self::evaluate_supervised_with_policy(&policy, effect)
    }

    /// The one cap comparison both money groups read (issue #2037).
    ///
    /// True only when the amount and the cap are **both** known and the amount
    /// is strictly under: an unstated amount and an unconfigured cap are not
    /// evidence of a small number, so neither is under anything.
    fn under_cap(amount: Option<f64>, cap: Option<f64>) -> bool {
        matches!((amount, cap), (Some(amount), Some(cap)) if amount < cap)
    }

    fn evaluate_supervised_with_cap(effect: &Effect, cap: Option<f64>) -> PolicyDecision {
        match effect.group() {
            // Spend under the cap is auto-allowed; anything else parks.
            EffectGroup::Spend => {
                if Self::under_cap(effect.amount_usd(), cap) {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::RequireApproval
                }
            }
            // First message to a new counterparty parks; established threads pass.
            EffectGroup::Send => {
                if effect.is_established_thread() && !effect.is_first_time_counterparty() {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::RequireApproval
                }
            }
            // Irreversible / identity-touching effects always park.
            EffectGroup::Sign | EffectGroup::Publish | EffectGroup::Identity => {
                PolicyDecision::RequireApproval
            }
            // Hiring is auto-allowed under the cap, and only for a
            // counterparty this company has dealt with before.
            EffectGroup::Hire => {
                if Self::under_cap(effect.amount_usd(), cap) && !effect.is_first_time_counterparty()
                {
                    PolicyDecision::Allow
                } else {
                    PolicyDecision::RequireApproval
                }
            }
            EffectGroup::Other => PolicyDecision::Allow,
        }
    }
}

#[async_trait]
impl ApprovalGate for ManifestApprovalGate {
    async fn evaluate(&self, _company: &CompanyId, effect: &Effect) -> Result<PolicyDecision> {
        // 0. The emergency stop (issue #86), ahead of every policy rule
        //    including `always_approve`.
        //
        //    `Deny`, not `RequireApproval`: parking would leave the queue as the
        //    escape hatch from the kill switch, so an operator who pulled it
        //    could re-authorise the very effects they just stopped without ever
        //    releasing it. Denial returns to the brain as a refusal it replans
        //    around, which is what "park all new work" has to mean.
        //
        //    `EffectGroup::Other` is exempt at this layer only. It used to be
        //    the carve-out that kept chat alive under a stop; since the runtime
        //    admits no cycle at all while stopped
        //    ([`CompanyRuntime::ensure_not_emergency_stopped`]), nothing reaches
        //    this gate to take the exemption during one. It remains so that
        //    releasing restores evaluation to exactly its pre-stop shape.
        if self.is_emergency() && effect.group != EffectGroup::Other {
            return Ok(PolicyDecision::Deny);
        }

        if !self.policy_hitl_enabled.load(Ordering::Relaxed) {
            let policy = self.policy.read().expect("policy lock poisoned");
            if policy.mode.eq_ignore_ascii_case("readonly")
                && effect.kind != crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND
            {
                return Ok(PolicyDecision::Deny);
            }
            return Ok(PolicyDecision::Allow);
        }

        // 1. `never_do` hard-deny — the delegation-rule compiler is a Phase-1
        //    stub, so this list is currently always empty.

        // 2. `always_approve` effect kinds park regardless of mode or amount.
        //
        //    The match is `always_approve::matches`, shared with the harness
        //    tool policy. This arm used to compare exactly while the harness
        //    also honoured a leading segment, so one operator list meant two
        //    different things depending on which brain was running (issue
        //    #684). Adopting the harness rule here widens this arm: an entry
        //    like `payment` now parks `payment.send` natively, where before it
        //    parked nothing. That direction is the fail-safe one — an operator
        //    who named a family meant the family.
        let policy = self.policy.read().expect("policy lock poisoned");
        if crate::policy::always_approve::matches(&policy.always_approve, effect.kind()) {
            return Ok(PolicyDecision::RequireApproval);
        }

        // Mode dispatch against the held snapshot. A word with no arm is not a
        // tier — the manifest validator rejects anything outside `POLICY_MODES`
        // before a company loads — so `None` here means a path that reached a
        // `Policy` without validation. It fails safe: require approval.
        Ok(Self::mode_decision(&policy, &policy.mode, effect)
            .unwrap_or(PolicyDecision::RequireApproval))
    }

    async fn park(&self, _company: &CompanyId, effect: Effect) -> Result<ApprovalId> {
        // The emergency stop (issue #86) vetoes the park path too, not just
        // `evaluate`. The harness approval route — a tool call OpenHuman already
        // gated inline — parks without ever consulting `evaluate`, so without
        // this check a gated effect queued *after* the switch was pulled could
        // be released for execution by an approver. This is the same veto
        // `evaluate` applies, so an `EffectGroup::Other` effect (chat) still
        // parks and an approval parked *before* the stop stays resolvable.
        if self.is_emergency() && effect.group != EffectGroup::Other {
            return Err(crate::OpenCompanyError::EmergencyStop(format!(
                "refusing to park {} while stopped",
                effect.kind
            )));
        }

        let id = ApprovalId::generate();
        self.parked.lock().expect("parked map poisoned").insert(
            id.clone(),
            ParkedEffect {
                effect,
                parked_at_millis: now_millis(),
            },
        );
        Ok(id)
    }

    async fn resolve(
        &self,
        id: &ApprovalId,
        verdict: Verdict,
        by: Actor,
    ) -> Result<Option<Effect>> {
        Ok(self.resolve_at(id, verdict, by, now_millis()))
    }
}

#[cfg(test)]
#[path = "gate_approval_tests.rs"]
mod gate_approval_tests;
#[cfg(test)]
#[path = "gate_resolution_tests.rs"]
mod gate_resolution_tests;
