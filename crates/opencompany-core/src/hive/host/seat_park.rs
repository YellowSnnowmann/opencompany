//! What one seat turn claims, what it leaves waiting on the operator, and how
//! the operator's decision reaches the seat again.
//!
//! A seat turn runs on the episode's own task, outside every chat cycle, so it
//! takes its own claims: an approval bucket keyed by its episode-seat turn key,
//! the explicit-request guard, a publish claim and an output claim. `after_turn`
//! drains the approval bucket and parks it through the runtime's shared
//! [`ApprovalParker`], under that same turn key, so the resolve path can find
//! its way back to this episode. Publishing from a seat is refused in-turn:
//! nothing files an episode's publishes yet, and a seat told its file was filed
//! would repeat that to the operator.

use std::sync::PoisonError;

use tinyhivemind::Sequence;

use super::{DESK_AUTHOR, DeskHost, SeatParking};
use crate::harness::built_in::policy::{
    ApprovalClaim, ApprovalRequest, ApprovalRequestQueue, ApprovalScope, DrainedRequests,
    MAX_APPROVAL_REQUESTS_PER_TURN,
};
use crate::harness::built_in::publish::{PendingPublishQueue, PublishClaim, PublishDestination};
use crate::harness::built_in::turn_outputs::TurnOutputClaim;
use crate::ports::types::{ApprovalId, ChatOutput, CompanyEvent, CompanyId, EventSeq, StoredEvent};
use crate::runtime::approval_park::{ApprovalParker, ParkSite};
use crate::runtime::episode_resume::{EpisodeReleases, SeatDecision, turn_key};
use crate::runtime::journal::{ApprovalConversation, TaskLink};

/// The shared queues a seat turn claims its buckets on.
#[derive(Clone)]
pub(crate) struct SeatQueues {
    pub(crate) approvals: ApprovalRequestQueue,
    pub(crate) publishes: PendingPublishQueue,
}

impl SeatQueues {
    /// Opens one seat turn's claims.
    pub(crate) fn claim(&self, episode_id: &str, seat: &str) -> SeatClaims {
        SeatClaims {
            approvals: self
                .approvals
                .claim(ApprovalScope::Seat(turn_key(episode_id, seat))),
            queue: self.approvals.clone(),
            publish: self.publishes.claim(PublishDestination::Unclaimed),
            outputs: self.publishes.output_collector().claim(),
            collected: Vec::new(),
        }
    }
}

/// One seat turn's claims, held from the turn's start until `after_turn`.
pub(crate) struct SeatClaims {
    approvals: ApprovalClaim,
    queue: ApprovalRequestQueue,
    publish: PublishClaim,
    outputs: TurnOutputClaim,
    collected: Vec<ChatOutput>,
}

/// What a seat turn left behind once its claims are released.
pub(crate) struct SettledTurn {
    requests: DrainedRequests,
    publishes: usize,
    outputs: usize,
}

impl SeatClaims {
    /// Runs `turn` inside every claim, and keeps what it produced.
    pub(crate) async fn run<F, T>(&mut self, turn: F) -> T
    where
        F: std::future::Future<Output = T> + Send,
    {
        let outcome = Box::pin(
            self.approvals.scoped(
                self.queue
                    .turn_scoped(self.publish.scoped(self.outputs.scoped(turn))),
            ),
        )
        .await;
        self.collected = self.outputs.drain();
        outcome
    }

    /// Drains the claims and releases them.
    pub(crate) fn settle(self) -> SettledTurn {
        SettledTurn {
            requests: self.approvals.drain(MAX_APPROVAL_REQUESTS_PER_TURN),
            publishes: self.publish.drain().len(),
            outputs: self.collected.len(),
        }
    }
}

/// What parking one seat's requests came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeatParked {
    /// The approvals now on the operator's queue.
    pub parked: Vec<ApprovalId>,
    /// Why each request that did not park failed, in words for the seat.
    pub refused: Vec<String>,
}

/// Parks a seat's requests on this company's approval gate, under the seat's
/// episode turn key and the desk's conversation.
pub struct EpisodeSeatParking {
    parker: ApprovalParker,
    company: CompanyId,
    desk_id: String,
    thread_root: Option<EventSeq>,
    episode_id: String,
}

