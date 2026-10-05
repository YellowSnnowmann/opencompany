//! The coordinator turn key, the held escalation answers, and the release
//! note a parked agent reads.

use super::*;

fn decision(id: &str, verdict: SeatVerdict) -> SeatDecision {
    SeatDecision {
        approval_id: ApprovalId::new(id),
        ask: SeatAsk::Request {
            title: "ship it".to_owned(),
        },
        verdict,
        answer: String::new(),
    }
}

#[test]
fn a_turn_key_round_trips_through_parse() {
    let key = turn_key("writer", Some("ep1"));
    assert_eq!(key, "hive-turn:writer:ep1");
    assert_eq!(
        parse(&key),
        Some(HiveSeat {
            agent_id: "writer".to_owned(),
            episode_id: Some("ep1".to_owned()),
        })
    );
    let direct = turn_key("writer", None);
    assert_eq!(direct, "hive-turn:writer:");
    assert_eq!(
        parse(&direct),
        Some(HiveSeat {
            agent_id: "writer".to_owned(),
            episode_id: None,
        })
    );
}

#[test]
fn other_turn_keys_are_not_hive_turns() {
    assert_eq!(parse("cycle-123"), None);
    assert_eq!(parse("workflow-run:abc"), None);
    assert_eq!(parse("hive-turn:writer"), None);
    assert_eq!(parse("hive-turn::ep1"), None);
    assert_eq!(parse("episode-seat:ep1:writer"), None);
}

#[test]
fn an_answer_is_held_until_taken_once() {
    let answers = HiveAnswers::default();
    let id = ApprovalId::new("a1");
    answers.answer(&id, SeatVerdict::Approved, "go".to_owned());
    assert_eq!(
        answers.take_answer(&id),
        Some((SeatVerdict::Approved, "go".to_owned()))
    );
    assert_eq!(answers.take_answer(&id), None);
}

#[test]
fn the_release_note_joins_every_decision_in_order() {
    assert_eq!(release_note(&[]), None);
    let note = release_note(&[
        decision("a1", SeatVerdict::Approved),
        decision("a2", SeatVerdict::Denied),
    ])
    .expect("a note");
    let lines: Vec<&str> = note.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("approved your request"), "{note}");
    assert!(lines[1].contains("denied your request"), "{note}");
}
