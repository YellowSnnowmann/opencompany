use super::tests_core::*;
use super::tests_core2::*;

#[tokio::test]
async fn delegate_to_desk_arm_records_handoff_and_rejects_unknown_desk() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), desk_manifest())
        .build()
        .await
        .unwrap();
    let host = CycleHostImpl::new(
        rt.id().clone(),
        "cyc".into(),
        &rt,
        None,
        false,
        ApprovalConversation::default(),
    );

    // Known desk (by name) → card assigned to the resolved desk id, lead noted.
    let ok = host
        .delegate_to_desk(
            serde_json::json!({ "desk": "Engineering", "instruction": "build invoicing" }),
        )
        .await
        .unwrap();
    assert!(ok.ok);
    assert_eq!(ok.output["desk"], "eng");
    assert_eq!(ok.output["lead"], "eng1");

    // Unknown desk → clean error, no card.
    let bad = host
        .delegate_to_desk(serde_json::json!({ "desk": "Legal", "instruction": "review" }))
        .await
        .unwrap();
    assert!(!bad.ok);
    assert_eq!(bad.output["status"], "unknown_desk");

    let cards = rt.tasks().list(rt.id()).await.unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].assignee, "eng");
    // Same as the spawn path: a handoff opens a card, it does not dispatch.
    assert_eq!(cards[0].column, COLUMN_TODO);
}

/// Issue #1872 (codex): the hosted path refuses an `auto` channel, and
/// says why.
///
/// This path deliberately does **not** refuse an ordinary leadless desk —
/// a hosted hand-off is a durable card, visible on the board whether or
/// not anyone leads the desk yet. An auto channel is different in kind: it
/// has no lead by design and never will, so accepting one wrote a card
/// noting "no lead member on the roster yet", which is false about a
/// staffed channel and permanently so — and it disagreed with the
/// built-in tool, which refuses. Remove the guard and this opens a card.
#[tokio::test]
async fn delegate_to_desk_refuses_an_auto_channel_on_the_hosted_path() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), desk_manifest())
        .build()
        .await
        .unwrap();
    let mut record = rt.store().load(rt.id()).await.unwrap().unwrap();
    record.overlay_desks.push(crate::ports::types::OverlayDesk {
        id: "launch".to_string(),
        name: "Launch week".to_string(),
        description: None,
        members: vec!["eng1".to_string()],
        responder: crate::ports::types::ResponderMode::Auto,
        hive: Default::default(),
    });
    rt.store().save(&record).await.unwrap();

    let host = CycleHostImpl::new(
        rt.id().clone(),
        "cyc".into(),
        &rt,
        None,
        false,
        ApprovalConversation::default(),
    );
    let refused = host
        .delegate_to_desk(serde_json::json!({ "desk": "launch", "instruction": "ship the launch" }))
        .await
        .unwrap();
    assert!(!refused.ok, "{:?}", refused.output);
    assert_eq!(refused.output["status"], "no_lead");
    let error = refused.output["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("picked per message"),
        "the refusal says why, rather than reusing the leadless-desk wording: {error}"
    );
    assert!(
        rt.tasks().list(rt.id()).await.unwrap().is_empty(),
        "a refused hand-off opens no card"
    );
}

#[tokio::test]
async fn call_tool_dispatches_delegation_tools() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .build()
        .await
        .unwrap();
    let host = CycleHostImpl::new(
        rt.id().clone(),
        "cyc".into(),
        &rt,
        None,
        false,
        ApprovalConversation::default(),
    );

    // Reached through the CycleHost trait exactly as the hosted brain does.
    let res = host
        .call_tool(ToolCall {
            tool: SPAWN_TASK_TOOL.to_string(),
            args: serde_json::json!({ "title": "via call_tool" }),
        })
        .await
        .unwrap();
    assert!(res.ok);
    assert_eq!(rt.tasks().list(rt.id()).await.unwrap().len(), 1);
}

