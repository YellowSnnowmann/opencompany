use super::*;

/// Issue #374: a resolution that minted only a STANDING grant must still
/// re-dispatch the agent.
///
/// This is the feature's happy path, and it was the one real gap in the
/// plan. `redispatch_granted_call` peeked only the single-use set and
/// no-ops silently on a miss — correct for every legitimate miss (a deny, a
/// native effect, a legacy park) and catastrophic here: the operator picks
/// the broader scope, the permission is armed, and the call they were
/// looking at never runs. It would have looked exactly like #243's original
/// bug, one scope over.
#[tokio::test]
async fn a_standing_grant_also_redispatches_its_agent() {
    let dir = tempfile::tempdir().unwrap();
    let log: Arc<dyn crate::ports::EventLog> =
        Arc::new(crate::store::FsEventLog::new(dir.path().to_path_buf()));
    let requests = crate::harness::policy::ApprovalRequestQueue::default();
    requests
        .grants()
        .grant_standing(crate::runtime::grants::StandingGrant {
            id: crate::runtime::grants::GrantId::new("g1"),
            agent: "ceo".into(),
            workflow: None,
            tool: "workspace_write".into(),
            verdict: Verdict::Approve,
            granted_by: crate::ports::types::Actor {
                kind: crate::ports::types::ActorKind::User,
                id: "user-1".into(),
            },
            approval_id: ApprovalId::new("appr-1"),
            at_millis: now_millis(),
            expires_at_millis: now_millis() + 60 * 60 * 1000,
            origin_thread: None,
            origin_parent: None,
            origin_task: None,
            scope: None,
        });
    let brain = brain_with_queue_and_events(dir.path(), requests, log.clone());

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
    assert_ne!(
        bubble.text, "Acknowledged.",
        "a standing grant must re-dispatch, not fall through to the no-op"
    );
    assert!(bubble.text.contains("workspace_write"), "{}", bubble.text);
    // No exact-arguments pin: a standing grant admits any arguments, which
    // is what the operator consented to by choosing this scope. Telling the
    // model to reproduce a specific argument object would make the broad
    // scope behave like the narrow one.
    assert!(
        !bubble.text.contains("Do not modify them"),
        "a standing grant must not pin arguments: {}",
        bubble.text
    );

    // Journaling the reply belongs to the runtime now (issue #469), so the
    // brain must not write a second copy. See
    // `server::operator::test::a_continuation_answers_in_the_thread_the_sign_off_was_raised_in`
    // for the round trip.
    assert!(no_replies_journaled(&log).await);
}

/// A DENIED approval runs no turn. "No" must never re-dispatch anything.
#[tokio::test]
async fn a_denied_approval_redispatches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let log: Arc<dyn crate::ports::EventLog> =
        Arc::new(crate::store::FsEventLog::new(dir.path().to_path_buf()));
    let requests = crate::harness::policy::ApprovalRequestQueue::default();
    // A grant for a DIFFERENT approval is live, to prove the arm keys on the
    // resolved id rather than reaching for whatever is lying around.
    requests
        .grants()
        .grant(crate::runtime::grants::GrantedCall {
            approval_id: ApprovalId::new("appr-other"),
            agent: "ceo".into(),
            tool: "composio_execute".into(),
            args: serde_json::json!({}),
            at_millis: now_millis(),
            origin_thread: None,
            origin_parent: None,
            origin_task: None,
        });
    requests
        .grants()
        .grant_standing(crate::runtime::grants::StandingGrant {
            id: crate::runtime::grants::GrantId::new("deny-1"),
            agent: "ceo".into(),
            workflow: None,
            tool: "workspace_write".into(),
            verdict: Verdict::Deny,
            granted_by: crate::ports::types::Actor {
                kind: crate::ports::types::ActorKind::User,
                id: "user-1".into(),
            },
            approval_id: ApprovalId::new("appr-1"),
            at_millis: now_millis(),
            expires_at_millis: now_millis() + 60_000,
            origin_thread: None,
            origin_parent: None,
            origin_task: None,
            scope: None,
        });
    let brain = brain_with_queue_and_events(dir.path(), requests, log.clone());

    let result = brain
        .run_cycle(
            cycle_over(vec![approval_resolved("appr-1", Verdict::Deny)]),
            &NoopHost,
        )
        .await
        .unwrap();

    assert_eq!(result.channel_responses.len(), 1);
    assert_eq!(
        result.channel_responses[0].text, "Acknowledged.",
        "a deny falls through to the fallback, exactly as before #243"
    );
    assert!(
        log.read_from(&CompanyId::new("acme"), crate::ports::EventSeq::new(0), 100)
            .await
            .unwrap()
            .is_empty(),
        "nothing is journaled for a deny"
    );
}

