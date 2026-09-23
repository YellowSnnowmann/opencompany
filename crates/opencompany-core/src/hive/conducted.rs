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

use std::sync::Arc;

use tinyhivemind::SESSION_WINDOW;
use tinyhivemind_driver::{BoundHive, BroadcastRouting, CompletionDriver, ConductPolicy, Door};
use tinyhivemind_embed::Router;
use tinyhivemind_openhuman::{HostedRunner, Report, SeatRunner, run_episode};
use tinyhivemind_tools::EpisodeTools;

use crate::error::{OpenCompanyError, Result};
use crate::harness::built_in::{HarnessDeps, HarnessPool};
use crate::hive::graph::DeskHive;
use crate::hive::host::{DeskHost, SeatParking};
use crate::hive::routing::EffectiveRouting;
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyRecord, EventSeq};

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
