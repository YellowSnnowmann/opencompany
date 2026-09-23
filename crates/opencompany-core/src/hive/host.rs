//! This company as the host of one completion episode.
//!
//! `tinyhivemind` owns the episode -- who runs next, what a committed row
//! means, the conversations a seat opens, the nudges, the walls, the
//! completion fold, parking on an approval, and the checkpoint a restart
//! resumes from. It owns no storage and names no type of this host, so it
//! asks for two things:
//!
//! - a **journal** ([`Journal`]): this company's event log to read a seat's
//!   rows from, and somewhere to append what the episode commits;
//! - a **host** ([`EpisodeHost`]): how this company builds one teammate as a
//!   seat, with the episode's tools on its belt.
//!
//! The seat is a **session host**, not an `AgentSpec`. A spec names its
//! tools from the runtime's registry, fixed when the agent is built; an
//! episode's tools are bound to one seat of one episode and drain into that
//! episode's record. Inheritance is the point of the hosted runner: the belt
//! is this company's belt plus the episode's, and the gate is the episode's
//! admission in front of this company's own `ApprovalPolicy`, so a call the
//! episode does not serve still reaches that policy and can still park.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use openhuman_core::agent::OpenHumanSessionHost;
use openhuman_core::agent::tinyagents::host::LastTurnUsage;
use tinyhivemind::{Sequence, SessionLog};
use tinyhivemind_driver::{Commit, Note};
use tinyhivemind_openhuman::{Disposition, EpisodeBelt, EpisodeHost, HostedTurn, Journal};

use crate::harness::built_in::cost::TurnUsage;
use crate::harness::built_in::{HarnessDeps, HarnessPool, build_episode_seat, meter_turn_costs};
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, CompanyId, CompanyRecord, EventSeq, TurnOutcome};

use super::session_log::EventLogSessionLog;

/// What a seat calls the episode's tools, in front of their served names.
///
/// This company already gives every teammate a `desk_`-prefixed speech belt,
/// so the episode's bare `post` and `complete_episode` would collide at the
/// gate: admission is by name, and a host tool sharing a bare name would be
/// admitted past this company's own policy.
const TOOL_PREFIX: &str = "desk_";

/// The author a desk note is written under: the episode speaking, not a
/// teammate. The session log reads a reserved id as a system row, which is
/// what keeps it out of the completion fold.
const DESK_AUTHOR: &str = crate::ports::SYSTEM_AUTHOR;

/// One company, hosting one episode on one of its desks.
pub struct DeskHost {
    company: CompanyId,
    desk_id: String,
    /// The thread this episode's rows are parented to.
    thread_root: Option<EventSeq>,
    events: Arc<dyn EventLog>,
    log: EventLogSessionLog,
    /// The company and what its agents are built from, for building a seat.
    /// A host that only journals -- a test over the commit path, say --
    /// names neither and is asked for no seat.
    roster: Option<(Arc<CompanyRecord>, Arc<HarnessDeps>)>,
    /// The pool this desk's teammates live in, for the lock a turn holds.
    ///
    /// A seat of this episode is a session of its own, so two desks running
    /// the same teammate no longer share its conversation -- but they do
    /// share the teammate: its spend cap, its workspace, its memory, and the
    /// one lane the console draws for it. The lock is what keeps those from
    /// being written by two turns at once.
    pool: Option<Arc<HarnessPool>>,
    /// This episode's id, carried on every bracket so the console can draw
    /// one seat's lane within one meeting.
    episode_id: String,
    /// Which wave is running, for the same reason. Bumped as each wave
    /// settles, which is the one moment the loop tells a host a wave ended.
    wave: AtomicU64,
    /// What this company does with whatever a turn left waiting on a human.
    ///
    /// Parking belongs to the cycle that opened the episode -- it drains the
    /// approval queue into the operator's inbox -- so it arrives as a hook
    /// rather than as something this type reaches for itself.
    parking: Option<Arc<dyn SeatParking>>,
}

/// What a host does with the approvals one seat's turn raised.
///
/// `true` when the seat is now waiting on a human and the episode must hold
/// it: the library stops proposing that seat, stops nudging it for silence,
/// and waits rather than treating the pause as an answer.
#[async_trait::async_trait]
pub trait SeatParking: Send + Sync {
    /// Park what `seat`'s turn left outstanding, and say whether it is held.
    async fn park(&self, seat: &str) -> bool;
}

impl std::fmt::Debug for DeskHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeskHost")
            .field("company", &self.company)
            .field("desk_id", &self.desk_id)
            .finish_non_exhaustive()
    }
}

impl DeskHost {
    /// Open this company's journal as one desk's episode host.
    #[must_use]
    pub fn new(
        company: CompanyId,
        desk_id: String,
        desk_name: String,
        events: Arc<dyn EventLog>,
    ) -> Self {
        let log = EventLogSessionLog::new(
            Arc::clone(&events),
            company.clone(),
            desk_id.clone(),
            desk_name,
        );
        Self {
            company,
            desk_id,
            thread_root: None,
            events,
            log,
            roster: None,
            pool: None,
            episode_id: String::new(),
            wave: AtomicU64::new(0),
            parking: None,
        }
    }

