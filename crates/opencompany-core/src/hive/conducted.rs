//! One completion episode, run on `tinyhivemind`'s own loop.
//!
//! This is the whole of the episode path. It builds the door the operator's
//! message opens, seats each teammate as a session host of its own, and
//! calls [`run_episode`]. Everything between -- who speaks next, what a
//! committed row means, the private conversations a seat opens, the nudges,
//! the walls, the completion fold, parking on an approval, and the
//! checkpoint a restart resumes from -- belongs to the library.
//!
//! What stays here is what the library cannot know: the journal rows are
//! this company's, the seats are its teammates, and a turn still takes the
//! teammate's lock and writes its brackets. Those reach the loop through
//! [`DeskHost`].

use std::collections::HashMap;
use std::sync::Arc;

use tinyhivemind::SESSION_WINDOW;
use tinyhivemind_driver::{BoundHive, BroadcastRouting, CompletionDriver, ConductPolicy, Door};
use tinyhivemind_embed::Router;
use tinyhivemind_openhuman::{HostedRunner, Report, SeatRunner, run_episode};
use tinyhivemind_tools::EpisodeTools;

use crate::error::{OpenCompanyError, Result};
use crate::harness::built_in::{HarnessDeps, HarnessPool};
use crate::hive::episode_store;
use crate::hive::graph::DeskHive;
use crate::hive::host::{DeskHost, SeatParking};
use crate::hive::routing::{EffectiveRouting, RoutingPlanDto, desk_routing, router_of};
use crate::ports::events::EventLog;
use crate::ports::types::CompanyEvent;
use crate::ports::types::{CompanyRecord, EventSeq, Mention};

/// Everything this company brings to one episode.
///
/// A struct rather than a dozen positional arguments, most of them the same
/// shape.
pub struct Episode<'a> {
    /// The company as it effectively stands, for seating a teammate.
    pub record: Arc<CompanyRecord>,
    /// What every one of its agents is built from.
    pub deps: Arc<HarnessDeps>,
    /// The pool its teammates live in, for the lock a turn holds.
    pub pool: Arc<HarnessPool>,
    /// The company's durable journal.
    pub events: Arc<dyn EventLog>,
    /// The desk this episode runs on.
    pub desk: &'a DeskHive,
    /// The routing this desk resolved: round width, and the policy a
    /// handoff is placed under.
    pub routing: &'a EffectiveRouting,
    /// The semantic router, when a credential resolved one. `None` places a
    /// handoff by lead and mention instead of by meaning.
    pub router: Option<&'a (dyn Router + 'a)>,
    /// This episode's id, as the console names it.
    pub episode_id: String,
    /// The thread the episode's rows are parented to.
    pub thread_root: Option<EventSeq>,
    /// The operator's row: where the episode opens.
    pub opened_at: EventSeq,
    /// The seats the opening routing plan named. Empty starts the desk's
    /// first member.
    pub starters: Vec<String>,
    /// What this company does with the approvals a turn raised.
    pub parking: Option<Arc<dyn SeatParking>>,
}