/// The re-run of a card carrying review feedback reads that feedback: the
/// operator's `[reviewer]` note block is part of the turn instruction the
/// fresh dispatch is built from, which is why `apply_review_feedback`
/// appends to the note *before* re-dispatch.
#[test]
fn task_instruction_carries_a_reviewer_note_block() {
    let mut card = card_in_review("card-1");
    card.note = Some("[reviewer] tighten the intro".to_string());
    let instruction = task_instruction(&card);
    assert!(
        instruction.contains("[reviewer] tighten the intro"),
        "the fresh run must see the reviewer's feedback: {instruction}"
    );
    assert!(instruction.starts_with(&format!("Task: {}", card.title)));
}

#[test]
fn public_research_task_instruction_keeps_prior_agent_results_out_of_the_current_assignment() {
    let mut card = card_in_review("card-1");
    card.note = Some(
        "[operator] Research competitors and cite sources.\n\n\
         [researcher] I stopped because my calls looped.\n\n\
         The old run produced no findings.\n\n\
         [reviewer] Include pricing where it is verifiable."
            .to_string(),
    );

    let instruction = task_instruction(&card);
    let history = instruction
        .split("## Prior attempt history")
        .nth(1)
        .and_then(|rest| rest.split("## Current assignment").next())
        .expect("history section");
    let assignment = instruction
        .split("## Current assignment")
        .nth(1)
        .expect("assignment section");

    assert!(history.contains("omitted from this turn"));
    assert!(!history.contains("I stopped because my calls looped"));
    assert!(!history.contains("The old run produced no findings"));
    assert!(!assignment.contains("old run produced no findings"));
    assert!(assignment.contains("Research competitors and cite sources"));
    assert!(assignment.contains("Include pricing where it is verifiable"));
    assert!(assignment.contains("Perform the current assignment now"));
    assert!(assignment.contains("do not read the tasks ledger to rediscover it"));
    assert!(assignment.contains("`web_search`"));
    assert!(assignment.contains("begin with `web_search` now"));
    assert!(assignment.contains("Do not inspect the company workspace"));
    assert!(assignment.contains("stop after that one call"));
    assert!(assignment.contains("Do not retry it with another query"));
}

#[test]
fn ordinary_task_instructions_do_not_claim_they_are_public_research() {
    let mut card = card_in_review("card-1");
    card.note = Some("[operator] Fix the checkout button.".to_string());
    let instruction = task_instruction(&card);
    assert!(!instruction.contains("begin with `web_search` now"));
}

/// **The reachability assertion.** A test that the drain works when called
/// is not coverage that the drain is reached — and on this path it was not.
///
/// `redispatch_granted_call` runs a full toolbelt turn and claimed publishes
/// only, so a `review_task` the re-issued call made was staged, answered
/// with "the card has moved to done", and destroyed by the next turn's
/// `clear()`. It **drains** rather than refusing, deliberately: `review_task`
/// is a gateable Write effect, so refusing here would make an operator's own
/// approval unspendable — approve, refuse, re-park.
#[tokio::test]
async fn a_granted_redispatch_drains_the_board_work_its_turn_queued() {
    let dir = tempfile::tempdir().unwrap();
    let requests = crate::harness::policy::ApprovalRequestQueue::default();
    requests.grants().grant(granted("appr-1", "review_task"));
    let base_url = spawn_model_script(vec![
        ScriptTurn::Call {
            tool: "review_task",
            args: serde_json::json!({ "task_id": "card-1", "decision": "approve" }),
        },
        ScriptTurn::Say("Approved."),
    ])
    .await;
    let brain = brain_over_script(dir.path(), requests, base_url);
    let tasks = brain.deps.tasks.clone().expect("task store");
    tasks
        .upsert(&CompanyId::new("acme"), &card_in_review("card-1"))
        .await
        .expect("seed the card");

    let result = brain
        .run_cycle(
            cycle_over(vec![approval_resolved("appr-1", Verdict::Approve)]),
            &NoopHost,
        )
        .await
        .expect("cycle runs");
    assert_eq!(result.channel_responses.len(), 1);

    let cards = tasks.list(&CompanyId::new("acme")).await.expect("list");
    assert_eq!(
        cards[0].column,
        crate::ports::tasks::COLUMN_DONE,
        "the approved card must actually move — staging it and returning was the defect"
    );
    assert_eq!(
        brain.deps.delegations.queued(),
        0,
        "and nothing may be left for a later turn's clear() to destroy"
    );
    assert!(
        !brain.deps.delegations.drain_committed(),
        "the claim releases with the re-dispatch turn"
    );
}

