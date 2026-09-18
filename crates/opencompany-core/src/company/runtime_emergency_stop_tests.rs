use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{CompanyEvent, CompanyRuntime};
use crate::ports::Brain;
use crate::ports::brain::CycleHost;
use crate::ports::types::{
    Actor, ActorKind, CycleRequest, CycleResult, OutboundMessage, TokenUsage,
};

/// A brain that does the three things a stopped company must not do:
/// take a turn, run a tool, and bill for the inference.
///
/// The "tool" is a recorded line rather than a real dispatcher because
/// the assertion is that the turn body never ran at all — a real tool
/// would be reached through the same `run_cycle` that is not called.
#[derive(Default)]
struct WorkingBrain {
    turns: AtomicUsize,
    tool_log: Mutex<Vec<String>>,
}

impl WorkingBrain {
    fn turns(&self) -> usize {
        self.turns.load(Ordering::SeqCst)
    }

    fn tool_calls(&self) -> Vec<String> {
        self.tool_log.lock().expect("tool log poisoned").clone()
    }
}

#[async_trait::async_trait]
impl Brain for WorkingBrain {
    async fn run_cycle(
        &self,
        req: CycleRequest,
        _host: &dyn CycleHost,
    ) -> crate::Result<CycleResult> {
        self.turns.fetch_add(1, Ordering::SeqCst);
        self.tool_log
            .lock()
            .expect("tool log poisoned")
            .push(format!("notify_slack({})", req.cycle_id));
        Ok(CycleResult {
            channel_responses: vec![OutboundMessage {
                message_id: None,
                task_id: None,
                outputs: Vec::new(),
                channel: "operator".into(),
                agent: Some("ceo".into()),
                text: "a full turn ran".into(),
                steps: Vec::new(),
                reply_to: None,
                mentions: Vec::new(),
            }],
            new_traces: Vec::new(),
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage {
                input: 4_000,
                output: 500,
                cached_input: 0,
                cost_usd: 0.12,
            },
        })
    }
}

/// A brain that parks an explicit `request_approval` call on every
/// `OperatorMessage` and counts every denial it is later told about.
#[derive(Default)]
struct ExplicitRequestBrain {
    denials: AtomicUsize,
}

