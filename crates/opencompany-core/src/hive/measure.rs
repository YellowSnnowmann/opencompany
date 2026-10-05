//! Coordination metrics over a company's journal — the store-reading twin of
//! `scripts/measure-coordination.mjs` (plan hive-desks, Phase 8; rewritten for
//! the OC-2 Coordinator).
//!
//! The question asked of the company hive is one number and two facts: do
//! agents run **at once**, do they **talk to each other**, and does every
//! episode **settle**. The Node script answers it from the live `/events`
//! stream a console sees; this answers it from the rows themselves, with no
//! host running — `opencompany measure --company <id>` after a run, or a test
//! over an in-memory journal — so the two can be compared and neither has to
//! be trusted alone.
//!
//! The fold mirrors `scripts/lib/coordination-metrics.mjs` frame for frame:
//!
//! * turn brackets keyed by turn id (`TurnStarted` → `TurnSettled` /
//!   `TurnFailed`) give the concurrency peak, every overlap, and the overlaps
//!   one agent had with itself (which must be zero);
//! * episodes are every episode id a row names — a turn bracket's
//!   `hive.episodeId`, a reply's `hive.episodeId`, a private row — closed by
//!   `HiveEpisodeSettled`, which says whether it settled or failed;
//! * contacts are `HiveMessage` rows: an agent's direct message to another, and
//!   a private desk line to its readers, each counted once per distinct
//!   `from→to` pair;
//! * `HiveAccepted.route` gives the starter-route histogram (mention, Jev,
//!   default).
//!
//! The thresholds ([`Thresholds`]) are the script's, so a measurement passes or
//! fails the same way on both paths.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::error::Result;
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, CompanyId, EventSeq, HiveDestination, StoredEvent};

/// Rows read per journal page.
const PAGE: usize = 512;

/// What a run must clear. Mirrors `DEFAULT_THRESHOLDS` in
/// `scripts/lib/coordination-metrics.mjs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thresholds {
    /// Least concurrency peak: agents must actually run at once.
    pub max_concurrent_turns: usize,
    /// Least agent→agent contacts (direct messages plus private lines).
    pub agent_contacts: usize,
    /// Least distinct `from→to` pairs.
    pub distinct_pairs: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            max_concurrent_turns: 2,
            agent_contacts: 1,
            distinct_pairs: 2,
        }
    }
}

/// One episode, as the journal shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeMeasure {
    /// The hive it ran in, once a row named it.
    pub hive_id: String,
    /// Turns that ran for it.
    pub turns: u32,
    /// Whether it settled normally.
    pub completed: bool,
    /// Why it stopped, when it failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// From the first row naming it to its settlement, epoch-millis apart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_complete_millis: Option<u64>,
}

/// The coordination report.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// The company measured.
    pub company: String,
    /// The first journal sequence folded.
    pub since_seq: u64,
    /// Rows folded.
    pub rows: usize,
    /// The concurrency peak.
    pub max_concurrent_turns: usize,
    /// Turn starts that found another turn open.
    pub overlaps: usize,
    /// Turn starts that found the same agent already running.
    pub same_agent_overlaps: usize,
    /// Turns never closed in the window.
    pub open_turns: usize,
    /// Episodes any row named.
    pub episodes_opened: usize,
    /// Episodes that settled normally.
    pub episodes_completed: usize,
    /// Episodes that stopped on a wall, an error or an interruption.
    pub episodes_failed: usize,
    /// Per episode.
    pub episodes: BTreeMap<String, EpisodeMeasure>,
    /// Agent→agent direct messages.
    pub direct_messages: usize,
    /// Private desk lines.
    pub private_lines: usize,
    /// Distinct `from→to` pairs over both.
    pub distinct_pairs: BTreeSet<String>,
    /// How operator messages chose their starters.
    pub starter_routes: BTreeMap<String, usize>,
    /// Coordinator turns interrupted and not replayed.
    pub interrupted_turns: usize,
}