/// The #476 nuance: one continuation cycle can run **several** re-dispatch
/// turns, one per batched resolution. The claim is therefore per turn, not
/// per cycle — each re-dispatch owns its own drain window.
///
/// A per-cycle claim would pass a single-approval test and fail here in the
/// worst way: the second turn's staged verdict would ride on the first
/// turn's already-spent window, or the second acquire would clear work the
/// first had not drained yet.
#[tokio::test]
async fn batched_resolutions_each_get_their_own_drain_window() {
    let dir = tempfile::tempdir().unwrap();
    let requests = crate::harness::policy::ApprovalRequestQueue::default();
    requests.grants().grant(granted("appr-1", "review_task"));
    requests.grants().grant(granted("appr-2", "review_task"));
    let base_url = spawn_model_script(vec![
        ScriptTurn::Call {
            tool: "review_task",
            args: serde_json::json!({ "task_id": "card-1", "decision": "approve" }),
        },
        ScriptTurn::Say("Approved card-1."),
        ScriptTurn::Call {
            tool: "review_task",
            args: serde_json::json!({ "task_id": "card-2", "decision": "revise" }),
        },
        ScriptTurn::Say("Sent card-2 back."),
    ])
    .await;
    let brain = brain_over_script(dir.path(), requests, base_url);
    let tasks = brain.deps.tasks.clone().expect("task store");
    let company = CompanyId::new("acme");
    for id in ["card-1", "card-2"] {
        tasks
            .upsert(&company, &card_in_review(id))
            .await
            .expect("seed the card");
    }

    let result = brain
        .run_cycle(
            cycle_over(vec![
                approval_resolved("appr-1", Verdict::Approve),
                approval_resolved("appr-2", Verdict::Approve),
            ]),
            &NoopHost,
        )
        .await
        .expect("cycle runs");
    assert_eq!(
        result.channel_responses.len(),
        2,
        "both resolutions re-dispatch"
    );

    let cards = tasks.list(&company).await.expect("list");
    let column = |id: &str| {
        cards
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.column.clone())
            .unwrap_or_else(|| panic!("{id} is on the board"))
    };
    assert_eq!(
        column("card-1"),
        crate::ports::tasks::COLUMN_DONE,
        "the first re-dispatch's verdict must survive the second re-dispatch's claim"
    );
    assert_eq!(
        column("card-2"),
        crate::ports::tasks::COLUMN_TODO,
        "and the second's own verdict lands too"
    );
    assert_eq!(brain.deps.delegations.queued(), 0);
    assert!(!brain.deps.delegations.drain_committed());
}

/// An approved resolution with NO grant behind it is a silent no-op.
///
/// This is the common case, not an edge: a native effect the runtime already
/// executed, a legacy parked effect from before `Effect::agent` existed (it
/// replays as `None` and mints nothing), a grant already consumed, and a
/// grant already swept all land here. Every one of them must keep the exact
/// pre-#243 behaviour rather than manufacturing a turn.
#[tokio::test]
async fn an_approval_with_no_grant_is_a_silent_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let log: Arc<dyn crate::ports::EventLog> =
        Arc::new(crate::store::FsEventLog::new(dir.path().to_path_buf()));
    let requests = crate::harness::policy::ApprovalRequestQueue::default();
    let brain = brain_with_queue_and_events(dir.path(), requests, log.clone());

    let result = brain
        .run_cycle(
            cycle_over(vec![approval_resolved("appr-native", Verdict::Approve)]),
            &NoopHost,
        )
        .await
        .unwrap();

    assert_eq!(result.channel_responses.len(), 1);
    assert_eq!(result.channel_responses[0].text, "Acknowledged.");
    assert!(
        log.read_from(&CompanyId::new("acme"), crate::ports::EventSeq::new(0), 100)
            .await
            .unwrap()
            .is_empty()
    );
}