/// Run one completion episode to quiescence.
///
/// # Errors
///
/// [`OpenCompanyError::Harness`] for a desk that seats nobody, a seat that
/// cannot be built, a journal that refuses a row, an episode that stalls or
/// runs past its wall, or one parked on the operator with nobody released.
pub async fn run(episode: Episode<'_>) -> Result<Report> {
    let members: Vec<String> = episode.desk.hive.members().map(str::to_owned).collect();
    if members.is_empty() {
        return Err(OpenCompanyError::Harness(format!(
            "desk `{}` seats nobody",
            episode.desk.desk_id
        )));
    }
    let starters = if episode.starters.is_empty() {
        vec![members[0].clone()]
    } else {
        episode.starters.clone()
    };

    let host = Arc::new({
        let mut host = DeskHost::new(
            episode.record.id.clone(),
            episode.desk.desk_id.clone(),
            episode.desk.desk_name.clone(),
            Arc::clone(&episode.events),
        )
        .in_thread(episode.thread_root)
        .episode(episode.episode_id.clone())
        .seating(Arc::clone(&episode.record), Arc::clone(&episode.deps))
        .locking(Arc::clone(&episode.pool));
        if let Some(parking) = episode.parking.clone() {
            host = host.parking(parking);
        }
        host
    });

    // Each seat is built once, here, and torn down with the episode: its
    // belt carries the episode's tools, which are bound to this seat of this
    // episode and to nothing else.
    let runner = HostedRunner::seat(
        Arc::clone(&host),
        Arc::new(EpisodeTools::new(members.iter().cloned())),
        &members,
        &episode.desk.desk_id,
        &episode.desk.desk_name,
        SESSION_WINDOW,
    )
    .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;

    // The desk's graph is the same one the pool's hive carries; only the
    // bindings differ, because these seats are this episode's sessions
    // rather than the pool's long-lived handles.
    let hive = BoundHive::new(episode.desk.hive.graph().clone(), runner.bindings())
        .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
    let driver = CompletionDriver::new(&hive, episode.routing.round_width)
        .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
    let route_policy = episode.routing.policy();

    run_episode(
        host.as_ref(),
        &runner,
        &driver,
        BroadcastRouting {
            // Threaded through deliberately: without it a handoff is still
            // placed, but by lead and mention rather than by meaning, and
            // nothing anywhere reports the difference.
            primary: episode.router,
            reasoning: None,
            policy: &route_policy,
            roster_version: episode.desk.roster_version,
            thread_context: &[],
        },
        ConductPolicy::default(),
        Door {
            chat: episode.desk.desk_id.clone(),
            desk_name: episode.desk.desk_name.clone(),
            members,
            starters,
            opened_at: tinyhivemind::Sequence(episode.opened_at.value()),
        },
    )
    .await
    .map_err(|error| OpenCompanyError::Harness(error.to_string()))
}

/// What opened an episode: the operator's row and what it said.
#[derive(Clone, Debug)]
pub struct Trigger {
    /// The row the message was journaled at.
    pub seq: EventSeq,
    /// What it said.
    pub text: String,
    /// The thread it was sent in, when it was sent in one.
    pub parent: Option<EventSeq>,
    /// Who it named.
    pub mentions: Vec<Mention>,
}

/// What one episode came to, in this host's words.
#[derive(Clone, Copy, Debug)]
pub struct EpisodeReport {
    /// Seat turns run.
    pub turns: u64,
    /// Waves proposed.
    pub waves: u64,
    /// Conversations concluded between seats.
    pub conversations: usize,
    /// Seats that reported their work finished.
    pub settled: usize,
}

impl From<Report> for EpisodeReport {
    fn from(report: Report) -> Self {
        Self {
            turns: report.turns,
            waves: report.waves,
            conversations: report.conversations,
            settled: report.settled,
        }
    }
}

/// One company's desks, and what it takes to run an episode on any of them.
pub struct HiveDispatcher {
    /// The company as it effectively stands.
    pub record: Arc<CompanyRecord>,
    /// Its durable journal.
    pub events: Arc<dyn EventLog>,
    /// Its bound desks, by id.
    pub hives: HashMap<String, Arc<DeskHive>>,
    /// The semantic router, when a credential resolved one.
    pub router: Option<Arc<dyn Router>>,
    /// What its agents are built from.
    pub deps: Arc<HarnessDeps>,
    /// The pool they live in, for the lock a turn holds.
    pub pool: Arc<HarnessPool>,
}

impl HiveDispatcher {
    /// The hive bound to `desk_id`, when that desk runs one.
    #[must_use]
    pub fn hive(&self, desk_id: &str) -> Option<Arc<DeskHive>> {
        self.hives.get(desk_id).cloned()
    }

    /// Whether `desk_id` runs episodes at all.
    #[must_use]
    pub fn runs_episodes(&self, desk_id: &str) -> bool {
        self.hives.contains_key(desk_id)
    }

