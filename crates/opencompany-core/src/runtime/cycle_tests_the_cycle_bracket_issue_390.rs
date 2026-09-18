use super::tests_core::*;
use super::tests_core2::*;
use super::*;

/// Two operators resolving two different tool calls at the same moment —
/// a routine shape, not an edge case — must each durably mint their own
/// grant without one's journal-then-arm sequence clobbering the other's.
/// Driven with `tokio::join!` on a multi-thread runtime so the two
/// `mint_grant` calls genuinely overlap rather than merely being awaited
/// back to back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_mints_of_different_tool_calls_both_survive_a_restart() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    // Two different teammates, not one effect cloned: an approval carries
    // the agent it was raised for, so a pair that names the same agent
    // races one subject's mint against itself and never reaches the
    // cross-agent case a company with more than one teammate produces.
    let (rt, ids) = park_two_blocked_tool_calls_for(
        home.clone(),
        [
            harness_effect("finance", "composio_execute", serde_json::json!({})),
            harness_effect("legal", "composio_execute", serde_json::json!({})),
        ],
    )
    .await;
    let (a, b) = tokio::join!(
        rt.resolve_approval(&ids[0], Verdict::Approve, operator()),
        rt.resolve_approval(&ids[1], Verdict::Approve, operator()),
    );
    a.expect("the first concurrent mint succeeds");
    b.expect("the second concurrent mint succeeds");
    assert_eq!(rt.grants.live_count(), 2, "both mints armed in memory");
    drop(rt);

    let restarted = RuntimeBuilder::fs_defaults(home, manifest("supervised"))
        .await
        .unwrap();
    assert_eq!(
        restarted.grants.live_count(),
        2,
        "both concurrent mints must have journaled durably — neither append may be lost \
         to the other running at the same time"
    );
    for id in ids {
        assert!(
            restarted.grants.peek(&id).is_some(),
            "grant {id} must survive the restart"
        );
    }
}

/// The bug: a brain outside the openhuman harness reported real token usage
/// and the cycle loop dropped it, so the Usage view read zero forever.
#[tokio::test]
async fn reported_cycle_usage_reaches_the_usage_meter() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(MeteredBrain::per_cycle(reported_usage(0.031))))
        .build()
        .await
        .unwrap();

    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "how are we doing".into(),
        by: None,
        chat: None,
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    let samples = rt.usage().query(rt.id(), 0).await.unwrap();
    assert_eq!(samples.len(), 1, "one inference sample per metered cycle");
    let sample = &samples[0];
    assert_eq!(sample.kind, crate::ports::usage::SampleKind::Inference);
    assert_eq!(sample.input_tokens, 1_200);
    assert_eq!(sample.output_tokens, 340);
    assert_eq!(sample.cached_input_tokens, 200);
    assert_eq!(sample.cost_usd, 0.031);
    assert_eq!(sample.provider, "medulla");
    assert_eq!(sample.agent, crate::metering::UNATTRIBUTED_AGENT);

    // Cost also lands on Finances as an `inference.spend` ledger entry.
    let record = rt.store().load(rt.id()).await.unwrap().unwrap();
    let spend: Vec<_> = record
        .ledger
        .iter()
        .filter(|e| e.kind == crate::metering::INFERENCE_SPEND_KIND)
        .collect();
    assert_eq!(spend.len(), 1);
    // Negative: an outflow, per the ledger convention (issue #1047).
    assert_eq!(spend[0].amount_usd, -0.031);
}

/// a `PerCycle`-metered brain's spend charges
/// `UNATTRIBUTED_AGENT` unconditionally, even on a single-agent company
/// where the cycle can only have been that one teammate's work. That
/// makes the spend invisible to `usd_spent_by_agent` for the real
/// teammate — and therefore invisible to that teammate's
/// `budget_usd_daily` cap, which sums exactly that function's output.
#[tokio::test]
async fn per_cycle_spend_is_invisible_to_the_real_agents_daily_cap() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(MeteredBrain::per_cycle(reported_usage(9.99))))
        .build()
        .await
        .unwrap();

    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "how are we doing".into(),
        by: None,
        chat: None,
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    let samples = rt.usage().query(rt.id(), 0).await.unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(
        crate::metering::daily_budget::usd_spent_by_agent(&samples, "ceo"),
        0.0,
        "the $9.99 this cycle spent is invisible to the only real teammate's daily spend \
         sum — a budget_usd_daily cap on `ceo` would never see it and could never trip"
    );
    assert_eq!(
        crate::metering::daily_budget::usd_spent_by_agent(
            &samples,
            crate::metering::UNATTRIBUTED_AGENT
        ),
        9.99,
        "the spend is real; it is just parked under the company-wide bucket instead of \
         the teammate whose turn it was"
    );
}

