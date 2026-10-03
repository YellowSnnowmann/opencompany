//! The cards one episode opens, and the card each seat's publish lands on.
//!
//! A room answers one operator message with several seats, so the board has
//! to be held to the message rather than to the seat: at most
//! [`EPISODE_CARD_CAP`] cards, one per normalized title, the card the REST
//! handler already opened for the message taken over by the first
//! `spawn_task` rather than duplicated, and every publish filed on a card the
//! episode already has before a new one is minted.
//!
//! [`EpisodeCards`] is that bookkeeping. It is the [`CardBudget`] a seat's
//! `spawn_task` reserves against in its own turn, and [`EpisodeCards::open`]
//! writes what the turn queued once it settles. A resumed episode reads its
//! cards back with [`EpisodeCards::recall`] so the limits survive the restart.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};

use crate::harness::built_in::card_budget::{CardBudget, CardRefusal, normalize_title};
use crate::harness::orchestrator::Delegation;
use crate::ports::tasks::{COLUMN_TODO, TaskOpener};
use crate::ports::types::{CompanyId, CompanyRecord, EventSeq};
use crate::ports::{TaskOrigin, TaskRecord, TaskStore, now_millis};
use crate::runtime::spawn_card::SpawnCard;

/// What a seat is told about opening cards, appended to its persona.
///
/// The roster briefs that describe the board are stripped from a seat
/// (`seat_persona`), so this is the only place a seat learns what
/// `spawn_task` does in a room.
///
/// # It must agree with the tool's own receipt
///
/// The receipt `spawn_task` hands a seat promises a reply **only when the write
/// fails** ("you will be told here if it cannot be"), because the card is
/// written after the turn ends and a turn that ends is not told anything more.
/// This note used to say "do not describe it as open until you are told it is",
/// which asks the seat to wait for a confirmation that never arrives on the
/// happy path. A live run showed what that costs: a seat with the card safely
/// queued would not claim it, and attributed the work to a teammate that never
/// took a turn.
///
/// So the two have to describe one contract, and the honest one is the
/// receipt's: silence means it landed. The seat is told that here and told it
/// again in the receipt, which is the only place a failure can reach it.
pub(crate) const SEAT_CARDS_NOTE: &str = "\n\n## Opening cards\n\nNothing said here is \
tracked unless somebody opens a card for it. When the operator asks for real work, open one with \
`spawn_task`: one card per piece of work, at most three for this whole conversation across every \
teammate in it, and never a second card for work that already has one. The card is written when \
your turn ends: you will hear back only if it could not be, so unless you are told otherwise it \
is open and you may say so. Do not say it is already on the board while your turn is still \
running. If the operator only asked a question, open nothing. Handing a card to somebody else is \
not available here: `desk_ask` the teammate who should take it.";

/// The most cards one episode may open.
pub(crate) const EPISODE_CARD_CAP: usize = 3;

/// One episode's cards, shared by every seat in it.
#[derive(Default)]
pub(crate) struct EpisodeCards {
    state: Mutex<State>,
    /// Held across a seat's whole settle, so two seats finishing together
    /// cannot both mint the message card.
    writing: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    /// Normalized titles held or opened.
    titles: BTreeSet<String>,
    /// Cards held or opened, the handler card counted once it is taken over.
    count: usize,
    /// The card that answers the operator's message.
    message_card: Option<String>,
    /// The handler card while nobody has taken it over.
    adoptable: Option<String>,
    /// Each seat's cards, oldest first.
    by_seat: BTreeMap<String, Vec<String>>,
    /// The cards already counted, so reading the board again counts nothing
    /// twice.
    known: BTreeSet<String>,
    /// Whether the board has been read at all.
    looked: bool,
}

/// Where an episode's cards are written, and who they belong to.
pub(crate) struct CardDesk<'a> {
    pub(crate) tasks: &'a dyn TaskStore,
    pub(crate) company: &'a CompanyId,
    pub(crate) record: Option<&'a CompanyRecord>,
    pub(crate) desk_id: &'a str,
    pub(crate) thread_root: Option<EventSeq>,
    pub(crate) episode_id: &'a str,
    pub(crate) opened_at: Option<EventSeq>,
}

/// What writing one seat's queued cards came to.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Opened {
    /// The cards opened or taken over, in order.
    pub(crate) cards: Vec<String>,
    /// What the seat must be told, one line per card that did not land.
    pub(crate) problems: Vec<String>,
}