    /// Open one episode for an operator message and run it to quiescence.
    ///
    /// # Errors
    ///
    /// A desk that runs no hive, and whatever stops the episode.
    pub async fn run_desk_message(&self, desk_id: &str, trigger: Trigger) -> Result<EpisodeReport> {
        let desk = self.hive(desk_id).ok_or_else(|| {
            OpenCompanyError::InvalidRequest(format!("desk `{desk_id}` runs no hive"))
        })?;
        let thread_root = trigger.parent.unwrap_or(trigger.seq);
        let routing = desk_routing(&self.record, desk_id);
        let (starters, plan_dto) = self.opening(&desk, &routing, &trigger, thread_root).await?;
        let episode_id = uuid::Uuid::new_v4().simple().to_string();
        self.events
            .append(
                &self.record.id,
                CompanyEvent::EpisodeOpened {
                    chat_id: desk.desk_id.clone(),
                    episode_id: episode_id.clone(),
                    opened_by_seq: trigger.seq.value(),
                    parent: Some(thread_root),
                    participants: starters.clone(),
                    plan: plan_dto.clone(),
                    hop: 0,
                },
            )
            .await?;
        let report = run(Episode {
            record: Arc::clone(&self.record),
            deps: Arc::clone(&self.deps),
            pool: Arc::clone(&self.pool),
            events: Arc::clone(&self.events),
            desk: &desk,
            routing: &routing,
            router: self.router.as_deref(),
            episode_id: episode_id.clone(),
            thread_root: Some(thread_root),
            opened_at: trigger.seq,
            starters,
            parking: None,
        })
        .await?;
        // The episode's closing row. `run_episode` returns only once every
        // seat has recorded its part -- a wall, a stall or a fold it could
        // not explain comes back as an error instead, and is journaled by
        // whoever handles it -- so a return here *is* `complete_episode`.
        //
        // Without this the console sees an episode that opened and never
        // closed: `EpisodeOpened` was written above, `episode_store` folds
        // the pair, and the operator's list would hold it open forever.
        self.events
            .append(
                &self.record.id,
                CompanyEvent::EpisodeCompleted {
                    chat_id: desk.desk_id.clone(),
                    episode_id: episode_id.clone(),
                    revision: report.waves,
                    // The library reports what happened, not who spoke last:
                    // every seat completed, so no one seat closed it.
                    completed_by: None,
                    rounds: u32::try_from(report.waves).unwrap_or(u32::MAX),
                    reason: crate::ports::types::EpisodeReason::CompleteEpisode,
                    summary_seq: None,
                },
            )
            .await?;
        tracing::info!(
            desk = %desk_id,
            episode = %episode_id,
            turns = report.turns,
            waves = report.waves,
            "[hive] episode finished"
        );
        Ok(report.into())
    }

    /// Who the opening routing plan starts, or the desk in order when no
    /// router answered.
    async fn opening(
        &self,
        desk: &DeskHive,
        routing: &EffectiveRouting,
        trigger: &Trigger,
        thread_root: EventSeq,
    ) -> Result<(Vec<String>, RoutingPlanDto)> {
        let lead = desk.lead().ok_or_else(|| {
            OpenCompanyError::Harness(format!("desk `{}` has no seats", desk.desk_id))
        })?;
        let explicit = trigger.mentions.iter().find_map(|mention| {
            desk.hive
                .members()
                .find(|member| match &mention.target {
                    crate::ports::types::MentionTarget::Agent { id } => *member == id,
                    _ => false,
                })
                .map(str::to_owned)
        });
        let request = desk.hive.desk_request(
            trigger.text.clone(),
            Vec::new(),
            Some(tinyhivemind::Sequence(thread_root.value())),
            desk.roster_version,
            routing.policy(),
        );
        let plan = desk
            .hive
            .route_desk(
                self.router.as_deref(),
                None,
                &request,
                explicit.as_deref(),
                &lead,
            )
            .await
            .map_err(|error| OpenCompanyError::Harness(error.to_string()))?;
        tracing::debug!(desk = %desk.desk_id, router = ?router_of(&plan), "[hive] opening routed");
        let dto = RoutingPlanDto::from(&plan);
        let mut starters = dto.agent_ids();
        if starters.is_empty() {
            // A clarification is a routing answer the room cannot act on:
            // the lead answers, and asks if it must.
            starters.push(lead);
        }
        // Deliberately *not* padded to the desk's `round_width`. That width
        // bounds how many recipients a broadcast may be placed to and how
        // many queued handoffs a seat may hold; it does not size a wave. A
        // wave is whoever is due, and who opens is the routing plan's answer,
        // not a number this host applies to it.
        Ok((starters, dto))
    }
}

/// The journal as the episode store, for a follow-up that joins an open one.
pub use episode_store::open_episode_for;