/// The sibling test above pins the single-agent shape; this is the wider
/// one the docstring on `record_cycle_usage` describes as still worse: a
/// multi-agent roster, several cycles deep, and two concurrent cycles
/// landing at once — none of which the existing test's one-agent,
/// one-cycle, one-call shape could distinguish from a coincidence.
///
/// Every real teammate's `usd_spent_by_agent` sum stays at exactly zero
/// for the whole run while the pooled `UNATTRIBUTED_AGENT` bucket
/// accumulates every dollar — including past the point where a
/// `budget_usd_daily` cap set on either teammate would, if it saw its own
/// spend, have tripped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_cycle_spend_stays_invisible_across_a_multi_agent_roster_and_concurrent_cycles() {
    let toml_src = r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [[agent]]
        id = "cfo"
        role = "Finance"

        [policy]
        mode = "full"
        "#;
    let manifest: CompanyManifest = toml::from_str(toml_src).expect("parse manifest");
    let home_dir = tmp_home();
    let rt = RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest)
        .with_brain(Arc::new(MeteredBrain::per_cycle(reported_usage(4.0))))
        .build()
        .await
        .unwrap();

    // Addressed to a teammate rather than left company-wide: an
    // unaddressed turn resolves to no single agent, so `run_bracketed`
    // takes the company serial lock and the two cycles queue behind each
    // other. Their metering writes would then never overlap, and a lost
    // update between them would go unnoticed by the very case meant to
    // catch it.
    let turn = |text: &'static str, chat: Option<&'static str>| {
        rt.run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: text.into(),
            by: None,
            chat: chat.map(str::to_string),
            deliverable: None,
            attachments: Vec::new(),
        }])
    };
    let (a, b) = tokio::join!(turn("first", Some("ceo")), turn("second", Some("cfo")));
    a.expect("the first concurrent cycle completes");
    b.expect("the second concurrent cycle completes");
    // A third, sequential cycle — STATE across more than one moment in
    // time, not just concurrency at one moment.
    turn("third", Some("ceo"))
        .await
        .expect("the third cycle completes");

    let samples = rt.usage().query(rt.id(), 0).await.unwrap();
    assert_eq!(samples.len(), 3, "one sample per cycle, all three landed");

    for real_agent in ["ceo", "cfo"] {
        assert_eq!(
            crate::metering::daily_budget::usd_spent_by_agent(&samples, real_agent),
            0.0,
            "`{real_agent}` must see none of this roster's PerCycle spend, however many \
             cycles ran or how many landed at once"
        );
    }
    let pooled = crate::metering::daily_budget::usd_spent_by_agent(
        &samples,
        crate::metering::UNATTRIBUTED_AGENT,
    );
    assert_eq!(
        pooled, 12.0,
        "all three cycles' spend pools under the company-wide bucket, undiminished"
    );
    // BOUND: a `budget_usd_daily` cap set below the pooled total — an
    // ordinary, sane cap for either teammate — is a threshold this
    // roster's real spend has already crossed, and neither teammate's
    // own sum ever saw it cross.
    let plausible_daily_cap_usd = 10.0;
    assert!(pooled > plausible_daily_cap_usd);
    for real_agent in ["ceo", "cfo"] {
        assert!(
            crate::metering::daily_budget::usd_spent_by_agent(&samples, real_agent)
                < plausible_daily_cap_usd,
            "`{real_agent}`'s own attributed sum must stay under a cap the company's real \
             spend already exceeded"
        );
    }
}

/// A stopped company does not take the turn at all — not "takes it and
/// performs no effect".
///
/// The switch used to be read only inside the native-effect path, so the
/// model call itself still ran and still billed; the turn was refused only
/// at whatever it tried to *do*. The refusal belongs at admission, where
/// nothing has been spent yet.
#[tokio::test]
async fn emergency_pause_stops_a_turn_from_running_and_spending() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(MeteredBrain::per_cycle(reported_usage(9.99))))
        .build()
        .await
        .unwrap();

    rt.emergency_pause(operator(), Some("incident".to_string()))
        .await
        .unwrap();
    assert!(rt.is_emergency_paused());

    let refused = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "how are we doing".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await;
    assert!(
        matches!(refused, Err(OpenCompanyError::EmergencyStop(_))),
        "{refused:?}"
    );

    let samples = rt.usage().query(rt.id(), 0).await.unwrap();
    assert!(
        samples.is_empty(),
        "and nothing was billed, because the model was never called: {samples:?}"
    );
}

