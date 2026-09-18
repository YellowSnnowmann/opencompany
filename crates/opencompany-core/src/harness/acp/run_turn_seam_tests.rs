use super::run_turn_test_fixtures::*;
use super::*;

/// The claim the whole slice rests on: this is usable anywhere the
/// OpenHuman implementation is.
///
/// Driven through `&dyn RunTurn` rather than through the concrete type,
/// because that is how the company cycle holds it (`DelegationRunner` takes
/// `&'a dyn RunTurn`). A type that satisfied the trait but was not
/// object-safe would compile here and fail at the one site that matters.
#[tokio::test]
async fn it_is_usable_through_the_run_turn_seam() {
    let agent = Arc::new(Scripted::answering(vec![
        AcpUpdate::ThoughtChunk,
        AcpUpdate::ToolCall {
            id: "t1".into(),
            title: "Read".into(),
        },
        AcpUpdate::ToolCallUpdate {
            id: "t1".into(),
            status: "completed".into(),
            result: Some("4 items".into()),
        },
        AcpUpdate::MessageChunk("all done".into()),
    ]));
    let run_turn: &dyn RunTurn = &AcpRunTurn::new(agent);

    let outcome = run_turn
        .run(&CompanyId::new("acme"), "ceo", "go", ChatTarget::default())
        .await
        .expect("a turn runs");

    assert_eq!(outcome.reply, "all done");
    assert_eq!(outcome.steps.len(), 2);
    assert_eq!(outcome.steps[1].status, TurnStepStatus::Ok);
    assert_eq!(outcome.steps[1].result.as_deref(), Some("7 characters"));
}

#[tokio::test]
async fn a_steered_turn_still_returns_an_outcome() {
    // Cancellation in ACP is cooperative: the agent still answers, with
    // `stopReason: "cancelled"`. Abandoning the future on a steer would
    // leave a harness mid-tool-call with nothing reading its output, so the
    // contract is that a steered turn still produces an outcome.
    let agent = Arc::new(Scripted::answering(vec![AcpUpdate::MessageChunk(
        "partial".into(),
    )]));
    let run_turn: &dyn RunTurn = &AcpRunTurn::new(agent);
    let control = crate::company::steer::SteerControl::new();
    control.request(crate::company::steer::SteerAction::Cancel);

    let outcome = run_turn
        .run_steered(
            &CompanyId::new("acme"),
            "ceo",
            "go",
            &control,
            ChatTarget::default(),
            None,
        )
        .await
        .expect("a steered turn still answers");
    assert_eq!(outcome.reply, "partial");
    // The pending action survives for the disposition site to read, which
    // is what decides where the card lands.
    assert!(
        control.pending().is_some(),
        "the steer must not be consumed here"
    );
}

#[tokio::test]
async fn a_failed_cancel_is_logged_and_the_turn_still_drains() {
    // `session/cancel` can fail (the subprocess is mid-shutdown, say), but
    // that must not turn a cancelled turn into a failure of its own: the
    // cancel is advisory, the error is logged, and the turn still answers.
    // The prompt holds until the cancel arrives so the steer check is
    // actually reached — a prompt that resolves first would exit the loop
    // and leave the cancel path unexercised.
    let mut agent = Scripted::answering(vec![AcpUpdate::MessageChunk("done".into())]);
    agent.cancel_fails = true;
    agent.hold_for_cancel = true;
    let cancels = agent.cancels.clone();
    let agent = Arc::new(agent);
    let run_turn: &dyn RunTurn = &AcpRunTurn::new(agent);
    let control = crate::company::steer::SteerControl::new();
    control.request(crate::company::steer::SteerAction::Cancel);

    let outcome = run_turn
        .run_steered(
            &CompanyId::new("acme"),
            "ceo",
            "go",
            &control,
            ChatTarget::default(),
            None,
        )
        .await
        .expect("a failed cancel still ends in a turn");
    assert_eq!(outcome.reply, "done");
    assert_eq!(
        cancels.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the failed cancel was still attempted exactly once"
    );
}

