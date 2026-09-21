//! Where a chat message goes: the surface it lands on, and the episode it
//! opens when that surface is a desk with a room.
//!
//! This is the chat body of the harness brain's cycle (plan hive-desks,
//! Phase 5). A message on a **desk of two or more bound seats** opens (or
//! joins) an episode on that desk's hive and is answered by the rounds the
//! driver runs; the brain pushes no bubble for it, because every seat's
//! utterance is already a journaled `AgentReply` and the console sees each one
//! land live. Every other surface — a DM, `#general`, a workflow thread, a
//! desk of one — is one ordinary turn on the responder the host's own rules
//! pick: the teammate the message named, else the desk's default responder,
//! else the orchestrator. No router and no driver touch those.
//!
//! The dispatcher for a company is built from the harness pool's live agents
//! per message rather than cached: a hive is a validation and a handful of
//! `Arc` clones, and a roster or desk change is then in force on the next
//! message with nothing to invalidate.

use std::collections::HashMap;
use std::sync::Arc;

use tinyhivemind_embed::Router;

use crate::hive::driver::{EpisodeReport, HiveDispatcher, SeatRunner, Trigger};
use crate::hive::graph::desk_hives;
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyRecord, EventSeq, Mention};

/// The surface a chat message landed on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Surface {
    /// A desk (or a thread in one) whose hive runs episodes.
    Room {
        /// The canonical desk id.
        desk_id: String,
    },
    /// Everything else: one turn on one responder.
    Single,
}

/// Which surface `chat` is, given the hives this company runs.
#[must_use]
pub fn surface_of(
    record: &CompanyRecord,
    hives: &HashMap<String, Arc<crate::hive::graph::DeskHive>>,
    chat: Option<&str>,
) -> Surface {
    let Some(chat) = chat else {
        return Surface::Single;
    };
    if crate::server::chat_history::is_general_chat(Some(chat)) {
        return Surface::Single;
    }
    match record.resolve_desk_id(chat) {
        Some(desk_id) if hives.contains_key(&desk_id) => Surface::Room { desk_id },
        _ => Surface::Single,
    }
}

/// Builds the company's hives over the agents `bind` resolves.
#[must_use]
pub fn hives_for(
    record: &CompanyRecord,
    bind: &dyn Fn(&str) -> Option<openhuman_embed::Agent>,
) -> HashMap<String, Arc<crate::hive::graph::DeskHive>> {
    // Echoed by a Jev evaluation and compared within one request; a
    // per-build counter would be no more meaningful than the roster size.
    let roster_version = record.effective_agents().len() as u64;
    let (hives, errors) = desk_hives(record, roster_version, bind);
    for error in errors {
        tracing::warn!(company = %record.id, %error, "[hive] a desk got no hive");
    }
    hives
}

/// The Jev router this host routes with, if a TinyHumans key resolves.
#[must_use]
pub fn host_router() -> Option<Arc<dyn Router>> {
    match crate::hive::jev::jev_router(&crate::app::config::ProcessEnv, None) {
        Ok(Some(router)) => Some(Arc::new(router)),
        Ok(None) => {
            tracing::info!("[hive] no TinyHumans key: desks route by lead and mention");
            None
        }
        Err(error) => {
            tracing::warn!(%error, "[hive] the Jev router is misconfigured; routing by lead and mention");
            None
        }
    }
}

/// The message that opens or joins an episode, from a journaled operator
/// message.
#[must_use]
pub fn trigger_for(
    seq: Option<EventSeq>,
    text: &str,
    parent: Option<EventSeq>,
    mentions: &[Mention],
) -> Trigger {
    Trigger {
        seq: seq.unwrap_or(EventSeq::new(0)),
        text: text.to_string(),
        parent,
        mentions: mentions.to_vec(),
        hop: 0,
        origin: None,
        referred_from: None,
    }
}

/// Assembles a dispatcher for one company.
#[must_use]
pub fn dispatcher(
    record: Arc<CompanyRecord>,
    events: Arc<dyn EventLog>,
    hives: HashMap<String, Arc<crate::hive::graph::DeskHive>>,
    seats: Arc<dyn SeatRunner>,
    runs: Option<Arc<dyn crate::ports::RunStore>>,
) -> Arc<HiveDispatcher> {
    Arc::new(HiveDispatcher {
        record,
        events,
        hives,
        router: host_router(),
        seats,
        runs,
    })
}

/// Drives the episode on its own task and returns at once.
///
/// The cycle that accepted the message must not wait on the room: rounds
/// run for as long as the seats take, the operator's request is already
/// journaled, and the console follows the episode frames live. The task is
/// type-erased for the reason `driver::spawn_desk_message` is.
pub fn spawn_episode(
    dispatcher: Arc<HiveDispatcher>,
    desk_id: String,
    trigger: Trigger,
) -> tokio::task::JoinHandle<Option<EpisodeReport>> {
    let task: std::pin::Pin<Box<dyn std::future::Future<Output = Option<EpisodeReport>> + Send>> =
        Box::pin(async move {
            match dispatcher.run_desk_message(&desk_id, trigger).await {
                Ok(report) => Some(report),
                Err(error) => {
                    tracing::warn!(desk = %desk_id, %error, "[hive] the episode failed");
                    None
                }
            }
        });
    tokio::spawn(task)
}