    /// Where a seat is built from: the company as it effectively stands,
    /// and what every one of its agents is built with.
    #[must_use]
    pub fn seating(mut self, record: Arc<CompanyRecord>, deps: Arc<HarnessDeps>) -> Self {
        self.roster = Some((record, deps));
        self
    }

    /// What to do with the approvals a seat's turn raised.
    #[must_use]
    pub fn parking(mut self, parking: Arc<dyn SeatParking>) -> Self {
        self.parking = Some(parking);
        self
    }

    /// The episode these turns belong to, as the console names it.
    #[must_use]
    pub fn episode(mut self, episode_id: impl Into<String>) -> Self {
        self.episode_id = episode_id.into();
        self
    }

    /// The pool whose per-teammate lock a seat's turn takes.
    ///
    /// Without one a turn runs unserialised, which is only safe for a host
    /// whose teammates sit on exactly one desk.
    #[must_use]
    pub fn locking(mut self, pool: Arc<HarnessPool>) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Thread every row this episode commits under `root`.
    #[must_use]
    pub const fn in_thread(mut self, root: Option<EventSeq>) -> Self {
        self.thread_root = root;
        self
    }

    /// Append one row to the company's journal, blocking the episode's task
    /// on the write.
    ///
    /// The port is async and the journal's seam is not, because the
    /// conductor hands the host one row at a time and takes its sequence
    /// straight back: there is never a second row in flight to interleave
    /// with. Run on the episode's own runtime.
    fn append(&self, event: CompanyEvent) -> crate::Result<EventSeq> {
        let events = Arc::clone(&self.events);
        let company = self.company.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(events.append(&company, event))
        })
    }

    /// One row as this company stores it.
    fn reply(
        &self,
        author: &str,
        text: String,
        thread: Option<Sequence>,
        only_for: Option<&str>,
    ) -> CompanyEvent {
        CompanyEvent::AgentReply {
            chat_id: self.desk_id.clone(),
            agent_id: author.to_owned(),
            text,
            steps: Vec::new(),
            outputs: Vec::new(),
            task_id: None,
            episode: None,
            // A row of a conversation hangs off the ask that rooted it;
            // otherwise off the thread the episode itself was opened in.
            parent: thread
                .map(|root| EventSeq::new(root.0))
                .or(self.thread_root),
            mentions: Vec::new(),
            mention_depth: 0,
            audience: only_for
                .map(|seat| vec![seat.to_owned()])
                .unwrap_or_default(),
        }
    }
}

/// The failure an episode reports when this company's journal refuses a row.
fn refused(error: &crate::error::OpenCompanyError) -> tinyhivemind_openhuman::Error {
    tinyhivemind_openhuman::Error::Harness(anyhow::anyhow!("{error}"))
}

impl Journal for DeskHost {
    fn log(&self) -> &dyn SessionLog {
        &self.log
    }

    fn commit(&self, commit: &Commit) -> tinyhivemind_openhuman::Result<Sequence> {
        let event = self.reply(
            &commit.author,
            commit.utterance.message().to_owned(),
            commit.thread,
            commit.only_for.as_deref(),
        );
        let seq = self.append(event).map_err(|error| refused(&error))?;
        Ok(Sequence(seq.value()))
    }

    /// A wave settled. The snapshot is the host's to keep; this one counts
    /// waves so a bracket can name the round it belongs to.
    fn checkpoint(
        &self,
        _state: &tinyhivemind_driver::ConductorState,
    ) -> tinyhivemind_openhuman::Result<()> {
        self.wave.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn note(&self, note: &Note) -> tinyhivemind_openhuman::Result<()> {
        let event = self.reply(
            DESK_AUTHOR,
            note.body.clone(),
            note.thread,
            note.only_for.as_deref(),
        );
        self.append(event).map_err(|error| refused(&error))?;
        Ok(())
    }
}

impl EpisodeHost for DeskHost {
    fn build_seat(
        &self,
        seat: &str,
        belt: EpisodeBelt,
    ) -> tinyhivemind_openhuman::Result<OpenHumanSessionHost> {
        // This company's own gate goes behind the episode's admission: a
        // call the episode does not serve is still the policy's to decide,
        // and can still park for the operator.
        let gate = belt.admit(None);
        let Some((record, deps)) = self.roster.as_ref() else {
            return Err(tinyhivemind_openhuman::Error::Harness(anyhow::anyhow!(
                "hive episode: no roster to seat `{seat}` from"
            )));
        };
        build_episode_seat(record, deps, seat, belt.tools, gate).map_err(|error| refused(&error))
    }

    fn tool_prefix(&self) -> String {
        TOOL_PREFIX.to_owned()
    }

