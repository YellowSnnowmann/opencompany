use super::*;

fn candidates() -> Vec<SelectorCandidate> {
    vec![
        SelectorCandidate {
            id: "backend_engineer".to_string(),
            role: "Backend Engineer".to_string(),
            description: Some("Owns the API surface.".to_string()),
            tools: Vec::new(),
        },
        SelectorCandidate {
            id: "designer".to_string(),
            role: "Product Designer".to_string(),
            description: None,
            tools: Vec::new(),
        },
    ]
}

/// The decoration small models add around a token — quotes, backticks, a
/// trailing period, case drift — parses to the candidate's own id.
#[test]
fn parse_tolerates_decoration_and_answers_the_candidates_casing() {
    for reply in [
        "backend_engineer",
        " backend_engineer \n",
        "\"backend_engineer\"",
        "`backend_engineer`",
        "Backend_Engineer.",
    ] {
        assert_eq!(
            SelectorVerdict::parse(reply, &candidates()),
            SelectorVerdict::Member("backend_engineer".to_string()),
            "reply {reply:?} should parse"
        );
    }
}

/// An id outside the membership is `Unavailable`, never routed verbatim —
/// an out-of-set pick would address a turn to a teammate the channel does
/// not contain. Revert the clamp in [`SelectorVerdict::parse`] and this
/// answers `Member("ceo")`.
#[test]
fn parse_clamps_to_the_candidate_set() {
    assert_eq!(
        SelectorVerdict::parse("ceo", &candidates()),
        SelectorVerdict::Unavailable
    );
    assert_eq!(
        SelectorVerdict::parse("", &candidates()),
        SelectorVerdict::Unavailable
    );
    assert_eq!(
        SelectorVerdict::parse(
            "backend_engineer because the message is about the API",
            &candidates()
        ),
        SelectorVerdict::Unavailable,
        "an answer with an explanation attached is not an id"
    );
}

/// The request lays the membership out id-first, so the only valid answer
/// tokens are on the page, and frames the message as the thing being
/// routed rather than instructions to follow.
#[test]
fn selection_request_names_ids_roles_and_the_message() {
    let request = selection_request("who owns the login flow?", &candidates());
    assert!(request.contains("- backend_engineer — Backend Engineer: Owns the API surface."));
    assert!(request.contains("- designer — Product Designer\n"));
    assert!(request.contains("Message:\nwho owns the login flow?"));
}
/// **Capability is routed on, not just prose.**
///
/// The failure this exists for: "fetch the latest issues of <repo>" was
/// routed to a product manager on role and description alone, and could
/// never have succeeded — that teammate holds no `composio` grant, so it
/// cannot reach GitHub at all. The one member that could was reachable only
/// by being named. A selector told what each member can USE has the fact
/// that decides the question; one told only what they are FOR does not.
#[test]
fn selection_request_says_what_each_member_can_use() {
    let candidates = vec![
        SelectorCandidate {
            id: "product_manager".to_string(),
            role: "Product Manager".to_string(),
            description: Some("Owns the roadmap.".to_string()),
            tools: vec!["workspace.read".to_string()],
        },
        SelectorCandidate {
            id: "support_specialist".to_string(),
            role: "Support Specialist".to_string(),
            description: None,
            tools: vec!["composio".to_string(), "web.*".to_string()],
        },
    ];
    let request = selection_request("fetch the latest open issues", &candidates);
    assert!(
        request.contains("- support_specialist — Support Specialist [can use: composio, web.*]"),
        "the member that can reach GitHub says so: {request}"
    );
    assert!(
        request.contains(
            "- product_manager — Product Manager: Owns the roadmap. [can use: workspace.read]"
        ),
        "and capability rides beside the mandate, not instead of it: {request}"
    );
}

/// A company that grants nothing renders exactly the prompt it did before
/// this field existed — an empty list is absent, never `[can use: ]`.
#[test]
fn a_member_with_no_grants_renders_no_capability_clause() {
    let request = selection_request("who owns the login flow?", &candidates());
    assert!(!request.contains("can use"), "{request}");
}