#[tokio::test]
async fn fallback_call_tool_parks_an_explicit_approval_request() {
    let home_dir = tmp_home();
    let rt = RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("full"))
        .build()
        .await
        .unwrap();
    let host = CycleHostImpl::new(
        rt.id().clone(),
        "fallback-approval".into(),
        &rt,
        None,
        false,
        ApprovalConversation::default(),
    );

    let result = host
        .call_tool(ToolCall {
            tool: crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND.to_string(),
            args: serde_json::json!({
                "title": "Submit filing",
                "question": "May I submit it?"
            }),
        })
        .await
        .unwrap();

    assert!(result.ok);
    assert_eq!(result.output["status"], "pending");
    let pending = rt.pending_approvals();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].kind,
        crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND
    );
}

#[tokio::test]
async fn handed_task_awareness_surfaces_open_cards_on_a_direct_query() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let rt = RuntimeBuilder::new(home.clone(), desk_manifest())
        .with_brain(Arc::new(CapturingBrain { seen: seen.clone() }))
        .build()
        .await
        .unwrap();

    // Hand work to the Engineering desk (card assigned to the desk id).
    rt.tasks()
        .upsert(
            rt.id(),
            &TaskRecord {
                id: "t1".into(),
                title: TaskTitle::authored("Ship invoicing"),
                note: Some("build the importer".into()),
                column: COLUMN_TODO.into(),
                priority: "medium".into(),
                assignee: "eng".into(),
                updated_at_millis: 0,
                origin: None,
                parent_task_id: None,
                // Nothing has run yet, so there is no deliverable to point at
                // (issue #339). The first successful settle stamps it.
                output: None,
                plan: None,
                planning_attempts: Vec::new(),
                deliverable: crate::ports::tasks::TaskDeliverable::Once,
                workflow_proposal: None,
                origin_run_id: None,
                origin_workflow_id: None,
                origin_message_seq: None,
                bounced: None,
            },
        )
        .await
        .unwrap();

    // Asking the desk directly (by name) surfaces the handed task...
    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "what are you working on?".into(),
        by: None,
        chat: Some("Engineering".into()),
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    // ...and asking with no address (the orchestrator) does NOT get the
    // desk's briefing folded into it.
    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "status?".into(),
        by: None,
        chat: None,
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(
        seen[0].contains("Open work already handed to you") && seen[0].contains("Ship invoicing"),
        "direct query carries the briefing: {:?}",
        seen[0]
    );
    assert!(
        !seen[1].contains("Open work already handed to you"),
        "unaddressed query has no desk briefing: {:?}",
        seen[1]
    );
}

#[tokio::test]
async fn awareness_skips_done_cards() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let rt = RuntimeBuilder::new(home.clone(), desk_manifest())
        .with_brain(Arc::new(CapturingBrain { seen: seen.clone() }))
        .build()
        .await
        .unwrap();
    rt.tasks()
        .upsert(
            rt.id(),
            &TaskRecord {
                id: "t1".into(),
                title: TaskTitle::authored("Already finished"),
                note: None,
                column: "done".into(),
                priority: "medium".into(),
                assignee: "eng".into(),
                updated_at_millis: 0,
                origin: None,
                parent_task_id: None,
                // Nothing has run yet, so there is no deliverable to point at
                // (issue #339). The first successful settle stamps it.
                output: None,
                plan: None,
                planning_attempts: Vec::new(),
                deliverable: crate::ports::tasks::TaskDeliverable::Once,
                workflow_proposal: None,
                origin_run_id: None,
                origin_workflow_id: None,
                origin_message_seq: None,
                bounced: None,
            },
        )
        .await
        .unwrap();
    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "what's up?".into(),
        by: None,
        chat: Some("eng".into()),
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();
    let seen = seen.lock().unwrap().clone();
    assert!(
        !seen[0].contains("Open work already handed to you"),
        "done cards are not surfaced as open work: {:?}",
        seen[0]
    );
}

