//! Offline mocks for the sidecar seams.
//!
//! [`MockSidecarTransport`] scripts a [`SidecarFrame`] plan per cycle and records
//! every call the brain makes (posts, tool registrations, acks, tool answers,
//! inference answers). [`MockInferenceClient`] returns a canned completion and
//! records the requests it received. Together they drive the whole sidecar brain
//! offline — no network, no Node process.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::{self, BoxStream};

use crate::Result;
use crate::ports::types::TokenUsage;

use crate::brain::medulla::wire::{
    self, EffectResult, EventsAccepted, EventsRequest, OrchErrorCode, ToolManifestEntry,
    ToolResultFrame,
};

use super::transport::{InferenceClient, SidecarTransport};
use super::types::{InferenceRequest, InferenceResponse, SidecarFrame};

/// An inference answer the mock transport recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedInference {
    /// The correlation id answered.
    pub call_id: String,
    /// The completion returned.
    pub response: InferenceResponse,
}

/// Everything the mock transport recorded, for test assertions.
#[derive(Default)]
struct Recorded {
    posted_events: Vec<EventsRequest>,
    registered_tools: Vec<Vec<ToolManifestEntry>>,
    acks: Vec<EffectResult>,
    tool_answers: Vec<ToolResultFrame>,
    inference_answers: Vec<RecordedInference>,
    /// Cycle ids the brain asked for that nothing had scripted.
    ///
    /// An unscripted cycle yields a bare `CycleComplete`, which is a *silent*
    /// empty cycle: every downstream assertion then reads "expected 1, saw 0"
    /// and points at the effect rather than at the miss. Recording it turns
    /// "the fixture and the runtime disagree about the cycle id" into something
    /// a test can assert (issue #800).
    unmatched_cycles: Vec<String>,
}

/// An in-memory [`SidecarTransport`] that scripts cycle frames and records calls.
#[derive(Default)]
pub struct MockSidecarTransport {
    /// Scripted frames keyed by cycle id.
    plans: Mutex<HashMap<String, Vec<SidecarFrame>>>,
    /// Frames for the **next** cycle the brain opens, when no exact id is
    /// scripted. Consumed on use — see [`Self::script_any_cycle`].
    fallback_plan: Mutex<Option<Vec<SidecarFrame>>>,
    /// Whether a fallback was ever scripted, so a later empty cycle reads as
    /// expected rather than as a missed plan.
    fallback_scripted: Mutex<bool>,
    /// An error to return from every `post_events`.
    post_events_error: Mutex<Option<OrchErrorCode>>,
    /// Recorded calls.
    recorded: Mutex<Recorded>,
}

impl MockSidecarTransport {
    /// Creates an empty mock with no scripted cycles.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scripts the frames [`Self::cycle_frames`] yields for `cycle_id`.
    ///
    /// A trailing [`SidecarFrame::CycleComplete`] is appended automatically so
    /// the brain's drain loop always terminates.
    pub fn script_cycle(&self, cycle_id: impl Into<String>, mut frames: Vec<SidecarFrame>) {
        if !matches!(frames.last(), Some(SidecarFrame::CycleComplete)) {
            frames.push(SidecarFrame::CycleComplete);
        }
        self.plans.lock().unwrap().insert(cycle_id.into(), frames);
    }

    /// Scripts frames for **whichever** cycle the brain opens.
    ///
    /// Prefer this in end-to-end tests. Keying a plan to a hand-computed cycle
    /// id couples the test to the event sequence the runtime happens to assign
    /// — which is how the two `e2e_*` tests here silently stopped exercising
    /// anything: they scripted `seq 0` while the runtime posts the event log's
    /// real seq, so the brain drained an empty cycle and every assertion failed
    /// against a plausible-looking zero (issue #800). The id's own format is
    /// covered by `wire::cycle_id`'s tests; a cycle-drain test should not
    /// re-derive it.
    /// **One-shot.** The plan is consumed by the first unscripted cycle, and
    /// every later cycle drains empty. A turn does not always open exactly one
    /// cycle — resolving a parked approval opens another — and a replaying plan
    /// would re-emit its effect there, re-parking what the test just resolved.
    pub fn script_any_cycle(&self, mut frames: Vec<SidecarFrame>) {
        if !matches!(frames.last(), Some(SidecarFrame::CycleComplete)) {
            frames.push(SidecarFrame::CycleComplete);
        }
        *self.fallback_plan.lock().unwrap() = Some(frames);
        *self.fallback_scripted.lock().unwrap() = true;
    }

    /// Cycle ids the brain opened that nothing had scripted.
    pub fn unmatched_cycles(&self) -> Vec<String> {
        self.recorded.lock().unwrap().unmatched_cycles.clone()
    }

    /// Makes every subsequent `post_events` fail with `code`.
    pub fn fail_post_events(&self, code: OrchErrorCode) {
        *self.post_events_error.lock().unwrap() = Some(code);
    }

