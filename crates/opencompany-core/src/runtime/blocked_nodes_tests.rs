use super::*;
use serde_json::json;

#[test]
fn arm_then_release_hands_back_the_stashed_facts() {
    let q = BlockedNodeQueue::default();
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "topic": "x" }),
        &StartedBy::Agent("ceo".into()),
    );
    assert!(q.is_armed("workflow-node:run-1:draft"));

    let block = q.release("workflow-node:run-1:draft").expect("armed");
    assert_eq!(block.workflow_id, "digest");
    assert_eq!(block.input, json!({ "topic": "x" }));
    assert_eq!(
        block.started_by,
        StartedBy::Agent("ceo".into()),
        "the blocked run's own attribution must ride the stash, not reset on release"
    );
    assert_eq!(q.waiting(), 0, "release drops the stash");
    assert!(!q.is_armed("workflow-node:run-1:draft"));
}

#[test]
fn release_of_an_unheld_turn_is_none() {
    let q = BlockedNodeQueue::default();
    assert!(q.release("workflow-node:run-9:ghost").is_none());
}

#[test]
fn first_arm_wins_for_a_repeated_key() {
    let q = BlockedNodeQueue::default();
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "topic": "first" }),
        &StartedBy::Operator,
    );
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "topic": "second" }),
        &StartedBy::Agent("ceo".into()),
    );
    let block = q.release("workflow-node:run-1:draft").expect("armed");
    assert_eq!(block.input, json!({ "topic": "first" }));
    assert_eq!(
        block.started_by,
        StartedBy::Operator,
        "first write wins for started_by too, same as workflow_id/input"
    );
}

/// Issue #1816: rehydrating from the journal's still-live stashes re-arms the
/// queue so a post-restart release finds the run to re-dispatch — the boot
/// path a process replacement between park and approve depends on.
#[test]
fn rearm_rehydrates_stashes_a_restart_would_have_lost() {
    let q = BlockedNodeQueue::default();
    q.rearm(vec![
        (
            "workflow-node:run-1:draft".to_string(),
            "digest".to_string(),
            json!({ "topic": "x" }),
            StartedBy::Agent("ceo".into()),
        ),
        (
            "workflow-node:run-2:draft".to_string(),
            "digest".to_string(),
            json!({ "topic": "y" }),
            StartedBy::Operator,
        ),
    ]);
    assert_eq!(q.waiting(), 2, "both durable stashes came back");
    let block = q.release("workflow-node:run-1:draft").expect("rehydrated");
    assert_eq!(block.workflow_id, "digest");
    assert_eq!(block.input, json!({ "topic": "x" }));
    assert_eq!(
        block.started_by,
        StartedBy::Agent("ceo".into()),
        "rearm must carry the real attribution through, not degrade every \
         rehydrated stash to Operator"
    );
}

/// A live stash (inherited on a rebuild) is never clobbered by a journal
/// replay of the same turn — `rearm` is first-write-wins like `arm`.
#[test]
fn rearm_does_not_clobber_a_live_stash() {
    let q = BlockedNodeQueue::default();
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "n": "live" }),
        &StartedBy::Operator,
    );
    q.rearm(vec![(
        "workflow-node:run-1:draft".to_string(),
        "digest".to_string(),
        json!({ "n": "replayed" }),
        StartedBy::Agent("ceo".into()),
    )]);
    let block = q.release("workflow-node:run-1:draft").expect("armed");
    assert_eq!(block.input, json!({ "n": "live" }), "live wins over replay");
    assert_eq!(
        block.started_by,
        StartedBy::Operator,
        "live wins over replay for started_by too"
    );
}

/// A fresh stash starts unapproved, and `mark_approved` flips it — the
/// state `resume_blocked_agent_node` reads alongside the release batch.
#[test]
fn mark_approved_flips_the_stashed_flag() {
    let q = BlockedNodeQueue::default();
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "n": 1 }),
        &StartedBy::Operator,
    );

    q.mark_approved("workflow-node:run-1:draft");

    let block = q.release("workflow-node:run-1:draft").expect("armed");
    assert!(
        block.approved,
        "marking approved before release must survive to the release"
    );
}

/// `mark_approved` on a turn with no stash is a no-op, not a panic or a
/// phantom entry — there is nothing yet for the fact to attach to.
#[test]
fn mark_approved_on_an_unarmed_turn_is_a_noop() {
    let q = BlockedNodeQueue::default();
    q.mark_approved("workflow-node:run-9:ghost");
    assert_eq!(q.waiting(), 0, "no stash was created");
}