impl CardBudget for EpisodeCards {
    fn reserve(&self, title: &str) -> Result<(), CardRefusal> {
        let mut state = self.state();
        let key = normalize_title(title);
        if state.titles.contains(&key) {
            return Err(CardRefusal::Duplicate);
        }
        if state.count >= EPISODE_CARD_CAP {
            return Err(CardRefusal::Full {
                cap: EPISODE_CARD_CAP,
            });
        }
        state.titles.insert(key);
        state.count += 1;
        Ok(())
    }

    fn release(&self, title: &str) {
        let mut state = self.state();
        if state.titles.remove(&normalize_title(title)) {
            state.count = state.count.saturating_sub(1);
        }
    }
}

impl EpisodeCards {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the lock a seat's settle writes under.
    pub(crate) async fn writing(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.writing.lock().await
    }

    /// Reads back what the board already holds for this episode: the cards
    /// its seats opened, and the card that answers its message.
    pub(crate) fn recall(
        &self,
        cards: &[TaskRecord],
        desk_id: &str,
        episode_id: &str,
        opened_at: Option<EventSeq>,
    ) {
        let mut state = self.state();
        state.looked = true;
        for card in cards {
            if let Some(opener) = card.opened_by.as_ref()
                && opener.episode_id.as_deref() == Some(episode_id)
                && state.known.insert(card.id.clone())
            {
                if state.titles.insert(normalize_title(&card.title)) {
                    state.count += 1;
                }
                state
                    .by_seat
                    .entry(opener.agent_id.clone())
                    .or_default()
                    .push(card.id.clone());
            }
            if opened_at.is_some()
                && card.origin_message_seq == opened_at
                && state.message_card.is_none()
            {
                state.message_card = Some(card.id.clone());
                if is_adoptable(card, desk_id) {
                    state.adoptable = Some(card.id.clone());
                }
            }
        }
        tracing::debug!(
            episode = %episode_id,
            cards = state.count,
            message_card = ?state.message_card,
            "[hive] recalled the episode's cards"
        );
    }

    /// The card a seat's publish lands on: its own latest card, else the
    /// message's.
    pub(crate) fn publish_target(&self, seat: &str) -> Option<String> {
        let state = self.state();
        state
            .by_seat
            .get(seat)
            .and_then(|cards| cards.last().cloned())
            .or_else(|| state.message_card.clone())
    }

    /// Records the card a publish minted as the one answering the message.
    pub(crate) fn minted_message_card(&self, id: &str) {
        let mut state = self.state();
        if state.message_card.is_none() {
            state.message_card = Some(id.to_owned());
        }
    }

    /// Reads the board for this episode's cards, and again while the
    /// message's card has not been found.
    pub(crate) async fn look(&self, at: &CardDesk<'_>) {
        {
            let state = self.state();
            if state.looked && (state.message_card.is_some() || at.opened_at.is_none()) {
                return;
            }
        }
        match at.tasks.list(at.company).await {
            Ok(cards) => self.recall(&cards, at.desk_id, at.episode_id, at.opened_at),
            Err(error) => tracing::warn!(
                company = %at.company,
                episode = %at.episode_id,
                %error,
                "[hive] could not read the board for the episode's cards"
            ),
        }
    }