/// The headline: approving with the broader scope arms a standing grant, and
/// mints **no** single-use grant beside it.
///
/// The second half is not tidiness. A redundant single-use grant would go
/// unredeemed — the standing grant already admits the re-issued call — and
/// fifteen minutes later the TTL sweep would tell the operator "the agent
/// didn't act", about work that ran immediately.
#[tokio::test]
async fn approving_with_a_tool_scope_mints_a_standing_grant_and_no_single_use_one() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect("ops", "file_write", serde_json::json!({ "path": "a" })),
    )
    .await;

    let (_, follow_up) = rt
        .resolve_approval_spawned(&id, Verdict::Approve, operator(), tool_scope())
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;

    assert_eq!(rt.grants.standing_count(), 1);
    assert_eq!(
        rt.grants.live_count(),
        0,
        "no single-use grant is left behind to expire noisily"
    );
    let listed = rt.standing_grants();
    assert_eq!(listed[0].tool, "file_write");
    assert_eq!(listed[0].agent, "ops");
    assert_eq!(listed[0].approval_id, id, "provenance back to the card");
    assert_eq!(
        listed[0].granted_by.id, "owner",
        "the resolving actor is recorded, not a placeholder"
    );
}

/// Issue #457: the grant records **which provider the card was about**.
///
/// `composio_execute` carries every action of every connected toolkit under
/// one name, so a grant that recorded only the name turned "read from
/// GitHub" — the sentence on the card — into "make any Composio read,
/// anywhere". The toolkit is read off the parked effect's own payload, so
/// what is stored is what the operator was shown.
///
/// Gated on the harness feature because the toolkit comes from the vendored
/// catalogue; the default build cannot mint a Composio standing grant at all
/// (every action reads as a send there), which
/// `without_the_catalogue_every_composio_action_is_a_send` pins.
#[tokio::test]
#[cfg(feature = "openhuman")]
async fn a_standing_grant_on_a_composio_read_records_the_toolkit_it_was_shown_for() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect(
            "ops",
            crate::policy::consequence::COMPOSIO_EXECUTE,
            serde_json::json!({ "tool": "GITHUB_LIST_PULL_REQUESTS" }),
        ),
    )
    .await;

    let (_, follow_up) = rt
        .resolve_approval_spawned(&id, Verdict::Approve, operator(), tool_scope())
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;

    let listed = rt.standing_grants();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].scope.as_deref(),
        Some("github"),
        "the grant has to remember which account the operator was looking at"
    );
}

/// The counterpart: a tool whose name already is the whole of what it can do
/// records no scope, so its grant matches exactly as it always did.
#[tokio::test]
async fn a_standing_grant_on_an_ordinary_tool_records_no_scope() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect("ops", "file_write", serde_json::json!({ "path": "a" })),
    )
    .await;

    let (_, follow_up) = rt
        .resolve_approval_spawned(&id, Verdict::Approve, operator(), tool_scope())
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;

    assert_eq!(
        rt.standing_grants()[0].scope,
        None,
        "there is nothing to narrow `file_write` to"
    );
}

/// Issue #1458, from the deny side: a standing denial minted against a
/// scoped tool remembers **which slice** the operator refused.
///
/// The mint used to re-read the journal's payload-scrubbed copy of the
/// effect (issue #351), whose `Null` payload made `standing_scope_of`
/// answer `None` — and a stored `None` is a wildcard in `admits_scope`, so
/// refusing one web origin blocked every origin for that teammate until
/// expiry. The resolve now carries the parked effect whole, so the deny
/// records the same scope the card showed.
#[tokio::test]
async fn a_standing_deny_on_a_scoped_tool_keeps_the_scope_it_was_shown_for() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect(
            "ops",
            crate::policy::consequence::WEB_FETCH,
            serde_json::json!({ "url": "https://docs.rs/x" }),
        ),
    )
    .await;

    let (_, follow_up) = rt
        .resolve_approval_spawned(&id, Verdict::Deny, operator(), tool_scope())
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;

    let listed = rt.standing_grants();
    assert_eq!(listed.len(), 1, "one standing denial is minted");
    assert_eq!(listed[0].verdict, Verdict::Deny);
    assert_eq!(
        listed[0].scope.as_deref(),
        Some("https://docs.rs"),
        "the deny records the origin the operator refused, not a wildcard"
    );
}

