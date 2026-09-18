use super::*;

fn workflow() -> Confinement {
    Confinement::workflow("weekly_report")
}

/// The refusal names the tool and the workflow, and tells the model what to
/// do instead. A bare "denied" would have it retry the same call.
#[test]
fn the_refusal_names_the_boundary() {
    let policy = ConfinedToolPolicy::new(workflow());
    let refusal = policy.refusal("query_company");
    assert!(refusal.contains("query_company"), "{refusal}");
    assert!(refusal.contains("weekly_report"), "{refusal}");
    assert!(refusal.contains("company chat"), "{refusal}");
}

/// Every tool is denied — including the intrinsic ones a roster agent always
/// carries, which is the reach this issue is about.
#[tokio::test]
async fn every_tool_is_denied() {
    let policy = ConfinedToolPolicy::new(workflow());
    for tool in [
        "query_company",
        "spawn_task",
        "delegate_to_desk",
        "memory_forget",
        "memory_recall",
        "memory_store",
        "workspace_read",
        "file_read",
        "web_fetch",
        "web_search",
        "mcp_registry_tool_call",
        "shell",
        // A name no belt has ever carried: a model that invents one is
        // refused by the same rule rather than falling through it.
        "definitely_not_a_tool",
    ] {
        let decision = policy.check(&request(tool)).await;
        match decision {
            ToolPolicyDecision::Deny { reason } => {
                assert!(reason.contains(tool), "{tool}: {reason}")
            }
            other => panic!("{tool} was not denied: {other:?}"),
        }
    }
}

/// The persona states the boundary and what to do at its edge, because that
/// is the half a model acts on. The enforcement is the policy above; this is
/// what keeps the answer honest rather than merely tool-less.
#[test]
fn the_persona_states_the_boundary_and_the_edge_case() {
    let persona = confined_persona("Acme", &workflow());
    assert!(persona.contains("weekly_report"), "{persona}");
    assert!(persona.contains("no tools"), "{persona}");
    assert!(persona.contains("company chat"), "{persona}");
    assert!(persona.contains("cannot change the workflow"), "{persona}");
}

/// Issue #415. The turn may PROPOSE, and the persona has to be exact about
/// what that is: text in a reply that the operator applies, not something
/// the agent did. A model that reads "propose" as "do" would report changes
/// that never landed, which is worse than a copilot that cannot propose at
/// all. The confinement is untouched by this — proposing calls nothing, and
/// the tool policy still refuses everything.
#[test]
fn the_persona_allows_proposing_and_forbids_claiming_to_have_applied() {
    let persona = confined_persona("Acme", &workflow());
    assert!(persona.contains("PROPOSE"), "{persona}");
    assert!(
        persona.contains("the operator reads it as a diff"),
        "{persona}"
    );
    assert!(
        persona.contains("never say you have made a change"),
        "{persona}"
    );
}

/// The confined context is a hole: nothing written to it can be read back,
/// by this turn or any later one.
#[tokio::test]
async fn the_confined_context_stores_nothing() {
    let store = ConfinedContext;
    let company = CompanyId::new("acme");
    store
        .put(
            &company,
            ContextChunk {
                label: "k".into(),
                body: "the company's private note".into(),
            },
        )
        .await
        .expect("a confined put is accepted");
    assert!(
        store
            .search(&company, "private", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.list(&company, "").await.unwrap().is_empty());
    assert!(
        store
            .peek(&company, &ChunkAddr::new("confined/k"), None)
            .await
            .unwrap()
            .is_empty()
    );
}

fn request(tool_name: &str) -> ToolPolicyRequest {
    let context = oh::agent::tool_policy::ToolCallContext::session(
        "copilot-session",
        "operator",
        CONFINED_AGENT_ID,
        "call-1",
        0,
    );
    ToolPolicyRequest::new(tool_name, serde_json::json!({}), context)
}
