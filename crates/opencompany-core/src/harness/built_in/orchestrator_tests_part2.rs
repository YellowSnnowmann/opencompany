use super::*;

#[tokio::test]
async fn spawn_task_tool_enqueues_a_task() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    // An empty store loads no record, so assignee grounding fails open and
    // the string is queued exactly as typed — isolating this test to the
    // plain enqueue path. Grounding itself is covered separately below.
    let tool = SpawnTaskTool::new(
        queue.clone(),
        CompanyId::new("acme"),
        Arc::new(MemStore::default()),
    );
    tool.execute(json!({ "title": "Ship it", "note": "soon", "assignee": "eng" }))
        .await
        .expect("execute");
    let drained = queue.drain(MAX_DELEGATIONS_PER_TURN);
    assert_eq!(
        drained,
        vec![Delegation::SpawnTask {
            title: "Ship it".to_string(),
            note: Some("soon".to_string()),
            assignee: Some("eng".to_string()),
        }]
    );
}

#[tokio::test]
async fn spawn_task_tool_requires_a_title() {
    let queue = DelegationQueue::default();
    let tool = SpawnTaskTool::new(
        queue.clone(),
        CompanyId::new("acme"),
        Arc::new(MemStore::default()),
    );
    assert!(tool.execute(json!({ "note": "no title" })).await.is_err());
    assert_eq!(queue.queued(), 0);
}

#[tokio::test]
async fn assign_task_tool_enqueues_an_assignment() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = AssignTaskTool::new(queue.clone());
    tool.execute(json!({ "task_id": "t1", "assignee": "eng", "note": "closer to it" }))
        .await
        .expect("execute");
    assert_eq!(
        queue.drain(MAX_DELEGATIONS_PER_TURN),
        vec![Delegation::AssignTask {
            task_id: "t1".to_string(),
            assignee: "eng".to_string(),
            note: Some("closer to it".to_string()),
        }]
    );
}

#[tokio::test]
async fn assign_task_tool_requires_a_card_and_an_assignee() {
    let queue = DelegationQueue::default();
    let tool = AssignTaskTool::new(queue.clone());
    assert!(tool.execute(json!({ "assignee": "eng" })).await.is_err());
    assert!(tool.execute(json!({ "task_id": "t1" })).await.is_err());
    // A blank string is not an assignee.
    assert!(
        tool.execute(json!({ "task_id": "t1", "assignee": "  " }))
            .await
            .is_err()
    );
    assert_eq!(queue.queued(), 0);
}

#[tokio::test]
async fn review_task_tool_enqueues_both_verdicts() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = ReviewTaskTool::new(queue.clone());
    let approved = tool
        .execute(json!({ "task_id": "t1", "decision": "approve", "note": "good" }))
        .await
        .expect("approve");
    let revised = tool
        .execute(json!({ "task_id": "t2", "decision": "revise" }))
        .await
        .expect("revise");

    // Issue #453: staged truth, not the past tense. The card has not moved
    // when this sentence is written — the drain the claim promises is what
    // moves it — and saying otherwise is what made an undrained turn a lie
    // told through the agent.
    assert!(!approved.is_error);
    let text = approved.text();
    assert!(text.contains("Recorded your approval of card t1"), "{text}");
    assert!(text.contains("as this turn completes"), "{text}");
    assert!(!text.contains("has moved"), "nothing has moved yet: {text}");
    let text = revised.text();
    assert!(text.contains("card t2 returns to To-do"), "{text}");
    assert!(text.contains("as this turn completes"), "{text}");

    assert_eq!(
        queue.drain(MAX_DELEGATIONS_PER_TURN),
        vec![
            Delegation::ReviewTask {
                task_id: "t1".to_string(),
                decision: ReviewDecision::Approve,
                note: Some("good".to_string()),
            },
            Delegation::ReviewTask {
                task_id: "t2".to_string(),
                decision: ReviewDecision::Revise,
                note: None,
            },
        ]
    );
}

/// An unrecognised verdict is an error, never a silent approval — a card
/// must not pass review because the model typed something unexpected.
#[tokio::test]
async fn review_task_tool_rejects_an_unknown_verdict_rather_than_approving() {
    let queue = DelegationQueue::default();
    let tool = ReviewTaskTool::new(queue.clone());
    assert!(
        tool.execute(json!({ "task_id": "t1", "decision": "maybe" }))
            .await
            .is_err()
    );
    assert!(tool.execute(json!({ "task_id": "t1" })).await.is_err());
    assert_eq!(queue.queued(), 0, "nothing may be queued on a bad verdict");
}