/// **A private aside never crosses a desk boundary in a room's answer.**
///
/// `!aside @peer` is journaled as an ordinary `AgentReply` carrying the
/// pair in `audience`, so it lands inside the span an unconverged room's
/// answer is drawn from and can even be its last row. Carried home it would
/// publish a private exchange to a desk that was never in it — and unlike
/// an aside on one's own desk, where every teammate at least sees that it
/// happened, nobody on the asking desk could see anything to audit.
#[tokio::test]
async fn a_rooms_answer_leaves_its_asides_behind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let brain = hive_test_brain(dir.path());
    let host = ParkingHost::default();
    // The runner's own turn seam is never reached here — `turns_of` only
    // reads the journal — so any outcome does.
    let runner = hive_desk_runner(
        &brain,
        &host,
        crate::harness::built_in::TurnOutcome {
            reply: String::new(),
            steps: Vec::new(),
            hit_iteration_cap: false,
            abnormal_stop: None,
            halted_for_spend: None,
            budget_paused: None,
        },
    );
    // The crate's in-memory journal: `hive_test_brain` wires none, and this
    // reads a journal rather than running a turn.
    let events: Arc<dyn crate::ports::events::EventLog> =
        Arc::new(crate::hivemind::test::MemoryLog::default());
    let company = crate::hivemind::test::MemoryLog::company();

    // The question the room was convened on: every turn of a referred
    // episode is parented to it.
    let root = events
        .append(
            &company,
            CompanyEvent::OperatorMessage {
                text: "cancellations has put a question to this desk".to_string(),
                by: Some(crate::ports::types::Actor {
                    kind: crate::ports::types::ActorKind::Agent,
                    id: "cancellations".to_string(),
                }),
                chat: Some("returns".to_string()),
                parent: None,
                deliverable: None,
                mentions: Vec::new(),
                attachments: Vec::new(),
            },
        )
        .await
        .expect("journal");
    let row = |agent: &str, text: &str, audience: Vec<String>, parent: Option<EventSeq>| {
        CompanyEvent::AgentReply {
            chat_id: "returns".to_string(),
            agent_id: agent.to_string(),
            text: text.to_string(),
            steps: Vec::new(),
            task_id: None,
            outputs: Vec::new(),
            parent,
            mentions: Vec::new(),
            mention_depth: 0,
            audience,
        }
    };
    let first = events
        .append(
            &company,
            row(
                "exchanges",
                "no refund tool on this seat",
                Vec::new(),
                Some(root),
            ),
        )
        .await
        .expect("journal");
    // A private aside, inside the span and on the same thread.
    events
        .append(
            &company,
            row(
                "refunds",
                "aside @exchanges — do not tell them we are short-staffed",
                vec!["exchanges".to_string()],
                Some(root),
            ),
        )
        .await
        .expect("journal");
    // **Concurrent traffic on the same desk**, in the same span but on no
    // thread of this crossing — the desk's own other work.
    events
        .append(
            &company,
            row(
                "exchanges",
                "unrelated: the Thursday roster is posted",
                Vec::new(),
                None,
            ),
        )
        .await
        .expect("journal");
    let last = events
        .append(
            &company,
            row("refunds", "i hold the refund tool", Vec::new(), Some(root)),
        )
        .await
        .expect("journal");

    // Spelled out: `EpisodeOutcome` has no `Default`, on purpose — an
    // episode that never ran has no ending to report.
    let outcome = crate::hivemind::EpisodeOutcome {
        ending: crate::hivemind::EpisodeEnding::Idle,
        turns: 2,
        first_seq: Some(first),
        last_seq: Some(last),
        report_seq: None,
        violations: Vec::new(),
        failed_turns: 0,
        referrals: crate::hivemind::ReferralLedger::default(),
    };
    let carried = runner
        .turns_of(&events, &company, &outcome, "returns", root)
        .await
        .expect("the room said something");

    assert!(carried.contains("no refund tool on this seat"));
    assert!(carried.contains("i hold the refund tool"));
    assert!(
        !carried.contains("Thursday roster"),
        "the desk's own other work is not this room's answer: {carried}"
    );
    assert!(
        !carried.contains("short-staffed"),
        "an aside is not a turn and does not answer a crossing: {carried}"
    );
}

