use super::tests_core::*;
use super::tests_core2::*;
use super::*;

/// the reconcile's own steps are not atomic. Revoking the
/// shadowed opposite-polarity policy is journaled and applied in memory
/// *before* the new policy's own mint is journaled, so a failure on that
/// second append — the durable store erroring, a disk momentarily full —
/// leaves the company with the old policy gone and no new one in its
/// place. Nothing rolls the revoke back.
#[tokio::test]
async fn a_failed_mint_after_a_successful_revoke_leaves_neither_policy_live() {
    let home_dir = tmp_home();
    let store = std::sync::Arc::new(FailNthStandingMintStore::new(2));
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("supervised"))
            .with_brain(Arc::new(ParkingBrain {
                effect: grantable_effect(
                    "ops",
                    crate::policy::consequence::WEB_FETCH,
                    serde_json::json!({ "url": "https://docs.rs/x" }),
                ),
            }))
            .with_journal_store(store)
            .build()
            .await
            .unwrap(),
    );

    let mut ids = Vec::new();
    for text in ["do it", "again"] {
        let report = rt
            .run_cycle(vec![CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                parent: None,
                text: text.into(),
                by: None,
                chat: None,
                deliverable: None,
                attachments: Vec::new(),
            }])
            .await
            .unwrap();
        assert_eq!(report.parked.len(), 1);
        ids.push(report.parked[0].clone());
    }

    // First resolution: a standing denial. Its own mint is the first
    // `StandingGrantMinted` line, which the store lets through.
    rt.resolve_approval_spawned(&ids[0], Verdict::Deny, operator(), tool_scope())
        .await
        .expect("the first mint succeeds");
    assert_eq!(rt.standing_grants().len(), 1);
    assert_eq!(rt.standing_grants()[0].verdict, Verdict::Deny);

    // Second resolution: a standing approval of the same scope. The
    // reconcile revokes the denial (succeeds — a different record), then
    // mints the approval — the second `StandingGrantMinted` line, which
    // the store refuses.
    let second = rt
        .resolve_approval_spawned(&ids[1], Verdict::Approve, operator(), tool_scope())
        .await;
    assert!(
        second.is_err(),
        "the forced failure on the mint must surface, not be swallowed"
    );

    assert!(
        rt.standing_grants().is_empty(),
        "the revoke already landed and nothing rolled it back, so neither the old \
         denial nor the new approval governs this scope: {:?}",
        rt.standing_grants()
    );
}

/// The other half of the same non-atomic sequence: when the **revoke's**
/// own journal append fails — the first of the two steps, not the
/// second — nothing about the old policy may change and the new mint
/// must never be attempted at all. The `?` on `record_standing_revoked`
/// (before `revoke_standing` ever runs in memory) is what is supposed to
/// guarantee this; nothing before this test drove that specific ordering
/// through a real failure, only the mint-side failure the sibling test
/// above pins.
#[tokio::test]
async fn a_failed_revoke_journal_append_leaves_the_old_policy_untouched() {
    let home_dir = tmp_home();
    let store = std::sync::Arc::new(FailStandingRevokeStore::new());
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("supervised"))
            .with_brain(Arc::new(ParkingBrain {
                effect: grantable_effect(
                    "ops",
                    crate::policy::consequence::WEB_FETCH,
                    serde_json::json!({ "url": "https://docs.rs/x" }),
                ),
            }))
            .with_journal_store(store)
            .build()
            .await
            .unwrap(),
    );

    let mut ids = Vec::new();
    for text in ["do it", "again"] {
        let report = rt
            .run_cycle(vec![CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                parent: None,
                text: text.into(),
                by: None,
                chat: None,
                deliverable: None,
                attachments: Vec::new(),
            }])
            .await
            .unwrap();
        assert_eq!(report.parked.len(), 1);
        ids.push(report.parked[0].clone());
    }

    rt.resolve_approval_spawned(&ids[0], Verdict::Deny, operator(), tool_scope())
        .await
        .expect("the first mint has nothing to revoke, so it succeeds");
    assert_eq!(rt.standing_grants().len(), 1);
    let original = rt.standing_grants()[0].id.clone();

    // The reconcile's revoke append is forced to fail before the second
    // mint is ever attempted.
    let second = rt
        .resolve_approval_spawned(&ids[1], Verdict::Approve, operator(), tool_scope())
        .await;
    assert!(
        second.is_err(),
        "the forced failure on the revoke must surface, not be swallowed"
    );

    let listed = rt.standing_grants();
    assert_eq!(
        listed.len(),
        1,
        "a failed revoke must leave exactly the original policy in place: {listed:?}"
    );
    assert_eq!(
        listed[0].id, original,
        "the surviving policy must be the untouched original, not a partial write"
    );
    assert_eq!(
        listed[0].verdict,
        Verdict::Deny,
        "its polarity must be unchanged"
    );
}