/// Both lifecycle tools are internal delegation work, so the approval
/// policy must classify them as such — never as an external effect to park.
#[test]
fn the_lifecycle_tools_are_internal_delegation_tools() {
    assert!(is_delegation_tool(ASSIGN_TASK_TOOL));
    assert!(is_delegation_tool(REVIEW_TASK_TOOL));
}

/// Issue #884: the teammate hand-off is internal work too. Left out, the
/// approval policy would read it as an external effect and park every
/// hand-off behind an operator approval — and the new edge would sit outside
/// the loop checks every other delegation passes through.
#[test]
fn the_teammate_hand_off_is_an_internal_delegation_tool() {
    assert!(is_delegation_tool(DELEGATE_TO_TEAMMATE_TOOL));
}

/// The orchestrator is actually handed the new tools.
#[test]
fn delegation_tools_include_the_lifecycle_tools() {
    let company = CompanyId::new("acme");
    let store = Arc::new(MemStore::seeded(seeded_record(&company)));
    let names: Vec<String> = delegation_tools(&DelegationQueue::default(), company, store)
        .iter()
        .map(|t| t.name().to_string())
        .collect();
    assert!(names.contains(&ASSIGN_TASK_TOOL.to_string()), "{names:?}");
    assert!(names.contains(&REVIEW_TASK_TOOL.to_string()), "{names:?}");
    // …without dropping the ones that were already there.
    assert!(names.contains(&SPAWN_TASK_TOOL.to_string()), "{names:?}");
    assert!(
        names.contains(&DELEGATE_TO_DESK_TOOL.to_string()),
        "{names:?}"
    );
    // Issue #884: and the teammate hand-off, exactly once — a duplicate name
    // on one belt is what the `else if` in `build` exists to prevent.
    assert_eq!(
        names
            .iter()
            .filter(|n| *n == DELEGATE_TO_TEAMMATE_TOOL)
            .count(),
        1,
        "{names:?}"
    );
}

#[tokio::test]
async fn delegate_to_desk_tool_enqueues_a_hand_off() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = desk_tool(desks_record(&CompanyId::new("acme")), &queue);
    let result = tool
        .execute(json!({ "desk": "strategy", "instruction": "draft a plan" }))
        .await
        .expect("execute");
    assert!(!result.is_error, "a real desk with a lead is delegatable");
    let drained = queue.drain(MAX_DELEGATIONS_PER_TURN);
    assert_eq!(
        drained,
        vec![Delegation::DelegateToDesk {
            desk: "strategy".to_string(),
            instruction: "draft a plan".to_string(),
        }]
    );
}

/// Depth is the length of the scope chain, and it gates **hand-offs only**.
///
/// At the bound a `delegate_to_desk` is refused with the new
/// [`NoDrainReason::Depth`], while a `spawn_task` still stages — refusing
/// that too would push a member that has hit the bound into working silently
/// rather than leaving the work tracked.
#[test]
fn push_within_cap_refuses_a_hand_off_past_the_depth_bound() {
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let hand_off = || Delegation::DelegateToDesk {
        desk: "research".to_string(),
        instruction: "dig into it".to_string(),
    };
    let card = || Delegation::SpawnTask {
        title: "follow up".to_string(),
        note: None,
        assignee: None,
    };

    // Depth 0 (the orchestrator's own turn) under a bound of 1: allowed.
    assert_eq!(queue.scope_depth(), 0);
    assert_eq!(
        queue.push_within_cap(hand_off(), MAX_DELEGATIONS_PER_TURN, 1),
        Staged::Queued
    );
    queue.clear();

    // One level in, under a bound of 1: refused as depth-capped.
    let scope = queue.enter_scope("strategy".to_string());
    assert_eq!(queue.scope_depth(), 1);
    assert_eq!(
        queue.push_within_cap(hand_off(), MAX_DELEGATIONS_PER_TURN, 1),
        Staged::NoDrain(NoDrainReason::Depth)
    );
    // …while the board write at the same depth is untouched.
    assert_eq!(
        queue.push_within_cap(card(), MAX_DELEGATIONS_PER_TURN, 1),
        Staged::Queued
    );
    queue.clear();
    // …and the same hand-off under the default bound of 2 stages.
    assert_eq!(
        queue.push_within_cap(hand_off(), MAX_DELEGATIONS_PER_TURN, 2),
        Staged::Queued
    );
    queue.clear();

    // Two levels in, under a bound of 2: refused.
    let deeper = queue.enter_scope("research".to_string());
    assert_eq!(queue.scope_depth(), 2);
    assert_eq!(
        queue.push_within_cap(hand_off(), MAX_DELEGATIONS_PER_TURN, 2),
        Staged::NoDrain(NoDrainReason::Depth)
    );

    // The guards pop on drop, outermost last.
    drop(deeper);
    assert_eq!(queue.scope_depth(), 1);
    drop(scope);
    assert_eq!(queue.scope_depth(), 0);
}

