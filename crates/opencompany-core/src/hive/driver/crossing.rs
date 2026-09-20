//! The referral half of the episode host: deciding a crossing from a round's
//! utterances, opening the far desk's episode on its own task, and carrying
//! the answer home to reopen the seat that asked.
//!
//! Split from `driver.rs` along the seam the plan draws (Phase 6): everything
//! here leaves the room, everything there stays in it.

use std::sync::Arc;

use tinyhivemind_openhuman::CompletionDriver;

use super::{EpisodeReport, EpisodeRun, HiveDispatcher, Trigger, episode_lock};
use crate::error::Result;
use crate::hive::episode_store;
use crate::hive::referral::{DeskReferral, ReturnAddress};
use crate::hive::round::{RoundOutcome, SeatAssignment};
use crate::hive::routing::EffectiveRouting;
use crate::ports::types::CompanyEvent;

impl HiveDispatcher {
    /// Decides and dispatches the crossings a round's utterances raise.
    async fn refer(&self, run: &EpisodeRun, outcome: &RoundOutcome, routing: &EffectiveRouting) -> Result<()> {
        let policy = routing.referral_policy();
        if !policy.enabled {
            return Ok(());
        }
        let members = crate::runtime::delegation_tools::tinyhivemind_roster(&self.record);
        let snapshots = crate::runtime::delegation_tools::tinyhivemind_desks(&self.record);
        let desks = snapshots.set();
        let roster = tinyhivemind_core::roster::Roster::new(&members, &[], &[]);
        let queue = crate::hive::referral::JournalReferralQueue::new(self.events.as_ref(), &self.record.id);
        for record in &outcome.records {
            let Some(text) = outcome.texts.get(&record.agent_id) else {
                continue;
            };
            let mentions = tinyhivemind_core::mention::resolve(
                text,
                None,
                &tinyhivemind_core::mention::MentionAuthor::Agent {
                    id: record.agent_id.clone(),
                },
                &roster,
                &desks,
            );
            if mentions.is_empty() {
                continue;
            }
            let input = tinyhivemind::referral::ReferralInput {
                key: tinyhivemind::dispatch::DispatchKey {
                    trigger_sequence: record.sequence,
                },
                conversation: tinyhivemind::dispatch::DispatchConversation {
                    desk_id: run.desk.desk_id.clone(),
                    thread_root: Some(run.thread_root.value()),
                },
                author_id: record.agent_id.clone(),
                content: text.clone(),
                mentions,
                hop: run.hop,
                origin: None,
            };
            if let Err(error) =
                tinyhivemind::referral::dispatch_referral(&queue, policy, &input, &roster, &desks)
                    .await
            {
                tracing::warn!(episode = %run.episode_id, %error, "[hive] referral decision failed");
            }
        }
        for referral in queue.drain() {
            let crossing = DeskReferral::from_referral(
                &referral,
                &self.record,
                &run.episode_id,
                Some(run.thread_root),
                policy.returns,
            );
            self.dispatch_referral(crossing).await?;
        }
        Ok(())
    }

    /// Opens the far desk's episode on a crossing: seeds the question as a
    /// row under [`HIVE_REFERRAL_AUTHOR`](crate::hive::referral::HIVE_REFERRAL_AUTHOR),
    /// journals the forward marker, and drives it as its own task so the
    /// asking desk's rounds are not held on it.
    async fn dispatch_referral(&self, crossing: DeskReferral) -> Result<()> {
        if !self.runs_episodes(&crossing.to_desk) {
            tracing::info!(
                to_desk = %crossing.to_desk,
                "[hive] referral target runs no hive; the question stays on its desk"
            );
            return Ok(());
        }
        let seed = self
            .events
            .append(
                &self.record.id,
                CompanyEvent::AgentReply {
                    chat_id: crossing.to_desk.clone(),
                    agent_id: crate::hive::referral::HIVE_REFERRAL_AUTHOR.to_string(),
                    text: crossing.seed_text(),
                    steps: Vec::new(),
                    outputs: Vec::new(),
                    task_id: None,
                    parent: None,
                    mentions: Vec::new(),
                    mention_depth: 0,
                    audience: Vec::new(),
                    episode: None,
                },
            )
            .await?;
        let forward = self
            .events
            .append(&self.record.id, crossing.forward_event(None))
            .await?;
        let trigger = Trigger {
            seq: seed,
            text: crossing.content.clone(),
            parent: None,
            mentions: Vec::new(),
            hop: crossing.hop,
            origin: crossing.return_address(forward),
            referred_from: Some(crossing.from_desk_name.clone()),
        };
        spawn_desk_message(self.clone_for_task(), crossing.to_desk.clone(), trigger);
        Ok(())
    }