/// Issue #243's ordering claim, pinned rather than left to reading the code:
/// `mint_grant` journals `ApprovalGranted` *before* arming the single-use
/// grant in the live set (`settle_approved_effect` → `mint_grant`), so a
/// failure on that append must leave the grant un-armed rather than live
/// with no durable record. A crash between the two is meant to replay as
/// "granted", never to lose the write; forcing the write itself to fail
/// proves the arm genuinely comes after it in the code, not just in the
/// comment describing it.
#[tokio::test]
async fn a_failed_grant_mint_never_arms_the_live_grant() {
    let home_dir = tmp_home();
    let effect = harness_effect("finance", "composio_execute", serde_json::json!({}));
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("supervised"))
            .with_brain(Arc::new(ParkingBrain {
                effect: effect.clone(),
            }))
            .with_journal_store(Arc::new(FailGrantedMintStore::new()))
            .build()
            .await
            .unwrap(),
    );
    let report = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "do it".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await
        .unwrap();
    let id = report.parked[0].clone();

    let result = rt.resolve_approval(&id, Verdict::Approve, operator()).await;
    assert!(
        result.is_err(),
        "the forced failure on the journal append must surface, not be swallowed"
    );
    assert_eq!(
        rt.grants.live_count(),
        0,
        "the grant must not be armed when the journal write that was supposed to \
         precede it failed"
    );
    assert!(rt.grants.peek(&id).is_none());
}

/// Issue #1458 under concurrency: two opposite-polarity resolutions of the
/// **same** scope settled while both are in flight — the approve and the
/// deny each half-finished before either mints — must still leave a single
/// policy, not the deny permanently shadowing the approve.
///
/// Before the reconcile lock this could interleave: the journal appends
/// between the [`opposite_polarity`] snapshot and the `grant_standing`
/// insert are awaited, so a concurrent settle gets polled in that window,
/// snapshots the same empty opposite set, and then both insert. Because
/// `ApprovalPolicy` matches a standing denial above a standing grant, the
/// approve then sits listed but never admits a call whatever the operator's
/// true order. The lock serialises the two mints, so the second observes
/// the first's policy and supersedes it — the same single-policy state the
/// sequential tests above assert.
#[tokio::test]
async fn concurrent_opposite_polarity_resolutions_leave_one_policy() {
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

    let (a, b) = tokio::join!(
        rt.resolve_approval_spawned(&ids[0], Verdict::Approve, operator(), tool_scope()),
        rt.resolve_approval_spawned(&ids[1], Verdict::Deny, operator(), tool_scope()),
    );
    let (_, follow_up_a) = a.unwrap();
    let (_, follow_up_b) = b.unwrap();
    let _ = tokio::join!(
        crate::company::runtime::join_follow_up(follow_up_a),
        crate::company::runtime::join_follow_up(follow_up_b),
    );

    let listed = rt.standing_grants();
    assert_eq!(
        listed.len(),
        1,
        "the concurrent resolutions must not leave both polarities live"
    );
}

