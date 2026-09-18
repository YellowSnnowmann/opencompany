use super::*;

/// **Two operator messages to the same hive desk in one cycle must not
/// fold into each other.**
///
/// Both `OperatorMessage` events are journaled up front (mirroring
/// `CycleRequest::event_seqs`, which names a sequence every caller
/// already durable-wrote before the brain ever sees the event) and
/// `run_cycle_scoped` then answers each in turn on the *same* desk. The
/// first episode (`ALPHA_QUESTION`) runs to completion before the second
/// (`BETA_QUESTION`) ever opens, so by the time episode B's very first
/// `EpisodeDriver::run` iteration reads the desk's transcript, episode
/// A's turns already sit in the journal at sequences *above* B's own
/// trigger.
///
/// Before the fix, a top-level hive send never threaded its turns to the
/// triggering operator message (`in_thread(*parent)` with `parent: None`),
/// so both episodes shared the same desk-channel conversation. Episode
/// B's fold has only a lower watermark and no upper bound, so it read
/// episode A's already-carried `#alpha` votes as its own live traces and
/// converged on `#alpha` immediately — zero turns of its own, and on the
/// wrong question entirely.
///
/// After the fix, each episode's turns are parented to its own triggering
/// message, so episode B's conversation is a distinct thread and cannot
/// see episode A's turns at all: it deliberates on its own and converges
/// on `#beta`.
#[tokio::test]
async fn two_hive_desk_episodes_in_one_cycle_do_not_fold_into_each_other() {
    use crate::store::FsEventLog;

    let dir = tempfile::tempdir().unwrap();
    let events: Arc<dyn crate::ports::EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let company = CompanyId::new("acme");

    let message_a = CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "ALPHA_QUESTION".into(),
        by: None,
        chat: Some("eng_desk".into()),
        deliverable: None,
        attachments: Vec::new(),
    };
    let message_b = CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "BETA_QUESTION".into(),
        by: None,
        chat: Some("eng_desk".into()),
        deliverable: None,
        attachments: Vec::new(),
    };
    // Both journaled before the brain ever runs a cycle over them —
    // exactly the ordering `CycleRequest::event_seqs`'s doc names as the
    // caller's contract, and the ordering the finding depends on: episode
    // A's turns (journaled below) land at sequences above `seq_b`.
    let seq_a = events
        .append(&company, message_a.clone())
        .await
        .expect("journal message A");
    let seq_b = events
        .append(&company, message_b.clone())
        .await
        .expect("journal message B");

    let deps = HarnessDeps {
        emergency_gate: None,
        notifications: None,
        ledgers: None,
        ledger_registry: Default::default(),
        provider: Arc::new(HiveTopicProvider),
        provider_slug: "mock".to_string(),
        serves: None,
        context: Arc::new(FsContextStore::new(dir.path())),
        store: Arc::new(FsCompanyStore::new(dir.path())),
        meter: None,
        workspace_root: dir.path().to_path_buf(),
        mcp_home: None,
        workspace_git_enabled: false,
        audit_root: dir.path().to_path_buf(),
        model_override: None,
        tasks: None,
        artifacts: None,
        skills: None,
        skills_source_dir: None,
        skills_registry: std::sync::Arc::from([]),
        default_mcp_servers: Vec::new(),
        mcp_servers: Vec::new(),
        facts: None,
        events: Some(events.clone()),
        delegations: orchestrator::DelegationQueue::default(),
        workflow_runner: orchestrator::WorkflowRunnerHandle::default(),
        mcp_failures: crate::harness::mcp_probe::McpFailureQueue::default(),
        pending_publishes: crate::harness::publish::PendingPublishQueue::default(),
        workflow_refs: crate::harness::workflow_refs::WorkflowRefQueue::default(),
        run_outputs: crate::harness::orchestrator::RunOutputCache::default(),
        run_output_store: None,
        workflow_revisions: None,
        approval_requests: crate::harness::policy::ApprovalRequestQueue::default(),
        secrets: None,
        web_allowed_domains: Vec::new(),
        capabilities: crate::harness::toolbelt::CapabilityFilter::AllowAll,
        workflow_source_dir: None,
        plan: None,
        media: None,
        composio: None,
        #[cfg(feature = "chargebee")]
        chargebee: None,
        #[cfg(feature = "paypal")]
        paypal: None,
        hosting: None,
        steer: crate::company::steer::InflightRegistry::default(),
        run_supervisor: crate::runtime::RunSupervisor::default(),
        delivery: None,
        search: None,
        tenant_search: None,
        workspace: None,
        workflow_runs: None,
        deep_trace: None,
    };
    let brain = HarnessBrain::new(Arc::new(HarnessPool::new()), deps, record_with_hive_desk());

    let req = CycleRequest {
        cycle_id: "cycle-hive-isolation".to_string(),
        company_id: company.clone(),
        events: vec![message_a, message_b],
        event_seqs: vec![seq_a, seq_b],
        policy: None,
    };
    let result = brain
        .run_cycle(req, &NoopHost)
        .await
        .expect("both hive episodes in the cycle answer");

    assert_eq!(
        result.channel_responses.len(),
        2,
        "each operator message gets its own hive-report response: {:?}",
        result.channel_responses
    );
    let report_a = &result.channel_responses[0].text;
    let report_b = &result.channel_responses[1].text;
    assert!(
        report_a.contains("#alpha"),
        "episode A must settle on its own question: {report_a}"
    );
    assert!(
        report_b.contains("#beta") && !report_b.contains("#alpha"),
        "episode B must settle on its OWN question rather than inheriting \
         episode A's already-carried #alpha vote: {report_b}"
    );

    // The journal itself must show the two episodes parented to their own
    // triggering message, not sharing one unparented desk-channel thread.
    let logged = events
        .read_from(&company, EventSeq::new(0), usize::MAX)
        .await
        .expect("read the journal back");
    let turns_under = |root: EventSeq| {
        logged
            .iter()
            .filter(|stored| {
                matches!(
                    &stored.event,
                    CompanyEvent::AgentReply { parent, .. } if *parent == Some(root)
                )
            })
            .count()
    };
    assert!(
        turns_under(seq_a) > 0,
        "episode A's turns must be parented to message A: {logged:?}"
    );
    assert!(
        turns_under(seq_b) > 0,
        "episode B's turns must be parented to message B rather than left \
         unparented on the shared desk channel: {logged:?}"
    );
}

