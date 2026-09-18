use super::*;

#[test]
fn json_rpc_request_round_trips_with_version() {
    let rpc = JsonRpcRequest::new("tasks/send", serde_json::json!({ "skill": "seo.audit" }));
    assert_eq!(rpc.jsonrpc, "2.0");
    let json = serde_json::to_string(&rpc).expect("serialize");
    let back: JsonRpcRequest = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, rpc);
}

#[test]
fn directory_skill_decodes_camel_and_snake_case() {
    let camel = serde_json::json!({
        "agentId": "AgentX", "skillId": "seo.audit", "price": "25.00"
    });
    let snake = serde_json::json!({
        "agent_id": "AgentX", "skill_id": "seo.audit", "price": "25.00"
    });
    let a: DirectorySkill = serde_json::from_value(camel).expect("camel");
    let b: DirectorySkill = serde_json::from_value(snake).expect("snake");
    assert_eq!(a, b);
    assert_eq!(a.agent_id, "AgentX");
}

#[test]
fn paid_outcome_decodes_402_accepts_envelope() {
    // The x402 `{ accepts: [ … ] }` envelope decodes into a challenge.
    let body = serde_json::json!({
        "accepts": [ { "maxAmountRequired": "25.00", "payTo": "Recipient" } ]
    });
    let challenge = X402Challenge::from_body(&body).expect("challenge");
    let outcome: PaidOutcome<RegistryReceipt> = PaidOutcome::PaymentRequired(challenge.clone());
    assert_eq!(outcome, PaidOutcome::PaymentRequired(challenge));
}

#[test]
fn registry_receipt_decodes_from_wire() {
    let value = serde_json::json!({ "id": "r1", "addr": "AddrX", "feeUsd": 25.0 });
    let receipt: RegistryReceipt = serde_json::from_value(value).expect("decode");
    assert_eq!(receipt.addr, AgentAddr("AddrX".into()));
    assert_eq!(receipt.fee_usd, 25.0);
}

#[tokio::test]
async fn mock_records_calls_and_honors_reachable() {
    let mock = MockTinyplaceClient::new().with_resolve(Some(AgentAddr("Me".into())));
    assert_eq!(mock.resolve("acme").await.unwrap(), AgentAddr("Me".into()));
    assert_eq!(mock.count("resolve"), 1);

    mock.set_reachable(false);
    let err = mock.resolve("acme").await.unwrap_err();
    assert_eq!(err.code(), "tinyplace_unreachable");
    // Both attempts are logged even though the second failed.
    assert_eq!(mock.count("resolve"), 2);
}

#[test]
fn sha256_matches_known_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn query_string_builds_suffix() {
    assert_eq!(query_string(&DirectoryQuery::default()), "");
    assert_eq!(
        query_string(&DirectoryQuery {
            skill: Some("seo.audit".into()),
            tag: None,
        }),
        "?skill=seo.audit"
    );
}
