use super::tests_core::*;

/// Issue #435, guarding #379's decision: a grant's origin is **routing, not
/// identity**. Neither the channel nor the thread within it may join the
/// redemption match.
///
/// The failure this prevents is silent and expensive. If location were part
/// of the match, a re-dispatched turn that came back anywhere other than
/// where it started would simply fail to find its grant and re-park — the
/// operator approves, the agent asks again, and nothing anywhere says why.
/// `origin_parent` makes that mistake newly reachable by adding a second,
/// finer location to get wrong, so it is pinned here rather than left to
/// the comment on the field.
#[test]
fn a_grants_origin_is_routing_and_never_part_of_the_match() {
    let args = serde_json::json!({ "to": "a@b.test" });

    // Minted inside a thread; redeemed by a turn that knows only the call.
    // `consume` is not even given a location to compare against — that is
    // the shape of the guarantee.
    let set = GrantSet::default();
    set.grant(GrantedCall {
        origin_thread: Some("desk-finance".to_string()),
        origin_parent: Some(EventSeq::new(7)),
        origin_task: None,
        ..call("a1", "finance", "composio_execute", args.clone())
    });
    let redeemed = set
        .consume("finance", "composio_execute", &args)
        .expect("a thread-rooted grant is redeemed by the matching call");
    assert_eq!(
        redeemed.origin_parent,
        Some(EventSeq::new(7)),
        "and the location rides along on the consumed grant, for routing",
    );
    assert_eq!(redeemed.origin_thread, Some("desk-finance".to_string()));

    // Two grants differing *only* in origin are the same call as far as
    // matching is concerned: the first still redeems, so nothing about the
    // location narrowed it.
    for origin in [
        (None, None),
        (Some("desk-finance".to_string()), None),
        (Some("agent-cfo".to_string()), Some(EventSeq::new(9))),
    ] {
        let set = GrantSet::default();
        set.grant(GrantedCall {
            origin_thread: origin.0,
            origin_parent: origin.1,
            origin_task: None,
            ..call("a1", "finance", "composio_execute", args.clone())
        });
        assert!(
            set.consume("finance", "composio_execute", &args).is_some(),
            "the operator approved a call, not a location",
        );
    }
}

#[test]
fn a_grant_is_redeemed_exactly_once() {
    let set = GrantSet::default();
    let args = serde_json::json!({ "to": "a@b.test" });
    set.grant(call("a1", "finance", "composio_execute", args.clone()));

    assert!(set.consume("finance", "composio_execute", &args).is_some());
    assert!(
        set.consume("finance", "composio_execute", &args).is_none(),
        "one approval buys one call, not an open door"
    );
    assert_eq!(set.live_count(), 0);
    assert_eq!(set.drain_consumed().len(), 1);
    assert!(set.drain_consumed().is_empty(), "the drain is a take");
}

#[test]
fn a_grant_is_scoped_to_its_agent_and_its_exact_arguments() {
    let set = GrantSet::default();
    let args = serde_json::json!({ "to": "a@b.test", "body": "hi" });
    set.grant(call("a1", "finance", "composio_execute", args.clone()));

    // Another agent making the identical call is not who was approved.
    assert!(
        set.consume("marketing", "composio_execute", &args)
            .is_none()
    );
    // A different tool is not what was approved.
    assert!(set.consume("finance", "workspace_write", &args).is_none());
    // Different arguments are not what the operator saw.
    assert!(
        set.consume(
            "finance",
            "composio_execute",
            &serde_json::json!({ "to": "someone@else.test", "body": "hi" })
        )
        .is_none()
    );
    // An extra key is a different call too — matching is whole-value.
    assert!(
        set.consume(
            "finance",
            "composio_execute",
            &serde_json::json!({ "to": "a@b.test", "body": "hi", "cc": "x@y.test" })
        )
        .is_none()
    );
    // ...and none of those near-misses burned the grant.
    assert!(set.consume("finance", "composio_execute", &args).is_some());
}

#[test]
fn peek_reads_without_redeeming() {
    let set = GrantSet::default();
    let args = serde_json::json!({ "q": 1 });
    set.grant(call("a1", "finance", "web_fetch", args.clone()));

    let seen = set.peek(&ApprovalId::new("a1")).expect("grant is live");
    assert_eq!(seen.tool, "web_fetch");
    assert_eq!(set.live_count(), 1, "peeking must not consume");
    assert!(set.peek(&ApprovalId::new("nope")).is_none());
}

