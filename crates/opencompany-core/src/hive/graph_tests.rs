//! Tests for the per-desk hive graph.

use std::collections::HashMap;

use openhuman_embed::AgentSpec;

use super::*;
use crate::harness::openhuman_runtime::{RuntimeBoot, global};
use crate::hive::test_support::{TWO_DESKS, record};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shared_seat_is_the_same_agent_in_both_hives_and_a_desk_of_one_gets_none() {
    let runtime = global(RuntimeBoot::ephemeral()).await.expect("runtime");
    let salt = uuid::Uuid::new_v4().simple().to_string();
    let agents: HashMap<String, openhuman_embed::Agent> = ["ceo", "engineer", "writer"]
        .into_iter()
        .map(|id| {
            (
                id.to_string(),
                runtime
                    .agent(AgentSpec::new(format!("hive-graph-{id}-{}", &salt[..8])))
                    .expect("agent"),
            )
        })
        .collect();
    let mut record = record(TWO_DESKS);
    // A third desk of one, and one whose only other seat is unbound.
    record.manifest.group_chats.push(crate::company::GroupChat {
        id: "solo".into(),
        name: "Solo".into(),
        description: None,
        members: vec!["writer".into()],
        tools: Vec::new(),
        hive: Default::default(),
    });
    let (hives, errors) = desk_hives(&record, 7, &|id| agents.get(id).cloned());
    assert!(errors.is_empty(), "{errors:?}");
    let mut ids: Vec<&String> = hives.keys().collect();
    ids.sort();
    assert_eq!(ids, vec!["content", "engineering"]);
    let engineering = &hives["engineering"];
    let content = &hives["content"];
    assert_eq!(engineering.members(), vec!["engineer", "ceo"]);
    assert_eq!(engineering.lead().as_deref(), Some("engineer"));
    assert_eq!(engineering.roster_version, 7);
    assert_eq!(engineering.desk_name, "Engineering desk");
    let shared_here = engineering.hive.bound_agent("ceo").expect("ceo bound");
    let shared_there = content.hive.bound_agent("ceo").expect("ceo bound");
    assert_eq!(
        shared_here.0.id(),
        shared_there.0.id(),
        "one runtime agent, two hives"
    );
    let candidate = engineering
        .hive
        .graph()
        .candidates
        .iter()
        .find(|candidate| candidate.id == "engineer")
        .expect("candidate");
    assert_eq!(candidate.role.as_deref(), Some("Engineer"));
    assert!(candidate.available);

    // An unbound member leaves the desk with one seat: no hive.
    let (hives, errors) = desk_hives(&record, 8, &|id| (id != "ceo").then(|| agents[id].clone()));
    assert!(errors.is_empty(), "{errors:?}");
    assert!(hives.is_empty(), "{:?}", hives.keys().collect::<Vec<_>>());
}