impl Report {
    /// The thresholds this report misses, worded for a person; empty is a pass.
    #[must_use]
    pub fn failures(&self, thresholds: &Thresholds) -> Vec<String> {
        let mut failures = Vec::new();
        if self.max_concurrent_turns < thresholds.max_concurrent_turns {
            failures.push(format!(
                "max concurrent turns {} < {}",
                self.max_concurrent_turns, thresholds.max_concurrent_turns
            ));
        }
        if self.same_agent_overlaps > 0 {
            failures.push(format!(
                "same-agent overlaps {} (must be 0)",
                self.same_agent_overlaps
            ));
        }
        let contacts = self.direct_messages + self.private_lines;
        if contacts < thresholds.agent_contacts {
            failures.push(format!(
                "agent→agent contacts {contacts} < {}",
                thresholds.agent_contacts
            ));
        }
        if self.distinct_pairs.len() < thresholds.distinct_pairs {
            failures.push(format!(
                "distinct pairs {} < {}",
                self.distinct_pairs.len(),
                thresholds.distinct_pairs
            ));
        }
        if self.episodes_opened == 0 {
            failures.push("no episode opened".to_string());
        } else {
            let open: Vec<String> = self
                .episodes
                .iter()
                .filter(|(_, episode)| !episode.completed)
                .map(|(id, episode)| format!("{}/{id}", episode.hive_id))
                .collect();
            if !open.is_empty() {
                failures.push(format!(
                    "{} episode(s) never settled: {}",
                    open.len(),
                    open.join(", ")
                ));
            }
        }
        failures
    }

    /// The report as an aligned two-column table with its verdict.
    #[must_use]
    pub fn to_table(&self, thresholds: &Thresholds) -> String {
        let join = |items: Vec<String>| {
            if items.is_empty() {
                "-".to_string()
            } else {
                items.join(" ")
            }
        };
        let turns = join(
            self.episodes
                .iter()
                .map(|(id, episode)| format!("{id}={}", episode.turns))
                .collect(),
        );
        let times = join(
            self.episodes
                .iter()
                .filter_map(|(id, episode)| {
                    episode
                        .time_to_complete_millis
                        .map(|millis| format!("{id}={millis}"))
                })
                .collect(),
        );
        let failures_seen = join(
            self.episodes
                .iter()
                .filter_map(|(id, episode)| {
                    episode
                        .failure
                        .as_ref()
                        .map(|reason| format!("{id}={reason}"))
                })
                .collect(),
        );
        let routes = join(
            self.starter_routes
                .iter()
                .map(|(route, count)| format!("{route}={count}"))
                .collect(),
        );
        let failures = self.failures(thresholds);
        let verdict = if failures.is_empty() {
            "PASS: every threshold met".to_string()
        } else {
            format!(
                "FAIL ({}):\n  - {}",
                failures.len(),
                failures.join("\n  - ")
            )
        };
        let mut out = String::new();
        let mut line = |label: &str, value: String| {
            out.push_str(&format!("{label:<26}{value}\n"));
        };
        line(
            "company",
            format!(
                "{} (since seq {}, {} rows)",
                self.company, self.since_seq, self.rows
            ),
        );
        line(
            "max concurrent turns",
            self.max_concurrent_turns.to_string(),
        );
        line("turn overlaps", self.overlaps.to_string());
        line("same-agent overlaps", self.same_agent_overlaps.to_string());
        line("open turns", self.open_turns.to_string());
        line("interrupted turns", self.interrupted_turns.to_string());
        line(
            "episodes",
            format!(
                "{}/{} settled, {} failed",
                self.episodes_completed, self.episodes_opened, self.episodes_failed
            ),
        );
        line("turns per episode", turns);
        line(
            "direct / private",
            format!("{} / {}", self.direct_messages, self.private_lines),
        );
        line(
            "distinct pairs",
            format!(
                "{} {}",
                self.distinct_pairs.len(),
                self.distinct_pairs
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        );
        line("starter routes", routes);
        line("time to settle (ms)", times);
        line("failures", failures_seen);
        out.push('\n');
        out.push_str(&verdict);
        out.push('\n');
        out
    }
}

#[derive(Default)]
struct Fold {
    report: Report,
    /// Open turn id → the agent running it.
    open: HashMap<String, Option<String>>,
    /// Episode id → when a row first named it.
    first_seen: HashMap<String, u64>,
}

impl Fold {
    fn episode(&mut self, id: &str, hive_id: Option<&str>, at: u64) -> &mut EpisodeMeasure {
        self.first_seen.entry(id.to_string()).or_insert(at);
        let episode = self.report.episodes.entry(id.to_string()).or_default();
        if episode.hive_id.is_empty()
            && let Some(hive) = hive_id
        {
            episode.hive_id = hive.to_string();
        }
        episode
    }

