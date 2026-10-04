//! The reach rule `hivemind_send_agent` is decided under: desk peers, the
//! `delegates_to` allowlist, nobody when restricted to nothing, never oneself.

use super::*;
use crate::ports::types::CompanyId;

fn record(manifest: &str) -> CompanyRecord {
    CompanyRecord::from_manifest(
        CompanyId::new("acme"),
        toml::from_str(manifest).expect("valid manifest"),
    )
}

const RESTRICTED: &str = r#"
[company]
name = "Acme"

[[agent]]
id = "ceo"
role = "Chief Executive"

[[agent]]
id = "writer"
role = "Writer"
delegates_to = ["legal"]

[[agent]]
id = "counsel"
role = "Counsel"

[[agent]]
id = "designer"
role = "Designer"

[[group_chat]]
id = "content"
name = "Content"
members = ["writer", "ceo"]

[[group_chat]]
id = "legal"
name = "Legal"
members = ["counsel"]

[[group_chat]]
id = "design"
name = "Design"
members = ["designer"]
"#;

#[test]
fn an_unrestricted_teammate_reaches_anybody_but_itself() {
    let record = record(RESTRICTED);
    assert!(decide_direct(&record, "ceo", "designer").is_ok());
    let refusal = decide_direct(&record, "ceo", "ceo").expect_err("never oneself");
    assert!(refusal.contains("yourself"), "{refusal}");
}

#[test]
fn a_restricted_teammate_reaches_desk_peers_and_its_allowlist_only() {
    let record = record(RESTRICTED);
    assert!(decide_direct(&record, "writer", "ceo").is_ok(), "desk peer");
    assert!(decide_direct(&record, "writer", "counsel").is_ok(), "allowlisted desk");
    let refusal = decide_direct(&record, "writer", "designer").expect_err("out of reach");
    assert!(refusal.starts_with("refused:"), "{refusal}");
    assert!(refusal.contains("ceo") && refusal.contains("counsel"), "{refusal}");
}

#[test]
fn the_policy_resolves_coordinator_ids_and_refuses_strangers() {
    let policy = ReachPolicy::new();
    assert!(
        policy.may_message_agent("acme--writer", "acme--ceo").is_err(),
        "nothing is reachable before a record is installed"
    );
    let agents: HashMap<String, String> = ["ceo", "writer", "counsel", "designer"]
        .into_iter()
        .map(|id| (format!("acme--{id}"), id.to_string()))
        .collect();
    policy.install(Arc::new(record(RESTRICTED)), agents);
    assert!(policy.may_message_agent("acme--writer", "acme--ceo").is_ok());
    assert!(policy.may_message_agent("acme--writer", "acme--designer").is_err());
    let stranger = policy
        .may_message_agent("acme--writer", "acme--nobody")
        .expect_err("not on the roster");
    assert!(stranger.contains("hivemind_list_agents"), "{stranger}");
    assert_eq!(policy.manifest_id("acme--ceo").as_deref(), Some("ceo"));
}
