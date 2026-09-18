use super::*;
use crate::metering::calendar::{MILLIS_PER_DAY, days_from_civil};

fn at(y: i64, m: u32, d: u32) -> u64 {
    (days_from_civil(y, m, d) as u64) * MILLIS_PER_DAY + 12 * 3_600_000
}

fn inference(at_millis: u64, agent: &str, input: u64, output: u64, cost: f64) -> UsageSample {
    UsageSample {
        at_millis,
        agent: agent.to_string(),
        provider: "managed".to_string(),
        input_tokens: input,
        output_tokens: output,
        cached_input_tokens: 0,
        cost_usd: cost,
        kind: SampleKind::Inference,
        run_id: None,
        model: None,
    }
}

fn oauth(at_millis: u64, provider: &str) -> UsageSample {
    UsageSample {
        at_millis,
        agent: "ceo".to_string(),
        provider: provider.to_string(),
        input_tokens: 0,
        output_tokens: 0,
        cached_input_tokens: 0,
        cost_usd: 0.0,
        kind: SampleKind::OauthCall,
        run_id: None,
        model: None,
    }
}

#[test]
fn empty_samples_zero_fill_the_series() {
    let now = at(2026, 7, 16);
    let u = bucket_usage(&[], UsageRange::D7, now, &HashMap::new());
    assert_eq!(u.series.len(), 7);
    assert_eq!(u.series[0].date, "2026-07-10");
    assert_eq!(u.series[6].date, "2026-07-16");
    assert!(
        u.series
            .iter()
            .all(|p| p.input_tokens == 0 && p.output_tokens == 0)
    );
    assert_eq!(u.totals.tokens, 0);
    assert_eq!(u.totals.connections, 0);
    assert!(u.by_agent.is_empty());
    assert!(u.by_provider.is_empty());
}

#[test]
fn series_lengths_track_the_range() {
    let now = at(2026, 7, 16);
    assert_eq!(
        bucket_usage(&[], UsageRange::D7, now, &HashMap::new())
            .series
            .len(),
        7
    );
    assert_eq!(
        bucket_usage(&[], UsageRange::D30, now, &HashMap::new())
            .series
            .len(),
        30
    );
    assert_eq!(
        bucket_usage(&[], UsageRange::D90, now, &HashMap::new())
            .series
            .len(),
        90
    );
}

#[test]
fn tokens_land_in_the_right_day_bucket() {
    let now = at(2026, 7, 16);
    let samples = vec![
        inference(at(2026, 7, 16), "ceo", 100, 40, 0.5),
        inference(at(2026, 7, 15), "ceo", 10, 5, 0.1),
        inference(at(2026, 7, 15), "ceo", 20, 5, 0.1),
    ];
    let u = bucket_usage(&samples, UsageRange::D7, now, &HashMap::new());
    let today = u.series.last().unwrap();
    assert_eq!(today.date, "2026-07-16");
    assert_eq!((today.input_tokens, today.output_tokens), (100, 40));
    let yesterday = &u.series[5];
    assert_eq!(yesterday.date, "2026-07-15");
    assert_eq!((yesterday.input_tokens, yesterday.output_tokens), (30, 10));
}

#[test]
fn samples_outside_the_window_still_feed_totals_not_series() {
    let now = at(2026, 7, 16);
    let samples = vec![
        inference(at(2026, 7, 16), "ceo", 100, 40, 0.5),
        // 60 days ago: outside the 7-day window.
        inference(at(2026, 5, 17), "ceo", 999, 999, 9.0),
    ];
    let u = bucket_usage(&samples, UsageRange::D7, now, &HashMap::new());
    // Series only covers the 7-day window.
    assert_eq!(u.series.iter().map(|p| p.input_tokens).sum::<u64>(), 100);
    // Totals include the out-of-window sample.
    assert_eq!(u.totals.input_tokens, 1099);
    assert_eq!(u.totals.output_tokens, 1039);
    assert!((u.totals.cost_usd - 9.5).abs() < 1e-9);
}

#[test]
fn by_agent_resolves_display_names_and_sorts_desc() {
    let now = at(2026, 7, 16);
    let samples = vec![
        inference(at(2026, 7, 16), "strategy", 100, 50, 0.1),
        inference(at(2026, 7, 16), "creative", 300, 100, 0.2),
        inference(at(2026, 7, 16), "unknown", 10, 0, 0.0),
    ];
    let mut roster = HashMap::new();
    roster.insert("strategy".to_string(), "Strategy desk".to_string());
    roster.insert("creative".to_string(), "Creative studio".to_string());
    let u = bucket_usage(&samples, UsageRange::D7, now, &roster);
    assert_eq!(u.by_agent.len(), 3);
    assert_eq!(u.by_agent[0].name, "Creative studio");
    assert_eq!(u.by_agent[0].tokens, 400);
    assert_eq!(u.by_agent[1].name, "Strategy desk");
    assert_eq!(u.by_agent[1].tokens, 150);
    // Unknown id falls back to the raw id.
    assert_eq!(u.by_agent[2].name, "unknown");
}

#[test]
fn by_provider_counts_only_oauth_calls() {
    let now = at(2026, 7, 16);
    let samples = vec![
        inference(at(2026, 7, 16), "ceo", 100, 50, 0.1),
        oauth(at(2026, 7, 16), "github"),
        oauth(at(2026, 7, 15), "github"),
        oauth(at(2026, 7, 15), "gmail"),
    ];
    let u = bucket_usage(&samples, UsageRange::D7, now, &HashMap::new());
    assert_eq!(u.by_provider.len(), 2);
    assert_eq!(u.by_provider[0].provider, "github");
    assert_eq!(u.by_provider[0].calls, 2);
    assert_eq!(u.by_provider[1].provider, "gmail");
    assert_eq!(u.by_provider[1].calls, 1);
    assert_eq!(u.totals.oauth_calls, 3);
    assert_eq!(u.totals.connections, 2);
    // OAuth calls carry no tokens.
    assert_eq!(u.totals.tokens, 150);
}

#[test]
fn oauth_only_agents_do_not_appear_as_zero_token_teammates() {
    // "Tokens by teammate" is a token chart. An agent whose whole window is
    // connected-tool calls has no tokens to show, so it must not render as
    // an empty bar beside the agents that actually spent.
    let now = at(2026, 7, 16);
    let samples = vec![
        inference(at(2026, 7, 16), "ceo", 100, 50, 0.1),
        oauth(at(2026, 7, 16), "github"),
        // `ops` only ever made an OAuth call this window.
        UsageSample {
            agent: "ops".to_string(),
            ..oauth(at(2026, 7, 16), "gmail")
        },
    ];
    let u = bucket_usage(&samples, UsageRange::D7, now, &HashMap::new());
    assert_eq!(u.by_agent.len(), 1);
    assert_eq!(u.by_agent[0].name, "ceo");
    assert_eq!(u.by_agent[0].tokens, 150);
    // The calls still count — only the token attribution is withheld.
    assert_eq!(u.totals.oauth_calls, 2);
    assert_eq!(u.totals.connections, 2);
}