#[tokio::test]
async fn a_hung_cancel_rpc_does_not_block_the_turn() {
    // A cancellation RPC that never answers — a wedged host, a dead
    // subprocess — must not pin the steered turn forever. Both cancel calls
    // are bounded, so the turn still settles on the grace schedule.
    let mut agent = Scripted::answering(vec![AcpUpdate::MessageChunk("done".into())]);
    agent.cancel_hangs = true;
    agent.hold_for_cancel = true;
    let agent = Arc::new(agent);
    let run_turn = AcpRunTurn::new(agent);
    let control = crate::company::steer::SteerControl::new();
    control.request(crate::company::steer::SteerAction::Cancel);

    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        run_turn.steered_with_grace(
            &CompanyId::new("acme"),
            "ceo",
            "go",
            &control,
            None,
            None,
            CancelBounds {
                grace: Duration::from_millis(20),
                rpc: Duration::from_millis(50),
            },
        ),
    )
    .await
    .expect("the turn settles despite a hung cancel RPC")
    .expect("the release of the prompt lets the turn answer");

    assert_eq!(outcome.reply, "done");
}

#[tokio::test]
async fn a_cancelled_turn_that_ignores_the_cancel_is_abandoned() {
    // A harness inside a tool call that never returns is the one case the
    // cooperative wait must not honour: past the grace window the waiter
    // drops the turn with an error, and nudges `cancel` once more on the
    // way out — the only drain lever the port exposes.
    let agent = Arc::new(Scripted {
        turn: AcpTurn {
            updates: vec![],
            stop_reason: "end_turn".into(),
        },
        holds_ms: 0,
        hang: true,
        hold_for_cancel: false,
        cancel_hangs: false,
        cancel_fails: false,
        cancels: Default::default(),
        cancel_started: tokio::sync::Notify::new(),
    });
    let cancels = agent.cancels.clone();
    let run_turn = AcpRunTurn::new(agent);
    let control = crate::company::steer::SteerControl::new();
    control.request(crate::company::steer::SteerAction::Cancel);

    let err = run_turn
        .steered_with_grace(
            &CompanyId::new("acme"),
            "ceo",
            "go",
            &control,
            None,
            None,
            CancelBounds {
                grace: Duration::from_millis(20),
                rpc: Duration::from_millis(50),
            },
        )
        .await
        .expect_err("a hung turn is abandoned, not awaited");
    assert!(
        format!("{err}").contains("abandoning the turn"),
        "the error names the abandonment: {err}"
    );
    assert_eq!(
        cancels.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "one cancel on the steer, one best-effort nudge on the way out"
    );
}

#[tokio::test]
async fn a_turn_cancelled_before_it_starts_never_reaches_the_agent() {
    // The sharp edge serialising turns introduced (PR #1904 review):
    // `session/cancel` names a *session*, not a turn. A queued turn that
    // forwarded its cancel would stop whichever turn currently owns the
    // session — an unrelated turn, still working.
    //
    // Driven by holding the slot with a turn that never finishes, so the
    // second turn is unambiguously still queued when it is cancelled.
    let agent = Arc::new(Scripted {
        turn: AcpTurn {
            updates: vec![],
            stop_reason: "end_turn".into(),
        },
        holds_ms: 0,
        hang: true,
        hold_for_cancel: false,
        cancel_hangs: false,
        cancel_fails: false,
        cancels: Default::default(),
        cancel_started: tokio::sync::Notify::new(),
    });
    let cancels = agent.cancels.clone();
    let run_turn = Arc::new(AcpRunTurn::new(agent));
    let company = CompanyId::new("acme");

    // The lock owner: hangs forever, holding the slot.
    let owner = {
        let run_turn = Arc::clone(&run_turn);
        let company = company.clone();
        tokio::spawn(async move {
            let control = crate::company::steer::SteerControl::new();
            run_turn
                .run_steered(
                    &company,
                    "ceo",
                    "first",
                    &control,
                    ChatTarget::default(),
                    None,
                )
                .await
        })
    };
    // Let it take the slot before the queued turn asks for it.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let queued = crate::company::steer::SteerControl::new();
    queued.request(crate::company::steer::SteerAction::Cancel);
    let err = run_turn
        .run_steered(
            &company,
            "ceo",
            "second",
            &queued,
            ChatTarget::default(),
            None,
        )
        .await
        .expect_err("a turn cancelled while queued does not run");

    assert!(
        format!("{err}").contains("cancelled before it started"),
        "the error says it never started: {err}"
    );
    assert_eq!(
        cancels.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "and no cancel reached the agent, which would have stopped the OTHER turn"
    );

    owner.abort();
}

