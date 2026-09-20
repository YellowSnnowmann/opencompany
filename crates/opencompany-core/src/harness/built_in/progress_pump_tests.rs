//! Tests for the progress pump's derived readings: cost off the stream,
//! per-attempt segmentation, and the iteration-cap tell.

use super::*;

fn cost(total_usd: f64, iteration: u32) -> AgentProgress {
    AgentProgress::TurnCostUpdated {
        model: "m".to_string(),
        iteration,
        input_tokens: 10 * u64::from(iteration),
        output_tokens: 5,
        cached_input_tokens: 0,
        total_usd,
    }
}

#[test]
fn the_last_cumulative_cost_is_the_turn_total() {
    let events = vec![AgentProgress::TurnStarted, cost(0.1, 1), cost(0.3, 2)];
    let usage = last_observed_turn_cost(&events).expect("cost");
    assert_eq!(usage.input_tokens, 20);
    assert!((usage.cost_usd - 0.3).abs() < f64::EPSILON);
    assert!(last_observed_turn_cost(&[AgentProgress::TurnStarted]).is_none());
}

#[test]
fn attempts_are_segmented_at_each_turn_start() {
    let events = vec![
        AgentProgress::TurnStarted,
        cost(0.1, 1),
        AgentProgress::TurnStarted,
        cost(0.5, 1),
    ];
    let segments = attempt_event_segments(&events, 3);
    assert_eq!(segments.len(), 3);
    assert_eq!(segments[0].len(), 2);
    assert_eq!(segments[1].len(), 2);
    assert!(
        segments[2].is_empty(),
        "an attempt that never started has no events"
    );
    assert!(
        (last_observed_turn_cost(segments[1]).expect("cost").cost_usd - 0.5).abs() < f64::EPSILON
    );
}

#[test]
fn a_turn_whose_last_iteration_is_the_cap_paused_at_it() {
    let capped = vec![
        AgentProgress::TurnStarted,
        AgentProgress::IterationStarted {
            iteration: 1,
            max_iterations: 2,
        },
        AgentProgress::IterationStarted {
            iteration: 2,
            max_iterations: 2,
        },
        AgentProgress::TurnCompleted { iterations: 2 },
    ];
    assert!(hit_iteration_cap(&capped));
    let finished = vec![
        AgentProgress::TurnStarted,
        AgentProgress::IterationStarted {
            iteration: 1,
            max_iterations: 25,
        },
        AgentProgress::TurnCompleted { iterations: 1 },
    ];
    assert!(!hit_iteration_cap(&finished));
    assert!(!hit_iteration_cap(&[]));
    // Only the LAST attempt counts: a capped first attempt followed by a
    // clean retry is a finished turn.
    let mut retried = capped.clone();
    retried.extend(finished.clone());
    assert!(!hit_iteration_cap(&retried));
}

#[tokio::test]
async fn the_pump_returns_every_event_in_order_after_finish() {
    let pump = ProgressPump::start(StepLabels::default(), None, None);
    let tx = pump.sender();
    tx.send(AgentProgress::TurnStarted).await.expect("send");
    tx.send(AgentProgress::TurnCompleted { iterations: 1 })
        .await
        .expect("send");
    drop(tx);
    let events = pump.finish().await;
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], AgentProgress::TurnStarted));
}
