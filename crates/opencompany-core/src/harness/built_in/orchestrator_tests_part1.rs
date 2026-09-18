use super::*;

/// Issue #267: the brief's **shape** is the thing under test, because the
/// shape is what the model followed. Answering has to lead, the
/// never-a-card rule has to be stated rather than implied, authoring a
/// workflow has to read as something done in this turn, and the whole thing
/// has to be no longer than the version it replaced — a "rebalance" that
/// grew the brief would just be more prose competing with the lead.
#[test]
fn the_brief_leads_with_answering_and_did_not_grow() {
    let brief = orchestrator_brief();

    // The default leads. Measured by position, not by presence: the old
    // brief contained the same rule as its closing clause and behaviour
    // followed the enumeration instead.
    let answer_first = brief
        .find("MOST MESSAGES ARE QUESTIONS OR QUICK READS")
        .expect("the answering default is stated");
    for later in [
        "delegate_to_desk",
        "spawn_task",
        "create_workflow",
        "add_agent",
        "assign_task",
        "review_task",
    ] {
        let at = brief.find(later).unwrap_or_else(|| panic!("names {later}"));
        assert!(
            answer_first < at,
            "`{later}` is introduced before the answering default"
        );
    }

    assert!(
        brief.contains("is NEVER a card"),
        "the never-a-card rule must be stated, not implied: {brief}"
    );
    // The #442 two-decisions block survives the restructure.
    assert!(brief.contains("they are INDEPENDENT"), "{brief}");
    assert!(brief.contains("the hand-off IS the card"), "{brief}");
    // A "create a workflow" ask is authored now, not parked.
    assert!(
        brief.contains("author it NOW with `create_workflow`"),
        "the automate path must read as this-turn work: {brief}"
    );

    // The length of the brief this replaced. A ceiling, not a target.
    const PREVIOUS_LEN: usize = 2784;
    assert!(
        brief.len() <= PREVIOUS_LEN,
        "the brief grew to {} (was {PREVIOUS_LEN})",
        brief.len()
    );
}

/// Issue #276: both directions of the arming summary, including the name
/// and id, and neither the actor nor the reason.
#[test]
fn an_arming_change_summarizes_in_both_directions_without_the_actor_or_the_reason() {
    let event = |enabled, reason| CompanyEvent::WorkflowEnabledChanged {
        workflow_id: "digest".to_string(),
        name: "Daily digest".to_string(),
        enabled,
        reason,
        by: Some(crate::ports::types::Actor {
            kind: crate::ports::types::ActorKind::User,
            id: "u_secret".to_string(),
        }),
    };

    let off = summarize_event(&event(
        false,
        crate::ports::types::WorkflowEnabledReason::Disarmed,
    ));
    assert!(off.contains("switched off"), "{off}");
    assert!(off.contains("Daily digest"), "{off}");
    assert!(off.contains("digest"), "{off}");

    let on = summarize_event(&event(
        true,
        crate::ports::types::WorkflowEnabledReason::Operator,
    ));
    assert!(on.contains("switched on"), "{on}");
    assert!(on.contains("Daily digest"), "{on}");

    // The actor id never reaches the insight surface, and neither does the
    // rule-vs-person distinction — see the arm's comment.
    for summary in [&off, &on] {
        assert!(!summary.contains("u_secret"), "{summary}");
        assert!(!summary.contains("disarm"), "{summary}");
        assert!(!summary.contains("operator"), "{summary}");
    }
}

