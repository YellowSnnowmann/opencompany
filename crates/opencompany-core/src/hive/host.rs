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

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use openhuman_core::agent::OpenHumanSessionHost;
use openhuman_core::agent::tinyagents::host::LastTurnUsage;
use tinyhivemind::{Sequence, SessionLog};
use tinyhivemind_driver::{Commit, Note};
use tinyhivemind_openhuman::{Disposition, EpisodeBelt, EpisodeHost, HostedTurn, Journal};
use tinyhivemind_openhuman::{Lane, TurnResult};
use tinyhivemind_tools::Refusal;

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
    /// How a desk reply's mentions are resolved and notified (#2441).
    ///
    /// A reply that names `@someone` is resolved against the company's own
    /// directory and the people it names are told. Journaled on the row, so
    /// a later reader takes the mentions that were stored rather than
    /// resolving the same text a second time -- which is how two answers to
    /// "who is `@ada`" arise.
    mentions: Option<crate::runtime::mention_seam::MentionSeam>,
    /// The channel each open conversation's rows are written to, by the ask
    /// row that roots it.
    ///
    /// Filled when the conversation opens. A row landing in a thread is
    /// looked up here rather than routed by the conversation marker it
    /// carries, because a hand-off a seat makes *while* talking carries that
    /// marker and belongs to the room -- routing on it would file public
    /// work as private.
    conversations: Mutex<BTreeMap<u64, String>>,
    /// Each seat's standing prompt, kept from when it was built.
    ///
    /// Read back on every turn after a seat's first: those turns are seeded
    /// from the journal rather than composed, and a seeded turn renders no
    /// system prompt of its own, so without this a teammate runs with the
    /// episode brief and no persona at all.
    personas: Mutex<BTreeMap<String, String>>,
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
        seats: Vec<String>,
    ) -> Self {
        // The seats reach the log because a conversation two of them open is
        // written to their own pair channel, and the log has to know which of
        // those belong to this desk's transcript.
        let log = EventLogSessionLog::new(
            Arc::clone(&events),
            company.clone(),
            desk_id.clone(),
            desk_name,
            seats,
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
            mentions: None,
            conversations: Mutex::new(BTreeMap::new()),
            personas: Mutex::new(BTreeMap::new()),
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

    /// Resolve and notify the mentions a seat's reply carries (#2441).
    #[must_use]
    pub fn resolving_mentions(
        mut self,
        mentions: crate::runtime::mention_seam::MentionSeam,
    ) -> Self {
        self.mentions = Some(mentions);
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

    /// The channel a committed row is filed under.
    ///
    /// Three cases, read straight off the commit:
    ///
    /// * an **ask** opens a conversation and is its first row, so it goes to
    ///   the pair channel it names -- the question belongs with its answer,
    ///   not in the room's own timeline;
    /// * a row **landing in a thread** is inside that conversation, and the
    ///   thread is the ask row that roots it;
    /// * everything else is the desk's.
    ///
    /// Routed on the thread, never on `Commit::conversation`. A hand-off a
    /// seat makes *while* talking carries that marker and lands with no
    /// thread, because it belongs to the room: routing on the marker would
    /// file public work where only two seats could read it, and nothing
    /// would say so.
    ///
    /// # Errors
    ///
    /// Why, for the caller to dress: a thread whose conversation this host
    /// never saw open. Deliberately an
    /// error rather than a fall back to the desk: falling back would publish
    /// a private row, which is the failure this whole arrangement exists to
    /// prevent.
    fn channel_for(&self, commit: &Commit) -> Result<String, String> {
        if let tinyhivemind::speech::Utterance::Ask { to, .. } = &commit.utterance {
            return Ok(crate::hive::referral::pair_conversation(&commit.author, to));
        }
        let Some(root) = commit.thread else {
            return Ok(self.desk_id.clone());
        };
        self.conversations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&root.0)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "a row landed in conversation {} that never opened here",
                    root.0
                )
            })
    }

    /// The mentions `text` names, resolved against this company (#2441).
    ///
    /// Empty without a seam, which is what a host built with no user
    /// directory to resolve against should journal.
    fn resolve(&self, author: &str, text: &str) -> Vec<crate::ports::types::Mention> {
        let Some(seam) = self.mentions.as_ref() else {
            return Vec::new();
        };
        let by = crate::ports::types::Actor {
            kind: crate::ports::types::ActorKind::Agent,
            id: author.to_owned(),
        };
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(seam.resolve_mentions(
                &self.company,
                text,
                None,
                Some(&by),
            ))
        })
    }

    /// Tell the people a stored row named.
    fn notify(&self, mentions: &[crate::ports::types::Mention], at: EventSeq) {
        let Some(seam) = self.mentions.as_ref() else {
            return;
        };
        if mentions.is_empty() {
            return;
        }
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(seam.notify_mentions(
                &self.company,
                mentions,
                &at,
                None,
                &self.desk_id,
            ));
        });
    }

    /// Append a row the episode reports, or say why it could not be.
    ///
    /// These rows describe what the conductor decided rather than what a
    /// seat said, so a journal that refuses one must not stop the episode:
    /// the room carries on, and the console is the poorer for it.
    fn journal_or_warn(&self, row: CompanyEvent) {
        let kind = row.kind();
        if let Err(error) = self.append(row) {
            tracing::warn!(
                company = %self.company,
                desk = %self.desk_id,
                %kind,
                %error,
                "[hive] could not journal a conductor event"
            );
        }
    }

    /// One row as this company stores it.
    fn reply(
        &self,
        chat: &str,
        author: &str,
        text: String,
        thread: Option<Sequence>,
        only_for: Option<&str>,
    ) -> CompanyEvent {
        CompanyEvent::AgentReply {
            chat_id: chat.to_owned(),
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

/// The placement of a broadcast, as a routing plan.
///
/// One recipient is a `One`; several are a `Hive` led by the first. Nobody
/// is a `Fallback` naming the author, which is what "it fit no seat, so the
/// work stays yours" means in this vocabulary.
fn plan_for(author: &str, took: &[String]) -> crate::hive::routing::RoutingPlanDto {
    use crate::hive::routing::RoutingPlanDto;
    match took {
        [] => RoutingPlanDto::Fallback {
            primary_id: author.to_owned(),
            reason: "unplaced".to_owned(),
        },
        [only] => RoutingPlanDto::One {
            primary_id: only.clone(),
        },
        [first, rest @ ..] => RoutingPlanDto::Hive {
            primary_id: first.clone(),
            invited_ids: rest.to_vec(),
        },
    }
}

/// Who an utterance names, for the row's own metadata.
///
/// Only the two acts that address a peer have any: a `dm` names several, an
/// `ask` names the one it opens a conversation with. Everything else is the
/// whole desk's, and an empty list is how this crate spells that.
fn recipients(utterance: &tinyhivemind::speech::Utterance) -> Vec<String> {
    use tinyhivemind::speech::Utterance;
    match utterance {
        Utterance::Dm { to, .. } => to.clone(),
        Utterance::Ask { to, .. } => vec![to.clone()],
        _ => Vec::new(),
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
        // The error is built here rather than inside, so the miss stays a
        // small `String` on a private signature (`clippy::result_large_err`).
        let chat = self.channel_for(commit).map_err(|why| {
            tinyhivemind_openhuman::Error::Harness(anyhow::anyhow!("hive episode: {why}"))
        })?;
        let mut event = self.reply(
            &chat,
            &commit.author,
            commit.utterance.message().to_owned(),
            commit.thread,
            commit.only_for.as_deref(),
        );
        // A committed row is an episode's row, and says so. Without this the
        // console cannot tell one from an ordinary chat reply, the
        // utterance-kind histogram reads nothing, and the chat history has no
        // way to group a desk's episode rows. The note rows this host also
        // writes are the conductor speaking, not a seat, and carry none.
        if let CompanyEvent::AgentReply { episode, .. } = &mut event {
            *episode = Some(crate::ports::types::ReplyEpisode {
                id: self.episode_id.clone(),
                revision: self.wave.load(Ordering::SeqCst),
                kind: crate::ports::types::UtteranceKind::of(&commit.utterance),
                to: recipients(&commit.utterance),
                // Filled in by whoever records the routing, not here: the
                // conductor places a broadcast after the row is committed.
                routed_by: None,
            });
        }
        // A reply that names somebody resolves those names against this
        // company's directory before it is stored, and the people it names
        // are told after -- the notification needs the row's sequence, which
        // only exists once it is appended (#2441).
        let mentions = self.resolve(&commit.author, commit.utterance.message());
        if let CompanyEvent::AgentReply {
            mentions: stored, ..
        } = &mut event
        {
            stored.clone_from(&mentions);
        }
        let seq = self.append(event).map_err(|error| refused(&error))?;
        self.notify(&mentions, seq);
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

    /// A turn came back. Nothing here changes the episode; it is how an
    /// operator finds out why a room went quiet.
    fn turn_done(
        &self,
        seat: &str,
        lane: Lane,
        outcome: &TurnResult,
        refused: &[Refusal],
        recorded: usize,
    ) {
        let where_ = match lane {
            Lane::Desk => String::new(),
            Lane::Thread(root) => format!(" in thread {}", root.0),
        };
        match outcome {
            TurnResult::Replied(_) if recorded == 0 => tracing::warn!(
                company = %self.company, %seat,
                "[hive] @{seat}{where_} replied but recorded nothing"
            ),
            TurnResult::Replied(_) => {}
            TurnResult::Failed(error) => {
                tracing::warn!(
                    company = %self.company, %seat, %error,
                    "[hive] @{seat}{where_} failed"
                );
            }
            TurnResult::Parked => tracing::info!(
                company = %self.company, %seat,
                "[hive] @{seat}{where_} is waiting on the operator"
            ),
        }
        for refusal in refused {
            tracing::warn!(
                company = %self.company, %seat,
                tool = %refusal.tool, reason = %refusal.reason,
                "[hive] a call was refused"
            );
        }
    }

    /// What the conductor decided, as this company's journal records it.
    ///
    /// The library reports a whole vocabulary here -- a broadcast placed, a
    /// conversation opened and concluded, a seat nudged, parked, resumed,
    /// refused, discharged -- and until now this host implemented none of
    /// it, silently taking the default that does nothing. Everything the
    /// console would need to show *why* a room moved was dropped on the
    /// floor.
    ///
    /// Two of them have a company row already and are written here. The rest
    /// do not, and inventing rows for them is a bigger decision than this
    /// wiring: a conversation in particular has no `ConversationOpened` /
    /// `Concluded` pair, which is what a UI would bind an agent-to-agent
    /// thread to. The reply rows carry the conversation faithfully -- an ask
    /// row names its recipient and narrows its audience, and the rows of the
    /// conversation hang off it -- so what is missing is the lifecycle, not
    /// the content.
    fn event(&self, event: &tinyhivemind_driver::Event) {
        use tinyhivemind_driver::Event;
        // The conversation reference rows. They sit on the desk and point at
        // the exchange rather than carrying it, so the room's own timeline
        // stays the room's, and the console -- already subscribed to this
        // desk -- can raise the "two seats are talking" indicator without
        // watching every pair channel for one to start.
        if let Event::Asked { seat, askee, root } = event {
            let conversation_id = crate::hive::referral::pair_conversation(seat, askee);
            let row = CompanyEvent::ConversationOpened {
                chat_id: self.desk_id.clone(),
                episode_id: self.episode_id.clone(),
                conversation_id: conversation_id.clone(),
                root: root.0,
                asker: seat.clone(),
                askee: askee.clone(),
            };
            self.conversations
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(root.0, conversation_id);
            self.journal_or_warn(row);
            return;
        }
        if let Event::Concluded {
            root,
            asker,
            askee,
            forced,
            ..
        } = event
        {
            let row = CompanyEvent::ConversationConcluded {
                chat_id: self.desk_id.clone(),
                episode_id: self.episode_id.clone(),
                conversation_id: crate::hive::referral::pair_conversation(asker, askee),
                root: root.0,
                asker: asker.clone(),
                askee: askee.clone(),
                forced: *forced,
            };
            self.journal_or_warn(row);
            return;
        }
        let (seat, at, took) = match event {
            // A broadcast reached these seats. Without this the console can
            // see that a broadcast was said and not who picked it up.
            Event::Broadcast { seat, to, at } => (seat, at, to.clone()),
            // A broadcast that fit nobody is still a routing outcome, and
            // the author keeping its own work is the thing worth showing.
            Event::Unplaced { seat, at } => (seat, at, Vec::new()),
            _ => return,
        };
        let row = CompanyEvent::BroadcastRouted {
            chat_id: self.desk_id.clone(),
            episode_id: self.episode_id.clone(),
            revision: self.wave.load(Ordering::SeqCst),
            agent_id: seat.clone(),
            message_seq: at.0,
            plan: plan_for(seat, &took),
            // The conductor placed this one; a router's own probabilities
            // ride the opening plan, not a hand-off.
            probabilities: None,
            router: crate::hive::routing::Router::Fallback,
        };
        self.journal_or_warn(row);
    }

    fn note(&self, note: &Note) -> tinyhivemind_openhuman::Result<()> {
        let event = self.reply(
            &self.desk_id.clone(),
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
        let Some((record, deps)) = self.roster.as_ref() else {
            return Err(tinyhivemind_openhuman::Error::Harness(anyhow::anyhow!(
                "hive episode: no roster to seat `{seat}` from"
            )));
        };
        // The belt goes through whole, admission and all: this company's own
        // gate is only knowable once the teammate is built, and it has to go
        // *behind* the episode's admission rather than be replaced by it.
        let (session, persona) =
            build_episode_seat(record, deps, seat, belt).map_err(|error| refused(&error))?;
        self.personas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seat.to_owned(), persona);
        Ok(session)
    }

    fn persona(&self, seat: &str) -> Option<String> {
        self.personas
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(seat)
            .cloned()
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

#[cfg(test)]
#[path = "host_mentions_tests.rs"]
mod mentions_tests;
