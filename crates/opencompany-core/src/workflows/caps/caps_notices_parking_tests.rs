use super::tests_multi_call_notices::overflowing_runner_notices;
use super::tests_turn_dispatch::single_turn;
use super::*;

/// The ordering fix that rides with it. The `parking`-is-`None` guard
/// `return`s, and it used to sit **above** the overflow branch — so on a
/// runtime with no approvals gate the discard was not even reaching the
/// log, let alone the operator.
///
/// That is the worst case, not a corner: the survivors could not be parked
/// either, so the notice is the *only* thing the operator can be told.
#[tokio::test]
async fn the_notice_survives_a_runtime_with_no_approvals_gate() {
    let over = MAX_APPROVAL_REQUESTS_PER_TURN + 2;
    let (notices, _) = overflowing_runner_notices(over, false).await;
    assert_eq!(
        notices.len(),
        1,
        "no gate to park into is exactly when the operator most needs telling: {notices:?}"
    );
}

/// A node that stayed under the cap says nothing — the notice must be the
/// exception, not a line on every run.
#[tokio::test]
async fn a_node_within_the_cap_raises_no_notice() {
    let (notices, _) = overflowing_runner_notices(MAX_APPROVAL_REQUESTS_PER_TURN, true).await;
    assert!(notices.is_empty(), "nothing was discarded: {notices:?}");
}

/// Issue #1825 (P1, found by chatgpt-codex-connector): `park_gated_calls`
/// must arm a blocked node's in-memory continuation stash itself, before
/// it parks a single call — not leave that to the runner's block-settle
/// pass (`stash_blocked_agent_nodes` in `super::super::runner`), which
/// only runs after the agent has returned, the engine has settled, and —
/// on the halt path — the run's output has already been persisted.
///
/// # The race this closes
///
/// `park_and_journal` (inside the loop this test drives) is what makes a
/// blocked node's approval card durable and clickable. Before this fix,
/// nothing armed `BlockedNodeQueue` until well after that — an operator
/// who approved the card in that window found `continue_turn` consuming
/// their decision against an empty stash: the turn retired with nothing
/// to release, and the later block-settle pass then stashed facts for a
/// decision that had already been spent, permanently stranding the run
/// (exactly the loss `stashed_turns()`'s reconciliation retires as
/// "unapproved"). `HarnessAgentRunner` carrying no trigger input was why
/// the arm could not happen here before — see `RunContext::trigger_input`
/// and this struct's own `trigger_input` field.
///
/// # Why this drives `park_gated_calls` directly
///
/// No `stash_blocked_agent_nodes` block-settle pass runs anywhere in this
/// test — the queue is inspected immediately after the parking call
/// returns, the same way the resolve path's `peek` would find it if an
/// approval landed at that instant. Pre-fix this assertion fails: nothing
/// in `park_gated_calls` armed the queue, so the peek is `None`. Post-fix
/// it holds this run's own trigger input, proving the card cannot outrun
/// the stash that redeems it.
/// What the node's diagnosis promises has to match what deciding the card
/// actually does (CodeRabbit review on #1905).
///
/// A gated tool call and an agent's blocker ride the same `approval_ids`
/// and settle the node identically, but only the first resumes on approval:
/// its park carries the node's turn key, while a blocker is parked
/// `Unlinked` with `agent: None` and no continuation — deliberately, since
/// answering a question is not authorising a call. The diagnosis said
/// "Approving the card continues this run automatically" for both, which
/// for a blocker is an operator approving a card and then watching a run
/// that never moves.
///
/// Issue #2005 moved the truthful line rather than removing the rule: a
/// blocker's answer now DOES re-enter the step, but not by approving — the
/// four verdicts differ, and one of them stops the run. The card must
/// describe that, not borrow the gated call's sentence.
#[test]
fn the_diagnosis_only_promises_a_resume_it_can_keep() {
    let gated = ParkedCalls {
        tools: vec!["publish_artifact".to_string()],
        approval_ids: vec!["appr-1".to_string()],
        unparkable: 0,
        blockers: 0,
    };
    let text = blocked_diagnosis(Some("work"), "writer", &gated);
    assert!(
        text.contains("continues this run automatically"),
        "a gated call really does resume on approval: {text}"
    );

    let blocker = ParkedCalls {
        tools: vec!["escalate_to_human".to_string()],
        approval_ids: vec!["appr-1".to_string()],
        unparkable: 0,
        blockers: 1,
    };
    let text = blocked_diagnosis(Some("work"), "writer", &blocker);
    assert!(
        !text.contains("continues this run automatically"),
        "a blocker is not decided by approving it: {text}"
    );
    assert!(
        text.contains("re-enters this step"),
        "an answered blocker does re-enter the step it stopped: {text}"
    );
    assert!(
        text.contains(crate::ports::blockers::BLOCKER_VERDICT_CHOICES),
        "all four verdicts are reachable, so all four have to be named: {text}"
    );

    let mixed = ParkedCalls {
        tools: vec![
            "publish_artifact".to_string(),
            "escalate_to_human".to_string(),
        ],
        approval_ids: vec!["appr-1".to_string(), "appr-2".to_string()],
        unparkable: 0,
        blockers: 1,
    };
    let text = blocked_diagnosis(Some("work"), "writer", &mixed);
    assert!(
        text.contains("continue this run when approved")
            && text.contains("re-enter the step they stopped"),
        "a mixed node has to describe both, since neither sentence is true of all of it: \
         {text}"
    );
    assert!(
        text.contains(crate::ports::blockers::BLOCKER_VERDICT_CHOICES),
        "a mixed node's blocker cards offer the same four verdicts, worded from the same \
         fragment as the blocker-only branch above: {text}"
    );

    // Nothing was parked at all — every call failed to park — so there is
    // no card to promise anything about.
    let none_parked = ParkedCalls {
        tools: vec!["publish_artifact".to_string()],
        approval_ids: Vec::new(),
        unparkable: 1,
        blockers: 0,
    };
    let text = blocked_diagnosis(Some("work"), "writer", &none_parked);
    assert!(!text.contains("Approving the card"), "{text}");
    assert!(!text.contains("re-enters this step"), "{text}");
}