/// The regression for #172: a `RequireApproval` recorded during a turn is
/// **parked** on the host, so it lands in the journal the Approvals page
/// reads instead of being narrated away in chat and lost.
///
/// `ParkingHost` panics on `emit_effect`, which pins the other half of the
/// fix: the request must NOT be re-evaluated by the runtime gate (which
/// allows — and so silently "executes" — the `Other` group most gated tool
/// calls classify into).
#[tokio::test]
async fn approval_requests_are_parked_for_the_operator() {
    use crate::harness::policy::{ApprovalPolicy, ApprovalRequestQueue};
    use openhuman_core::agent::tool_policy::{
        ToolCallContext, ToolPolicy, ToolPolicyDecision, ToolPolicyRequest,
    };

    let dir = tempfile::tempdir().unwrap();
    let requests = ApprovalRequestQueue::default();
    let brain = brain_with_approval_queue(dir.path(), requests.clone());

    // Exactly what a supervised policy records when the agent reaches for a
    // gated tool mid-turn.
    let policy = ApprovalPolicy::new(
        &crate::company::Policy {
            mode: "supervised".to_string(),
            always_approve: Vec::new(),
            auto_approve_under_usd: None,
            approval_ttl_hours: None,
        },
        None,
    )
    .with_requests(requests.clone());
    let args = crate::policy::test_support::composio_send_args();
    let request = ToolPolicyRequest::new(
        "composio_execute",
        args.clone(),
        ToolCallContext::session("s", "chat", "ceo", "call-1", 0),
    );
    assert!(
        matches!(
            policy.check(&request).await,
            ToolPolicyDecision::RequireApproval { .. }
        ),
        "the fixture must reproduce a gated call"
    );
    assert_eq!(requests.queued(), 1, "the decision was recorded to park");

    let host = ParkingHost::default();
    brain
        .park_approval_requests(&host)
        .await
        .expect("the drain parks");

    let parked = host.parked();
    assert_eq!(parked.len(), 1, "one approval reached the operator");
    assert_eq!(parked[0].kind, "composio_execute");
    assert_eq!(
        parked[0].payload, args,
        "the call's arguments are preserved"
    );
    assert_eq!(requests.queued(), 0, "the queue is drained");
}