/// **Issue #248, the insight-surface twin of the sidecar guard.** This
/// one-liner is folded into the orchestrator's recent-activity context, so
/// it is read by a model rather than by the tenant. A delivery row's
/// `target` is a recipient's address and its `detail` quotes one when the
/// transport refuses, so neither may appear here. The exclusion was written
/// this way by #228; this pins it.
#[test]
fn a_finished_run_summarizes_to_counts_without_the_recipient_or_transport_text() {
    // `.invalid` is reserved by RFC 2606, so this fixture names nobody.
    const RECIPIENT: &str = "recipient@example.invalid";

    let summary = summarize_event(&CompanyEvent::WorkflowRunFinished {
        workflow_id: "digest".to_string(),
        scheduled: true,
        run_id: None,
        deliveries: vec![crate::ports::DeliveryReport {
            node: "owner_summary".to_string(),
            kind: "email".to_string(),
            target: Some(RECIPIENT.to_string()),
            status: crate::ports::DeliveryStatus::Failed,
            detail: format!(
                "the mail transport refused the message: 550 5.1.1 <{RECIPIENT}>: Recipient \
                 address rejected"
            ),
            reason: crate::ports::DeliveryReason::MailTransportRefused,
        }],
        pending_approvals: Vec::new(),
        error: None,
        cancelled: false,
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    });

    assert!(!summary.contains(RECIPIENT), "{summary}");
    assert!(!summary.contains("recipient@"), "{summary}");
    assert!(!summary.contains("Recipient address rejected"), "{summary}");
    assert!(!summary.contains("550"), "{summary}");
    // Still useful: which workflow, and that something did not go out.
    assert!(summary.contains("digest"), "{summary}");
    assert!(summary.contains("1 not delivered"), "{summary}");
}

/// **Issue #383, the twin of the sidecar's pin.** The insight tail is the
/// other non-tenant reader of a finished run, and it had the same hole: a
/// cancelled run carries no error, so it summarized as a clean finish and
/// invited the orchestrator to reason about — or redo — work an operator had
/// just stopped.
#[test]
fn a_cancelled_run_summarizes_as_stopped_rather_than_finished() {
    let summary = summarize_event(&CompanyEvent::WorkflowRunFinished {
        workflow_id: "digest".to_string(),
        scheduled: true,
        run_id: Some("run-1".to_string()),
        deliveries: Vec::new(),
        pending_approvals: Vec::new(),
        error: None,
        cancelled: true,
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    });

    assert!(summary.contains("stopped"), "{summary}");
    assert!(
        !summary.contains("finished"),
        "a stopped run must not read as a finished one: {summary}"
    );
}

/// **Issue #327.** A workspace write summarizes structurally — the change
/// word and the node id, nothing else.
///
/// The node's *name* is the exclusion with teeth. It is operator-authored
/// free text that routinely carries the substance of the note ("Q3 layoffs
/// shortlist"), and this string is a non-sensitive one-liner for the
/// insight surface, which is precisely where free text does not belong.
/// Same reasoning as the recipient exclusion two tests up; the arm was
/// written this way, and this is what pins it.
#[test]
fn a_workspace_write_summarizes_to_the_change_and_node_without_the_notes_name() {
    let summary = summarize_event(&CompanyEvent::WorkspaceChanged {
        node_id: "n-42".to_string(),
        change: "updated".to_string(),
    });

    // Exact, not `contains`: the whole claim is that nothing *else* is in
    // here. A future arm that looked the node up to add its name would keep
    // passing every `contains` assertion and fail this one.
    assert_eq!(summary, "workspace updated: n-42");
}

#[test]
fn orchestrator_id_prefers_the_tagged_agent() {
    let roster = vec![
        agent("ceo", None),
        agent("chief", Some("orchestrator")),
        agent("eng", Some("reasoning")),
    ];
    assert_eq!(orchestrator_id(&roster).as_deref(), Some("chief"));
}

#[test]
fn orchestrator_id_falls_back_to_first_agent() {
    let roster = vec![agent("ceo", None), agent("eng", None)];
    assert_eq!(orchestrator_id(&roster).as_deref(), Some("ceo"));
}

#[test]
fn orchestrator_id_is_none_for_an_empty_roster() {
    assert_eq!(orchestrator_id(&[]), None);
}

