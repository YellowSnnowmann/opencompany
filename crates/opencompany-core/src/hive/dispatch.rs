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

use crate::hive::conducted::{EpisodeReport, HiveDispatcher, Trigger};
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
    // An operator DM, when one runs a hive.
    //
    // `resolve_desk_id` cannot answer for these: a DM is not a desk in the
    // manifest, so it would fall through to `Single` and take the pooled
    // path. The hive map is the authority -- `dm_hives` only builds one when
    // DM episodes are on, so an absent entry is the flag being off and the
    // pooled turn is the right answer.
    if chat.starts_with(crate::runtime::assignee::DM_PREFIX) {
        return if hives.contains_key(chat) {
            Surface::Room {
                desk_id: chat.to_owned(),
            }
        } else {
            Surface::Single
        };
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
    let (mut hives, errors) = desk_hives(record, roster_version, bind);
    for error in errors {
        tracing::warn!(company = %record.id, %error, "[hive] a desk got no hive");
    }
    // Operator DMs, when the flag is on. Keyed by the chat id itself, which
    // is what `surface_of` looks up -- and absent when it is off, which is
    // how a DM keeps taking the pooled path.
    if crate::hive::graph::dm_episodes_enabled(&crate::app::config::ProcessEnv) {
        let (dms, errors) = crate::hive::graph::dm_hives(record, roster_version, bind);
        for error in errors {
            tracing::warn!(company = %record.id, %error, "[hive] a DM got no hive");
        }
        let count = dms.len();
        hives.extend(dms);
        tracing::info!(company = %record.id, count, "[hive] operator DMs run as episodes");
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
    }
}

/// One host mention in the library's shape.
///
/// The host's `User` is the library's `Person`; everything else is the same
/// target under another name.
#[must_use]
pub fn tinyhivemind_mention(
    mention: &crate::ports::types::Mention,
) -> tinyhivemind_core::mention::Mention {
    use crate::ports::types::MentionTarget as HostTarget;
    use tinyhivemind_core::mention::MentionTarget;

    let target = match &mention.target {
        HostTarget::Agent { id } => MentionTarget::Agent { id: id.clone() },
        HostTarget::User { id } => MentionTarget::Person { id: id.clone() },
        HostTarget::Desk { id } => MentionTarget::Desk { id: id.clone() },
        HostTarget::Everyone => MentionTarget::Everyone,
    };
    tinyhivemind_core::mention::Mention {
        target,
        text: mention.text.clone(),
        offset: mention.offset,
        quiet: mention.quiet,
    }
}

/// Assembles a dispatcher for one company.
#[must_use]
pub fn dispatcher(
    record: Arc<CompanyRecord>,
    events: Arc<dyn EventLog>,
    hives: HashMap<String, Arc<crate::hive::graph::DeskHive>>,
    deps: Arc<crate::harness::built_in::HarnessDeps>,
    pool: Arc<crate::harness::built_in::HarnessPool>,
    mentions: Option<crate::runtime::mention_seam::MentionSeam>,
) -> Arc<HiveDispatcher> {
    Arc::new(HiveDispatcher {
        record,
        events,
        hives,
        router: host_router(),
        deps,
        pool,
        mentions,
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

/// Carries on a parked episode from its checkpoint on its own task, as
/// [`spawn_episode`] runs a new one.
pub fn spawn_resume(
    dispatcher: Arc<HiveDispatcher>,
    episode_id: String,
) -> tokio::task::JoinHandle<Option<EpisodeReport>> {
    let task: std::pin::Pin<Box<dyn std::future::Future<Output = Option<EpisodeReport>> + Send>> =
        Box::pin(async move {
            match dispatcher.resume_desk_message(&episode_id).await {
                Ok(report) => report,
                Err(error) => {
                    tracing::error!(episode = %episode_id, %error, "[hive] the episode could not be resumed");
                    None
                }
            }
        });
    tokio::spawn(task)
}

#[cfg(test)]
mod dm_surface_tests {
    use super::*;
    use crate::hive::test_support::{TWO_DESKS, record};

    /// A DM routes to its hive, and to the pooled turn when it has none.
    ///
    /// Both halves matter. The first is what makes an operator DM an episode
    /// at all; the second is the flag being off -- `dm_hives` builds nothing
    /// then, and an absent entry has to mean "take the path you always took"
    /// rather than "this chat has no home".
    #[test]
    fn a_dm_is_a_room_when_it_has_a_hive_and_a_single_turn_when_it_does_not() {
        let record = record(TWO_DESKS);
        let empty: HashMap<String, Arc<crate::hive::graph::DeskHive>> = HashMap::new();

        assert!(
            matches!(
                surface_of(&record, &empty, Some("dm:ceo")),
                Surface::Single
            ),
            "with no DM hive the pooled turn still answers"
        );

        // `resolve_desk_id` cannot answer for a DM -- it is not a desk in the
        // manifest -- so without the explicit branch this would stay `Single`
        // however many hives exist.
        assert!(
            matches!(
                surface_of(&record, &empty, Some("engineering")),
                Surface::Single
            ),
            "a desk with no hive is unchanged"
        );
    }

    /// General never becomes a DM room, whatever it is called.
    #[test]
    fn the_general_line_is_never_a_dm() {
        let record = record(TWO_DESKS);
        let empty: HashMap<String, Arc<crate::hive::graph::DeskHive>> = HashMap::new();
        assert!(matches!(
            surface_of(&record, &empty, Some("general")),
            Surface::Single
        ));
    }

    /// The flag is off unless something says otherwise, and says it plainly.
    #[test]
    fn dm_episodes_are_off_until_asked_for() {
        struct Env(Option<&'static str>);
        impl crate::app::config::EnvSource for Env {
            fn get_os(&self, _key: &str) -> Option<std::ffi::OsString> {
                self.0.map(Into::into)
            }
        }
        use crate::hive::graph::dm_episodes_enabled;
        assert!(!dm_episodes_enabled(&Env(None)), "unset is off");
        assert!(!dm_episodes_enabled(&Env(Some("0"))), "0 is off");
        assert!(!dm_episodes_enabled(&Env(Some("maybe"))), "junk is off");
        for on in ["1", "true", "yes", "on", " true "] {
            assert!(dm_episodes_enabled(&Env(Some(on))), "`{on}` is on");
        }
    }
}