/// Issue #796: the window between a park and the operator's decision. A
/// parked approval mints no grant, so `any_for_task` would read `false`
/// without the pending mark — and an unrelated turn's checkout sweep would
/// then delete the parked step's tree. The mark keeps the task live until the
/// approval settles, and two approvals on one task clear independently.
#[test]
fn a_still_parked_approval_keeps_its_task_alive() {
    let set = GrantSet::default();
    assert!(!set.any_for_task("t-1"), "nothing names the task yet");

    // A parked approval: no grant, but the task is marked pending.
    set.mark_pending(&ApprovalId::new("a1"), "t-1".to_string());
    assert!(
        set.any_for_task("t-1"),
        "a pending approval must keep the task live"
    );

    // A second approval parks on the same task.
    set.mark_pending(&ApprovalId::new("a2"), "t-1".to_string());
    // Settling the first still leaves the second holding the task.
    set.clear_pending(&ApprovalId::new("a1"));
    assert!(
        set.any_for_task("t-1"),
        "the second pending approval still names the task"
    );
    // Settling the last (denied or expired, so no grant follows) drops it.
    set.clear_pending(&ApprovalId::new("a2"));
    assert!(
        !set.any_for_task("t-1"),
        "no pending approval and no grant leaves nothing to keep the task alive"
    );
}

#[test]
fn sweep_expires_only_grants_past_the_ttl() {
    let set = GrantSet::default();
    set.grant(call("old", "finance", "t", serde_json::json!({})));
    let mut fresh = call("new", "finance", "t2", serde_json::json!({}));
    fresh.at_millis = 900_000;
    set.grant(fresh);

    let expired = set.sweep(1_000 + GRANT_TTL_MILLIS, GRANT_TTL_MILLIS);
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].approval_id, ApprovalId::new("old"));
    assert_eq!(set.live_count(), 1, "the fresh grant survives");
}

#[test]
fn rehydrate_seeds_the_live_set() {
    let set = GrantSet::default();
    set.rehydrate([
        call("a1", "finance", "t", serde_json::json!({})),
        call("a2", "legal", "t", serde_json::json!({})),
    ]);
    assert_eq!(set.live_count(), 2);
    assert!(set.peek(&ApprovalId::new("a2")).is_some());
}

/// `rehydrate` seeds a `HashMap` keyed by approval id, so two journal
/// lines that name the same id (a replay quirk, or a caller that passes
/// the same call twice) do not double-count the grant — the later entry
/// in the iterator simply overwrites the earlier one, same as inserting
/// the same key twice into any map.
#[test]
fn rehydrate_with_a_duplicate_approval_id_keeps_only_the_last_entry() {
    let set = GrantSet::default();
    set.rehydrate([
        call("a1", "finance", "old_tool", serde_json::json!({"n": 1})),
        call("a1", "finance", "new_tool", serde_json::json!({"n": 2})),
    ]);
    assert_eq!(
        set.live_count(),
        1,
        "one approval id must seed exactly one live grant, not two"
    );
    let seeded = set.peek(&ApprovalId::new("a1")).expect("the id is live");
    assert_eq!(
        seeded.tool, "new_tool",
        "the later entry in the replay order wins"
    );
}

/// `rehydrate` enforces no ceiling of its own on how many grants a single
/// replay can seed — the boot-time journal is the only source of a cap
/// (if any), and this queue must not silently drop entries past some
/// count. Mirrors the standing-list no-ceiling pin for the live side.
#[test]
fn rehydrate_seeds_every_grant_in_a_large_replay_batch_with_no_cap() {
    let set = GrantSet::default();
    let calls: Vec<_> = (0..500)
        .map(|i| {
            call(
                &format!("a{i}"),
                "finance",
                "t",
                serde_json::json!({ "i": i }),
            )
        })
        .collect();
    set.rehydrate(calls);
    assert_eq!(
        set.live_count(),
        500,
        "every grant in the replay batch must be seeded; none held back"
    );
    assert!(set.peek(&ApprovalId::new("a499")).is_some());
}