    /// Bill the turn, then park whatever it left waiting on a human.
    ///
    /// Metering first, and unconditionally: a turn that ended by parking
    /// still spent tokens, and a seat the operator never gets back to would
    /// otherwise be free. The disposition then says whether the episode
    /// holds the seat -- which is the difference between a seat waiting on
    /// an approval and a seat that simply said nothing.
    fn after_turn(
        &self,
        seat: &str,
        usage: Option<&LastTurnUsage>,
    ) -> tinyhivemind_openhuman::Result<Disposition> {
        // Nothing here fails the turn: a spend this host could not record and
        // an approval it could not park are both worth a warning, not the
        // loss of work that already ran. So the block answers a disposition
        // rather than a result.
        let held = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                if let Some(usage) = usage {
                    self.meter(seat, usage).await;
                }
                match self.parking.as_ref() {
                    Some(parking) => parking.park(seat).await,
                    None => false,
                }
            })
        });
        Ok(if held {
            Disposition::Parked
        } else {
            Disposition::Done
        })
    }

    /// Run the seat's turn under this company's own machinery.
    ///
    /// The library builds the turn and does not start it; everything here
    /// wraps it, and the order is the whole point. The teammate's lock is
    /// taken first and released last, so a second desk wanting the same
    /// teammate waits at the door rather than running beside this one. Both
    /// bracket rows are written *inside* that lock: a bracket opened before
    /// it, or closed after it is released, overlaps its sibling on the
    /// journal exactly where the runtime did not, and overlap is the one
    /// invariant `opencompany measure` checks.
    ///
    /// A host with no pool runs unserialised and writes no brackets, which
    /// is what a test over the journal alone wants.
    fn wrap_turn<'a>(&'a self, seat: &'a str, turn: HostedTurn<'a>) -> HostedTurn<'a> {
        Box::pin(async move {
            let Some(pool) = self.pool.as_ref() else {
                return turn.await;
            };
            let Some(agent) = pool.agent(&self.company, seat).await else {
                return turn.await;
            };
            let bracket = Bracket {
                host: self,
                seat: seat.to_owned(),
                turn_id: crate::ports::generate_id(),
                wave: self.wave.load(Ordering::SeqCst),
            };
            let lock = agent.turn_lock();
            let held = lock.lock().await;
            bracket.started().await;
            let outcome = turn.await;
            bracket.settled(&outcome).await;
            drop(held);
            outcome
        })
    }
}

impl DeskHost {
    /// Bill what one turn spent, against the same ledger and meter an
    /// ordinary turn bills. A seat's tokens are the company's tokens.
    async fn meter(&self, seat: &str, usage: &LastTurnUsage) {
        let (Some(pool), Some((_, deps))) = (self.pool.as_ref(), self.roster.as_ref()) else {
            return;
        };
        let Some(agent) = pool.agent(&self.company, seat).await else {
            return;
        };
        let spent = [TurnUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cost_usd: usage.cost_usd,
        }];
        if let Err(error) = meter_turn_costs(
            &spent,
            seat,
            &self.company,
            deps,
            agent.chat_model().as_ref(),
            None,
        )
        .await
        {
            tracing::warn!(
                company = %self.company,
                %seat,
                %error,
                "[hive] could not meter a seat turn"
            );
        }
    }
}

/// One seat turn's pair of journal rows, written while the lock is held.
struct Bracket<'a> {
    host: &'a DeskHost,
    seat: String,
    turn_id: String,
    wave: u64,
}

impl Bracket<'_> {
    async fn started(&self) {
        self.write(CompanyEvent::TurnStarted {
            turn_id: self.turn_id.clone(),
            chat_id: self.host.desk_id.clone(),
            parent: self.host.thread_root,
            by: None,
            agent_id: Some(self.seat.clone()),
            episode_id: Some(self.host.episode_id.clone()),
            round_revision: Some(self.wave),
        })
        .await;
    }

    async fn settled(&self, outcome: &tinyhivemind_openhuman::Result<String>) {
        let event = match outcome {
            Ok(_) => CompanyEvent::TurnSettled {
                turn_id: self.turn_id.clone(),
                agent_id: Some(self.seat.clone()),
                chat_id: Some(self.host.desk_id.clone()),
                episode_id: Some(self.host.episode_id.clone()),
                round_revision: Some(self.wave),
                outcome: TurnOutcome::Committed,
            },
            Err(error) => CompanyEvent::TurnFailed {
                turn_id: self.turn_id.clone(),
                error: error.to_string(),
                agent_id: Some(self.seat.clone()),
                chat_id: Some(self.host.desk_id.clone()),
                episode_id: Some(self.host.episode_id.clone()),
                round_revision: Some(self.wave),
                // The library reports a timeout as an ordinary failure, so
                // this host does not claim to tell the two apart.
                outcome: Some(TurnOutcome::Failed),
            },
        };
        self.write(event).await;
    }

    /// A bracket that cannot be written is logged, never fatal: losing the
    /// console's lane is not worth losing the turn that ran.
    async fn write(&self, event: CompanyEvent) {
        if let Err(error) = self.host.events.append(&self.host.company, event).await {
            tracing::warn!(
                company = %self.host.company,
                seat = %self.seat,
                %error,
                "[hive] could not journal a seat turn's bracket"
            );
        }
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
