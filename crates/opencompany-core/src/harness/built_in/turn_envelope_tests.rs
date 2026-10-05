//! The post-turn reads both callers of the envelope make: pricing an attempt
//! from the progress stream, and how a turn stopped.

use super::*;
use oh::agent::progress::AgentProgress;

fn cost(total_usd: f64) -> AgentProgress {
    AgentProgress::TurnCostUpdated {
        model: "m".to_string(),
        iteration: 1,
        input_tokens: 10,
        output_tokens: 5,
        cached_input_tokens: 0,
        total_usd,
    }
}

#[test]
fn a_tap_that_saw_nothing_is_metered_from_the_stream() {
    let mut usages = vec![TurnUsage::default()];
    price_usages(
        "ceo",
        &mut usages,
        &[AgentProgress::TurnStarted, cost(0.25)],
    );
    assert_eq!(usages[0].input_tokens, 10);
    assert!((usages[0].cost_usd - 0.25).abs() < f64::EPSILON);
}

#[test]
fn a_tap_with_tokens_but_no_price_takes_the_streams_price_only() {
    let mut usages = vec![TurnUsage {
        input_tokens: 99,
        output_tokens: 1,
        cached_input_tokens: 0,
        cost_usd: 0.0,
    }];
    price_usages("ceo", &mut usages, &[AgentProgress::TurnStarted, cost(0.5)]);
    assert_eq!(usages[0].input_tokens, 99, "the tap's tokens stand");
    assert!((usages[0].cost_usd - 0.5).abs() < f64::EPSILON);
}

#[test]
fn a_priced_tap_is_left_alone() {
    let mut usages = vec![TurnUsage {
        input_tokens: 3,
        output_tokens: 3,
        cached_input_tokens: 0,
        cost_usd: 0.1,
    }];
    price_usages("ceo", &mut usages, &[cost(9.0)]);
    assert!((usages[0].cost_usd - 0.1).abs() < f64::EPSILON);
}

#[test]
fn findings_carry_each_pause_to_its_notice() {
    let findings = turn_findings(
        "ceo",
        &[],
        None,
        Some("credits ran out".into()),
        Some(("wall".into(), std::time::Duration::from_secs(3))),
    );
    assert!(!findings.hit_iteration_cap);
    assert_eq!(
        findings.budget_paused.map(|pause| pause.summary).as_deref(),
        Some("credits ran out")
    );
    let ceiling = findings.ceiling_paused.expect("ceiling");
    assert_eq!(ceiling.agent, "ceo");
    assert_eq!(ceiling.elapsed, std::time::Duration::from_secs(3));
}

#[test]
fn a_spend_halt_outranks_an_iteration_cap() {
    let halt = SpendHalt {
        agent: "ceo".into(),
        spent_usd: 2.0,
        cap_usd: 1.0,
    };
    let findings = turn_findings("ceo", &[], Some(halt), None, None);
    assert!(findings.halted_for_spend.is_some());
    assert!(!findings.hit_iteration_cap);
}