    fn pair(&mut self, from: &str, to: &str) {
        if from != to {
            self.report.distinct_pairs.insert(format!("{from}→{to}"));
        }
    }

    fn fold(&mut self, stored: &StoredEvent) {
        self.report.rows += 1;
        let at = stored.at_millis;
        match &stored.event {
            CompanyEvent::TurnStarted {
                turn_id,
                agent_id,
                hive,
                ..
            } => {
                if let Some(hive) = hive
                    && let Some(episode) = &hive.episode_id
                {
                    self.episode(episode, hive.hive_id.as_deref(), at).turns += 1;
                }
                if !self.open.is_empty() {
                    self.report.overlaps += 1;
                }
                if agent_id.is_some() && self.open.values().any(|open| open == agent_id) {
                    self.report.same_agent_overlaps += 1;
                }
                self.open.insert(turn_id.clone(), agent_id.clone());
                self.report.max_concurrent_turns =
                    self.report.max_concurrent_turns.max(self.open.len());
            }
            CompanyEvent::TurnSettled { turn_id, .. }
            | CompanyEvent::TurnFailed { turn_id, .. } => {
                self.open.remove(turn_id);
            }
            CompanyEvent::AgentReply {
                chat_id,
                hive: Some(hive),
                ..
            } => {
                if let Some(episode) = &hive.episode_id {
                    self.episode(episode, Some(chat_id), at);
                }
            }
            CompanyEvent::HiveMessage {
                sender,
                destination,
                episode_id,
                only_for,
                ..
            } => {
                if let Some(episode) = episode_id {
                    let hive = match destination {
                        HiveDestination::Hive(hive) => Some(hive.as_str()),
                        HiveDestination::Agent(_) => None,
                    };
                    self.episode(episode, hive, at);
                }
                match destination {
                    HiveDestination::Agent(to) => {
                        self.report.direct_messages += 1;
                        self.pair(sender, to);
                    }
                    HiveDestination::Hive(_) => {
                        self.report.private_lines += 1;
                        for reader in only_for {
                            self.pair(sender, reader);
                        }
                    }
                }
            }
            CompanyEvent::HiveAccepted {
                route: Some(route), ..
            } => {
                *self.report.starter_routes.entry(route.clone()).or_insert(0) += 1;
            }
            CompanyEvent::HiveEpisodeSettled {
                episode_id,
                hive_id,
                failure,
                ..
            } => {
                let first = self.first_seen.get(episode_id).copied();
                let episode = self.episode(episode_id, Some(hive_id), at);
                let fresh = !episode.completed && episode.failure.is_none();
                match failure {
                    None => episode.completed = true,
                    Some(reason) => episode.failure = Some(reason.clone()),
                }
                episode.time_to_complete_millis = first.map(|opened| at.saturating_sub(opened));
                if fresh {
                    if failure.is_none() {
                        self.report.episodes_completed += 1;
                    } else {
                        self.report.episodes_failed += 1;
                    }
                }
            }
            CompanyEvent::HiveTurnInterrupted { .. } => self.report.interrupted_turns += 1,
            _ => {}
        }
    }

    fn finish(mut self, company: &CompanyId, since: EventSeq) -> Report {
        self.report.company = company.to_string();
        self.report.since_seq = since.value();
        self.report.open_turns = self.open.len();
        self.report.episodes_opened = self.report.episodes.len();
        self.report
    }
}

/// Folds every row of `company` from `since` (inclusive) to the tail.
pub async fn measure(
    events: &dyn EventLog,
    company: &CompanyId,
    since: EventSeq,
) -> Result<Report> {
    let mut fold = Fold::default();
    let mut cursor = since;
    loop {
        let page = events.read_from(company, cursor, PAGE).await?;
        let Some(last) = page.last() else { break };
        let next = EventSeq::new(last.seq.value() + 1);
        for stored in &page {
            fold.fold(stored);
        }
        if page.len() < PAGE {
            break;
        }
        cursor = next;
    }
    Ok(fold.finish(company, since))
}

/// Folds rows already in hand — a test's, or a journal read some other way.
#[must_use]
pub fn measure_rows(company: &CompanyId, since: EventSeq, rows: &[StoredEvent]) -> Report {
    let mut fold = Fold::default();
    for stored in rows.iter().filter(|stored| stored.seq >= since) {
        fold.fold(stored);
    }
    fold.finish(company, since)
}

#[cfg(test)]
#[path = "measure_tests.rs"]
mod tests;