/// A second drain parks nothing: the queue is emptied, so a later cycle
/// can't re-park a request the operator has already been shown.
#[tokio::test]
async fn draining_twice_parks_nothing_the_second_time() {
    use crate::harness::policy::{ApprovalRequest, ApprovalRequestQueue};
    use crate::ports::types::EffectGroup;

    let dir = tempfile::tempdir().unwrap();
    let requests = ApprovalRequestQueue::default();
    let brain = brain_with_approval_queue(dir.path(), requests.clone());
    requests.push(ApprovalRequest {
        tool: "media_generate_image".to_string(),
        reason: "supervised".to_string(),
        effect: Effect {
            kind: "media_generate_image".to_string(),
            group: EffectGroup::Spend,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            payload: serde_json::json!({ "prompt": "a logo" }),
            agent: None,
            run_id: None,
        },
    });

    let host = ParkingHost::default();
    brain.park_approval_requests(&host).await.expect("drain");
    brain
        .park_approval_requests(&host)
        .await
        .expect("second drain");
    assert_eq!(host.parked().len(), 1, "parked once, not twice");
}

/// Issue #561: a turn that gates more calls than one turn may raise tells
/// the operator so, with the count.
///
/// The cap itself is not the bug and is not touched here. The bug is that
/// exceeding it was **silent**: the operator saw eight cards and had no way
/// to learn that five more gated calls had happened, been refused, and been
/// dropped. Eight cards and no notice is indistinguishable from "eight is
/// all there was".
#[tokio::test]
async fn a_turn_that_overflows_the_cap_tells_the_operator_how_many_were_dropped() {
    use crate::harness::policy::{ApprovalRequest, ApprovalRequestQueue};
    use crate::ports::types::EffectGroup;

    let cap = crate::harness::policy::MAX_APPROVAL_REQUESTS_PER_TURN;
    let over = 5;

    let dir = tempfile::tempdir().unwrap();
    let requests = ApprovalRequestQueue::default();
    let brain = brain_with_approval_queue(dir.path(), requests.clone());
    for i in 0..(cap + over) {
        requests.push(ApprovalRequest {
            tool: "composio_execute".to_string(),
            reason: "supervised".to_string(),
            effect: Effect {
                kind: "composio_execute".to_string(),
                group: EffectGroup::Send,
                amount_usd: None,
                established_thread: false,
                first_time_counterparty: false,
                // Distinct payloads, or `push` would dedupe them and the
                // queue would never reach the cap in the first place.
                payload: crate::policy::test_support::composio_unclassified_args_numbered(i),
                agent: None,
                run_id: None,
            },
        });
    }

    let host = ParkingHost::default();
    let notice = brain
        .park_approval_requests(&host)
        .await
        .expect("drain")
        .expect("an overflowing turn has something to tell the operator");

    assert_eq!(host.parked().len(), cap, "the cap still holds");
    assert!(
        notice.contains(&over.to_string()),
        "the operator is told HOW MANY were dropped, not just that some were: {notice}"
    );
    assert!(
        notice.contains(&cap.to_string()),
        "…and what the limit was, so the number means something: {notice}"
    );
}