impl EpisodeSeatParking {
    /// Parking for one episode on one desk.
    #[must_use]
    pub fn new(
        parker: ApprovalParker,
        company: CompanyId,
        desk_id: String,
        thread_root: Option<EventSeq>,
        episode_id: String,
    ) -> Self {
        Self {
            parker,
            company,
            desk_id,
            thread_root,
            episode_id,
        }
    }
}

#[async_trait::async_trait]
impl SeatParking for EpisodeSeatParking {
    async fn park(&self, seat: &str, requests: Vec<ApprovalRequest>) -> SeatParked {
        let mut outcome = SeatParked::default();
        for request in requests {
            let site = ParkSite {
                task: TaskLink::Unlinked,
                conversation: ApprovalConversation {
                    thread: Some(self.desk_id.clone()),
                    parent: self.thread_root,
                },
                turn: Some(turn_key(&self.episode_id, seat)),
            };
            match self.parker.park(&self.company, request.effect, site).await {
                Ok(id) => outcome.parked.push(id),
                Err(error) => {
                    tracing::error!(
                        company = %self.company,
                        episode = %self.episode_id,
                        %seat,
                        tool = %request.tool,
                        %error,
                        "[hive] a seat's approval request could not be parked"
                    );
                    outcome.refused.push(format!(
                        "Your `{}` request could not be put in front of the operator ({error}), \
                         so nobody was asked. Do not say it is waiting on them.",
                        request.tool
                    ));
                }
            }
        }
        outcome
    }
}

impl DeskHost {
    /// The queues seat turns claim on: this host's own, else the roster's.
    pub(super) fn seat_queues(&self) -> Option<SeatQueues> {
        self.queues.clone().or_else(|| {
            self.roster.as_ref().map(|(_, deps)| SeatQueues {
                approvals: deps.approval_requests.clone(),
                publishes: deps.pending_publishes.clone(),
            })
        })
    }

    /// Where decisions for parked seats arrive: this host's own registry,
    /// else the company's.
    pub(crate) fn seat_releases(&self) -> Option<EpisodeReleases> {
        self.releases.clone().or_else(|| {
            self.roster
                .as_ref()
                .map(|(_, deps)| deps.approval_requests.grants().episode_releases())
        })
    }

    /// Reads back what a resumed episode had open: its conversations, the
    /// conversation each still-parked seat is waiting in, and the wave.
    pub(crate) fn recall(&self, rows: &[StoredEvent], revision: u64) {
        let mut conversations = self
            .conversations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut lanes = self
            .parked_lanes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for stored in rows {
            match &stored.event {
                CompanyEvent::ConversationOpened {
                    episode_id,
                    conversation_id,
                    root,
                    ..
                } if *episode_id == self.episode_id => {
                    conversations.insert(*root, conversation_id.clone());
                }
                CompanyEvent::EpisodeSeatParked {
                    episode_id,
                    seat,
                    thread,
                    ..
                } if *episode_id == self.episode_id => {
                    lanes.insert(seat.clone(), thread.map(Sequence));
                }
                CompanyEvent::EpisodeSeatResumed {
                    episode_id, seat, ..
                } if *episode_id == self.episode_id => {
                    lanes.remove(seat);
                }
                _ => {}
            }
        }
        self.wave.store(
            revision.saturating_add(1),
            std::sync::atomic::Ordering::SeqCst,
        );
    }