/// Issue #1816: a restart between the first and second decision on a
/// two-call node loses the first decision from `ContinuationQueue`'s
/// released batch (see that module's docs), but `mark_approved` called at
/// decide time — before the restart wipes the in-memory queues — is what
/// this queue's own `approved` flag is for. Rehydrating the stash via
/// `rearm` alone reproduces the gap: the flag comes back `false` until the
/// caller also replays the durable approvals the way the boot builder does.
#[test]
fn rearm_alone_does_not_recover_a_pre_restart_approval() {
    let q = BlockedNodeQueue::default();
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "n": 1 }),
        &StartedBy::Operator,
    );
    q.mark_approved("workflow-node:run-1:draft");

    // Simulate the restart: the in-memory queue is gone, and boot rehydrates
    // the stash from the journal's still-live record — but not yet the
    // approval, which is a separate durable fact the caller must fold in.
    let rehydrated = BlockedNodeQueue::default();
    rehydrated.rearm(vec![(
        "workflow-node:run-1:draft".to_string(),
        "digest".to_string(),
        json!({ "n": 1 }),
        StartedBy::Operator,
    )]);

    let block = rehydrated
        .release("workflow-node:run-1:draft")
        .expect("rehydrated");
    assert!(
        !block.approved,
        "rearm alone does not know about the pre-restart approval — the \
         caller must also replay it via mark_approved, exactly as the boot \
         builder does from journal.blocked_node_approvals()"
    );
}

/// Two blocked nodes of two runs are independent stashes — a release of one
/// leaves the other untouched (the scope-disjointness a cross-continuation
/// would violate).
#[test]
fn two_blocked_nodes_do_not_share_a_stash() {
    let q = BlockedNodeQueue::default();
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "n": 1 }),
        &StartedBy::Operator,
    );
    q.arm(
        "workflow-node:run-2:draft",
        "digest",
        &json!({ "n": 2 }),
        &StartedBy::Operator,
    );

    let first = q.release("workflow-node:run-1:draft").expect("armed");
    assert_eq!(first.input, json!({ "n": 1 }));
    assert!(
        q.is_armed("workflow-node:run-2:draft"),
        "the other run stays"
    );
    assert_eq!(
        q.release("workflow-node:run-2:draft").unwrap().input,
        json!({ "n": 2 })
    );
}

/// `peek` reads the same facts `release` would hand back, but leaves the
/// stash in place — the whole reason issue #1816 Stage 4 adds it beside a
/// destructive `release`.
#[test]
fn peek_reads_the_stash_without_taking_it() {
    let q = BlockedNodeQueue::default();
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "n": 1 }),
        &StartedBy::Operator,
    );

    let seen = q.peek("workflow-node:run-1:draft").expect("armed");
    assert_eq!(seen.input, json!({ "n": 1 }));
    assert!(
        q.is_armed("workflow-node:run-1:draft"),
        "peek must not remove the stash — only release does"
    );

    // A second peek sees the same thing, and the eventual release still
    // hands back the untouched facts.
    assert_eq!(
        q.peek("workflow-node:run-1:draft").unwrap().input,
        json!({ "n": 1 })
    );
    assert_eq!(
        q.release("workflow-node:run-1:draft").unwrap().input,
        json!({ "n": 1 })
    );
}

/// `peek` on a turn with no stash is `None`, not a panic — the same
/// contract `release` has for an unarmed turn.
#[test]
fn peek_on_an_unarmed_turn_is_none() {
    let q = BlockedNodeQueue::default();
    assert!(q.peek("workflow-node:run-9:ghost").is_none());
}

/// `approved_turns` names only the turns whose flag is actually set — an
/// armed-but-undecided stash is not in the list, and neither is a turn
/// this queue holds no stash for at all.
#[test]
fn approved_turns_lists_only_the_marked_ones() {
    let q = BlockedNodeQueue::default();
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "n": 1 }),
        &StartedBy::Operator,
    );
    q.arm(
        "workflow-node:run-2:draft",
        "digest",
        &json!({ "n": 2 }),
        &StartedBy::Operator,
    );
    q.mark_approved("workflow-node:run-1:draft");

    assert_eq!(
        q.approved_turns(),
        vec!["workflow-node:run-1:draft".to_string()],
        "only the marked turn is reported; the still-undecided one is not"
    );
}

/// A freshly `arm`ed queue with nothing marked reports nothing approved —
/// boot reconciliation must not fire on a node that is merely blocked, not
/// yet decided.
#[test]
fn approved_turns_is_empty_with_nothing_marked() {
    let q = BlockedNodeQueue::default();
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "n": 1 }),
        &StartedBy::Operator,
    );
    assert!(q.approved_turns().is_empty());
}

/// `stashed_turns` names every held stash regardless of its `approved`
/// flag — unlike `approved_turns`, an unapproved (denied/expired) turn
/// still appears, which is exactly what lets boot reconciliation retire
/// it instead of losing track of it (issue #1825, P2 follow-up).
#[test]
fn stashed_turns_lists_approved_and_unapproved_alike() {
    let q = BlockedNodeQueue::default();
    q.arm(
        "workflow-node:run-1:draft",
        "digest",
        &json!({ "n": 1 }),
        &StartedBy::Operator,
    );
    q.arm(
        "workflow-node:run-2:draft",
        "digest",
        &json!({ "n": 2 }),
        &StartedBy::Operator,
    );
    q.mark_approved("workflow-node:run-1:draft");

    let mut turns = q.stashed_turns();
    turns.sort();
    assert_eq!(
        turns,
        vec![
            "workflow-node:run-1:draft".to_string(),
            "workflow-node:run-2:draft".to_string(),
        ],
        "both the approved and the still-undecided/unapproved stash are named"
    );
}

/// A queue holding nothing reports no stashed turns.
#[test]
fn stashed_turns_is_empty_with_nothing_armed() {
    let q = BlockedNodeQueue::default();
    assert!(q.stashed_turns().is_empty());
}
