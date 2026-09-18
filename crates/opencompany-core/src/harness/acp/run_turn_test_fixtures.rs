use super::*;

/// An agent that answers from a script, so the trait impl can be driven.
///
/// `hang` makes `prompt` never resolve (the grace-expiry path) and
/// `cancel_fails` makes `cancel` error (the logged-failure path). `cancels`
/// counts cancel calls so a test can assert the grace path nudged twice.
///
/// `hold_for_cancel` makes `prompt` wait until the first `cancel` arrives —
/// the shape of a turn that is mid-tool-call when the operator steers, which
/// is exactly the window the advisory cancel exists for. Without the gate a
/// prompt that resolves immediately exits the loop before the steer check
/// ever runs, and the cancel path goes unexercised. `cancel_hangs` makes
/// `cancel` never answer (the bounded-RPC path).
pub(super) struct Scripted {
    pub(super) turn: AcpTurn,
    /// Milliseconds to hold the turn open before answering — how a test
    /// owns the session's slot for a *bounded* window, so a second turn
    /// genuinely queues and then genuinely gets in.
    pub(super) holds_ms: u64,
    pub(super) hang: bool,
    pub(super) hold_for_cancel: bool,
    pub(super) cancel_hangs: bool,
    pub(super) cancel_fails: bool,
    pub(super) cancels: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub(super) cancel_started: tokio::sync::Notify,
}

impl Scripted {
    pub(super) fn answering(updates: Vec<AcpUpdate>) -> Self {
        Self {
            turn: AcpTurn {
                updates,
                stop_reason: "end_turn".into(),
            },
            holds_ms: 0,
            hang: false,
            hold_for_cancel: false,
            cancel_hangs: false,
            cancel_fails: false,
            cancels: Default::default(),
            cancel_started: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait]
impl AcpAgent for Scripted {
    async fn prompt(
        &self,
        _c: &CompanyId,
        _k: &str,
        _m: &str,
        observer: Option<&AcpObserver>,
    ) -> Result<AcpTurn> {
        // Observed before the hang/hold gates, so a steer test still sees
        // the frames a real transport would have already published by the
        // time the operator reaches for cancel.
        if let Some(observer) = observer {
            for update in &self.turn.updates {
                observer(update);
            }
        }
        if self.holds_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.holds_ms)).await;
        }
        if self.hang {
            std::future::pending::<()>().await;
        }
        if self.hold_for_cancel {
            self.cancel_started.notified().await;
        }
        Ok(self.turn.clone())
    }
    async fn cancel(&self, _c: &CompanyId, _k: &str) -> Result<()> {
        self.cancels
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.cancel_started.notify_waiters();
        if self.cancel_hangs {
            std::future::pending::<()>().await;
        }
        if self.cancel_fails {
            return Err(OpenCompanyError::Harness("cancel rejected".into()));
        }
        Ok(())
    }
}

/// The updates a coding turn produces: a thought, a tool call that runs
/// and then completes, and the answer.
pub(super) fn a_working_turn() -> Vec<AcpUpdate> {
    vec![
        AcpUpdate::ThoughtChunk,
        AcpUpdate::ThoughtChunk,
        AcpUpdate::ToolCall {
            id: "c1".into(),
            title: "Read src/main.rs".into(),
        },
        AcpUpdate::ToolCallUpdate {
            id: "c1".into(),
            status: "in_progress".into(),
            result: None,
        },
        AcpUpdate::ToolCallUpdate {
            id: "c1".into(),
            status: "completed".into(),
            result: Some("42 lines".into()),
        },
        AcpUpdate::MessageChunk("done".into()),
    ]
}