#[test]
fn delegation_tool_names_are_classified_internal() {
    assert!(is_delegation_tool(SPAWN_TASK_TOOL));
    assert!(is_delegation_tool(DELEGATE_TO_DESK_TOOL));
    assert!(is_delegation_tool(ADD_AGENT_TOOL));
    assert!(is_delegation_tool(CREATE_WORKFLOW_TOOL));
    // The read tool is NOT a delegation tool.
    assert!(!is_delegation_tool(QUERY_COMPANY_TOOL));
    assert!(!is_delegation_tool("send_email"));
}

#[test]
fn queue_drains_fifo_up_to_cap_and_discards_the_rest() {
    let queue = DelegationQueue::default();
    for i in 0..5 {
        queue.push(Delegation::SpawnTask {
            title: format!("t{i}"),
            note: None,
            assignee: None,
        });
    }
    assert_eq!(queue.queued(), 5);
    let drained = queue.drain(MAX_DELEGATIONS_PER_TURN);
    assert_eq!(drained.len(), 3);
    // The first three (FIFO) survive; the queue is emptied.
    assert_eq!(
        drained[0],
        Delegation::SpawnTask {
            title: "t0".to_string(),
            note: None,
            assignee: None,
        }
    );
    assert_eq!(queue.queued(), 0);
}

#[test]
fn clear_empties_the_queue() {
    let queue = DelegationQueue::default();
    queue.push(Delegation::DelegateToDesk {
        desk: "strategy".to_string(),
        instruction: "plan".to_string(),
    });
    queue.clear();
    assert_eq!(queue.queued(), 0);
}

/// The queue itself refuses past the cap rather than accepting work the
/// drain will destroy.
#[test]
fn push_within_cap_refuses_once_the_turn_is_full() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    for i in 0..MAX_DELEGATIONS_PER_TURN {
        assert_eq!(
            queue.push_within_cap(
                Delegation::SpawnTask {
                    title: format!("t{i}"),
                    note: None,
                    assignee: None,
                },
                MAX_DELEGATIONS_PER_TURN,
                NO_DEPTH_BOUND,
            ),
            Staged::Queued
        );
    }
    assert_eq!(
        queue.push_within_cap(
            Delegation::SpawnTask {
                title: "one too many".to_string(),
                note: None,
                assignee: None,
            },
            MAX_DELEGATIONS_PER_TURN,
            NO_DEPTH_BOUND,
        ),
        Staged::OverCap
    );
    assert_eq!(queue.queued(), MAX_DELEGATIONS_PER_TURN);
}

/// The commitment is checked before the cap, and it is the answer an
/// unclaimed queue gives however empty it is. A model told "this turn is
/// full" would try again next turn; on an unclaimed path the next turn fails
/// identically, so the two refusals must stay distinguishable.
#[test]
fn an_unclaimed_queue_refuses_before_the_cap_is_even_consulted() {
    let queue = DelegationQueue::default();
    assert!(!queue.drain_committed(), "uncommitted is the default");
    assert_eq!(
        queue.push_within_cap(
            Delegation::SpawnTask {
                title: "first and only".to_string(),
                note: None,
                assignee: None,
            },
            MAX_DELEGATIONS_PER_TURN,
            NO_DEPTH_BOUND,
        ),
        Staged::NoDrain(NoDrainReason::Unwired),
        "an EMPTY unclaimed queue is still a queue nothing drains"
    );
    assert_eq!(queue.queued(), 0);
}

