use super::*;

fn answers() -> SetupAnswers {
    SetupAnswers {
        industry: "E-commerce — I sell homeware online".to_string(),
        team_hint: String::new(),
        automate: "Meta ads, order dispatch".to_string(),
    }
}

#[test]
fn a_fenced_answer_parses() {
    let draft = parse_draft(
        "Here you go:\n```json\n{\"agents\":[{\"name\":\"Ops\",\"role\":\"Operations \
         Manager\",\"description\":\"Keeps things moving.\"}]}\n```\nHope that helps!",
    )
    .expect("a fenced object parses");
    assert_eq!(draft.agents.len(), 1);
    assert_eq!(draft.agents[0].role, "Operations Manager");
}

/// A row missing a field must not discard the whole answer — the other rows
/// are still usable, and `validate_roster` is what judges the broken one.
#[test]
fn a_partial_row_does_not_discard_the_answer() {
    let draft = parse_draft("{\"agents\":[{\"role\":\"Analyst\"},{\"name\":\"X\"}]}")
        .expect("partial rows still parse");
    assert_eq!(draft.agents.len(), 2);
    assert_eq!(draft.agents[0].description, "");
}

/// Prose with no object, and an empty roster, are both "unreadable" — the
/// caller ships the template rather than guessing.
#[test]
fn an_unusable_answer_is_none() {
    assert!(parse_draft("I think you should hire a marketer.").is_none());
    assert!(parse_draft("{\"agents\":[]}").is_none());
    assert!(parse_draft("").is_none());
    assert!(parse_draft("}{").is_none());
}

/// The evidence handed to the model must actually contain the curated team
/// it is being asked to rewrite — without it the call is a blank-page
/// generation, which is the thing this pass exists not to be.
#[test]
fn the_prompt_carries_the_curated_team_and_the_answers() {
    let answers = answers();
    let template = match_template(&answers);
    let jobs = job_items(&answers.automate);
    let prompt = user_prompt(template, &answers, &jobs);
    assert!(prompt.contains("Logistics Coordinator"), "{prompt}");
    assert!(prompt.contains("ecommerce"), "{prompt}");
    // The jobs arrive NUMBERED, because the numbering is what the coverage
    // claim refers back to.
    assert!(prompt.contains("0. Meta ads"), "{prompt}");
    assert!(prompt.contains("1. order dispatch"), "{prompt}");
}

/// An unanswered question reads as unstated rather than as an empty
/// instruction, so the model is not left inferring meaning from a blank.
#[test]
fn an_unanswered_question_is_marked_unstated() {
    let prompt = user_prompt(
        match_template(&SetupAnswers::default()),
        &SetupAnswers::default(),
        &[],
    );
    assert!(prompt.contains("(not stated)"), "{prompt}");
}

/// The schema in the system prompt must agree with the bounds validation
/// enforces, or the model is being asked for something that will be
/// silently reshaped.
#[test]
fn the_system_prompt_states_the_real_bounds() {
    let prompt = system_prompt();
    assert!(prompt.contains(&MIN_AGENTS.to_string()), "{prompt}");
    assert!(prompt.contains(&MAX_AGENTS.to_string()), "{prompt}");
    assert!(prompt.contains(&MAX_DESCRIPTION.to_string()), "{prompt}");
}

// ---------------------------------------------------------------------
// Coverage: the host checks the claim against its own list
// ---------------------------------------------------------------------

use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tinyinference::Result as TaResult;
use tinyinference::model::ChatModel;

/// A model that answers from a script, one reply per call, and remembers the
/// prompts it was sent.
struct SequencedModel {
    replies: StdMutex<Vec<String>>,
    prompts: StdMutex<Vec<String>>,
    calls: AtomicUsize,
}