/// A standing *denial* is the half of this that fails open when it is lost:
/// an operator who refused a tool for good gets that refusal silently
/// forgotten, and the next boot admits the call again. It must survive the
/// same plain `RuntimeBuilder::build` reboot an approval does.
#[tokio::test]
async fn a_standing_denial_survives_a_restart() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let (rt, id) = park_one_blocked_tool_call(
        home.clone(),
        grantable_effect(
            "ops",
            crate::policy::consequence::WEB_FETCH,
            serde_json::json!({ "url": "https://docs.rs/x" }),
        ),
    )
    .await;

    let (_, follow_up) = rt
        .resolve_approval_spawned(&id, Verdict::Deny, operator(), tool_scope())
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;
    let refused = rt.standing_grants()[0].clone();
    assert_eq!(refused.verdict, Verdict::Deny);

    let rebooted = Arc::new(
        RuntimeBuilder::new(home, manifest("supervised"))
            .build()
            .await
            .unwrap(),
    );

    let replayed = rebooted.standing_grants();
    assert_eq!(
        replayed.len(),
        1,
        "a restart must not forget a refusal the operator made stand"
    );
    assert_eq!(replayed[0].id, refused.id);
    assert_eq!(
        replayed[0].verdict,
        Verdict::Deny,
        "it must come back as a refusal, not as a permission"
    );
    assert_eq!(
        replayed[0].scope.as_deref(),
        refused.scope.as_deref(),
        "and refusing exactly what it refused before"
    );
}

/// Issue #1458: when two identical cards park and the operator resolves the
/// first as a standing **denial** and the second as a standing **approval**,
/// the newer decision wins. `ApprovalPolicy` checks a deny above a standing
/// grant, so without reconciliation the approval would list as a live
/// permission and never admit a call until the refusal expired — the
/// operator's later "yes" silently inert.
#[tokio::test]
async fn a_new_standing_approval_revokes_an_older_standing_denial_for_the_same_scope() {
    let home_dir = tmp_home();
    let (rt, ids) = park_two_blocked_tool_calls(
        home_dir.path().to_path_buf(),
        grantable_effect(
            "ops",
            crate::policy::consequence::WEB_FETCH,
            serde_json::json!({ "url": "https://docs.rs/x" }),
        ),
    )
    .await;

    let (_, follow_up) = rt
        .resolve_approval_spawned(&ids[0], Verdict::Deny, operator(), tool_scope())
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;
    assert_eq!(
        rt.standing_grants()[0].verdict,
        Verdict::Deny,
        "the first resolution arms a standing denial"
    );

    let (_, follow_up) = rt
        .resolve_approval_spawned(&ids[1], Verdict::Approve, operator(), tool_scope())
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;

    let listed = rt.standing_grants();
    assert_eq!(listed.len(), 1, "the deny is revoked, not left shadowing");
    assert_eq!(listed[0].verdict, Verdict::Approve);
    assert_eq!(
        listed[0].scope.as_deref(),
        Some("https://docs.rs"),
        "the surviving policy keeps the scope both were minted for"
    );
}

/// The mirror direction: a standing **denial** minted after a standing
/// **approval** of the same scope revokes the grant. Enforcement would
/// already have the deny win, but a listed-but-dead grant is a wrong
/// contract for the operator who approved it.
#[tokio::test]
async fn a_new_standing_denial_revokes_an_older_standing_approval_for_the_same_scope() {
    let home_dir = tmp_home();
    let (rt, ids) = park_two_blocked_tool_calls(
        home_dir.path().to_path_buf(),
        grantable_effect(
            "ops",
            crate::policy::consequence::WEB_FETCH,
            serde_json::json!({ "url": "https://docs.rs/x" }),
        ),
    )
    .await;

    let (_, follow_up) = rt
        .resolve_approval_spawned(&ids[0], Verdict::Approve, operator(), tool_scope())
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;
    assert_eq!(
        rt.standing_grants()[0].verdict,
        Verdict::Approve,
        "the first resolution arms a standing grant"
    );

    let (_, follow_up) = rt
        .resolve_approval_spawned(&ids[1], Verdict::Deny, operator(), tool_scope())
        .await
        .unwrap();
    let _ = crate::company::runtime::join_follow_up(follow_up).await;

    let listed = rt.standing_grants();
    assert_eq!(listed.len(), 1, "the grant is revoked by the newer refusal");
    assert_eq!(listed[0].verdict, Verdict::Deny);
}