/// The RAII half, which is the one that was missing everywhere before #453:
/// an early exit — a `?`, a panic, a `return` from the middle of a turn —
/// must leave the queue empty and uncommitted, so the *next* caller inherits
/// a refusal rather than this one's abandoned work.
#[test]
fn a_claim_that_exits_early_un_commits_and_clears() {
    let queue = DelegationQueue::default();

    // A turn that queues work and then bails before draining.
    fn bail(queue: &DelegationQueue) -> Result<(), &'static str> {
        let _claim = queue.claim();
        assert_eq!(
            queue.push_within_cap(
                Delegation::ReviewTask {
                    task_id: "t1".to_string(),
                    decision: ReviewDecision::Approve,
                    note: None,
                },
                MAX_DELEGATIONS_PER_TURN,
                NO_DEPTH_BOUND,
            ),
            Staged::Queued
        );
        assert_eq!(queue.queued(), 1, "staged while the claim is live");
        Err("the turn failed after queuing")
    }

    assert!(bail(&queue).is_err());
    assert_eq!(
        queue.queued(),
        0,
        "the abandoned delegation must not survive the claim that staged it"
    );
    assert!(
        !queue.drain_committed(),
        "and the next caller must inherit a refusal, not this one's promise"
    );

    // Acquiring also clears, so a prior turn's leftovers can never be
    // executed for the caller that comes next.
    queue.push(Delegation::SpawnTask {
        title: "left behind".to_string(),
        note: None,
        assignee: None,
    });
    let _claim = queue.claim();
    assert_eq!(queue.queued(), 0);
}

/// The headline refusal, per tool, with the sentence each one owes the
/// model. The effect clause is the tool's own — a generic "refused" would
/// leave the model guessing which of its calls did not happen.
#[tokio::test]
async fn every_delegation_tool_refuses_when_nothing_will_drain() {
    let queue = DelegationQueue::default();
    let store =
        Arc::new(MemStore::seeded(desks_record(&CompanyId::new("acme")))) as Arc<dyn CompanyStore>;

    let cases: Vec<(Box<dyn Tool>, Value, &str)> = vec![
        (
            Box::new(SpawnTaskTool::new(
                queue.clone(),
                CompanyId::new("acme"),
                store.clone(),
            )),
            json!({ "title": "Ship it" }),
            "the card \"Ship it\" was NOT opened",
        ),
        (
            Box::new(DelegateToDeskTool::new(
                queue.clone(),
                CompanyId::new("acme"),
                store,
            )),
            json!({ "desk": "strategy", "instruction": "draft a plan" }),
            "nothing was handed to the strategy desk",
        ),
        (
            Box::new(AssignTaskTool::new(queue.clone())),
            json!({ "task_id": "t1", "assignee": "writer" }),
            "card t1 was NOT assigned",
        ),
        (
            Box::new(ReviewTaskTool::new(queue.clone())),
            json!({ "task_id": "t1", "decision": "approve" }),
            "card t1 was NOT reviewed",
        ),
    ];

    for (tool, args, effect) in cases {
        let name = tool.name().to_string();
        let result = tool.execute(args).await.expect("execute");
        assert!(result.is_error, "{name} must refuse: {}", result.text());
        let text = result.text();
        assert!(text.contains(effect), "{name}: {text}");
        // Not retryable — the next turn on this path drains no better.
        assert!(text.contains("Do not retry"), "{name}: {text}");
        // And the model must not narrate it as done, which is the whole
        // failure this replaces.
        assert!(text.contains("report the action as done"), "{name}: {text}");
        // Deliberately NOT the cap sentence: this is a different problem
        // with a different remedy.
        assert!(!text.contains("delegations"), "{name}: {text}");
    }
    assert_eq!(queue.queued(), 0, "nothing may be staged by a refusal");
}