/// Tokens without USD (the managed passthrough bills backend-side) still
/// count on the Usage surface, but must not post a `$0.00` spend line.
#[tokio::test]
async fn token_only_usage_meters_without_a_ledger_entry() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(MeteredBrain::per_cycle(reported_usage(0.0))))
        .build()
        .await
        .unwrap();

    rt.run_cycle(Vec::new()).await.unwrap();

    assert_eq!(rt.usage().query(rt.id(), 0).await.unwrap().len(), 1);
    let record = rt.store().load(rt.id()).await.unwrap().unwrap();
    assert!(
        !record
            .ledger
            .iter()
            .any(|e| e.kind == crate::metering::INFERENCE_SPEND_KIND)
    );
}

/// A cycle that spent nothing writes nothing — an idle cycle or the offline
/// echo brain must not mint an empty sample.
#[tokio::test]
async fn a_zero_usage_cycle_writes_no_sample() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(MeteredBrain::per_cycle(TokenUsage::default())))
        .build()
        .await
        .unwrap();

    rt.run_cycle(Vec::new()).await.unwrap();

    assert!(rt.usage().query(rt.id(), 0).await.unwrap().is_empty());
}

/// The openhuman harness meters every turn itself, so the cycle seam must
/// stay out of its way: a self-metering path's cycle usage is ignored rather
/// than charged a second time.
#[tokio::test]
async fn a_self_metering_brain_is_not_metered_twice() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(MeteredBrain {
            usage: reported_usage(9.99),
            metering: UsageMetering::PerTurn,
        }))
        .build()
        .await
        .unwrap();

    rt.run_cycle(Vec::new()).await.unwrap();

    assert!(rt.usage().query(rt.id(), 0).await.unwrap().is_empty());
    let record = rt.store().load(rt.id()).await.unwrap().unwrap();
    assert!(
        !record
            .ledger
            .iter()
            .any(|e| e.kind == crate::metering::INFERENCE_SPEND_KIND)
    );
}

/// `UsageMetering::None` means "no model runs on this path", so the cycle
/// seam must enforce it too. Without that arm a `None` brain reporting
/// non-zero usage was still metered under its own slug — the echo brain
/// would post a `provider: "none"` row into `byProvider`.
#[tokio::test]
async fn a_brain_that_runs_no_model_is_not_metered() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(MeteredBrain {
            usage: reported_usage(4.2),
            metering: UsageMetering::None,
        }))
        .build()
        .await
        .unwrap();

    rt.run_cycle(Vec::new()).await.unwrap();

    assert!(rt.usage().query(rt.id(), 0).await.unwrap().is_empty());
    let record = rt.store().load(rt.id()).await.unwrap().unwrap();
    assert!(
        !record
            .ledger
            .iter()
            .any(|e| e.kind == crate::metering::INFERENCE_SPEND_KIND)
    );
}

/// Every cycle meters independently, so a multi-turn conversation
/// accumulates rather than overwriting.
#[tokio::test]
async fn each_cycle_meters_its_own_usage() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain(Arc::new(MeteredBrain::per_cycle(reported_usage(0.01))))
        .build()
        .await
        .unwrap();

    for _ in 0..3 {
        rt.run_cycle(Vec::new()).await.unwrap();
    }

    let samples = rt.usage().query(rt.id(), 0).await.unwrap();
    assert_eq!(samples.len(), 3);
    let total: u64 = samples.iter().map(|s| s.input_tokens).sum();
    assert_eq!(total, 3_600);
}

#[tokio::test]
async fn cycles_are_serial_per_company() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let peak = Arc::new(AtomicUsize::new(0));
    let brain = Arc::new(ConcurrencyBrain {
        active: Arc::new(AtomicUsize::new(0)),
        peak: peak.clone(),
    });
    let rt = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("full"))
            .with_brain(brain)
            .build()
            .await
            .unwrap(),
    );

    let a = {
        let rt = rt.clone();
        tokio::spawn(async move { rt.run_cycle(Vec::new()).await })
    };
    let b = {
        let rt = rt.clone();
        tokio::spawn(async move { rt.run_cycle(Vec::new()).await })
    };
    a.await.unwrap().unwrap();
    b.await.unwrap().unwrap();

    // The serial lock kept the two cycles from overlapping.
    assert_eq!(peak.load(Ordering::SeqCst), 1);
}

// --- The cycle bracket (issue #390) --------------------------------------