/// A scope the runtime must not honour changes **nothing**: the approval is
/// still parked and no verdict was journaled.
///
/// This is why the check runs before `resolve_outcome`. Validating after it
/// would have dropped the card from the queue and recorded a resolution,
/// leaving the operator with nothing to re-decide and a verdict whose effect
/// never happened.
#[tokio::test]
async fn a_refused_scope_leaves_the_approval_parked_and_unjournaled() {
    for effect in [
        // A named consequence group — stays a per-call decision.
        harness_effect("finance", "composio_execute", serde_json::json!({})),
        // A native effect — no teammate and no tool to grant.
        Effect {
            kind: EMAIL_SEND_KIND.into(),
            group: EffectGroup::Other,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            payload: serde_json::json!({ "channel": "operator", "text": "hi" }),
            agent: None,
            run_id: None,
        },
    ] {
        let home_dir = tmp_home();
        let (rt, id) =
            park_one_blocked_tool_call(home_dir.path().to_path_buf(), effect.clone()).await;

        let err = rt
            .resolve_approval_spawned(&id, Verdict::Approve, operator(), tool_scope())
            .await
            .expect_err("a scope the host cannot honour is refused");
        assert!(
            matches!(err, OpenCompanyError::InvalidRequest(_)),
            "refusal must be a bad-request, not a server fault: {err:?}"
        );

        assert_eq!(
            rt.pending_approvals().len(),
            1,
            "the card is still there to be decided: {}",
            effect.kind
        );
        assert_eq!(rt.grants.standing_count(), 0);
        assert_eq!(rt.grants.live_count(), 0);

        // And the card is still decidable — nothing about the refused
        // request consumed it. Declining rather than approving, so this
        // asserts the queue state without dragging in whether the host has
        // a mailer wired for the native case.
        rt.resolve_approval(&id, Verdict::Deny, operator())
            .await
            .unwrap();
        assert!(rt.pending_approvals().is_empty());
    }
}

/// Issue #444: `may_be_granted_standing` is what makes an agent's standing
/// ALLOW unmintable in production, but the only coverage of it lived on
/// the pure function in isolation. This drives the refusal through a real
/// park-then-resolve for the three genuinely different mechanisms a live
/// company can produce an ungrantable card from: a Composio action the
/// provider's own catalogue classifies as a send, a bare `shell` command
/// (declared, not argument-graded), and an MCP bridge call (argument-graded
/// on its own server/tool pair rather than the Composio catalogue). Each is
/// a distinct code path inside `consequence_of`, so a regression in any one
/// of them would not be caught by testing only one.
///
/// A grantable tool rides along as the boundary: the same verdict, scope
/// and card shape, differing only in the one fact that decides the
/// outcome, must succeed rather than refuse.
#[tokio::test]
async fn the_three_reachable_ungrantable_kinds_refuse_a_standing_approve() {
    for (label, effect) in [
        (
            "composio send action",
            harness_effect(
                "ops",
                "composio_execute",
                crate::policy::test_support::composio_send_args(),
            ),
        ),
        (
            "shell command",
            harness_effect(
                "ops",
                crate::policy::consequence::SHELL,
                serde_json::json!({ "command": "rm -rf build/" }),
            ),
        ),
        (
            "mcp bridge call",
            harness_effect(
                "ops",
                crate::policy::consequence::MCP_CALL_TOOL,
                serde_json::json!({ "server": "jira", "tool": "get_issue", "arguments": {} }),
            ),
        ),
    ] {
        let home_dir = tmp_home();
        let (rt, id) = park_one_blocked_tool_call(home_dir.path().to_path_buf(), effect).await;

        let err = rt
            .resolve_approval_spawned(&id, Verdict::Approve, operator(), tool_scope())
            .await
            .expect_err(&format!("{label} must refuse a standing approve"));
        assert!(
            matches!(err, OpenCompanyError::InvalidRequest(_)),
            "{label}: refusal must be a bad-request, not a server fault: {err:?}"
        );
        assert_eq!(
            rt.pending_approvals().len(),
            1,
            "{label}: the card stays parked for a per-call decision"
        );
        assert_eq!(
            rt.grants.standing_count(),
            0,
            "{label}: no standing grant minted"
        );
        assert_eq!(
            rt.grants.live_count(),
            0,
            "{label}: no single-use grant minted either"
        );
    }

    // The boundary: a tool the catalogue classifies as grantable, offered
    // the identical verdict and scope, must succeed rather than refuse.
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
    rt.resolve_approval_spawned(&id, Verdict::Approve, operator(), tool_scope())
        .await
        .expect("a grantable tool must not be refused by the same check");
    assert_eq!(rt.grants.standing_count(), 1);
}