/// The ordinary turn stays quiet. A notice on every cycle would train the
/// operator to scroll past the one that matters.
#[tokio::test]
async fn a_turn_within_the_cap_raises_no_notice() {
    use crate::harness::policy::{ApprovalRequest, ApprovalRequestQueue};
    use crate::ports::types::EffectGroup;

    let dir = tempfile::tempdir().unwrap();
    let requests = ApprovalRequestQueue::default();
    let brain = brain_with_approval_queue(dir.path(), requests.clone());
    requests.push(ApprovalRequest {
        tool: "composio_execute".to_string(),
        reason: "supervised".to_string(),
        effect: Effect {
            kind: "composio_execute".to_string(),
            group: EffectGroup::Send,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            payload: crate::policy::test_support::composio_send_args(),
            agent: None,
            run_id: None,
        },
    });

    let host = ParkingHost::default();
    assert!(
        brain
            .park_approval_requests(&host)
            .await
            .expect("drain")
            .is_none(),
        "one request, a cap of 8: nothing was dropped and nothing is said"
    );
    assert_eq!(host.parked().len(), 1, "and the request itself still parks");
}

/// One failed park must not take the rest of the batch — or the turn's reply
/// — down with it. `drain` has already emptied the shared queue, so a `?`
/// here would lose every later request forever and abort `run_cycle`,
/// reproducing for the remainder of the batch exactly the silent
/// disappearance this issue fixes.
#[tokio::test]
async fn a_failed_park_does_not_drop_the_rest_of_the_batch() {
    use crate::harness::policy::{ApprovalRequest, ApprovalRequestQueue};
    use crate::ports::types::EffectGroup;

    let dir = tempfile::tempdir().unwrap();
    let requests = ApprovalRequestQueue::default();
    let brain = brain_with_approval_queue(dir.path(), requests.clone());
    for tool in ["first_tool", "second_tool", "third_tool"] {
        requests.push(ApprovalRequest {
            tool: tool.to_string(),
            reason: "supervised".to_string(),
            effect: Effect {
                kind: tool.to_string(),
                group: EffectGroup::Other,
                amount_usd: None,
                established_thread: false,
                first_time_counterparty: false,
                payload: serde_json::json!({ "tool": tool }),
                agent: None,
                run_id: None,
            },
        });
    }

    let host = FlakyParkingHost::default();
    let notice = brain
        .park_approval_requests(&host)
        .await
        .expect("a park failure is surfaced without aborting the batch")
        .expect("the operator is told a request was not saved");

    // The first park failed; the two after it still reached the operator.
    let parked = host.parked();
    assert_eq!(parked.len(), 2, "the batch continued past the failure");
    assert_eq!(parked[0].kind, "second_tool");
    assert_eq!(parked[1].kind, "third_tool");
    assert!(notice.contains("1 approval request could not be saved"));
    assert!(notice.contains("Ask the agent to request approval again"));
}

/// The arm that made #243 visible: an approved grant re-dispatches its agent
/// with the exact arguments, answers on that agent's channel, and journals
/// the reply.
///
/// Before this arm existed, `ApprovalResolved` fell into `_ => {}`: no turn,
/// no response, and the cycle ended on the "Acknowledged." fallback. The
/// operator approved, read "Acknowledged.", and nothing ran — which looks
/// exactly like success.
///
/// `MockProvider` echoes the user message back, so the reply text IS the
/// instruction the agent received — which is what makes argument fidelity
/// assertable offline.
#[tokio::test]
async fn an_approved_grant_redispatches_its_agent_with_the_exact_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let log: Arc<dyn crate::ports::EventLog> =
        Arc::new(crate::store::FsEventLog::new(dir.path().to_path_buf()));
    let requests = crate::harness::policy::ApprovalRequestQueue::default();
    // Issue #470: a real catalogued send, with the action's own parameters
    // under `arguments` where the tool's schema puts them — so the
    // re-dispatch path this test covers carries a call the classifier can
    // actually read.
    let args = crate::policy::test_support::composio_args_with(
        crate::policy::test_support::COMPOSIO_SEND_SLUG,
        serde_json::json!({ "to": "a@b.test" }),
    );
    requests
        .grants()
        .grant(crate::runtime::grants::GrantedCall {
            approval_id: ApprovalId::new("appr-1"),
            agent: "ceo".into(),
            tool: "composio_execute".into(),
            args: args.clone(),
            at_millis: now_millis(),
            origin_thread: None,
            origin_parent: None,
            origin_task: None,
        });
    let brain = brain_with_queue_and_events(dir.path(), requests, log.clone());

    let result = brain
        .run_cycle(
            cycle_over(vec![approval_resolved("appr-1", Verdict::Approve)]),
            &NoopHost,
        )
        .await
        .expect("cycle runs");

    // A real bubble, on the GRANTING agent's channel — not the generic
    // "Acknowledged." fallback and not the operator channel.
    assert_eq!(result.channel_responses.len(), 1);
    let bubble = &result.channel_responses[0];
    assert_eq!(bubble.channel, "ceo");
    assert_ne!(bubble.text, "Acknowledged.");

    // The instruction carried the tool and the arguments VERBATIM. A model
    // that re-issues with drifted arguments re-parks (see the policy tests),
    // so the fidelity of this string is what makes the round-trip land.
    assert!(bubble.text.contains("composio_execute"), "{}", bubble.text);
    assert!(
        bubble.text.contains(&serde_json::to_string(&args).unwrap()),
        "the exact approved arguments must reach the agent: {}",
        bubble.text
    );
    assert!(
        bubble.text.contains("Do not modify them"),
        "{}",
        bubble.text
    );

    // Journaling the reply is no longer this function's job (issue #469):
    // the runtime journals every continuation reply once, in
    // `CompanyRuntime::publish_continuation`, so that the answers of
    // continuations this arm produces nothing for are not lost either. The
    // round trip — reply journaled into the thread the sign-off was raised
    // in, reaching the console's event stream — is covered end to end over
    // the real router by
    // `server::operator::test::a_continuation_answers_in_the_thread_the_sign_off_was_raised_in`.
    assert!(
        no_replies_journaled(&log).await,
        "the brain must not journal the reply a second time; the runtime owns it"
    );
}