/// The refusal has to be countable and distinguishable from the two that
/// preceded it, and its text must not claim either of their causes — the
/// same message would tell a fully capable company that its context cannot
/// do board work.
#[test]
fn the_depth_refusal_is_its_own_reason_and_its_own_sentence() {
    assert_eq!(NoDrainReason::Depth.as_str(), "depth_capped");
    for other in [NoDrainReason::Unwired, NoDrainReason::Triage] {
        assert_ne!(NoDrainReason::Depth.as_str(), other.as_str());
    }
    let text = no_drain(
        DELEGATE_TO_DESK_TOOL,
        "nothing was handed to the research desk",
        NoDrainReason::Depth,
    );
    assert!(text.contains("as far as this company allows"), "{text}");
    assert!(
        text.contains("`spawn_task`"),
        "the model must be told what still works: {text}"
    );
    assert!(
        !text.contains("question"),
        "a depth refusal must not borrow the triage cause: {text}"
    );
    assert!(
        !text.contains("unavailable in this context"),
        "a depth refusal must not borrow the unwired cause: {text}"
    );
}

/// The chain ends with the claim, on **both** boundaries.
///
/// The exit half is the load-bearing one: a `ScopeGuard` pops on every
/// ordinary exit, but a panic inside a nested turn unwinds past it, and a
/// chain left standing would make the next operator message start at depth 2
/// and refuse its first hand-off. An ordinary `clear()` must NOT reset it —
/// clearing happens between delegations inside a live chain.
#[test]
fn the_scope_chain_resets_with_the_claim_and_survives_a_clear() {
    let queue = DelegationQueue::default();
    {
        let _claim = queue.claim();
        std::mem::forget(queue.enter_scope("strategy".to_string()));
        std::mem::forget(queue.enter_scope("research".to_string()));
        assert_eq!(queue.scope_chain(), ["strategy", "research"]);
        queue.clear();
        assert_eq!(
            queue.scope_chain(),
            ["strategy", "research"],
            "clear() runs between delegations inside a live chain and must not reset depth"
        );
    }
    assert_eq!(
        queue.scope_depth(),
        0,
        "the claim's Drop must reset a chain leaked past its guards"
    );
    // …and the acquire resets too, for a claim taken after a leak.
    std::mem::forget(queue.enter_scope("strategy".to_string()));
    let _claim = queue.claim();
    assert_eq!(queue.scope_depth(), 0);
}

/// A member may not hand work back up its own chain (A→B→A), and the
/// refusal is recorded for the card as well as returned to the model.
#[tokio::test]
async fn a_member_may_not_hand_work_back_to_a_desk_on_the_chain() {
    let company = CompanyId::new("acme");
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    // The chain the orchestrator's hand-off to `strategy` opened, with the
    // writer's own turn running inside it.
    let _scope = queue.enter_scope("strategy".to_string());
    // `writer` leads `strategy`, so it is BOTH on the chain and self-led;
    // give it a wildcard allowlist so the allowlist check cannot be what
    // refuses.
    let tool = DelegateToDeskTool::for_member(
        queue.clone(),
        company.clone(),
        Arc::new(MemStore::seeded(nested_desks_record(&company))) as Arc<dyn CompanyStore>,
        MemberScope {
            member: "writer".to_string(),
            delegates_to: vec!["*".to_string()],
        },
    );
    let result = tool
        .execute(json!({ "desk": "strategy", "instruction": "start over" }))
        .await
        .expect("execute");
    assert!(result.is_error, "a cycle must be refused");
    let text = result.output_for_llm(true);
    assert!(text.contains("strategy"), "{text}");
    assert_eq!(queue.queued(), 0, "nothing may be staged for a cycle");
    assert_eq!(
        queue.drain_refusals(MAX_DELEGATIONS_PER_TURN),
        vec!["strategy".to_string()],
        "the drain must be able to record the attempt on the card"
    );

    // A desk that is neither on the chain nor led by the caller goes
    // through.
    let ok = tool
        .execute(json!({ "desk": "research", "instruction": "dig into it" }))
        .await
        .expect("execute");
    assert!(!ok.is_error, "{}", ok.output_for_llm(true));
}