#[tokio::test]
async fn park_gated_calls_arms_the_stash_before_any_block_settle_pass_runs() {
    use crate::harness::policy::{ApprovalRequest, ApprovalScope};
    use crate::ports::types::{Effect, EffectGroup};

    let dir = tempfile::Builder::new()
        .prefix("oc-1825-p1-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let parking = deps
        .delivery
        .clone()
        .expect("gated_tool_turn_tests::deps wires delivery")
        .parking
        .clone()
        .expect("gated_tool_turn_tests::deps wires parking");
    let queue = deps.approval_requests.clone();
    let trigger_input = json!({ "request": "quarterly numbers" });
    let board_claim = Arc::new(deps.delegations.claim_board("run-1825-p1"));
    let publish_refusal_claim =
        Arc::new(deps.pending_publishes.claim_refusals_for_run("run-1825-p1"));
    let runner = HarnessAgentRunner::new(
        single_turn(&deps),
        deps,
        crate::workflows::gated_tool_turn_tests::record(),
        CompanyId::new("acme"),
        "wf-1825-p1".to_string(),
        "run-1825-p1".to_string(),
        None,
        trigger_input.clone(),
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let node_turn = crate::runtime::workflow_resume::workflow_node_turn_key(&runner.run_id, "work");

    // Pushed inside the run's own scope, exactly as its turn would.
    let claim = queue.claim(ApprovalScope::Run("run-1825-p1".to_string()));
    claim
        .scoped(async {
            queue.push(ApprovalRequest {
                tool: "shell".to_string(),
                reason: "gated".to_string(),
                effect: Effect {
                    kind: "shell".to_string(),
                    group: EffectGroup::Other,
                    amount_usd: None,
                    established_thread: false,
                    first_time_counterparty: false,
                    payload: json!({ "cmd": "rm -rf /" }),
                    agent: Some("ceo".to_string()),
                    run_id: None,
                },
            });
        })
        .await;

    // The real call a turn's tool loop makes. No block-settle pass runs
    // anywhere in this test.
    claim
        .scoped(runner.park_gated_calls(Some("work"), "work", &node_turn))
        .await;

    let stashed = parking.blocked_nodes.peek(&node_turn).expect(
        "the stash must be armed by park_gated_calls itself, before any block-settle \
         pass runs — an operator approving this node's just-parked card must always find \
         something to release",
    );
    assert_eq!(stashed.workflow_id, "wf-1825-p1");
    assert_eq!(stashed.input, trigger_input);
}

/// Issue #1825 (P1, second follow-up — found by chatgpt-codex-connector):
/// `park_gated_calls` must durably stash a blocked node's continuation
/// facts itself, before it parks a single call, not leave the durable
/// mirror to `stash_blocked_agent_nodes`'s block-settle pass alone.
///
/// # The race this closes
///
/// The test above proves the *in-memory* arm can no longer be outrun by
/// an operator acting on a just-published card. But `park_and_journal`
/// (inside the loop this test also drives) is what makes that card
/// **host-durable** and clickable across a restart — and until this fix,
/// nothing durable backed the in-memory arm until
/// `stash_blocked_agent_nodes` ran, which is strictly later: only once
/// the agent has returned and the engine has settled. A process that
/// died in that window left a restart with a recoverable card
/// (`ApprovalParked` is `Durability::Host` for a workflow-scoped effect)
/// and no matching `BlockedNodeStashed` record for `BlockedNodeQueue`'s
/// own `rearm` to rebuild a stash from — approving the recovered card
/// then consumed it against nothing, the identical shape the in-memory
/// race above closes, one durability tier up.
///
/// # Why this drives `park_gated_calls` directly, and reads the journal
///
/// Exactly like the test above: no `stash_blocked_agent_nodes` block-
/// settle pass runs anywhere here, so a durable stash observed right
/// after `park_gated_calls` returns can only have come from the park-time
/// write this fix adds. Pre-fix this assertion fails: `blocked_stashes()`
/// is empty, because nothing durable is written until settle. Post-fix it
/// holds this run's own trigger input, proving the durable record cannot
/// outrun the card that redeems it either.
#[tokio::test]
async fn park_gated_calls_durably_stashes_before_any_block_settle_pass_runs() {
    use crate::harness::policy::{ApprovalRequest, ApprovalScope};
    use crate::ports::types::{Effect, EffectGroup};

    let dir = tempfile::Builder::new()
        .prefix("oc-1825-p1b-")
        .tempdir()
        .expect("tempdir");
    let (deps, journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let queue = deps.approval_requests.clone();
    let trigger_input = json!({ "request": "quarterly numbers" });
    let board_claim = Arc::new(deps.delegations.claim_board("run-1825-p1b"));
    let publish_refusal_claim = Arc::new(
        deps.pending_publishes
            .claim_refusals_for_run("run-1825-p1b"),
    );
    let runner = HarnessAgentRunner::new(
        single_turn(&deps),
        deps,
        crate::workflows::gated_tool_turn_tests::record(),
        CompanyId::new("acme"),
        "wf-1825-p1b".to_string(),
        "run-1825-p1b".to_string(),
        None,
        trigger_input.clone(),
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let node_turn = crate::runtime::workflow_resume::workflow_node_turn_key(&runner.run_id, "work");

    let claim = queue.claim(ApprovalScope::Run("run-1825-p1b".to_string()));
    claim
        .scoped(async {
            queue.push(ApprovalRequest {
                tool: "shell".to_string(),
                reason: "gated".to_string(),
                effect: Effect {
                    kind: "shell".to_string(),
                    group: EffectGroup::Other,
                    amount_usd: None,
                    established_thread: false,
                    first_time_counterparty: false,
                    payload: json!({ "cmd": "rm -rf /" }),
                    agent: Some("ceo".to_string()),
                    run_id: None,
                },
            });
        })
        .await;

    // The real call a turn's tool loop makes. No block-settle pass runs
    // anywhere in this test.
    claim
        .scoped(runner.park_gated_calls(Some("work"), "work", &node_turn))
        .await;

    let stashed = journal
        .blocked_stashes()
        .into_iter()
        .find(|(turn, ..)| turn == &node_turn)
        .expect(
            "the durable stash must be written by park_gated_calls itself, before any \
             block-settle pass runs — a restart landing after this node's card goes \
             durable must always find a matching stash to rebuild from",
        );
    assert_eq!(stashed.1, "wf-1825-p1b");
    assert_eq!(stashed.2, trigger_input);
}

/// A node whose turn parks neither a gated call nor a blocker must never
/// touch the blocked-node stash at all — `park_gated_calls` runs on every
/// ordinary node, and most never block. The release-on-total-failure
/// cleanup this queues must only fire for a turn this very call armed.
#[tokio::test]
async fn park_gated_calls_leaves_an_unstashed_turn_untouched() {
    let dir = tempfile::Builder::new()
        .prefix("oc-2005-release-guard-")
        .tempdir()
        .expect("tempdir");
    let (deps, _journal) = crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let trigger_input = json!({ "topic": "quarterly numbers" });
    let board_claim = Arc::new(deps.delegations.claim_board("run-2005-guard"));
    let publish_refusal_claim = Arc::new(
        deps.pending_publishes
            .claim_refusals_for_run("run-2005-guard"),
    );
    let runner = HarnessAgentRunner::new(
        single_turn(&deps),
        deps,
        crate::workflows::gated_tool_turn_tests::record(),
        CompanyId::new("acme"),
        "reporting".to_string(),
        "run-2005-guard".to_string(),
        None,
        trigger_input,
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let node_turn = crate::runtime::workflow_resume::workflow_node_turn_key(&runner.run_id, "work");

    // Nothing was ever queued for this node's turn — no blocker, no gated
    // call — so this call never armed a stash for it.
    let summary = runner
        .park_gated_calls(Some("work"), "work", &node_turn)
        .await;
    assert_eq!(summary.approval_ids.len(), 0);

    // A turn that never touches the journal never creates the file.
    let raw = tokio::fs::read_to_string(dir.path().join("journal.jsonl"))
        .await
        .unwrap_or_default();
    assert!(
        !raw.contains("BlockedNodeReleased"),
        "a turn this call never stashed must not durably record a release for it: {raw}"
    );
}

/// Issue #1825 (P2, third follow-up — found by chatgpt-codex-connector): a
/// node whose every gated call fails to park must not leave a stash behind
/// with nothing that can ever redeem it.
///
/// The arm and the durable stash run unconditionally, before the request
/// loop attempts a single park — required, since that ordering is what
/// closes the P1 race. But when every request in the batch then fails
/// (journal outage), `summary.approval_ids` comes back empty: no approval
/// id was ever minted for this turn, so nothing will ever call
/// `continue_turn` for it, and the stash this call armed and durably wrote
/// would otherwise sit forever — one workflow id and trigger payload
/// retained in memory for the process's life, and durably on every replay.
///
/// Forces every park to fail by pointing `parking.journal` at a path whose
/// parent directory does not exist, so `record_parked` inside
/// `park_and_journal` fails for each request — the gate's own `park` stays
/// in-memory and always succeeds, so this isolates the journal failure
/// without needing a custom `ApprovalGate` double.
#[tokio::test]
async fn a_node_with_no_successfully_parked_call_leaves_no_stash_behind() {
    use crate::harness::policy::{ApprovalRequest, ApprovalScope};
    use crate::ports::types::{Effect, EffectGroup};

    let dir = tempfile::Builder::new()
        .prefix("oc-1825-p2c-")
        .tempdir()
        .expect("tempdir");
    let (mut deps, _journal) =
        crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    // `FsJournalStore::append_journal` calls `create_dir_all` on the
    // parent, so a merely-missing directory would not fail the write — it
    // would just get created. A regular file standing where the journal's
    // parent directory needs to be does: `create_dir_all` cannot turn a
    // file into a directory, so every append genuinely fails.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("write blocker file");
    let broken_journal = Arc::new(crate::runtime::journal::RuntimeJournal::new(
        blocker.join("journal.jsonl"),
    ));
    let delivery = deps
        .delivery
        .as_mut()
        .expect("gated_tool_turn_tests::deps wires delivery");
    let parking = delivery
        .parking
        .as_mut()
        .expect("gated_tool_turn_tests::deps wires parking");
    parking.journal = broken_journal.clone();
    let queue = deps.approval_requests.clone();
    let trigger_input = json!({ "request": "quarterly numbers" });
    let board_claim = Arc::new(deps.delegations.claim_board("run-1825-p2c"));
    let publish_refusal_claim = Arc::new(
        deps.pending_publishes
            .claim_refusals_for_run("run-1825-p2c"),
    );
    let runner = HarnessAgentRunner::new(
        single_turn(&deps),
        deps,
        crate::workflows::gated_tool_turn_tests::record(),
        CompanyId::new("acme"),
        "wf-1825-p2c".to_string(),
        "run-1825-p2c".to_string(),
        None,
        trigger_input.clone(),
        crate::ports::types::StartedBy::Operator,
        RunNotices::default(),
        RunBoard::default(),
        RunBlocks::default(),
        RunCappedNodes::default(),
        RunApprovals::default(),
        RunArtifacts::default(),
        board_claim,
        publish_refusal_claim,
    );

    let node_turn = crate::runtime::workflow_resume::workflow_node_turn_key(&runner.run_id, "work");

    let claim = queue.claim(ApprovalScope::Run("run-1825-p2c".to_string()));
    claim
        .scoped(async {
            queue.push(ApprovalRequest {
                tool: "shell".to_string(),
                reason: "gated".to_string(),
                effect: Effect {
                    kind: "shell".to_string(),
                    group: EffectGroup::Other,
                    amount_usd: None,
                    established_thread: false,
                    first_time_counterparty: false,
                    payload: json!({ "cmd": "rm -rf /" }),
                    agent: Some("ceo".to_string()),
                    run_id: None,
                },
            });
        })
        .await;

    let summary = claim
        .scoped(runner.park_gated_calls(Some("work"), "work", &node_turn))
        .await;

    assert!(
        summary.approval_ids.is_empty(),
        "precondition: the broken journal must fail every park attempt"
    );
    assert_eq!(summary.unparkable, 1);
    assert!(
        !runner
            .deps
            .delivery
            .as_ref()
            .expect("delivery wired")
            .parking
            .as_ref()
            .expect("parking wired")
            .blocked_nodes
            .is_armed(&node_turn),
        "a node with zero successfully parked calls must not leave an unredeemable \
         in-memory stash behind"
    );
    assert!(
        broken_journal
            .blocked_stashes()
            .into_iter()
            .all(|(turn, ..)| turn != node_turn),
        "a node with zero successfully parked calls must not leave a durable stash \
         behind either"
    );
}