    /// Writes what `seat`'s turn queued.
    ///
    /// Each card was reserved when the seat called `spawn_task`; a card that
    /// cannot be written gives its reservation back and becomes a line for
    /// the seat. Anything but a card is logged and dropped: the seat's claim
    /// refuses every other delegation before it is queued.
    pub(crate) async fn open(
        &self,
        at: &CardDesk<'_>,
        seat: &str,
        delegations: Vec<Delegation>,
    ) -> Opened {
        let mut opened = Opened::default();
        if delegations.is_empty() {
            return opened;
        }
        self.look(at).await;
        for delegation in delegations {
            let Delegation::SpawnTask {
                title,
                note,
                assignee,
            } = delegation
            else {
                tracing::warn!(
                    company = %at.company,
                    episode = %at.episode_id,
                    %seat,
                    "[hive] a seat queued a delegation other than a card; dropped"
                );
                continue;
            };
            let owner = assignee
                .as_deref()
                .and_then(|name| {
                    at.record.map(|record| {
                        crate::runtime::assignee::resolve(record, name)
                            .canonical()
                            .unwrap_or_default()
                            .to_owned()
                    })
                })
                .unwrap_or_default();
            let opener = TaskOpener {
                agent_id: seat.to_owned(),
                episode_id: Some(at.episode_id.to_owned()),
            };
            let written = match self.take_adoptable() {
                Some(id) => {
                    match adopt(at, &id, seat, &title, note.as_deref(), &owner, &opener).await {
                        Ok(true) => Ok(id),
                        Ok(false) => mint(at, title.clone(), note, owner, opener).await,
                        Err(error) => Err(error),
                    }
                }
                None => mint(at, title.clone(), note, owner, opener).await,
            };
            match written {
                Ok(id) => {
                    tracing::info!(
                        company = %at.company,
                        episode = %at.episode_id,
                        %seat,
                        task_id = %id,
                        "[hive] a seat's card is on the board"
                    );
                    let mut state = self.state();
                    state.known.insert(id.clone());
                    state
                        .by_seat
                        .entry(seat.to_owned())
                        .or_default()
                        .push(id.clone());
                    drop(state);
                    opened.cards.push(id);
                }
                Err(error) => {
                    self.release(&title);
                    tracing::error!(
                        company = %at.company,
                        episode = %at.episode_id,
                        %seat,
                        %error,
                        "[hive] a seat's queued card could not be written"
                    );
                    opened.problems.push(format!(
                        "The card \"{title}\" you queued could not be written to the board \
                         ({error}). It is not tracked. Tell the operator plainly."
                    ));
                }
            }
        }
        opened
    }

    fn take_adoptable(&self) -> Option<String> {
        self.state().adoptable.take()
    }
}

/// Whether the handler card is still nobody's: in To-do, and owned by no
/// teammate (blank, or the desk it was addressed to).
fn is_adoptable(card: &TaskRecord, desk_id: &str) -> bool {
    card.opened_by.is_none()
        && card.column == COLUMN_TODO
        && (card.assignee.is_empty() || card.assignee == desk_id)
}

/// Takes over the handler card for a seat's first card. `false` when it has
/// moved on since it was read, so the caller mints instead.
async fn adopt(
    at: &CardDesk<'_>,
    id: &str,
    seat: &str,
    title: &str,
    note: Option<&str>,
    owner: &str,
    opener: &TaskOpener,
) -> crate::Result<bool> {
    let Some(observed) = at
        .tasks
        .list(at.company)
        .await?
        .into_iter()
        .find(|card| card.id == id)
    else {
        return Ok(false);
    };
    if !is_adoptable(&observed, at.desk_id) {
        return Ok(false);
    }
    let mut card = observed.clone();
    card.assignee = if owner.is_empty() {
        seat.to_owned()
    } else {
        owner.to_owned()
    };
    let line = match note {
        Some(note) => format!("Tracking this as \"{title}\". {note}"),
        None => format!("Tracking this as \"{title}\"."),
    };
    card.note = Some(crate::runtime::delegation::append_note(
        card.note.as_deref(),
        seat,
        &line,
    ));
    card.opened_by = Some(opener.clone());
    card.updated_at_millis = now_millis();
    at.tasks
        .update_if_column(at.company, &card, &observed, COLUMN_TODO)
        .await
}

async fn mint(
    at: &CardDesk<'_>,
    title: String,
    note: Option<String>,
    owner: String,
    opener: TaskOpener,
) -> crate::Result<String> {
    let card = SpawnCard {
        title,
        note,
        assignee: owner,
        origin: TaskOrigin::new(Some(at.desk_id.to_owned()), at.thread_root),
        parent_task_id: None,
        origin_run_id: None,
        origin_workflow_id: None,
        opened_by: Some(opener),
    }
    .into_record();
    at.tasks.upsert(at.company, &card).await?;
    Ok(card.id)
}

/// Marks a card a publish minted as the one answering the message, so a
/// resumed episode finds it again.
pub(crate) async fn stamp_message_card(at: &CardDesk<'_>, id: &str) -> crate::Result<()> {
    let Some(opened_at) = at.opened_at else {
        return Ok(());
    };
    let Some(mut card) = at
        .tasks
        .list(at.company)
        .await?
        .into_iter()
        .find(|card| card.id == id)
    else {
        return Ok(());
    };
    if card.origin_message_seq.is_some() {
        return Ok(());
    }
    card.origin_message_seq = Some(opened_at);
    at.tasks.upsert(at.company, &card).await
}

#[cfg(test)]
#[path = "seat_cards_tests.rs"]
mod tests;