/// **Issue #267 review, finding 3.** The two no-drain causes stop sharing a
/// sentence.
///
/// Written for a genuinely inert context, the refusal was then inherited by
/// a fully capable company whose triage read the message as a question —
/// where "board actions are unavailable in this context" is simply false as
/// the operator will hear it. Paired with a triage miss the experience was
/// *ask for a landing page → "I could not do it; board actions are
/// unavailable"*, with nothing to suggest that rephrasing would work.
///
/// The halves both causes need stay on both; what differs is what the model
/// is told happened, and what it can offer next.
#[tokio::test]
async fn the_triage_refusal_says_it_read_a_question_and_offers_a_way_forward() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim_answering();
    let refused = SpawnTaskTool::new(
        queue.clone(),
        CompanyId::new("acme"),
        Arc::new(MemStore::default()),
    )
    .execute(json!({ "title": "Build the landing page" }))
    .await
    .expect("execute");
    assert!(refused.is_error, "{}", refused.text());
    let text = refused.text();

    assert!(
        text.contains("read as a question"),
        "it must name what actually happened: {text}"
    );
    assert!(
        text.contains("this message only"),
        "…and scope it to this message, not to the whole context: {text}"
    );
    assert!(
        text.contains("restate it"),
        "…and leave the model something recoverable to offer: {text}"
    );
    // The two claims that are false here, and were the whole complaint.
    assert!(
        !text.contains("nothing here can carry out board work"),
        "a capable company must not claim it cannot do board work: {text}"
    );
    assert!(
        !text.contains("unavailable in this context"),
        "the context is fine; the message was a question: {text}"
    );
    // …while everything both causes owe the model survives.
    assert!(text.contains("Do not retry"), "{text}");
    assert!(text.contains("report the action as done"), "{text}");
    assert!(
        text.contains("the card \"Build the landing page\" was NOT opened"),
        "the tool's own effect clause is untouched: {text}"
    );
    assert_eq!(queue.queued(), 0);
}

/// …and the inert-context refusal keeps saying the thing that is true only
/// of it, so the split is a split rather than a rename.
#[tokio::test]
async fn the_unwired_refusal_still_says_the_context_cannot_do_board_work() {
    let queue = DelegationQueue::default();
    let refused = SpawnTaskTool::new(
        queue.clone(),
        CompanyId::new("acme"),
        Arc::new(MemStore::default()),
    )
    .execute(json!({ "title": "Ship it" }))
    .await
    .expect("execute");
    let text = refused.text();
    assert!(refused.is_error, "{text}");
    assert!(
        text.contains("nothing here can carry out board work"),
        "{text}"
    );
    assert!(!text.contains("read as a question"), "{text}");
}

/// The measurement finding 3 asks for: the two causes are distinguishable
/// as data, not only as prose. Without this the rate at which the triage
/// gate fires — the residual miss rate of a keyword classifier with teeth —
/// could not be counted apart from a genuinely unwired context.
#[test]
fn the_two_no_drain_causes_are_countable_apart() {
    let queue = DelegationQueue::default();
    let spawn = || Delegation::SpawnTask {
        title: "Build the landing page".to_string(),
        note: None,
        assignee: None,
    };
    assert_eq!(
        queue.push_within_cap(spawn(), MAX_DELEGATIONS_PER_TURN, NO_DEPTH_BOUND),
        Staged::NoDrain(NoDrainReason::Unwired)
    );
    let claim = queue.claim_answering();
    assert_eq!(
        queue.push_within_cap(spawn(), MAX_DELEGATIONS_PER_TURN, NO_DEPTH_BOUND),
        Staged::NoDrain(NoDrainReason::Triage)
    );
    // …and a hand-off is not refused at all under the same claim, because it
    // is how the question gets answered (finding 2).
    assert_eq!(
        queue.push_within_cap(
            Delegation::DelegateToDesk {
                desk: "eng".to_string(),
                instruction: "what did you ship?".to_string(),
            },
            MAX_DELEGATIONS_PER_TURN,
            NO_DEPTH_BOUND,
        ),
        Staged::Queued
    );
    drop(claim);
    assert_ne!(
        NoDrainReason::Unwired.as_str(),
        NoDrainReason::Triage.as_str(),
        "the log field must separate them"
    );
}