/// **The placement test.** The bracket opens *before* the serial lock, and
/// this is the only thing that says so.
///
/// Condition of the fix rather than a nice-to-have: `cycle_id` used to be
/// minted inside `run_locked`, and moving it is the riskiest part of #390.
/// The failure mode is a correctly-typed id written in the wrong place —
/// invisible to the compiler and invisible to every other test, because a
/// bracket that opens after the lock still opens, still closes, and still
/// reads correctly once the cycle is over.
///
/// So this holds the lock and asserts the cycle is *already* visible as open
/// while it is still queued behind it. Move the mint or the `started` write
/// back inside `run_locked` and this fails; nothing else does.
#[tokio::test]
async fn a_cycles_bracket_opens_before_the_serial_lock() {
    let home_dir = tmp_home();
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("full"))
            .build()
            .await
            .unwrap(),
    );

    assert!(rt.journal.open_cycles().is_empty(), "nothing has run yet");

    // Hold the lock, so any cycle we start is stuck on the near side of it —
    // the window a post-lock bracket cannot see.
    let guard = rt.serial.lock().await;

    let spawned = {
        let rt = rt.clone();
        tokio::spawn(async move { rt.run_cycle(Vec::new()).await })
    };

    // Wait for the bracket, not for the cycle — the cycle cannot proceed.
    let mut open = Vec::new();
    for _ in 0..200 {
        open = rt.journal.open_cycles();
        if !open.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert_eq!(
        open.len(),
        1,
        "a cycle blocked on the serial lock must already be bracketed; if this \
         is empty the `started` write moved back inside the lock and #390's \
         whole case — a continuation that dies waiting — is invisible again"
    );
    assert_eq!(open[0].trigger, "empty", "no events drove this one");

    // …and it closes once the lock is released.
    drop(guard);
    spawned.await.unwrap().unwrap();
    assert!(
        rt.journal.open_cycles().is_empty(),
        "the bracket closes when the cycle ends"
    );
}

/// Codex review finding on PR #2140 (`3951723394`): `ensure_accepting` is
/// checked by the caller *before* this bracket even requests the lock, and
/// that wait is unbounded behind a busy company. A cycle that queued before
/// the emergency stop was engaged, but only reaches the front of the lock
/// after, must still be refused — otherwise the stop's own "halts
/// admission" promise has a hole exactly the size of that queue.
///
/// Reuses `a_cycles_bracket_opens_before_the_serial_lock`'s setup: holding
/// `rt.serial` directly stands in for "another cycle is running", and
/// waiting on `journal.open_cycles()` proves the queued cycle is already
/// past `ensure_accepting` and stuck on the near side of the lock — the
/// exact window this fix closes.
#[tokio::test]
async fn a_cycle_queued_behind_the_lock_is_refused_once_the_stop_engages_while_it_waits() {
    let home_dir = tmp_home();
    let rt = Arc::new(
        RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest("full"))
            .build()
            .await
            .unwrap(),
    );

    let guard = rt.serial.lock().await;

    let spawned = {
        let rt = rt.clone();
        tokio::spawn(async move { rt.run_cycle(Vec::new()).await })
    };

    let mut open = Vec::new();
    for _ in 0..200 {
        open = rt.journal.open_cycles();
        if !open.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        open.len(),
        1,
        "the queued cycle must already be bracketed before the stop engages"
    );

    rt.emergency_pause(
        Actor {
            kind: ActorKind::Operator,
            id: "owner".into(),
        },
        None,
    )
    .await
    .expect("pause");

    drop(guard);
    let result = spawned.await.unwrap();
    assert!(
        matches!(
            result,
            Err(crate::error::OpenCompanyError::EmergencyStop(_))
        ),
        "a cycle queued before the stop but reaching the lock after it must still be \
         refused, got {result:?}"
    );
    assert!(
        rt.journal.open_cycles().is_empty(),
        "the bracket must still close on a stop-refused cycle"
    );
}

/// The trigger label distinguishes the case #390 is about from an ordinary
/// chat turn, which is the whole reason an operator can read the surface.
#[test]
fn the_trigger_label_names_what_drove_the_cycle() {
    assert_eq!(cycle_trigger(&[]), "empty");
    assert_eq!(
        cycle_trigger(&[CompanyEvent::ApprovalResolved {
            approval_id: ApprovalId::new("appr-1"),
            verdict: Verdict::Approve,
            by: operator(),
        }]),
        "approval-continuation"
    );
    assert_eq!(
        cycle_trigger(&[CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "hello".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }]),
        "operator-message"
    );
}

#[tokio::test]
async fn distinct_companies_run_concurrently() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let one = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_id(CompanyId::new("one"))
        .build()
        .await
        .unwrap();
    let two = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_id(CompanyId::new("two"))
        .build()
        .await
        .unwrap();

    let (ra, rb) = tokio::join!(
        one.run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "a".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }]),
        two.run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "b".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }]),
    );
    assert_eq!(ra.unwrap().responses.len(), 1);
    assert_eq!(rb.unwrap().responses.len(), 1);
}