/// **A budget-paused hive turn is a hard error, not a folded reply.**
///
/// Before this fix `HiveDeskRunner::speak` returned `Ok(outcome.reply)`
/// unconditionally, so `EpisodeDriver` journaled the host's "add credits"
/// placeholder as a genuine `AgentReply` under the member's own identity —
/// indistinguishable, from the transcript alone, from the member actually
/// answering — and the episode never counted the turn as failed.
#[tokio::test]
async fn hive_speak_turns_a_budget_pause_into_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let brain = hive_test_brain(dir.path());
    let host = NoopHost;
    let runner = hive_desk_runner(&brain, &host, budget_paused_outcome("theorist"));
    let err = runner
        .speak("theorist", "Settle the derivation.")
        .await
        .expect_err("a budget pause must surface as an error, not Ok(reply)");
    assert!(
        err.to_string().contains("theorist"),
        "the error must name the agent so an operator reading it knows who paused: {err}"
    );
}

/// The same terminal state, for a spend halt rather than a budget pause.
#[tokio::test]
async fn hive_speak_turns_a_spend_halt_into_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let brain = hive_test_brain(dir.path());
    let host = NoopHost;
    let runner = hive_desk_runner(&brain, &host, spend_halted_outcome("theorist"));
    let err = runner
        .speak("theorist", "Settle the derivation.")
        .await
        .expect_err("a spend halt must surface as an error, not Ok(reply)");
    assert!(
        err.to_string().contains("theorist"),
        "the error must name the agent: {err}"
    );
}

/// **The same bug, on the far-desk referral runner.**
///
/// `HiveReferralRunner::refer` shares the exact same shape:
/// `EpisodeReferrals` only ever treated an `Err` result as "the far desk
/// did not answer", so a budget-paused or spend-halted far turn slipped
/// through as though it were a real referral answer and was carried back
/// to the asking desk as content.
#[tokio::test]
async fn hive_refer_turns_a_budget_pause_into_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let brain = hive_test_brain(dir.path());
    let host = NoopHost;
    let runner = hive_desk_runner(&brain, &host, budget_paused_outcome("sre"));
    let err = runner
        .refer("platform", "sre", "What is the failover budget?")
        .await
        .expect_err("a budget pause on the far desk must surface as an error too");
    assert!(err.to_string().contains("sre"), "{err}");
}

#[tokio::test]
async fn hive_refer_turns_a_spend_halt_into_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let brain = hive_test_brain(dir.path());
    let host = NoopHost;
    let runner = hive_desk_runner(&brain, &host, spend_halted_outcome("sre"));
    let err = runner
        .refer("platform", "sre", "What is the failover budget?")
        .await
        .expect_err("a spend halt on the far desk must surface as an error too");
    assert!(err.to_string().contains("sre"), "{err}");
}

/// **A pre-dispatch refusal (`abnormal_stop`) is a hard error too.**
///
/// A fail-closed spend gate that cannot read the meter behind a declared
/// cap refuses dispatch before any model call runs and reports the
/// refusal only through `abnormal_stop` — `budget_paused` and
/// `halted_for_spend` both stay `None`, since no turn ran to pause or
/// halt. Without this, `speak` folded the refusal notice as `Ok(reply)`
/// the same way it once did for a budget pause.
#[tokio::test]
async fn hive_speak_turns_an_abnormal_stop_into_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let brain = hive_test_brain(dir.path());
    let host = NoopHost;
    let runner = hive_desk_runner(
        &brain,
        &host,
        abnormal_stop_outcome("dispatch refused: spend unreadable"),
    );
    let err = runner
        .speak("theorist", "Settle the derivation.")
        .await
        .expect_err("a pre-dispatch refusal must surface as an error, not Ok(reply)");
    assert!(
        err.to_string().contains("theorist"),
        "the error must name the agent: {err}"
    );
}

/// The same terminal state, on the far-desk referral runner.
#[tokio::test]
async fn hive_refer_turns_an_abnormal_stop_into_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let brain = hive_test_brain(dir.path());
    let host = NoopHost;
    let runner = hive_desk_runner(
        &brain,
        &host,
        abnormal_stop_outcome("dispatch refused: spend unreadable"),
    );
    let err = runner
        .refer("platform", "sre", "What is the failover budget?")
        .await
        .expect_err("a pre-dispatch refusal on the far desk must surface as an error too");
    assert!(err.to_string().contains("sre"), "{err}");
}