    /// The `EventsRequest`s the brain posted, in order.
    pub fn posted_events(&self) -> Vec<EventsRequest> {
        self.recorded.lock().unwrap().posted_events.clone()
    }

    /// The tool manifests the brain registered, in order.
    pub fn registered_tools(&self) -> Vec<Vec<ToolManifestEntry>> {
        self.recorded.lock().unwrap().registered_tools.clone()
    }

    /// The effect acks the brain emitted, in order.
    pub fn acks(&self) -> Vec<EffectResult> {
        self.recorded.lock().unwrap().acks.clone()
    }

    /// The tool answers the brain emitted, in order.
    pub fn tool_answers(&self) -> Vec<ToolResultFrame> {
        self.recorded.lock().unwrap().tool_answers.clone()
    }

    /// The inference answers the brain emitted, in order.
    pub fn inference_answers(&self) -> Vec<RecordedInference> {
        self.recorded.lock().unwrap().inference_answers.clone()
    }
}

#[async_trait]
impl SidecarTransport for MockSidecarTransport {
    async fn post_events(&self, req: EventsRequest) -> Result<EventsAccepted> {
        if let Some(code) = self.post_events_error.lock().unwrap().clone() {
            return Err(code.to_error("mock post_events failure"));
        }
        let cycle_id = wire::cycle_id(&req.counterpart_agent_id, &req.session_id, req.event.seq);
        self.recorded.lock().unwrap().posted_events.push(req);
        Ok(EventsAccepted {
            accepted: true,
            cycle_id,
        })
    }

    async fn register_tools(&self, tools: Vec<ToolManifestEntry>) -> Result<()> {
        self.recorded.lock().unwrap().registered_tools.push(tools);
        Ok(())
    }

    fn cycle_frames(&self, cycle_id: &str) -> BoxStream<'static, Result<SidecarFrame>> {
        let exact = self.plans.lock().unwrap().get(cycle_id).cloned();
        let frames = match exact {
            Some(frames) => frames,
            None => match self.fallback_plan.lock().unwrap().take() {
                Some(frames) => frames,
                // A consumed fallback means later cycles are *expected* empty,
                // so they are not recorded as misses.
                None if *self.fallback_scripted.lock().unwrap() => {
                    vec![SidecarFrame::CycleComplete]
                }
                None => {
                    // Nothing scripted for this cycle, exactly or otherwise.
                    // Still an empty cycle, so existing tests that script no
                    // plan keep their behaviour — but recorded, so a test can
                    // tell an empty cycle apart from a missed one (#800).
                    self.recorded
                        .lock()
                        .unwrap()
                        .unmatched_cycles
                        .push(cycle_id.to_string());
                    vec![SidecarFrame::CycleComplete]
                }
            },
        };
        stream::iter(frames.into_iter().map(Ok)).boxed()
    }

    async fn ack_effect(&self, ack: EffectResult) -> Result<()> {
        self.recorded.lock().unwrap().acks.push(ack);
        Ok(())
    }

    async fn answer_tool_call(&self, ans: ToolResultFrame) -> Result<()> {
        self.recorded.lock().unwrap().tool_answers.push(ans);
        Ok(())
    }

    async fn answer_inference(&self, call_id: &str, resp: InferenceResponse) -> Result<()> {
        self.recorded
            .lock()
            .unwrap()
            .inference_answers
            .push(RecordedInference {
                call_id: call_id.to_string(),
                response: resp,
            });
        Ok(())
    }
}

/// An offline [`InferenceClient`] that returns a canned completion and records
/// every request it received.
pub struct MockInferenceClient {
    text: String,
    token_usage: TokenUsage,
    requests: Mutex<Vec<InferenceRequest>>,
}

impl Default for MockInferenceClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MockInferenceClient {
    /// Builds a client that always answers with an empty completion.
    pub fn new() -> Self {
        Self {
            text: String::new(),
            token_usage: TokenUsage::default(),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Sets the canned completion text.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// Sets the token usage each completion reports.
    ///
    /// Mutates only the token fields so a `cost_usd` already set by
    /// [`Self::with_cost`] survives — the two builders compose in either order.
    pub fn with_tokens(mut self, input: u64, output: u64) -> Self {
        self.token_usage.input = input;
        self.token_usage.output = output;
        self
    }

    /// Sets the USD cost each completion reports, so a test can drive the
    /// cycle-level cost metering the runtime does.
    pub fn with_cost(mut self, cost_usd: f64) -> Self {
        self.token_usage.cost_usd = cost_usd;
        self
    }

    /// The inference requests this client received, in order.
    pub fn requests(&self) -> Vec<InferenceRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl InferenceClient for MockInferenceClient {
    async fn infer(&self, req: InferenceRequest) -> Result<InferenceResponse> {
        self.requests.lock().unwrap().push(req);
        Ok(InferenceResponse {
            text: self.text.clone(),
            token_usage: self.token_usage,
        })
    }
}
