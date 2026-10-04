//! The coordination fold, rule by rule, over rows built by hand.

use super::*;
use crate::ports::types::{HiveRef, HiveTurnRef, TurnOutcome};
use crate::store::FsEventLog;

fn company() -> CompanyId {
    CompanyId::new("acme")
}

/// Rows at one-millisecond intervals, sequenced from 1.
fn rows(events: Vec<CompanyEvent>) -> Vec<StoredEvent> {
    events
        .into_iter()
        .enumerate()
        .map(|(index, event)| StoredEvent {
            seq: EventSeq::new(index as u64 + 1),
            company: company(),
            event,
            at_millis: 1_000 + index as u64,
        })
        .collect()
}

fn started(turn: &str, agent: &str, episode: &str) -> CompanyEvent {
    CompanyEvent::TurnStarted {
        turn_id: turn.into(),
        chat_id: "engineering".into(),
        parent: None,
        by: None,
        agent_id: Some(agent.into()),
        hive: Some(HiveTurnRef {
            hive_id: Some("engineering".into()),
            episode_id: Some(episode.into()),
        }),
    }
}

fn settled(turn: &str, agent: &str) -> CompanyEvent {
    CompanyEvent::TurnSettled {
        turn_id: turn.into(),
        agent_id: Some(agent.into()),
        chat_id: Some("engineering".into()),
        hive: None,
        outcome: TurnOutcome::Committed,
    }
}

fn reply(agent: &str, episode: &str, sequence: u64) -> CompanyEvent {
    CompanyEvent::AgentReply {
        chat_id: "engineering".into(),
        agent_id: agent.into(),
        text: "…".into(),
        steps: Vec::new(),
        outputs: Vec::new(),
        task_id: None,
        parent: None,
        mentions: Vec::new(),
        mention_depth: 0,
        audience: Vec::new(),
        hive: Some(HiveRef {
            sequence,
            episode_id: Some(episode.into()),
            thread: None,
        }),
    }
}

fn direct(from: &str, to: &str) -> CompanyEvent {
    CompanyEvent::HiveMessage {
        sequence: 9,
        sender: from.into(),
        destination: HiveDestination::Agent(to.into()),
        text: "a word".into(),
        thread: None,
        episode_id: None,
        only_for: Vec::new(),
    }
}

fn private(from: &str, readers: &[&str], episode: &str) -> CompanyEvent {
    CompanyEvent::HiveMessage {
        sequence: 10,
        sender: from.into(),
        destination: HiveDestination::Hive("engineering".into()),
        text: "between us".into(),
        thread: None,
        episode_id: Some(episode.into()),
        only_for: readers.iter().map(|r| (*r).to_string()).collect(),
    }
}

fn settled_episode(episode: &str, failure: Option<&str>) -> CompanyEvent {
    CompanyEvent::HiveEpisodeSettled {
        episode_id: episode.into(),
        hive_id: "engineering".into(),
        opened_at: 1,
        thread: None,
        failure: failure.map(str::to_string),
    }
}

fn accepted(route: &str) -> CompanyEvent {
    CompanyEvent::HiveAccepted {
        message_id: "op:1".into(),
        sequence: 1,
        chat_id: "engineering".into(),
        source: Some(EventSeq::new(1)),
        starters: vec!["engineer".into()],
        route: Some(route.into()),
    }
}

/// Two agents bracketed at once peak at two and overlap once; the same agent
/// starting again while its own turn is open is the one overlap that counts
/// against the run.
#[test]
fn turn_brackets_give_the_peak_and_flag_a_same_agent_overlap() {
    let report = measure_rows(
        &company(),
        EventSeq::new(0),
        &rows(vec![
            started("t1", "engineer", "ep1"),
            started("t2", "ceo", "ep1"),
            settled("t2", "ceo"),
            started("t3", "engineer", "ep1"),
            settled("t1", "engineer"),
            settled("t3", "engineer"),
        ]),
    );
    assert_eq!(report.max_concurrent_turns, 2);
    assert_eq!(report.overlaps, 2);
    assert_eq!(report.same_agent_overlaps, 1);
    assert_eq!(report.open_turns, 0);
    assert_eq!(report.episodes["ep1"].turns, 3);
}