/// A member may only reach the desks its manifest entry names, and the
/// refusal lists them — the model has no other way to learn its allowlist.
#[tokio::test]
async fn a_member_may_only_reach_the_desks_its_manifest_allows() {
    let company = CompanyId::new("acme");
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = member_desk_tool(nested_desks_record(&company), &queue);

    let refused = tool
        .execute(json!({ "desk": "legal", "instruction": "review it" }))
        .await
        .expect("execute");
    assert!(refused.is_error, "an off-allowlist desk must be refused");
    let text = refused.output_for_llm(true);
    assert!(text.contains("legal"), "{text}");
    assert!(
        text.contains("research"),
        "the permitted set must be named so the model can retry in-turn: {text}"
    );
    assert_eq!(queue.queued(), 0);

    let allowed = tool
        .execute(json!({ "desk": "research", "instruction": "dig into it" }))
        .await
        .expect("execute");
    assert!(!allowed.is_error, "{}", allowed.output_for_llm(true));
    assert_eq!(queue.queued(), 1);
}

/// The **orchestrator's** copy is unrestricted: no allowlist, no cycle
/// guard, and it reaches every desk exactly as it did before #176.
#[tokio::test]
async fn the_orchestrators_copy_is_unrestricted() {
    let company = CompanyId::new("acme");
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    // Even from inside a chain — which the orchestrator never is, but the
    // contrast is the point.
    let _scope = queue.enter_scope("legal".to_string());
    let tool = desk_tool(nested_desks_record(&company), &queue);
    for desk in ["strategy", "research", "legal"] {
        let result = tool
            .execute(json!({ "desk": desk, "instruction": "go" }))
            .await
            .expect("execute");
        assert!(
            !result.is_error,
            "the orchestrator must reach {desk}: {}",
            result.output_for_llm(true)
        );
        queue.clear();
    }
}

/// A member's hand-off fails **closed** when the record cannot be read, and
/// the orchestrator's still fails open.
///
/// The asymmetry is the whole point. The allowlist and the cycle guard are
/// enforced at this tool boundary and nowhere else — `run_delegation`
/// executes whatever the queue holds without re-deriving either — so a
/// member queued ungrounded reaches every desk in the company for as long
/// as the store is unhappy. The orchestrator has no allowlist to lose, so
/// an unreadable record leaves it exactly where #272 left it.
#[tokio::test]
async fn a_members_hand_off_is_refused_when_the_record_cannot_be_read() {
    let company = CompanyId::new("acme");
    let scope = || MemberScope {
        member: "writer".to_string(),
        delegates_to: vec!["research".to_string()],
    };

    // Ok(None) — no record under that id.
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let missing = DelegateToDeskTool::for_member(
        queue.clone(),
        company.clone(),
        Arc::new(MemStore::default()) as Arc<dyn CompanyStore>,
        scope(),
    );
    let refused = missing
        .execute(json!({ "desk": "research", "instruction": "dig into it" }))
        .await
        .expect("execute");
    assert!(
        refused.is_error,
        "a member may not be queued against a record nobody could read: {}",
        refused.output_for_llm(true)
    );
    let text = refused.output_for_llm(true);
    assert!(text.contains("research"), "{text}");
    assert!(
        text.contains("writer"),
        "the refusal must name whose allowlist went unchecked: {text}"
    );
    assert_eq!(queue.queued(), 0, "nothing may be staged ungrounded");
    assert_eq!(
        queue.drain_refusals(MAX_DELEGATIONS_PER_TURN),
        vec!["research".to_string()],
        "the drain must be able to record the attempt on the card"
    );

    // Err(..) — the store is there and unhappy. Same answer.
    let broken = DelegateToDeskTool::for_member(
        queue.clone(),
        company.clone(),
        Arc::new(BrokenStore) as Arc<dyn CompanyStore>,
        scope(),
    );
    let refused = broken
        .execute(json!({ "desk": "research", "instruction": "dig into it" }))
        .await
        .expect("execute");
    assert!(
        refused.is_error,
        "a store error must refuse too: {}",
        refused.output_for_llm(true)
    );
    assert_eq!(queue.queued(), 0);
    queue.clear();
    let _ = queue.drain_refusals(MAX_DELEGATIONS_PER_TURN);

    // …and the orchestrator's copy over the same broken store still queues.
    let orchestrator = DelegateToDeskTool::new(
        queue.clone(),
        company,
        Arc::new(BrokenStore) as Arc<dyn CompanyStore>,
    );
    let queued = orchestrator
        .execute(json!({ "desk": "research", "instruction": "dig into it" }))
        .await
        .expect("execute");
    assert!(
        !queued.is_error,
        "a store hiccup must not take the orchestrator's delegation offline: {}",
        queued.output_for_llm(true)
    );
    assert_eq!(queue.queued(), 1);
}