/// A turn that finishes cleanly is unaffected: `speak`/`refer` still
/// return `Ok(reply)` when neither terminal flag is set.
#[tokio::test]
async fn hive_speak_and_refer_pass_through_an_ordinary_reply() {
    let ok = |text: &str| crate::harness::built_in::TurnOutcome {
        reply: text.to_string(),
        steps: Vec::new(),
        hit_iteration_cap: false,
        abnormal_stop: None,
        halted_for_spend: None,
        budget_paused: None,
    };
    let dir = tempfile::tempdir().unwrap();
    let brain = hive_test_brain(dir.path());
    let host = NoopHost;
    let runner = hive_desk_runner(&brain, &host, ok("The rollout is ready to stage."));
    assert_eq!(
        runner
            .speak("theorist", "Settle the derivation.")
            .await
            .expect("an ordinary reply is not an error"),
        "The rollout is ready to stage."
    );
    let runner = hive_desk_runner(&brain, &host, ok("The failover budget is $2,000/month."));
    assert_eq!(
        runner
            .refer("platform", "sre", "What is the failover budget?")
            .await
            .expect("an ordinary referral answer is not an error"),
        "The failover budget is $2,000/month."
    );
}

/// **Regression: an approval request a hive turn queues is parked before
/// the episode ends, not only after it.**
///
/// Before this fix, `HiveDeskRunner::speak` never touched
/// `self.deps.approval_requests` — only the *cycle's* single
/// `park_approval_requests(host)` call, after `driver.run(trigger)` had
/// already returned, ever drained it. A member's `request_approval` call
/// mid-episode therefore sat in the internal queue, invisible to
/// `scripts/hive-euler.py`'s concurrent approval pump, for every
/// remaining turn the room took.
///
/// This drives exactly one `speak` call — the queue is populated by
/// `FixedOutcomeTurn` the same way a real `request_approval` refusal
/// would populate it during a turn — and asserts the request already
/// reached `host.park_effect` immediately after that single turn, with no
/// second turn and no `EpisodeDriver` in the picture. Before the fix this
/// assertion fails: nothing is parked until a cycle-level drain that
/// never runs here.
#[tokio::test]
async fn hive_speak_parks_a_queued_approval_before_the_episode_continues() {
    let dir = tempfile::tempdir().unwrap();
    let requests = crate::harness::policy::ApprovalRequestQueue::default();
    let brain = brain_with_approval_queue(dir.path(), requests.clone());
    let host = ParkingHost::default();
    let runner = HiveDeskRunner {
        run_turn: Arc::new(FixedOutcomeTurn {
            outcome: crate::harness::built_in::TurnOutcome {
                reply: "blocked, requires approval".to_string(),
                steps: Vec::new(),
                hit_iteration_cap: false,
                abnormal_stop: None,
                halted_for_spend: None,
                budget_paused: None,
            },
            // Mirrors what a supervised `ApprovalPolicy` records when the
            // agent reaches for a gated tool mid-turn: the request lands
            // on the shared queue, and the turn still completes normally.
            approval_requests: Some(requests.clone()),
        }),
        company: CompanyId::new("acme"),
        chat_id: Some("lab".to_string()),
        thread_root: None,
        trigger_seq: None,
        brain: &brain,
        host: &host,
    };

    let reply =
        crate::hivemind::HiveTurnRunner::speak(&runner, "programmer", "Run the computation.")
            .await
            .expect("a turn that only queued an approval request still replies");
    assert_eq!(reply, "blocked, requires approval");

    // The core regression: parked after this ONE turn, not after a whole
    // episode of turns. `FixedOutcomeTurn` queues
    // `MAX_APPROVAL_REQUESTS_PER_TURN + 1` requests per call (mirroring the
    // existing overflow fixtures), so a single `speak` already fills the
    // per-turn cap.
    let parked = host.parked();
    assert_eq!(
        parked.len(),
        crate::harness::policy::MAX_APPROVAL_REQUESTS_PER_TURN,
        "the gated calls from this single turn must already be on the operator's queue"
    );
    assert_eq!(parked[0].kind, "test_tool_0");
    assert_eq!(
        requests.queued(),
        0,
        "the shared queue is drained by the per-turn park, not left for a later cycle-level drain"
    );
}
