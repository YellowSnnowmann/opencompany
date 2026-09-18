use super::*;
use crate::metering::{AgentTokens, UsageTotals};

#[test]
fn redacted_cost_is_explicit_and_never_serializes_as_zero() {
    let dto = UsageDto::new(
        Usage {
            series: Vec::new(),
            by_agent: vec![AgentTokens {
                name: "Ops".to_string(),
                tokens: 10,
                cost_usd: 2.5,
            }],
            by_provider: Vec::new(),
            totals: UsageTotals {
                input_tokens: 8,
                output_tokens: 2,
                tokens: 10,
                cost_usd: 2.5,
                oauth_calls: 0,
                connections: 0,
                search_calls: 0,
            },
        },
        false,
    );
    let value = serde_json::to_value(dto).expect("serialize usage");
    assert_eq!(value["costHidden"], true);
    assert!(value["totals"].get("costUsd").is_none(), "{value}");
    assert!(value["byAgent"][0].get("costUsd").is_none(), "{value}");
}
