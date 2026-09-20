//! Tests for the per-seat fold of a round.

use super::*;

#[test]
fn a_spoken_turn_commits_and_a_silent_one_retries_then_salvages() {
    let spoke = fold_seat(
        "ceo",
        1,
        Ok(SeatOutcome {
            reply: "thinking".into(),
            utterances: vec![Utterance::Post {
                message: "hi".into(),
            }],
            ..Default::default()
        }),
        prompt::DESK_KINDS,
    );
    assert!(matches!(
        spoke,
        Fold::Done(
            Settled {
                utterance: Utterance::Post { .. },
                forced: None,
                ..
            },
            TurnOutcome::Committed
        )
    ));
    let silent = fold_seat(
        "ceo",
        1,
        Ok(SeatOutcome {
            reply: "no tool".into(),
            ..Default::default()
        }),
        prompt::DESK_KINDS,
    );
    assert!(matches!(silent, Fold::Retry));
    let last = fold_seat(
        "ceo",
        MAX_ATTEMPTS,
        Ok(SeatOutcome {
            reply: "<<<POST here it is POST>>>".into(),
            ..Default::default()
        }),
        prompt::DESK_KINDS,
    );
    match last {
        Fold::Done(settled, TurnOutcome::NoUtterance) => {
            assert_eq!(settled.utterance.message(), "here it is");
            assert!(matches!(
                settled.utterance,
                Utterance::CompleteEpisode { .. }
            ));
            assert_eq!(settled.forced, Some(EpisodeReason::Failed));
        }
        other => panic!("{}", matches!(other, Fold::Retry)),
    }
    let empty = fold_seat(
        "ceo",
        MAX_ATTEMPTS,
        Ok(SeatOutcome::default()),
        prompt::DESK_KINDS,
    );
    assert!(matches!(
        empty,
        Fold::Done(Settled { utterance: Utterance::CompleteEpisode { message }, .. }, _) if message == "(no action)"
    ));
}

#[test]
fn a_solo_seat_is_answered_by_its_bare_reply_and_never_broadcasts() {
    let bare = fold_seat(
        "writer",
        1,
        Ok(SeatOutcome {
            reply: "Just the answer.".into(),
            ..Default::default()
        }),
        prompt::SOLO_KINDS,
    );
    assert!(matches!(
        bare,
        Fold::Done(Settled { utterance: Utterance::CompleteEpisode { message }, forced: None, .. }, TurnOutcome::Committed) if message == "Just the answer."
    ));
    let widened = narrow(
        Utterance::Broadcast {
            message: "anyone?".into(),
        },
        prompt::SOLO_KINDS,
    );
    assert!(matches!(widened, Utterance::Post { message } if message == "anyone?"));
    let kept = narrow(
        Utterance::Broadcast {
            message: "anyone?".into(),
        },
        prompt::DESK_KINDS,
    );
    assert!(matches!(kept, Utterance::Broadcast { .. }));
}

#[test]
fn a_failed_or_timed_out_seat_is_completed_on_its_behalf() {
    let timed_out = fold_seat("ceo", 1, Err(SeatFailure::TimedOut), prompt::DESK_KINDS);
    assert!(matches!(
        timed_out,
        Fold::Failed(
            Settled {
                forced: Some(EpisodeReason::Timeout),
                ..
            },
            SeatFailure::TimedOut
        )
    ));
    let failed = fold_seat(
        "ceo",
        1,
        Err(SeatFailure::Failed("boom".into())),
        prompt::DESK_KINDS,
    );
    assert!(matches!(
        failed,
        Fold::Failed(
            Settled {
                forced: Some(EpisodeReason::Failed),
                ..
            },
            SeatFailure::Failed(_)
        )
    ));
    let episode = ReplyEpisode {
        id: "e".into(),
        revision: 0,
        kind: UtteranceKind::Dm,
        to: vec!["a".into()],
        routed_by: None,
    };
    assert!(matches!(
        utterance_of(&episode, "x".into()),
        Utterance::Dm { to, message } if to == vec!["a".to_string()] && message == "x"
    ));
}