/// The refusal above is specific to the tool's own consequence, not to
/// which subject the card names — a workflow gate wrapping an ungrantable
/// inner call must refuse a standing approve exactly the same way an
/// agent's own call does, via the identical `gate_inner_call` read
/// `may_be_granted_standing` already uses. Nothing before this test drove
/// a workflow-subject card through this specific refusal; the only
/// workflow-subject coverage on `check_broadly_scoped` was the DENY-side
/// refusal (`a_standing_deny_on_a_workflow_gate_is_refused_and_mints_nothing`),
/// a different branch of the same function.
#[tokio::test]
async fn a_workflow_gate_wrapping_an_ungrantable_inner_call_refuses_a_standing_approve_too() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        Effect {
            kind: crate::runtime::workflow_resume::WORKFLOW_APPROVE_KIND.to_string(),
            group: EffectGroup::Other,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            payload: serde_json::json!({
                "workflow_id": "sports_digest",
                "node_id": "run_shell",
                "tool": "shell",
                "args": { "command": "echo hi" },
            }),
            agent: None,
            run_id: None,
        },
    )
    .await;

    let err = rt
        .resolve_approval_spawned(&id, Verdict::Approve, operator(), tool_scope())
        .await
        .expect_err("a workflow gate wrapping an ungrantable inner call must refuse too");
    assert!(
        matches!(
            err,
            OpenCompanyError::InvalidRequest(ref msg)
                if msg.contains("cannot be granted for a period")
        ),
        "{err:?}"
    );
    assert_eq!(rt.pending_approvals().len(), 1);
    assert_eq!(rt.grants.standing_count(), 0);
    assert_eq!(rt.grants.live_count(), 0);
}

/// Issue #1458: a standing **denial** for a workflow is refused at the
/// edge — the workflow gate does not enforce a `Deny` verdict
/// (`src/workflows/gate.rs`), so a time-bounded refusal would be a control
/// that never took effect. The card stays parked so the operator can still
/// deny it once.
#[tokio::test]
async fn a_standing_deny_on_a_workflow_gate_is_refused_and_mints_nothing() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        Effect {
            kind: crate::runtime::workflow_resume::WORKFLOW_APPROVE_KIND.to_string(),
            group: EffectGroup::Other,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            payload: serde_json::json!({
                "workflow_id": "sports_digest",
                "node_id": "fetch_bbc",
                "tool": "web_fetch",
                "args": { "url": "https://docs.rs/x" },
            }),
            agent: None,
            run_id: None,
        },
    )
    .await;

    let err = match rt
        .resolve_approval_spawned(&id, Verdict::Deny, operator(), tool_scope())
        .await
    {
        Ok(_) => panic!("a workflow standing denial must be refused at the edge"),
        Err(err) => err,
    };
    assert!(
        matches!(
            err,
            OpenCompanyError::InvalidRequest(ref msg)
                if msg.contains("'web_fetch' is a workflow call")
                    && msg.contains("does not enforce a standing refusal")
        ),
        "{err:?}"
    );

    assert_eq!(
        rt.pending_approvals().len(),
        1,
        "the card is still there to be denied once"
    );
    assert_eq!(rt.grants.standing_count(), 0, "no refusal is minted");
    assert_eq!(rt.grants.live_count(), 0);

    // And the operator can still deny it once — the refused request did not
    // consume the card.
    rt.resolve_approval(&id, Verdict::Deny, operator())
        .await
        .unwrap();
    assert!(rt.pending_approvals().is_empty());
}

