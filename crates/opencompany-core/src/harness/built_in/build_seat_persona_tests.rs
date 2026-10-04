use super::*;
use oh::agent::prompts::GROUNDING_HEADING;

const STYLE_HEADING: &str = "# Writing style";

fn blueprint(dir: &std::path::Path, is_orchestrator: bool) -> AgentBlueprint {
    let deps = pin_deps(dir.to_path_buf());
    let manifest_agent = ManifestAgent {
        provider: None,
        global: false,
        id: "qa_engineer".to_string(),
        role: "QA Engineer".to_string(),
        name: Some("Quinn".to_string()),
        description: Some("Finds the failing case.".to_string()),
        tier: None,
        harness: None,
        tools: None,
        skills: None,
        delegates_to: Vec::new(),
        context: None,
        budget_usd_daily: None,
        prompt: None,
        prompt_files: Vec::new(),
        prompt_files_resolved: Vec::new(),
        classes: Vec::new(),
        ledgers: None,
        can_declare_ledgers: true,
        model: None,
    };
    build_agent(
        &CompanyId::new("acme"),
        "Acme",
        &manifest_agent,
        Arc::new(ApprovalPolicy::new(&Policy::default(), None)),
        &deps,
        &[],
        &[],
        &[],
        None,
        is_orchestrator,
    )
    .expect("agent builds")
}

#[test]
fn the_reader_brief_sets_the_audience_not_a_length_budget() {
    assert!(READER_BRIEF.contains("A person reads what you post"));
    assert!(READER_BRIEF.contains("belong in tool calls"));
    assert!(
        !READER_BRIEF.contains('—'),
        "OpenHuman's style forbids em-dashes"
    );
    for budget in ["word limit", "at most", "no more than", "concise"] {
        assert!(
            !READER_BRIEF.to_lowercase().contains(budget),
            "`{budget}` reads as a length rule: {READER_BRIEF}"
        );
    }
}

#[test]
fn the_mention_brief_models_a_name_not_a_roster_id() {
    let examples: Vec<&str> = MENTION_BRIEF.split('"').skip(1).step_by(2).collect();
    assert!(
        !examples.is_empty(),
        "the brief shows an example: {MENTION_BRIEF}"
    );
    for example in examples {
        let id_like = example
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .any(|token| token.contains('_') || token.contains('-'));
        assert!(!id_like, "the example reads as a roster id: {example}");
    }
}