/// The defect #419 names: the tool told the model "it will be opened on the
/// board this turn" for a card the drain then threw away, so a turn asked
/// for five cards, reported five, and left two. The call past the cap is now
/// an **error** naming the bound, and nothing is queued.
#[tokio::test]
async fn spawn_task_refuses_past_the_cap_instead_of_promising_a_discarded_card() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = SpawnTaskTool::new(
        queue.clone(),
        CompanyId::new("acme"),
        Arc::new(MemStore::default()),
    );
    for i in 0..MAX_DELEGATIONS_PER_TURN {
        let ok = tool
            .execute(json!({ "title": format!("item {i}") }))
            .await
            .expect("execute");
        assert!(!ok.is_error, "within the cap: {}", ok.text());
    }
    let refused = tool
        .execute(json!({ "title": "the fourth item" }))
        .await
        .expect("execute");
    assert!(refused.is_error, "{}", refused.text());
    let text = refused.text();
    assert!(text.contains("the fourth item"), "{text}");
    assert!(text.contains("NOT opened"), "{text}");
    assert!(
        text.contains(&MAX_DELEGATIONS_PER_TURN.to_string()),
        "the refusal names the bound: {text}"
    );
    // The queue is exactly full — the refusal queued nothing, so the drain
    // has nothing left over to destroy.
    assert_eq!(queue.queued(), MAX_DELEGATIONS_PER_TURN);
    assert_eq!(queue.drain(MAX_DELEGATIONS_PER_TURN).len(), 3);
}

/// Same for the hand-off tool, whose success line ("Its lead will answer
/// this turn") was the more misleading of the two: it claimed a teammate had
/// been given work nobody would ever run.
#[tokio::test]
async fn delegate_to_desk_refuses_past_the_cap() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    // An empty store loads no record, so desk grounding fails open and the
    // hand-off is queued exactly as it was before #272 — which isolates this
    // test to the cap.
    let store = Arc::new(MemStore::default()) as Arc<dyn CompanyStore>;
    let tool = DelegateToDeskTool::new(queue.clone(), CompanyId::new("acme"), store);
    for i in 0..MAX_DELEGATIONS_PER_TURN {
        let ok = tool
            .execute(json!({ "desk": "eng", "instruction": format!("item {i}") }))
            .await
            .expect("execute");
        assert!(!ok.is_error, "within the cap: {}", ok.text());
    }
    let refused = tool
        .execute(json!({ "desk": "eng", "instruction": "one more" }))
        .await
        .expect("execute");
    assert!(refused.is_error, "{}", refused.text());
    assert!(
        refused.text().contains("nothing was handed"),
        "{}",
        refused.text()
    );
    assert_eq!(queue.queued(), MAX_DELEGATIONS_PER_TURN);
}

/// The two board-lifecycle tools share the queue and therefore the cap, so
/// they share the refusal — an `assign_task` that silently did not assign is
/// the same defect wearing a different hat.
#[tokio::test]
async fn the_lifecycle_tools_refuse_past_the_cap_too() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let assign = AssignTaskTool::new(queue.clone());
    let review = ReviewTaskTool::new(queue.clone());
    for i in 0..MAX_DELEGATIONS_PER_TURN {
        assign
            .execute(json!({ "task_id": format!("t{i}"), "assignee": "eng" }))
            .await
            .expect("execute");
    }
    let refused_assign = assign
        .execute(json!({ "task_id": "t9", "assignee": "eng" }))
        .await
        .expect("execute");
    assert!(refused_assign.is_error, "{}", refused_assign.text());
    assert!(
        refused_assign.text().contains("NOT assigned"),
        "{}",
        refused_assign.text()
    );
    let refused_review = review
        .execute(json!({ "task_id": "t9", "decision": "approve" }))
        .await
        .expect("execute");
    assert!(refused_review.is_error, "{}", refused_review.text());
    assert!(
        refused_review.text().contains("NOT reviewed"),
        "{}",
        refused_review.text()
    );
    assert_eq!(queue.queued(), MAX_DELEGATIONS_PER_TURN);
}
