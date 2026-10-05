//! The operator's line into the company hive (OC-2).
//!
//! An operator message on a desk, on `#general` or in a teammate's DM is no
//! longer answered by a turn this cycle runs: it is handed to the company
//! hive's Coordinator with `send_as_host`, and the turns that answer it run on
//! the hive's own task. This file is that hand-off — where the message goes,
//! who starts it, and the `HiveAccepted` row that records it — plus the lookup
//! that tells a hive turn's parked approval from a cycle's.

use std::sync::Arc;

use tinyhivemind_hives::{Destination, Error as HiveError, SendMessage};

use super::HarnessBrain;
use crate::error::{OpenCompanyError, Result};
use crate::ports::general_channel::GENERAL_CHANNEL_ID;
use crate::ports::types::{ApprovalId, CompanyEvent, EventSeq, Mention};
use crate::runtime::hive_resume::HiveSeat;

/// Where an operator line goes in the hive, and who starts it.
struct Addressed {
    destination: Destination,
    starters: Vec<String>,
    route: Option<&'static str>,
    /// The manifest teammate of a direct line.
    direct_to: Option<String>,
}

impl HarnessBrain {
    /// Hands one operator message to the company hive. `Ok(false)` when the
    /// company runs no hive (no store wired), and the caller answers with a
    /// pooled turn instead.
    ///
    /// The Coordinator message id is `op:{seq}` — the journal line it was said
    /// as — so a redelivered cycle resends idempotently, and a resend whose
    /// starters differ (Jev routed it differently the second time) is
    /// recognised as already accepted rather than as a conflict.
    pub(super) async fn send_to_hive(
        &self,
        event_seq: Option<EventSeq>,
        text: &str,
        chat: Option<&str>,
        parent: Option<EventSeq>,
        mentions: &[Mention],
    ) -> Result<bool> {
        let record = self.record();
        let Some(hive) = self.pool.hive(&record.id).await else {
            return Ok(false);
        };
        let chat_id = chat.unwrap_or(GENERAL_CHANNEL_ID).to_string();
        let addressed = self.address(&hive, &chat_id, text, mentions).await?;
        let mut thread = None;
        if matches!(addressed.destination, Destination::Hive(_))
            && let Some(parent) = parent
        {
            thread = hive.projector().hive_sequence_of(parent).await;
        }
        let message_id = match event_seq {
            Some(seq) => format!("op:{}", seq.value()),
            None => format!("op:{}", uuid::Uuid::new_v4().simple()),
        };
        let send = |thread: Option<u64>| SendMessage {
            message_id: message_id.clone(),
            sender: String::new(),
            destination: addressed.destination.clone(),
            body: text.to_string(),
            thread,
            only_for: Vec::new(),
            starters: addressed.starters.clone(),
        };
        let coordinator = hive.coordinator();
        let receipt = match coordinator.send_as_host(send(thread)).await {
            Ok(receipt) => receipt,
            // The line answers a thread the hive cannot see from here (a row it
            // never journaled, or another desk's): said at the top level rather
            // than lost.
            Err(HiveError::InvalidThread(_)) => coordinator
                .send_as_host(send(None))
                .await
                .map_err(hive_error)?,
            // Backpressure: the teammate already has as many messages waiting
            // as the hive holds for one agent. Said to the operator in the
            // chat rather than failed, so they know to wait, not resend.
            Err(HiveError::InboxFull { agent_id, limit }) => {
                let who = hive.manifest_id(&agent_id);
                self.notice_in(
                    &chat_id,
                    format!(
                        "{who} already has {limit} messages waiting and is not taking more yet. \
                         Your message was not delivered — send it again once they have caught up."
                    ),
                )
                .await;
                return Ok(true);
            }
            Err(HiveError::MessageConflict(_)) => {
                tracing::info!(
                    company = %record.id,
                    %message_id,
                    "[hive] an operator line the hive already accepted was redelivered; not \
                     sending it twice"
                );
                return Ok(true);
            }
            Err(error) => return Err(hive_error(error)),
        };
        hive.projector()
            .note_host_line(
                receipt.sequence,
                &chat_id,
                event_seq,
                addressed
                    .direct_to
                    .as_deref()
                    .and_then(|agent| hive.coordinator_id(agent))
                    .as_deref(),
            )
            .await;
        if let Some(events) = self.deps.events.as_ref() {
            let starters = addressed
                .starters
                .iter()
                .map(|coordinator_id| hive.manifest_id(coordinator_id))
                .collect();
            if let Err(error) = events
                .append(
                    &record.id,
                    CompanyEvent::HiveAccepted {
                        message_id,
                        sequence: receipt.sequence,
                        chat_id,
                        source: event_seq,
                        starters,
                        route: addressed.route.map(str::to_string),
                    },
                )
                .await
            {
                tracing::warn!(
                    company = %record.id,
                    %error,
                    "[hive] the hive accepted an operator line but its acceptance row could not \
                     be journaled"
                );
            }
        }
        Ok(true)
    }