    /// Carries a completed referral's answer home and reopens the seat that
    /// asked.
    async fn deliver_answer(&self, run: &EpisodeRun, origin: &ReturnAddress, report: &EpisodeReport) -> Result<()> {
        let answered_by = report
            .completed_by
            .clone()
            .unwrap_or_else(|| run.desk.desk_id.clone());
        let answer = report
            .summary
            .clone()
            .unwrap_or_else(|| format!("(the desk closed without an answer: {:?})", report.reason));
        let text = crate::hive::referral::returned_note(&answered_by, &run.desk.desk_name, &answer);
        let answer_seq = self
            .events
            .append(
                &self.record.id,
                CompanyEvent::AgentReply {
                    chat_id: origin.desk.clone(),
                    agent_id: crate::hive::referral::HIVE_REFERRAL_AUTHOR.to_string(),
                    text,
                    steps: Vec::new(),
                    outputs: Vec::new(),
                    task_id: None,
                    parent: origin.thread_root,
                    mentions: Vec::new(),
                    mention_depth: 0,
                    audience: Vec::new(),
                    episode: None,
                },
            )
            .await?;
        self.events
            .append(
                &self.record.id,
                crate::hive::referral::return_event(
                    &self.record,
                    origin,
                    &run.desk.desk_id,
                    &answered_by,
                    answer_seq,
                    &run.episode_id,
                ),
            )
            .await?;
        let Some(desk) = self.hive(&origin.desk) else {
            return Ok(());
        };
        let routing = crate::hive::routing::desk_routing(&self.record, &desk.desk_id);
        let Some(persisted) =
            episode_store::latest_state(self.events.as_ref(), &self.record.id, &origin.episode_id)
                .await?
        else {
            return Ok(());
        };
        let mut home = self.resume_from(&desk, persisted, &routing).await?;
        if home
            .state
            .episode()
            .participants
            .iter()
            .all(|participant| participant.agent_id != origin.asker)
        {
            return Ok(());
        }
        home.reassign(&origin.asker, answer_seq, &routing)?;
        home.assignments.insert(
            origin.asker.clone(),
            SeatAssignment::answer(&run.desk.desk_name, answer_seq),
        );
        spawn_drive(self.clone_for_task(), home);
        Ok(())
    }

}

/// Drives a desk message on its own task. Type-erased so the referral chain
/// — an episode that opens an episode that answers back into an episode —
/// is not one infinitely recursive future type.
fn spawn_desk_message(dispatcher: Arc<HiveDispatcher>, desk_id: String, trigger: Trigger) {
    let task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        Box::pin(async move {
            if let Err(error) = dispatcher.run_desk_message(&desk_id, trigger).await {
                tracing::warn!(desk = %desk_id, %error, "[hive] a referred episode failed");
            }
        });
    tokio::spawn(task);
}

/// Drives a reopened episode on its own task, carrying its answer home when
/// it has one. Type-erased for the reason [`spawn_desk_message`] is.
fn spawn_drive(dispatcher: Arc<HiveDispatcher>, mut home: EpisodeRun) {
    let task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        Box::pin(async move {
            let lock = episode_lock(&dispatcher.record.id, &home.desk.desk_id, home.thread_root);
            let _driving = lock.lock().await;
            match dispatcher.drive(&mut home).await {
                Ok(report) => {
                    if let Some(origin) = home.origin.clone()
                        && let Err(error) = dispatcher.deliver_answer(&home, &origin, &report).await
                    {
                        tracing::warn!(%error, "[hive] an answer could not be carried home");
                    }
                }
                Err(error) => tracing::warn!(%error, "[hive] a reopened episode failed"),
            }
        });
    tokio::spawn(task);
}