#[tokio::test]
async fn a_cancel_landing_as_the_slot_frees_still_stops_the_turn() {
    // The race the 250ms poll cannot win alone (PR #1904 review): the
    // cancel arrives while this turn is queued, and the slot frees BEFORE
    // the next tick — so `lock_owned()` wins the select and the control is
    // never consulted. Without the check on acquiring, a cancelled turn
    // would reach the agent.
    //
    // The owner holds for 50ms against a 250ms poll, so the lock branch
    // wins deterministically.
    let mut owner_agent = Scripted::answering(vec![AcpUpdate::MessageChunk("first".into())]);
    owner_agent.holds_ms = 50;
    let agent = Arc::new(owner_agent);
    let cancels = agent.cancels.clone();
    let run_turn = Arc::new(AcpRunTurn::new(agent));
    let company = CompanyId::new("acme");

    let owner = {
        let run_turn = Arc::clone(&run_turn);
        let company = company.clone();
        tokio::spawn(async move {
            let control = crate::company::steer::SteerControl::new();
            run_turn
                .run_steered(
                    &company,
                    "ceo",
                    "first",
                    &control,
                    ChatTarget::default(),
                    None,
                )
                .await
        })
    };
    // Long enough that the owner holds the slot, short enough that it is
    // still holding it when the queued turn asks.
    tokio::time::sleep(Duration::from_millis(10)).await;

    let queued = crate::company::steer::SteerControl::new();
    queued.request(crate::company::steer::SteerAction::Cancel);
    let err = run_turn
        .run_steered(
            &company,
            "ceo",
            "second",
            &queued,
            ChatTarget::default(),
            None,
        )
        .await
        .expect_err("a turn cancelled while queued does not run");

    assert!(
        format!("{err}").contains("cancelled before it started"),
        "the error says it never started: {err}"
    );
    assert_eq!(
        cancels.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "and no cancel reached the agent, which would have stopped the OTHER turn"
    );
    owner.await.expect("owner joins").expect("owner answers");
}

#[tokio::test]
async fn a_pending_cancel_on_a_free_slot_still_runs_and_is_forwarded() {
    // The other side of that boundary, and the reason the refusal is
    // scoped to queued turns only. With no other turn on the session there
    // is nothing a forwarded cancel could stop by mistake, so a pending
    // cancel keeps its long-standing meaning: the turn runs, the cancel
    // goes to the agent, and the agent winds down and reports — an `Ok`
    // outcome the caller settles as cancelled rather than failed.
    let mut agent = Scripted::answering(vec![AcpUpdate::MessageChunk("done".into())]);
    agent.hold_for_cancel = true;
    let agent = Arc::new(agent);
    let cancels = agent.cancels.clone();
    let run_turn = AcpRunTurn::new(agent);

    let control = crate::company::steer::SteerControl::new();
    control.request(crate::company::steer::SteerAction::Cancel);

    let outcome = run_turn
        .run_steered(
            &CompanyId::new("acme"),
            "ceo",
            "go",
            &control,
            ChatTarget::default(),
            None,
        )
        .await
        .expect("an uncontended turn still runs and returns its outcome");

    assert_eq!(outcome.reply, "done");
    assert_eq!(
        cancels.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the cancel was forwarded, because this turn was the one running"
    );
}