    /// The destination and starters of a line said on `chat_id`: a desk (or
    /// `#general`) is its hive, started by the mention / Jev / default ladder;
    /// anything else that names a teammate is that teammate's direct line.
    async fn address(
        &self,
        hive: &Arc<crate::hive::runtime::CompanyHive>,
        chat_id: &str,
        text: &str,
        mentions: &[Mention],
    ) -> Result<Addressed> {
        let record = self.record();
        let desk = if chat_id == GENERAL_CHANNEL_ID {
            Some(GENERAL_CHANNEL_ID.to_string())
        } else if crate::runtime::assignee::dm_key(chat_id).is_none() {
            record.resolve_desk_id(chat_id)
        } else {
            None
        };
        if let Some(desk) = desk {
            let router = crate::hive::route::host_router(&record.id, self.deps.secrets.as_ref()).await;
            let starters = crate::hive::route::choose(
                &record,
                &desk,
                text,
                mentions,
                router.as_deref(),
                &self.orchestrator(),
            )
            .await;
            return Ok(Addressed {
                destination: Destination::Hive(crate::hive::hive_id_for_chat(&desk)),
                starters: starters
                    .members
                    .iter()
                    .filter_map(|member| hive.coordinator_id(member))
                    .collect(),
                route: Some(starters.route.as_str()),
                direct_to: None,
            });
        }
        let agent = self.responder_for(Some(chat_id));
        let coordinator_id = hive.coordinator_id(&agent).ok_or_else(|| {
            OpenCompanyError::InvalidRequest(format!(
                "`{agent}` is not on this company's hive, so a message to it has nowhere to go"
            ))
        })?;
        Ok(Addressed {
            destination: Destination::Agent(coordinator_id),
            starters: Vec::new(),
            route: None,
            direct_to: Some(agent),
        })
    }

    /// Says `text` in `chat_id` as the system, outside any turn.
    async fn notice_in(&self, chat_id: &str, text: String) {
        let Some(events) = self.deps.events.as_ref() else {
            return;
        };
        let event = CompanyEvent::AgentReply {
            chat_id: chat_id.to_string(),
            agent_id: crate::ports::SYSTEM_AUTHOR.to_string(),
            text,
            steps: Vec::new(),
            outputs: Vec::new(),
            task_id: None,
            parent: None,
            mentions: Vec::new(),
            mention_depth: 0,
            audience: Vec::new(),
            hive: None,
        };
        if let Err(error) = events.append(&self.record().id, event).await {
            tracing::warn!(%error, "[hive] a hive notice could not be journaled");
        }
    }

    /// The hive turn that parked `approval_id`, if a coordinator turn did.
    pub(super) fn hive_seat_of(&self, approval_id: &ApprovalId) -> Option<HiveSeat> {
        self.deps
            .approval_parker
            .as_ref()?
            .turn_of(approval_id)
            .as_deref()
            .and_then(crate::runtime::hive_resume::parse)
    }
}

fn hive_error(error: HiveError) -> OpenCompanyError {
    OpenCompanyError::Harness(format!("company hive: {error}"))
}