#[tokio::test]
async fn an_explicit_approval_continues_without_reissuing_the_request_tool() {
    assert_explicit_decision_continues(Verdict::Approve, "APPROVED").await;
}

#[tokio::test]
async fn an_explicit_denial_also_returns_to_the_requesting_agent() {
    assert_explicit_decision_continues(Verdict::Deny, "DENIED").await;
}

/// A threaded approval continuation must preserve the approval's thread root
/// when it re-dispatches the granted call. The bound agent's observable
/// context proves the target is threaded rather than channel-only.
#[tokio::test]
async fn an_approved_threaded_grant_redispatches_in_its_origin_thread() {
    let dir = tempfile::tempdir().unwrap();
    let requests = crate::harness::policy::ApprovalRequestQueue::default();
    let root = crate::ports::types::EventSeq::new(7);
    requests
        .grants()
        .grant(crate::runtime::grants::GrantedCall {
            approval_id: ApprovalId::new("appr-threaded"),
            agent: "ceo".into(),
            tool: "workspace_write".into(),
            args: serde_json::json!({}),
            at_millis: now_millis(),
            origin_thread: Some("general".into()),
            origin_parent: Some(root),
            origin_task: None,
        });
    let base = brain_with_queue_and_events(
        dir.path(),
        requests,
        Arc::new(crate::store::FsEventLog::new(dir.path().to_path_buf())),
    );
    let pool = Arc::new(HarnessPool::new());
    let brain = HarnessBrain::new(pool.clone(), (*base.deps).clone(), record());
    pool.ensure(&record(), &brain.deps).await.expect("ensure");

    brain
        .run_cycle(
            cycle_over(vec![approval_resolved("appr-threaded", Verdict::Approve)]),
            &NoopHost,
        )
        .await
        .expect("cycle runs");

    let agent = pool
        .agents
        .read()
        .await
        .get(&CompanyId::new("acme"))
        .and_then(|roster| roster.iter().find(|agent| agent.agent_id == "ceo"))
        .cloned()
        .expect("the approved turn keeps the agent resident");
    // Issue #1890 I reverses this. It asserted `None` — that an approval's
    // continuation binds to nothing, because it runs unstreamed and "binding
    // is covered by the delegated target".
    //
    // The delegated target does cover the *drain*, which was already bound
    // by `in_thread(grant.origin_parent())`. It never covered the re-issued
    // call itself: that turn ran against whatever history the agent
    // happened to be holding and then published its answer into the origin
    // thread regardless — grounded in one conversation, answering into
    // another. Identity is no longer inferred from the absent stream, so
    // the turn now binds to the conversation the grant recorded.
    assert_eq!(
        *agent.bound_chat.lock().await,
        Some(("general".to_string(), Some(root))),
        "the re-issued call binds to the conversation the approval was raised in"
    );
}