impl SequencedModel {
    fn new(replies: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            replies: StdMutex::new(replies.iter().rev().map(|r| (*r).to_string()).collect()),
            prompts: StdMutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn prompt(&self, index: usize) -> String {
        self.prompts
            .lock()
            .unwrap()
            .get(index)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl ChatModel<()> for SequencedModel {
    async fn invoke(&self, _state: &(), request: ModelRequest) -> TaResult<ModelResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.prompts.lock().unwrap().push(
            request
                .messages
                .iter()
                .map(|m| m.text())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        assert!(
            request.tools.is_empty(),
            "the setup pass must expose NO tools"
        );
        let reply = self.replies.lock().unwrap().pop().unwrap_or_default();
        Ok(ModelResponse::assistant(reply))
    }
}

impl HarnessModel for SequencedModel {
    fn telemetry_provider_id(&self) -> String {
        "managed".to_string()
    }
}

/// A model that cannot be reached: `invoke` errors, as a provider that is
/// down or a key the host refuses does. The pass must report this as an
/// unreachable model rather than "no model", because the operator's next
/// move differs — a key is already wired.
struct UnreachableModel;

#[async_trait]
impl ChatModel<()> for UnreachableModel {
    async fn invoke(&self, _state: &(), _request: ModelRequest) -> TaResult<ModelResponse> {
        Err(tinyinference::Error::Model(
            "provider refused the call".to_string(),
        ))
    }
}

impl HarnessModel for UnreachableModel {
    fn telemetry_provider_id(&self) -> String {
        "managed".to_string()
    }
}

/// Three jobs, so a gap is expressible.
fn three_jobs() -> SetupAnswers {
    SetupAnswers {
        industry: "I run a yoga studio and sell mats online".to_string(),
        team_hint: String::new(),
        automate: "class reminders, restocking mats, chasing invoices".to_string(),
    }
}

fn roster_json(rows: &[(&str, &str, &[usize])]) -> String {
    let agents: Vec<String> = rows
        .iter()
        .map(|(role, focus, covers)| {
            format!(
                r#"{{"name":"{role}","role":"{role}","description":"Owns it.","focus":"{focus}","covers":{covers:?}}}"#
            )
        })
        .collect();
    format!(r#"{{"agents":[{}]}}"#, agents.join(","))
}

fn builder(model: Arc<SequencedModel>) -> RosterBuilder {
    RosterBuilder::new(model, "test-model")
}

/// The happy path costs exactly one call. The check is free when the answer
/// is already right — a pass that always re-asked would double every
/// operator's wait to catch a minority case.
#[tokio::test]
async fn a_roster_that_owns_every_job_is_asked_for_once() {
    let model = SequencedModel::new(&[&roster_json(&[
        ("Bookings", "operations", &[0]),
        ("Stock", "operations", &[1]),
        ("Billing", "analysis", &[2]),
        ("Studio Ops", "operations", &[]),
    ])]);
    let (proposal, _) = builder(model.clone()).propose(&three_jobs()).await;

    assert_eq!(model.calls(), 1, "a covered roster must not be re-asked");
    assert!(proposal.uncovered.is_empty(), "{:?}", proposal.uncovered);
    assert_eq!(proposal.source, RosterSource::Model);
    assert_eq!(proposal.jobs.len(), 3);
    // The checklist reached the model numbered.
    assert!(
        model.prompt(0).contains("0. class reminders"),
        "{}",
        model.prompt(0)
    );
}

/// A gap buys exactly one more call, and the better roster wins.
#[tokio::test]
async fn a_gap_is_re_asked_once_and_the_covering_roster_wins() {
    let model = SequencedModel::new(&[
        // Nobody owns job 2 (chasing invoices).
        &roster_json(&[
            ("Bookings", "operations", &[0]),
            ("Stock", "operations", &[1]),
            ("Marketing", "writing", &[]),
            ("Studio Ops", "operations", &[]),
        ]),
        // The re-ask keeps the ORIGINAL numbering, so covering "chasing
        // invoices" is still a claim on job 2.
        &roster_json(&[
            ("Bookings", "operations", &[0]),
            ("Stock", "operations", &[1]),
            ("Billing", "analysis", &[2]),
            ("Studio Ops", "operations", &[]),
        ]),
    ]);
    let (proposal, _) = builder(model.clone()).propose(&three_jobs()).await;

    assert_eq!(model.calls(), 2, "a gap must buy exactly one more call");
    let reask = model.prompt(1);
    assert!(
        reask.contains("2. chasing invoices  <-- NOBODY OWNS THIS"),
        "the re-ask must mark the gap IN PLACE, keeping the first ask's \
         numbering: {reask}"
    );
    assert!(proposal.agents.iter().any(|a| a.role == "Billing"));
    assert!(proposal.uncovered.is_empty(), "{:?}", proposal.uncovered);
}

/// Two is the ceiling. A third phrasing of the same request is a
/// conversation, and this pass runs while somebody waits.
#[tokio::test]
async fn an_unowned_job_is_reported_rather_than_re_asked_forever() {
    let thin = roster_json(&[
        ("Bookings", "operations", &[0]),
        ("Stock", "operations", &[1]),
        ("Marketing", "writing", &[]),
        ("Studio Ops", "operations", &[]),
    ]);
    let model = SequencedModel::new(&[&thin, &thin]);
    let (proposal, _) = builder(model.clone()).propose(&three_jobs()).await;

    assert_eq!(model.calls(), 2, "never more than one re-ask");
    assert_eq!(proposal.uncovered, vec!["chasing invoices"]);
    assert_eq!(
        proposal.source,
        RosterSource::Model,
        "an honest gap is still a designed team, not a fallback"
    );
}

/// A dropped agent takes its claim with it. Counting the claim of a
/// teammate validation removed would report a gap as covered — the exact
/// failure a self-reported check invites.
#[tokio::test]
async fn a_claim_dies_with_the_duplicate_that_made_it() {
    // Two `Bookings` rows: the second is dropped as a duplicate role, and its
    // claim on job 2 must not survive it.
    let first = format!(
        r#"{{"agents":[{},{},{},{},{}]}}"#,
        r#"{"name":"Bookings","role":"Bookings","description":"d","focus":"operations","covers":[0]}"#,
        r#"{"name":"Stock","role":"Stock","description":"d","focus":"operations","covers":[1]}"#,
        r#"{"name":"Bookings","role":"bookings","description":"d","focus":"operations","covers":[2]}"#,
        r#"{"name":"Ops","role":"Ops","description":"d","focus":"operations","covers":[]}"#,
        r#"{"name":"Front","role":"Front Desk","description":"d","focus":"operations","covers":[]}"#
    );
    let model = SequencedModel::new(&[&first, &first]);
    let (proposal, _) = builder(model.clone()).propose(&three_jobs()).await;

    assert_eq!(
        proposal.uncovered,
        vec!["chasing invoices"],
        "the dropped duplicate's claim must not count"
    );
}

/// The focus the model chose reaches the proposal, because it is what
/// decides the teammate's tool belt.
#[tokio::test]
async fn the_focus_reaches_the_proposal() {
    let model = SequencedModel::new(&[&roster_json(&[
        ("Bookings", "operations", &[0]),
        ("Stock", "operations", &[1]),
        ("Billing", "analysis", &[2]),
        ("Research", "research", &[]),
    ])]);
    let (proposal, _) = builder(model).propose(&three_jobs()).await;

    let research = proposal
        .agents
        .iter()
        .find(|a| a.role == "Research")
        .unwrap();
    assert_eq!(research.focus, Some(AgentFocus::Research));
    // And an invented one costs that teammate its narrowing, nothing more.
    assert!(proposal.agents.iter().all(|a| a.role != "Nonsense"));
}

/// The system prompt must name the focus vocabulary it expects back, or the
/// model is being asked for a value from a list it was never shown.
#[test]
fn the_system_prompt_states_the_focus_vocabulary() {
    let prompt = system_prompt();
    for focus in AgentFocus::ALL {
        assert!(
            prompt.contains(focus.as_str()),
            "{} missing",
            focus.as_str()
        );
    }
    assert!(prompt.contains("covers"), "{prompt}");
}

/// The prompt must actually ask for the operator's words. The reference team
/// was being copied sentence-for-sentence — three of six mandates in a real
/// run were the template's, one of them verbatim — which made half a
/// designed roster indistinguishable from a canned one.
#[test]
fn the_system_prompt_forbids_reusing_the_reference_sentences() {
    let prompt = system_prompt();
    let lower = prompt.to_lowercase();
    assert!(lower.contains("own terms"), "{prompt}");
    assert!(lower.contains("do not reuse"), "{prompt}");
}

/// A model that hands the reference team straight back is reported as
/// **curated**, not designed. The line-up is the substantive claim, and
/// "built from what you told us" is the one sentence on the review screen an
/// operator cannot check for themselves.
#[tokio::test]
async fn the_reference_team_handed_back_is_reported_as_curated() {
    let answers = SetupAnswers {
        industry: "I sell homeware online".to_string(),
        team_hint: String::new(),
        automate: "meta ads, dispatch".to_string(),
    };
    // Exactly the ecommerce reference team, which is what this pass is shown.
    let echoed = roster_json(&[
        ("Meta Ads Specialist", "operations", &[0]),
        ("SEO Specialist", "analysis", &[]),
        ("Logistics Coordinator", "operations", &[1]),
        ("Fulfillment Manager", "operations", &[]),
        ("Accountant", "analysis", &[]),
    ]);
    let model = SequencedModel::new(&[&echoed]);
    let (proposal, _) = builder(model).propose(&answers).await;

    assert_eq!(
        proposal.source,
        RosterSource::Fallback,
        "a copy of the reference team must not be reported as designed"
    );
    // The jobs still ride along: they are the operator's own words, and the
    // review screen shows them whichever way the roster was produced.
    assert_eq!(proposal.jobs, vec!["meta ads", "dispatch"]);
}

/// The copy bug the vague-input test exposed: every fallback reported
/// "we couldn't reach a model", including the two where a model answered
/// fine and its answer was unusable. The operator was then pointed at adding
/// a key when what they needed was to say more.
#[tokio::test]
async fn an_unusable_answer_reports_a_different_reason_than_an_unreachable_model() {
    let answers = SetupAnswers {
        industry: "just me and my laptop".to_string(),
        team_hint: String::new(),
        automate: "everything honestly".to_string(),
    };

    // Reached, answered, and the answer was the reference team verbatim.
    let echoed = roster_json(&[
        ("Operations Lead", "operations", &[]),
        ("Researcher", "research", &[]),
        ("Writer", "writing", &[]),
        ("Analyst", "analysis", &[]),
        ("Support Specialist", "operations", &[]),
    ]);
    let (proposal, _) = builder(SequencedModel::new(&[&echoed]))
        .propose(&answers)
        .await;
    assert_eq!(proposal.source, RosterSource::Fallback);
    assert_eq!(
        proposal.reason,
        Some(FallbackReason::NotDesignable),
        "a model that answered must not be reported as unreachable"
    );

    // Reached, answered, unreadable — same reason, same next step.
    let (proposal, _) = builder(SequencedModel::new(&["not json at all"]))
        .propose(&answers)
        .await;
    assert_eq!(proposal.reason, Some(FallbackReason::NotDesignable));
}

/// A call that never lands is not "no model": a builder exists (that is why
/// the call was made), so the operator's next move is to retry or check the
/// provider, not to add a key that is already wired.
#[tokio::test]
async fn an_unreachable_call_reports_unreachable_not_no_model() {
    let answers = SetupAnswers {
        industry: "I sell homeware online".to_string(),
        team_hint: String::new(),
        automate: "Meta ads, order dispatch".to_string(),
    };
    let (proposal, _) = RosterBuilder::new(Arc::new(UnreachableModel), "test-model")
        .propose(&answers)
        .await;
    assert_eq!(proposal.source, RosterSource::Fallback);
    assert_eq!(
        proposal.reason,
        Some(FallbackReason::ModelUnreachable),
        "a configured but unreachable model must not be reported as no_model"
    );
    assert_eq!(
        proposal.reason.map(|r| r.as_str()),
        Some("model_unreachable"),
        "the wire spelling must round-trip"
    );
}

/// A designed roster reports no reason at all — there is nothing to explain.
#[tokio::test]
async fn a_designed_roster_carries_no_fallback_reason() {
    let model = SequencedModel::new(&[&roster_json(&[
        ("Bookings", "operations", &[0]),
        ("Stock", "operations", &[1]),
        ("Billing", "analysis", &[2]),
        ("Studio Ops", "operations", &[]),
    ])]);
    let (proposal, _) = builder(model).propose(&three_jobs()).await;
    assert_eq!(proposal.source, RosterSource::Model);
    assert_eq!(proposal.reason, None);
}