/// Contacts, pairs, routes and settlement, the way the Node twin folds them.
#[test]
fn the_fold_mirrors_the_node_twin_frame_for_frame() {
    let report = measure_rows(
        &company(),
        EventSeq::new(0),
        &rows(vec![
            accepted("jev"),
            accepted("mention"),
            started("t1", "engineer", "ep1"),
            reply("engineer", "ep1", 2),
            direct("engineer", "ceo"),
            private("ceo", &["engineer"], "ep1"),
            settled("t1", "engineer"),
            settled_episode("ep1", None),
            started("t2", "ceo", "ep2"),
            settled_episode("ep2", Some("turn wall reached")),
            CompanyEvent::HiveTurnInterrupted {
                agent_id: "ceo".into(),
                episode_id: Some("ep2".into()),
                message_ids: vec!["op:2".into()],
                reason: "process restarted during turn".into(),
            },
        ]),
    );
    assert_eq!(report.direct_messages, 1);
    assert_eq!(report.private_lines, 1);
    assert_eq!(
        report.distinct_pairs.iter().cloned().collect::<Vec<_>>(),
        vec!["ceo→engineer".to_string(), "engineer→ceo".to_string()]
    );
    assert_eq!(report.starter_routes["jev"], 1);
    assert_eq!(report.starter_routes["mention"], 1);
    assert_eq!(report.episodes_opened, 2);
    assert_eq!(report.episodes_completed, 1);
    assert_eq!(report.episodes_failed, 1);
    assert!(report.episodes["ep1"].completed);
    assert_eq!(report.episodes["ep1"].time_to_complete_millis, Some(5));
    assert_eq!(
        report.episodes["ep2"].failure.as_deref(),
        Some("turn wall reached")
    );
    assert_eq!(report.interrupted_turns, 1);
}

#[test]
fn the_verdict_names_what_is_missing() {
    let report = measure_rows(
        &company(),
        EventSeq::new(0),
        &rows(vec![started("t1", "engineer", "ep1"), settled("t1", "engineer")]),
    );
    let failures = report.failures(&Thresholds::default());
    assert!(failures.iter().any(|f| f.starts_with("max concurrent turns 1")));
    assert!(failures.iter().any(|f| f.starts_with("agent→agent contacts 0")));
    assert!(failures.iter().any(|f| f.contains("never settled")));
    assert!(report.to_table(&Thresholds::default()).contains("FAIL ("));
}

/// `since` drops the rows before it, and the JSON shape is camelCase.
#[test]
fn since_narrows_the_window_and_the_report_serializes_camel_case() {
    let all = rows(vec![
        reply("engineer", "old", 1),
        settled_episode("old", None),
        reply("engineer", "new", 2),
    ]);
    let report = measure_rows(&company(), EventSeq::new(3), &all);
    assert_eq!(report.rows, 1);
    assert_eq!(report.since_seq, 3);
    assert_eq!(report.episodes_opened, 1);
    let wire = serde_json::to_value(&report).unwrap();
    assert_eq!(wire["maxConcurrentTurns"], 0);
    assert_eq!(wire["episodesOpened"], 1);
    assert_eq!(wire["episodes"]["new"]["hiveId"], "engineering");
    assert_eq!(wire["directMessages"], 0);
}

/// The same fold over the port, paged.
#[tokio::test]
async fn measure_reads_the_journal_through_the_port() {
    let dir = tempfile::tempdir().unwrap();
    let log = FsEventLog::new(dir.path());
    for event in [
        started("t1", "engineer", "ep1"),
        started("t2", "ceo", "ep1"),
        settled("t2", "ceo"),
        settled("t1", "engineer"),
        settled_episode("ep1", None),
    ] {
        log.append(&company(), event).await.unwrap();
    }
    let report = measure(&log, &company(), EventSeq::new(0)).await.unwrap();
    assert_eq!(report.rows, 5);
    assert_eq!(report.max_concurrent_turns, 2);
    assert_eq!(report.episodes_completed, 1);
}