/// Issue #1846 review (Codex #3869725683) — **the regression.** Same
/// fixture as `an_approved_grant_redispatches_its_agent_with_the_exact_arguments`
/// above, but the re-issued call's provider is now out of credits.
///
/// `run_steered_background` runs through the SAME `run_inner` the
/// interactive chat path does, so it parks a re-issue marker for the
/// granting agent exactly as an ordinary paused message would — proven
/// below by reading it straight off `BudgetPauseSet`. Before this fix, the
/// bubble `redispatch_granted_call` built from that outcome carried
/// `outcome.reply` (the budget-paused placeholder text) verbatim, so the
/// operator saw an ordinary-looking reply rather than the runtime's own
/// pause notice.
///
/// Issue #1846 review (Codex #3870562590): the notice it now carries is the
/// NO-RESEND one. The marker asserted below is real but not redeemable —
/// `run_steered_background` parks it with `background: true`, the one shape
/// `redeem_budget_pause` refuses (`src/server/ops/budget_pause.rs`) — so
/// the redeemable prefix would have drawn a CTA that returned 400 on every
/// click. Both prefixes are asserted: matching the new one is only half the
/// contract, since the console branches on the old one.
#[tokio::test]
async fn a_budget_paused_approval_continuation_surfaces_the_notice_and_parks_a_marker() {
    let dir = tempfile::tempdir().unwrap();
    let log: Arc<dyn crate::ports::EventLog> =
        Arc::new(crate::store::FsEventLog::new(dir.path().to_path_buf()));
    let requests = crate::harness::policy::ApprovalRequestQueue::default();
    let args = crate::policy::test_support::composio_args_with(
        crate::policy::test_support::COMPOSIO_SEND_SLUG,
        serde_json::json!({ "to": "a@b.test" }),
    );
    requests
        .grants()
        .grant(crate::runtime::grants::GrantedCall {
            approval_id: ApprovalId::new("appr-1"),
            agent: "ceo".into(),
            tool: "composio_execute".into(),
            args: args.clone(),
            at_millis: now_millis(),
            origin_thread: None,
            origin_parent: None,
            origin_task: None,
        });
    let brain = brain_with_queue_and_events_and_budget_exhausted_provider(
        dir.path(),
        requests,
        log.clone(),
    );

    let result = brain
        .run_cycle(
            cycle_over(vec![approval_resolved("appr-1", Verdict::Approve)]),
            &NoopHost,
        )
        .await
        .expect("cycle runs");

    assert_eq!(result.channel_responses.len(), 1);
    let bubble = &result.channel_responses[0];
    assert_eq!(bubble.channel, "ceo");
    assert!(
        bubble
            .text
            .starts_with(BUDGET_PAUSE_NOTICE_NO_RESEND_PREFIX),
        "an approval continuation parks a background marker the redeem route refuses, so \
         its notice must carry the non-redeemable prefix — got: {}",
        bubble.text
    );
    assert!(
        !bubble.text.starts_with(BUDGET_PAUSE_NOTICE_PREFIX),
        "the pre-fix defect: this prefix is what the console keys its \"Add credits & \
         resend\" CTA off, and this marker's redeem returns 400: {}",
        bubble.text
    );
    assert!(
        bubble.text.to_ascii_lowercase().contains("add credits"),
        "the actionable ask survives into the notice: {}",
        bubble.text
    );

    // And a re-issue marker really was parked for the granting agent: the
    // notice is non-redeemable because of HOW it was parked (background),
    // not because nothing was parked at all.
    let marker = crate::runtime::grants::budget_pauses_for(&CompanyId::new("acme"))
        .peek("ceo")
        .expect("run_steered_background parks a marker on the same terms run_inner does");
    assert_eq!(marker.agent, "ceo");
}