    /// Claims seat turns on `approvals` rather than on the roster's.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn claiming(mut self, approvals: ApprovalRequestQueue) -> Self {
        self.queues = Some(SeatQueues {
            approvals,
            publishes: PendingPublishQueue::default(),
        });
        self
    }

    /// Takes decisions for parked seats from `releases`.
    #[must_use]
    pub fn releasing(mut self, releases: EpisodeReleases) -> Self {
        self.releases = Some(releases);
        self
    }

    pub(super) fn keep_seat_claims(&self, seat: &str, claims: SeatClaims) {
        self.seat_claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seat.to_owned(), claims);
    }

    pub(super) fn take_seat_claims(&self, seat: &str) -> Option<SeatClaims> {
        self.seat_claims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(seat)
    }

    /// Parks what a seat's turn raised, telling the seat about anything that
    /// did not reach the operator. `true` when something parked.
    pub(super) async fn park_seat(&self, seat: &str, settled: SettledTurn) -> bool {
        let SettledTurn {
            requests,
            publishes,
            outputs,
        } = settled;
        let mut problems = Vec::new();
        if publishes > 0 {
            problems.push(format!(
                "{publishes} file(s) you published in this room were not filed anywhere. Tell \
                 the operator plainly that they were not delivered."
            ));
        }
        if outputs > 0 {
            tracing::debug!(
                company = %self.company,
                episode = %self.episode_id,
                %seat,
                outputs,
                "[hive] a seat turn's outputs are not attached to any row"
            );
        }
        if let Some(notice) = requests.overflow_notice() {
            problems.push(notice);
        }
        let mut parked = Vec::new();
        if !requests.requests.is_empty() {
            match self.parking.as_ref() {
                Some(parking) => {
                    let outcome = parking.park(seat, requests.requests).await;
                    parked = outcome.parked;
                    problems.extend(outcome.refused);
                }
                None => {
                    tracing::error!(
                        company = %self.company,
                        episode = %self.episode_id,
                        %seat,
                        count = requests.requests.len(),
                        "[hive] a seat raised approvals on a desk that cannot park them"
                    );
                    problems.push(
                        "What you asked the operator for could not be recorded here, so nobody \
                         was asked. Do not say it is waiting on them."
                            .to_owned(),
                    );
                }
            }
        }
        if !problems.is_empty() {
            let body = problems.join("\n\n");
            if let Err(error) = self.append_note(seat, None, body).await {
                tracing::error!(
                    company = %self.company,
                    %seat,
                    %error,
                    "[hive] could not tell a seat its request did not reach the operator"
                );
            }
        }
        if parked.is_empty() {
            return false;
        }
        tracing::info!(
            company = %self.company,
            episode = %self.episode_id,
            %seat,
            approvals = parked.len(),
            "[hive] a seat parked on the operator"
        );
        self.parked_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seat.to_owned(), parked);
        true
    }

    /// The row saying `seat` parked, in the conversation it parked in.
    pub(super) fn seat_parked_row(&self, seat: &str, thread: Option<Sequence>) -> CompanyEvent {
        self.parked_lanes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(seat.to_owned(), thread);
        let approval_ids = self
            .parked_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(seat)
            .unwrap_or_default();
        CompanyEvent::EpisodeSeatParked {
            chat_id: self.desk_id.clone(),
            episode_id: self.episode_id.clone(),
            seat: seat.to_owned(),
            thread: thread.map(|root| root.0),
            approval_ids,
        }
    }

    /// Tells `seat` what the operator decided, where it parked.
    pub(super) async fn tell_decisions(
        &self,
        seat: &str,
        decisions: &[SeatDecision],
    ) -> crate::Result<EventSeq> {
        let lane = self
            .parked_lanes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(seat)
            .flatten();
        let body = decisions
            .iter()
            .map(SeatDecision::note)
            .collect::<Vec<_>>()
            .join("\n\n");
        tracing::info!(
            company = %self.company,
            episode = %self.episode_id,
            %seat,
            decisions = decisions.len(),
            "[hive] a parked seat was released by the operator"
        );
        self.append_note(seat, lane, body).await
    }

    /// A note to one seat, in the conversation `lane` roots, else on the desk.
    async fn append_note(
        &self,
        seat: &str,
        lane: Option<Sequence>,
        body: String,
    ) -> crate::Result<EventSeq> {
        let chat = lane.and_then(|root| {
            self.conversations
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&root.0)
                .cloned()
        });
        let (chat, thread) = match chat {
            Some(chat) => (chat, lane),
            None => (self.desk_id.clone(), None),
        };
        let event = self.reply(&chat, DESK_AUTHOR, body, thread, Some(seat));
        self.events.append(&self.company, event).await
    }
}

#[cfg(test)]
#[path = "seat_park_tests.rs"]
mod tests;