/// The depth bound comes off the **live company record**, not a build-time
/// snapshot — an operator can edit `[tools].max_delegation_depth` without
/// the cached belt being rebuilt.
#[tokio::test]
async fn the_depth_bound_is_read_from_the_manifest_at_call_time() {
    let company = CompanyId::new("acme");
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let mut record = nested_desks_record(&company);
    record.manifest.tools.max_delegation_depth = Some(1);
    let tool = member_desk_tool(record, &queue);
    // One level in, under the manifest's bound of 1.
    let _scope = queue.enter_scope("strategy".to_string());
    let result = tool
        .execute(json!({ "desk": "research", "instruction": "dig into it" }))
        .await
        .expect("execute");
    assert!(result.is_error, "depth 1 must stop a member re-delegating");
    assert!(
        result
            .output_for_llm(true)
            .contains("as far as this company allows"),
        "{}",
        result.output_for_llm(true)
    );
    assert_eq!(queue.queued(), 0);
}

/// The member's belt is exactly `spawn_task` + the two hand-off tools —
/// never the orchestrator's authority tools.
#[test]
fn a_members_delegation_belt_is_the_two_hand_off_tools() {
    let company = CompanyId::new("acme");
    let queue = DelegationQueue::default();
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(nested_desks_record(&company)));
    let tools = member_delegation_tools(
        &queue,
        company,
        store,
        MemberScope {
            member: "writer".to_string(),
            delegates_to: vec!["research".to_string()],
        },
    );
    let mut names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            DELEGATE_TO_DESK_TOOL,
            DELEGATE_TO_TEAMMATE_TOOL,
            SPAWN_TASK_TOOL
        ]
    );
}

/// D1 at the boundary: the lead's hand-off to the peer beside it is
/// **accepted**, and queues the delegation the drain runs that teammate's
/// turn from.
#[tokio::test]
async fn a_lead_may_hand_work_to_a_peer_on_its_own_desk() {
    let company = CompanyId::new("acme");
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = member_teammate_tool(peers_record(&company), &queue);
    let result = tool
        .execute(json!({ "teammate": "editor", "instruction": "tighten the copy" }))
        .await
        .expect("execute");
    assert!(!result.is_error, "{}", result.output_for_llm(true));
    assert_eq!(
        queue.drain(MAX_DELEGATIONS_PER_TURN),
        vec![Delegation::DelegateToTeammate {
            teammate: "editor".to_string(),
            instruction: "tighten the copy".to_string(),
        }]
    );
}

/// A key that is nobody is refused before anything is queued, and the
/// attempt is recorded for the drain to report on the card — the same
/// independence #272 gave the desk refusals.
#[tokio::test]
async fn a_teammate_that_is_not_on_the_roster_is_refused_and_recorded() {
    let company = CompanyId::new("acme");
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = member_teammate_tool(peers_record(&company), &queue);
    let result = tool
        .execute(json!({ "teammate": "ghost", "instruction": "do it" }))
        .await
        .expect("execute");
    assert!(result.is_error, "{}", result.output_for_llm(true));
    assert!(
        result.output_for_llm(true).contains("editor"),
        "the refusal must name who CAN be reached: {}",
        result.output_for_llm(true)
    );
    assert_eq!(queue.queued(), 0);
    assert_eq!(
        queue.drain_refusals(MAX_DELEGATIONS_PER_TURN),
        vec!["ghost".to_string()]
    );
}

/// A real teammate on neither the caller's desk nor an allowlisted one is
/// refused; one on an allowlisted desk is not. The allowlist is #176's, read
/// at teammate granularity rather than duplicated.
#[tokio::test]
async fn the_allowlist_bounds_which_teammates_a_member_may_reach() {
    let company = CompanyId::new("acme");
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = member_teammate_tool(peers_record(&company), &queue);

    let refused = tool
        .execute(json!({ "teammate": "legal_counsel", "instruction": "review it" }))
        .await
        .expect("execute");
    assert!(refused.is_error, "{}", refused.output_for_llm(true));
    assert_eq!(queue.queued(), 0);

    // `analyst` sits on `research`, which `writer`'s `delegates_to` names.
    let allowed = tool
        .execute(json!({ "teammate": "analyst", "instruction": "pull the numbers" }))
        .await
        .expect("execute");
    assert!(!allowed.is_error, "{}", allowed.output_for_llm(true));
    assert_eq!(queue.queued(), 1);
}