/// A boot-time `rehydrate` must not clobber grants a concurrent `consume`
/// is already working with: it seeds only the ids it was handed, under
/// the same lock as every other `GrantSet` operation, so a batch replay
/// can never wipe a grant that was live before the batch arrived.
#[test]
fn rehydrate_only_adds_its_own_batch_and_leaves_other_live_grants_alone() {
    let set = GrantSet::default();
    let pre_existing_args = serde_json::json!({ "amount_usd": 12.0 });
    set.grant(call(
        "pre-existing",
        "finance",
        "pay_invoice",
        pre_existing_args,
    ));

    let batch: Vec<_> = (0..50)
        .map(|i| {
            call(
                &format!("new{i}"),
                "ops",
                "t",
                serde_json::json!({ "i": i }),
            )
        })
        .collect();
    set.rehydrate(batch);

    assert!(
        set.peek(&ApprovalId::new("pre-existing")).is_some(),
        "a grant already live before the batch must survive the rehydrate"
    );
    assert_eq!(
        set.live_count(),
        51,
        "the pre-existing grant plus every grant in the batch must all be live"
    );
}

/// Concurrent redemption of one grant: exactly one caller wins.
///
/// The match and the removal are one critical section precisely so this
/// cannot double-fire. A read-then-remove would let both threads see the
/// grant and both proceed, turning one approval into two executions of a
/// tool the operator approved once.
#[test]
fn two_threads_racing_one_grant_yield_exactly_one_winner() {
    let set = GrantSet::default();
    let args = serde_json::json!({ "amount_usd": 40.0 });
    set.grant(call("a1", "finance", "pay_invoice", args.clone()));

    let barrier = Arc::new(std::sync::Barrier::new(8));
    let winners = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let set = set.clone();
            let args = args.clone();
            let barrier = Arc::clone(&barrier);
            let winners = Arc::clone(&winners);
            std::thread::spawn(move || {
                barrier.wait();
                if set.consume("finance", "pay_invoice", &args).is_some() {
                    winners.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("thread");
    }

    assert_eq!(winners.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(set.live_count(), 0);
    assert_eq!(set.drain_consumed().len(), 1, "one consumption journaled");
}

/// Consumption is buffered in `GrantState::consumed`, not journaled — the
/// durable `GrantConsumed` record is written one layer up, by the cycle
/// runner's drain, strictly after `consume` already returned. A restart
/// between "the tool ran" (`consume` removed the grant here) and "the
/// drain journalled it" therefore replays the pre-consumption journal
/// state: the grant comes back exactly as if it had never been redeemed,
/// and the identical call is admitted a **second** time with no second
/// approval ever asked. Modelled at the `GrantSet` layer: rehydrating the
/// same `GrantedCall` into a fresh set is exactly what a restart's replay
/// does when the `GrantConsumed` line never reached disk (see
/// `a_grant_consumed_but_not_yet_drained_replays_as_live_after_a_restart`
/// in `journal.rs` for the journal-file half of this).
#[test]
fn a_consumed_but_undrained_grant_re_admits_the_identical_call_after_rehydrate() {
    let live = GrantSet::default();
    let args = serde_json::json!({ "to": "a@b.test" });
    let grant = call("appr-crash", "finance", "composio_execute", args.clone());
    live.grant(grant.clone());

    // The tool runs. `consume` removes it from `live` and buffers the id
    // — the real, synchronous, journal-less path `ToolPolicy::check`
    // takes deep inside a turn.
    assert!(
        live.consume("finance", "composio_execute", &args).is_some(),
        "the first call is admitted normally"
    );
    assert_eq!(live.live_count(), 0);
    // The drain that would journal `GrantConsumed` never runs here —
    // modelling the crash between the tool running and the next cycle's
    // drain picking the buffered id up.
    assert_eq!(
        live.drain_consumed().len(),
        1,
        "the consumption sits buffered, exactly as it would right before the crash"
    );

    // Restart: replay seeds a fresh set from the journal, which never
    // learned the grant was spent.
    let after_restart = GrantSet::default();
    after_restart.rehydrate([grant]);
    assert_eq!(
        after_restart.live_count(),
        1,
        "the undrained consumption re-arms the grant on replay"
    );

    // AUTH: a different agent claiming the same tool and arguments must
    // not redeem it — only the agent the grant actually names may
    // collect the replay.
    assert!(
        after_restart
            .consume("legal", "composio_execute", &args)
            .is_none(),
        "the re-armed grant must still only admit the agent it was minted for"
    );
    assert_eq!(
        after_restart.live_count(),
        1,
        "the wrong-agent attempt must not have consumed the grant"
    );

    // BOUND: different arguments for the right agent and tool must not
    // redeem it either — the re-arm is an exact-match replay, not a
    // blanket re-authorization of the tool.
    assert!(
        after_restart
            .consume(
                "finance",
                "composio_execute",
                &serde_json::json!({ "to": "someone-else@b.test" })
            )
            .is_none(),
        "the re-armed grant must not admit a call with different arguments"
    );
    assert_eq!(after_restart.live_count(), 1);

    // FAIL: the actual duplication — the identical (agent, tool, args)
    // call the operator approved exactly once is admitted a SECOND time,
    // with no new approval in between. This is the concrete failure
    // TOOL-005 names, not a list-length assertion one layer removed
    // from it.
    assert!(
        after_restart
            .consume("finance", "composio_execute", &args)
            .is_some(),
        "documented duplication window: the crash-then-replay grant re-admits the \
         identical call a second time with no second approval"
    );
    assert_eq!(after_restart.live_count(), 0);
}

/// The two subject fields reconcile in exactly one place, and a line with no
/// `workflow` is an agent permission — which is what makes every journal line
/// written before issue #1098 replay unchanged.
#[test]
fn subject_reads_a_workflow_line_as_a_workflow_and_everything_else_as_an_agent() {
    assert_eq!(
        standing("g1", "maya", "web_fetch", 9_999).subject(),
        GrantSubject::agent("maya")
    );
    assert_eq!(
        standing_workflow("g2", "sports_blog", "web_fetch", None, 9_999).subject(),
        GrantSubject::workflow("sports_blog")
    );
}

/// A journal line written before the field existed carries no `workflow` key
/// at all. It must deserialize, and it must replay as the agent permission it
/// was — not as a workflow one keyed on an empty string.
#[test]
fn a_pre_1098_journal_line_replays_as_an_agent_permission() {
    let line = r#"{
        "id": "g-old",
        "agent": "maya",
        "tool": "web_fetch",
        "granted_by": { "kind": "user", "id": "user-1" },
        "approval_id": "approval-old",
        "at_millis": 1000,
        "expires_at_millis": 9999
    }"#;
    let replayed: StandingGrant =
        serde_json::from_str(line).expect("a pre-#1098 line still deserializes");
    assert_eq!(replayed.workflow, None);
    assert_eq!(replayed.subject(), GrantSubject::agent("maya"));

    let set = GrantSet::default();
    set.grant_standing(replayed);
    assert!(
        set.match_standing(&GrantSubject::agent("maya"), "web_fetch", None, 2_000)
            .is_some(),
        "a replayed line must still admit the calls it always admitted"
    );
}

/// A line naming neither `agent` nor `workflow` is not one either format
/// wrote on purpose — both fields default on deserialize, so it still
/// replays rather than refusing to load. `subject` resolves it the same
/// way a pre-#1098 line resolves: no `workflow` key means agent, and an
/// empty agent string is still a value, so it replays as a permission held
/// by the empty-string agent. Nothing mints a line shaped like this today;
/// this pins that a malformed one does not panic or silently vanish, and
/// that it cannot be mistaken for a workflow permission.
#[test]
fn a_line_naming_neither_agent_nor_workflow_replays_as_the_empty_string_agent() {
    let line = r#"{
        "id": "g-malformed",
        "tool": "web_fetch",
        "granted_by": { "kind": "user", "id": "user-1" },
        "approval_id": "approval-malformed",
        "at_millis": 1000,
        "expires_at_millis": 9999
    }"#;
    let replayed: StandingGrant =
        serde_json::from_str(line).expect("agent and workflow both default on load");
    assert_eq!(replayed.agent, "");
    assert_eq!(replayed.workflow, None);
    assert_eq!(replayed.subject(), GrantSubject::agent(""));
    assert_ne!(
        replayed.subject(),
        GrantSubject::workflow(""),
        "an empty agent must never resolve to a workflow subject"
    );
}

/// The two subjects are separate namespaces. A workflow named like a teammate
/// must not spend that teammate's permission, in either direction.
#[test]
fn an_agent_and_a_workflow_of_the_same_name_do_not_share_a_permission() {
    let set = GrantSet::default();
    set.grant_standing(standing("g1", "digest", "web_fetch", 9_999));

    assert!(
        set.match_standing(&GrantSubject::workflow("digest"), "web_fetch", None, 2_000)
            .is_none(),
        "a workflow must not spend a teammate's permission"
    );

    let set = GrantSet::default();
    set.grant_standing(standing_workflow("g2", "digest", "web_fetch", None, 9_999));
    assert!(
        set.match_standing(&GrantSubject::agent("digest"), "web_fetch", None, 2_000)
            .is_none(),
        "a teammate must not spend a workflow's permission"
    );
}

/// The scope machinery is subject-agnostic: a workflow permission narrows by
/// host on exactly the terms a teammate's does.
#[test]
fn a_workflow_permission_is_narrowed_by_its_host_like_any_other() {
    let set = GrantSet::default();
    set.grant_standing(standing_workflow(
        "g1",
        "sports_blog",
        "web_fetch",
        Some("https://www.bbc.co.uk"),
        9_999,
    ));
    let subject = GrantSubject::workflow("sports_blog");

    assert!(
        set.match_standing(&subject, "web_fetch", Some("https://www.bbc.co.uk"), 2_000)
            .is_some(),
        "the host it was granted for keeps passing"
    );
    assert!(
        set.match_standing(&subject, "web_fetch", Some("https://www.espn.com"), 2_000)
            .is_none(),
        "a repointed host re-parks — scope equality is the invalidation"
    );
    assert!(
        set.match_standing(&subject, "web_fetch", None, 2_000)
            .is_none(),
        "a call whose host cannot be read is refused by a scoped permission"
    );
}

/// The whole point of the scope: the tool stops asking, whatever the
/// arguments, until the deadline.
#[test]
fn a_standing_grant_admits_varying_arguments_until_it_expires() {
    let set = GrantSet::default();
    set.grant_standing(standing("g1", "ops", "shell", 10_000));

    assert!(
        set.match_standing(&GrantSubject::agent("ops"), "shell", None, 2_000)
            .is_some()
    );
    // Matching does not depend on arguments at all — there are none to
    // depend on. Two different calls, same admission.
    assert!(
        set.match_standing(&GrantSubject::agent("ops"), "shell", None, 9_999)
            .is_some()
    );
    assert_eq!(
        set.standing_count(),
        1,
        "redeeming a standing grant must not remove it — that is single-use's job"
    );

    // The deadline instant itself is already past.
    assert!(
        set.match_standing(&GrantSubject::agent("ops"), "shell", None, 10_000)
            .is_none()
    );
    assert!(
        set.match_standing(&GrantSubject::agent("ops"), "shell", None, 10_001)
            .is_none()
    );
}

#[test]
fn a_standing_deny_matches_only_its_subject_tool_and_deadline() {
    let set = GrantSet::default();
    let mut deny = standing("deny-1", "ops", "shell", 10_000);
    deny.verdict = crate::ports::types::Verdict::Deny;
    set.grant_standing(deny);

    assert!(
        set.match_standing_with_verdict(
            &GrantSubject::agent("ops"),
            "shell",
            None,
            crate::ports::types::Verdict::Deny,
            2_000,
        )
        .is_some()
    );
    assert!(
        set.match_standing_with_verdict(
            &GrantSubject::agent("ops"),
            "shell",
            None,
            crate::ports::types::Verdict::Deny,
            10_000,
        )
        .is_none()
    );
    assert!(
        set.match_standing_with_verdict(
            &GrantSubject::agent("ops"),
            "shell",
            None,
            crate::ports::types::Verdict::Approve,
            2_000,
        )
        .is_none()
    );
}

/// Issue #1458: a standing **denial** must not keep a task's checkout alive.
///
/// A denied approval is never re-dispatched — the brain's `ApprovalResolved`
/// arm runs only on `Approve` — so nothing will ever reclaim the held tree,
/// and counting the deny's `origin_task` would retain it for the denial's
/// full duration. Only an approving grant names a task that may still resume.
#[test]
fn a_standing_deny_does_not_keep_a_task_checkout_alive() {
    let set = GrantSet::default();
    let mut deny = standing("deny-1", "ops", "shell", 10_000);
    deny.verdict = crate::ports::types::Verdict::Deny;
    deny.origin_task = Some("t-1".to_string());
    set.grant_standing(deny);
    assert!(
        !set.any_for_task("t-1"),
        "a deny is never resumed, so it must not hold the task's checkout"
    );

    // An approving standing grant for the same task is still a live reason.
    let mut grant = standing("grant-1", "ops", "shell", 10_000);
    grant.origin_task = Some("t-1".to_string());
    set.grant_standing(grant);
    assert!(
        set.any_for_task("t-1"),
        "an approving standing grant still keeps the checkout live"
    );
}