/// INPUT: the existing pin of this refusal drives exactly one inner tool
/// (`web_fetch`) through the wrapper. The refusal is read off
/// `subject_of` alone — it does not look at which tool the gate wraps —
/// so a regression that quietly narrowed it to `web_fetch` specifically
/// (a hardcoded name check instead of the subject check) would still
/// pass the existing test. A different inner tool, routed through
/// `gate_inner_call` the identical way, closes that gap.
#[tokio::test]
async fn a_workflow_gate_wrapping_a_different_inner_tool_is_also_refused_a_standing_deny() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        Effect {
            kind: crate::runtime::workflow_resume::WORKFLOW_APPROVE_KIND.to_string(),
            group: EffectGroup::Other,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            payload: serde_json::json!({
                "workflow_id": "sales_digest",
                "node_id": "notify_finance",
                "tool": "composio_execute",
                "args": crate::policy::test_support::composio_send_args(),
            }),
            agent: None,
            run_id: None,
        },
    )
    .await;

    let err = rt
        .resolve_approval_spawned(&id, Verdict::Deny, operator(), tool_scope())
        .await
        .expect_err(
            "a workflow gate wrapping ANY inner tool must refuse a standing deny, not just \
             web_fetch",
        );
    assert!(
        matches!(
            err,
            OpenCompanyError::InvalidRequest(ref msg)
                if msg.contains("'composio_execute' is a workflow call")
                    && msg.contains("does not enforce a standing refusal")
        ),
        "{err:?}"
    );
    assert_eq!(rt.grants.standing_count(), 0);
    assert_eq!(rt.pending_approvals().len(), 1);
}

/// AUTH: the refusal is driven by [`subject_of`], and `subject_of` reads
/// only `effect.kind == WORKFLOW_APPROVE_KIND` — it never inspects
/// whether the payload's inner call actually parses. A gate whose
/// `gate_inner_call` cannot resolve a tool (a malformed or partial
/// payload) must still be recognised as a workflow subject and refused,
/// naming the wrapper kind itself rather than silently falling through
/// to the agent-subject path and minting a refusal that governs nobody's
/// real call.
#[tokio::test]
async fn a_workflow_gate_with_an_unparseable_inner_call_is_still_refused_a_standing_deny() {
    let home_dir = tmp_home();
    let (rt, id) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        Effect {
            kind: crate::runtime::workflow_resume::WORKFLOW_APPROVE_KIND.to_string(),
            group: EffectGroup::Other,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            // No `tool`/`args` keys at all — `gate_inner_call` cannot
            // resolve an inner call from this payload.
            payload: serde_json::json!({
                "workflow_id": "sales_digest",
                "node_id": "notify_finance",
            }),
            agent: None,
            run_id: None,
        },
    )
    .await;

    let err = rt
        .resolve_approval_spawned(&id, Verdict::Deny, operator(), tool_scope())
        .await
        .expect_err("subject_of does not require gate_inner_call to succeed");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert_eq!(rt.grants.standing_count(), 0);
    assert_eq!(rt.pending_approvals().len(), 1);
}

/// Issue #1458, the console half: a workflow-gate card must not offer a
/// standing **denial**.
///
/// `check_broadly_scoped` refuses a workflow standing denial with a 400 —
/// the gate does not enforce a `Deny` verdict — so a card that advertised
/// the control would let the operator click "don't ask again" and get an
/// error that leaves the approval parked. The grant half is still offered:
/// a workflow *can* hold a standing permission. Only the deny control is
/// withheld.
#[tokio::test]
async fn a_workflow_gate_card_is_not_advertised_as_broadly_deniable() {
    let home_dir = tmp_home();
    let (rt, _) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        Effect {
            kind: crate::runtime::workflow_resume::WORKFLOW_APPROVE_KIND.to_string(),
            group: EffectGroup::Other,
            amount_usd: None,
            established_thread: false,
            first_time_counterparty: false,
            payload: serde_json::json!({
                "workflow_id": "sports_digest",
                "node_id": "fetch_bbc",
                "tool": "web_fetch",
                "args": { "url": "https://docs.rs/x" },
            }),
            agent: None,
            run_id: None,
        },
    )
    .await;

    assert!(
        !rt.pending_approvals()[0].broadly_deniable,
        "a workflow card must not advertise a standing refusal nothing enforces"
    );
    assert!(
        rt.pending_approvals()[0].broadly_grantable,
        "a workflow card can still hold a standing permission"
    );

    // The same tool, parked from an agent turn, offers the deny control.
    let home_dir = tmp_home();
    let (rt, _) = park_one_blocked_tool_call(
        home_dir.path().to_path_buf(),
        grantable_effect(
            "ops",
            crate::policy::consequence::WEB_FETCH,
            serde_json::json!({ "url": "https://docs.rs/x" }),
        ),
    )
    .await;
    assert!(rt.pending_approvals()[0].broadly_deniable);
}