impl ExplicitRequestBrain {
    fn denials(&self) -> usize {
        self.denials.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Brain for ExplicitRequestBrain {
    async fn run_cycle(
        &self,
        req: CycleRequest,
        host: &dyn CycleHost,
    ) -> crate::Result<CycleResult> {
        for event in &req.events {
            match event {
                CompanyEvent::OperatorMessage { .. } => {
                    host.park_effect(crate::ports::types::Effect {
                        kind: crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND.into(),
                        group: crate::ports::types::EffectGroup::Sign,
                        amount_usd: Some(42.0),
                        established_thread: false,
                        first_time_counterparty: false,
                        payload: serde_json::json!({
                            "title": "Submit the filing",
                            "question": "May I submit it?"
                        }),
                        agent: Some("ceo".into()),
                        run_id: None,
                    })
                    .await?;
                }
                CompanyEvent::ApprovalResolved {
                    verdict: crate::ports::types::Verdict::Deny,
                    ..
                } => {
                    self.denials.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }
        Ok(CycleResult {
            channel_responses: Vec::new(),
            new_traces: Vec::new(),
            ledger_deltas: Vec::new(),
            token_usage: TokenUsage::default(),
        })
    }
}

fn manifest() -> crate::company::CompanyManifest {
    toml::from_str(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n\
         [policy]\nmode = \"full\"\n",
    )
    .expect("manifest")
}

fn operator() -> Actor {
    Actor {
        kind: ActorKind::Operator,
        id: "owner".into(),
    }
}

fn ask() -> CompanyEvent {
    CompanyEvent::OperatorMessage {
        text: "ship the release".into(),
        by: Some(operator()),
        chat: None,
        parent: None,
        deliverable: None,
        mentions: Vec::new(),
        attachments: Vec::new(),
    }
}

/// A settled verdict, the receipt `spawn_follow_up` turns into a
/// continuation turn.
fn settled(approval: &str) -> super::ResolveReceipt {
    use crate::ports::types::{ApprovalId, Verdict};
    super::ResolveReceipt::Settled(Box::new(CompanyEvent::ApprovalResolved {
        approval_id: ApprovalId::new(approval),
        verdict: Verdict::Approve,
        by: operator(),
    }))
}

async fn working_company() -> (Arc<CompanyRuntime>, Arc<WorkingBrain>, tempfile::TempDir) {
    let home = tempfile::Builder::new()
        .prefix("opencompany-emergency-")
        .tempdir()
        .expect("tempdir");
    let brain = Arc::new(WorkingBrain::default());
    let rt = Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
            .with_brain(brain.clone())
            .build()
            .await
            .expect("runtime"),
    );
    (rt, brain, home)
}

/// How many inference samples the meter holds — the bill.
async fn billed(rt: &CompanyRuntime) -> usize {
    rt.usage()
        .query(rt.id(), 0)
        .await
        .expect("usage query")
        .len()
}

/// The stop must survive the runtime being *assembled*, not only the
/// runtime being constructed.
///
/// `CompanyRuntime::new` gates the workflow gate queue, and then the
/// builder replaces that field wholesale with the queue it prepared —
/// a fresh one on a boot, the outgoing runtime's on a rebuild. Neither
/// has a company to ask, so neither carries a gate, and the queue that
/// actually reaches production carried none: a batch whose last sibling
/// expired during a stop was released and destroyed, and the approved
/// work in it could not be recovered.
///
/// Built through the real builder rather than by hand, because
/// assembling it by hand is what hid this.
#[tokio::test]
async fn a_builder_assembled_runtime_still_refuses_to_release_a_batch_while_stopped() {
    use crate::ports::types::{Effect, EffectGroup, Verdict};
    use crate::runtime::workflow_resume::{PAYLOAD_NODE_ID, WORKFLOW_APPROVE_KIND};

    let (rt, _brain, _home) = working_company().await;

    let gate = Effect {
        kind: WORKFLOW_APPROVE_KIND.to_string(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::json!({ PAYLOAD_NODE_ID: "node-a" }),
        agent: None,
        run_id: Some("wr-1".to_string()),
    };
    let id = crate::ports::types::ApprovalId::new("appr-a");
    rt.workflow_gates().arm("turn-1", &id, &gate);
    rt.workflow_gates().decide("turn-1", &id, Verdict::Approve);

    rt.workflow_gates()
        .release("turn-1")
        .expect("running, so the batch releases")
        .expect("a batch was armed");

    rt.workflow_gates().arm("turn-2", &id, &gate);
    rt.workflow_gates().decide("turn-2", &id, Verdict::Approve);
    rt.emergency_pause(operator(), None).await.expect("stop");

    rt.workflow_gates()
        .release("turn-2")
        .expect_err("a stopped company must not release a decided batch");
    assert!(
        rt.workflow_gates().is_armed("turn-2"),
        "the refused batch keeps every verdict it banked, for a redrive after the stop"
    );
    assert_eq!(
        rt.workflow_gates().ready_for_release(),
        vec!["turn-2".to_string()],
        "and it is discoverable, which is what makes the approved work recoverable"
    );
}

/// **The defect.** With the stop engaged, a new turn must not run, the
/// tool it would have called must not execute, and no inference may be
/// billed.
///
/// The first cycle is deliberately run *before* the stop, so a fixture
/// that silently never works cannot pass this by doing nothing.
#[tokio::test]
async fn a_stopped_company_runs_no_turn_calls_no_tool_and_bills_nothing() {
    let (rt, brain, _home) = working_company().await;

    rt.run_cycle(vec![ask()]).await.expect("a running company");
    assert_eq!(brain.turns(), 1, "the fixture must really run a turn");
    assert_eq!(brain.tool_calls().len(), 1);
    assert_eq!(billed(&rt).await, 1, "the fixture must really bill");

    assert!(
        rt.emergency_pause(operator(), Some("stop everything".into()))
            .await
            .expect("pause"),
        "this call engaged the stop"
    );

    let refused = rt.run_cycle(vec![ask()]).await;
    assert!(
        matches!(refused, Err(crate::OpenCompanyError::EmergencyStop(_))),
        "a stopped company must refuse a new cycle, got {refused:?}"
    );
    assert_eq!(
        brain.turns(),
        1,
        "no turn may run while the emergency stop is engaged"
    );
    assert_eq!(
        brain.tool_calls().len(),
        1,
        "no tool may execute while the emergency stop is engaged"
    );
    assert_eq!(
        billed(&rt).await,
        1,
        "no inference may be billed while the emergency stop is engaged"
    );
}

/// The journaled entry point is the one the chat route uses, so it owes
/// the same refusal — otherwise the switch is bypassed by whichever
/// ingress happens to append first.
#[tokio::test]
async fn a_stopped_company_refuses_a_journaled_cycle_too() {
    let (rt, brain, _home) = working_company().await;
    rt.emergency_pause(operator(), None).await.expect("pause");

    let seq = rt.events().append(rt.id(), ask()).await.expect("append");
    let refused = rt.run_journaled_cycle(vec![(seq, ask())], None).await;
    assert!(
        matches!(refused, Err(crate::OpenCompanyError::EmergencyStop(_))),
        "the journaled entry point must refuse too, got {refused:?}"
    );
    assert_eq!(brain.turns(), 0);
    assert_eq!(billed(&rt).await, 0);
}

/// The continuation funnel: every follow-up turn — an operator's
/// verdict, a TTL expiry, a released blocker, a workflow replay —
/// reaches its dispatch through `spawn_follow_up`. A stop that guarded
/// only the ingress would leave that whole family running, and the TTL
/// sweep reaches it without passing an ingress at all.
#[tokio::test]
async fn a_stopped_company_runs_no_continuation_turn() {
    let (rt, brain, _home) = working_company().await;
    rt.emergency_pause(operator(), None).await.expect("pause");

    let refused = rt
        .spawn_follow_up(settled("appr-continuation"))
        .await
        .expect("the follow-up task joins");
    assert!(
        matches!(refused, Err(crate::OpenCompanyError::EmergencyStop(_))),
        "a follow-up turn must be refused while stopped, got {refused:?}"
    );
    assert_eq!(
        brain.turns(),
        0,
        "a continuation must not run a turn while the stop is engaged"
    );
    assert_eq!(billed(&rt).await, 0);
}

/// Releasing restores **all** of it. A company that cannot resume is a
/// worse bug than one that cannot stop.
#[tokio::test]
async fn releasing_the_stop_restores_turns_tools_and_billing() {
    let (rt, brain, _home) = working_company().await;
    rt.emergency_pause(operator(), None).await.expect("pause");
    assert!(rt.run_cycle(vec![ask()]).await.is_err());

    assert!(
        rt.emergency_resume(operator(), Some("all clear".into()))
            .await
            .expect("resume"),
        "this call released the stop"
    );

    rt.run_cycle(vec![ask()]).await.expect("a released company");
    assert_eq!(brain.turns(), 1, "the turn runs again after the release");
    assert_eq!(brain.tool_calls().len(), 1, "tools execute again");
    assert_eq!(billed(&rt).await, 1, "inference is billed again");

    rt.spawn_follow_up(settled("appr-released"))
        .await
        .expect("the follow-up task joins")
        .expect("a released company");
    assert_eq!(
        brain.turns(),
        2,
        "continuations run again after the release"
    );
}

/// The stop survives a restart as **enforcement**, not only as a flag.
///
/// `emergency_paused: true` on a rebooted company that still runs turns
/// is the exact shape of the defect, so the reboot is asserted by
/// dispatching a cycle into it.
#[tokio::test]
async fn the_stop_survives_a_restart_and_the_rebooted_company_still_refuses_work() {
    let home = tempfile::Builder::new()
        .prefix("opencompany-emergency-reboot-")
        .tempdir()
        .expect("tempdir");

    let first = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
        .with_brain(Arc::new(WorkingBrain::default()))
        .build()
        .await
        .expect("runtime");
    first
        .emergency_pause(operator(), None)
        .await
        .expect("pause");
    drop(first);

    let brain = Arc::new(WorkingBrain::default());
    let rebooted = Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
            .with_brain(brain.clone())
            .build()
            .await
            .expect("runtime"),
    );
    assert!(rebooted.is_emergency_paused(), "the stop replayed");

    let refused = rebooted.run_cycle(vec![ask()]).await;
    assert!(
        matches!(refused, Err(crate::OpenCompanyError::EmergencyStop(_))),
        "a rebooted stopped company must still refuse work, got {refused:?}"
    );
    assert_eq!(brain.turns(), 0);
    assert_eq!(billed(&rebooted).await, 0);

    // And the release still works on the rebooted runtime.
    rebooted
        .emergency_resume(operator(), None)
        .await
        .expect("resume");
    rebooted.run_cycle(vec![ask()]).await.expect("released");
    assert_eq!(brain.turns(), 1);
}

/// The native-effect path is untouched: an engaged stop still denies a
/// side-effecting effect and still refuses to park one, exactly as
/// before, and releasing restores both.
#[tokio::test]
async fn native_effect_denial_is_unchanged_by_the_admission_gate() {
    use crate::ports::approvals::ApprovalGate;
    use crate::ports::types::{Effect, EffectGroup, PolicyDecision};

    let (rt, _brain, _home) = working_company().await;
    let effect = Effect {
        kind: "filing.submit".into(),
        group: EffectGroup::Sign,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::json!({}),
        agent: Some("ceo".into()),
        run_id: None,
    };

    rt.emergency_pause(operator(), None).await.expect("pause");
    assert_eq!(
        rt.approval_gate
            .evaluate(rt.id(), &effect)
            .await
            .expect("evaluate"),
        PolicyDecision::Deny,
        "the gate still denies a side-effecting effect while stopped"
    );
    assert!(
        matches!(
            rt.approval_gate.park(rt.id(), effect.clone()).await,
            Err(crate::OpenCompanyError::EmergencyStop(_))
        ),
        "the gate still refuses to park one while stopped"
    );

    rt.emergency_resume(operator(), None).await.expect("resume");
    assert_eq!(
        rt.approval_gate
            .evaluate(rt.id(), &effect)
            .await
            .expect("evaluate"),
        PolicyDecision::Allow,
        "releasing restores the company's own `full` policy"
    );
}

/// An explicit-request continuation whose dispatch a live stop refused
/// is not a blocked-node stash, so `reconcile_stranded_blocked_nodes`
/// never sees it. Releasing the stop must still redeliver it on this
/// same live process rather than leaving it for the next restart.
#[tokio::test]
async fn releasing_the_stop_redelivers_a_continuation_the_stop_itself_refused() {
    let home = tempfile::Builder::new()
        .prefix("opencompany-emergency-continuation-")
        .tempdir()
        .expect("tempdir");
    let gate = Arc::new(
        crate::policy::ManifestApprovalGate::new(manifest().policy.clone()).with_ttl_millis(0),
    );
    let brain = Arc::new(ExplicitRequestBrain::default());
    let rt = Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
            .with_brain(brain.clone())
            .with_approvals(gate)
            .build()
            .await
            .expect("runtime"),
    );

    let report = rt
        .run_cycle(vec![ask()])
        .await
        .expect("a running company parks the request");
    assert_eq!(
        report.parked.len(),
        1,
        "the fixture must really park an explicit request"
    );
    let approval_id = report.parked[0].clone();

    rt.emergency_pause(operator(), None).await.expect("pause");

    // The gate's zero TTL means the approval is already past its
    // deadline: the sweep retires it, mints its continuation, and
    // tries to dispatch it — the dispatch `spawn_follow_up`'s own
    // check refuses while the company is stopped.
    rt.sweep_expired_approvals().await.expect("sweep");

    // Let the refused dispatch's spawned task actually run (and fail)
    // before asserting on it.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    assert_eq!(
        brain.denials(),
        0,
        "the stop must have refused the continuation's dispatch"
    );
    assert!(
        rt.grants.peek_continuation(&approval_id).is_some(),
        "a continuation the stop refused to dispatch must stay durable, not be lost"
    );

    rt.emergency_resume(operator(), None).await.expect("resume");

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while brain.denials() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "releasing the stop must redeliver the continuation it refused, without a \
             restart; continuation_live={}",
            rt.grants.peek_continuation(&approval_id).is_some()
        )
    });
    assert_eq!(brain.denials(), 1);
}
