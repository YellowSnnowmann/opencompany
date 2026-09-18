//! Tests for the workflow builder pass (issue #580).
//!
//! Two tiers, the same split the planning station uses. The **unit** tier covers
//! the pure decisions — the parse, the graph-vs-not-automatable resolution, the
//! host-assigned id, the spec → `RawWorkflow` conversion — because a wrong answer
//! there is silent. The **pass** tier runs the real [`run_workflow_build_pass`]
//! against a real [`CompanyRuntime`] with a real store and a scripted model,
//! because the things most likely to be wrong — that a proposal lands In Review,
//! that a bad answer returns the card to To-do with no proposal, that the attempt
//! row settles, and that an operator's move wins — are properties of the whole
//! pass.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use serde_json::{Value, json};
use tinyinference::model::{ChatModel, ModelProfile, ModelResponse};
use tinyinference::tool::ToolCall;
use tinyinference::usage::Usage;
use tinyinference::{Error as InferenceError, Result as TaResult};

use super::*;
use crate::company::CompanyManifest;
use crate::ports::types::CompanyId;
use crate::ports::{UsageMeter, UsageSample};

// ---------------------------------------------------------------------------
// A scripted model
// ---------------------------------------------------------------------------

/// A model that answers with a canned script (or fails), counts its calls, and —
/// optionally — mutates the board mid-call to simulate an operator moving the
/// card out from under the pass.
///
/// The script is a sequence: the Nth call returns the Nth reply, and once the
/// script is exhausted it repeats the last reply. A single-reply model therefore
/// answers every call the same (the card-builder shape), while a multi-reply
/// script drives the create-time copilot's draft→correct loop (issue #813): a
/// `bad → good` script proves the retry recovers, a single `bad` proves a second
/// failure folds to not-automatable.
pub(crate) struct ScriptedModel {
    replies: Vec<String>,
    /// When true the model errors instead of answering — the brain being down.
    fail: bool,
    calls: AtomicUsize,
    /// When set, the model moves the card to To-do on invoke, before answering —
    /// the operator's drag landing while the pass is waiting on the model.
    pub(super) move_card: StdMutex<Option<(Weak<CompanyRuntime>, String)>>,
}

impl ScriptedModel {
    pub(crate) fn replying(reply: impl Into<String>) -> Arc<Self> {
        Self::scripting(vec![reply.into()])
    }

    /// A model that answers each call with the next reply in `replies`, repeating
    /// the last once the script runs out.
    pub(crate) fn scripting(replies: Vec<String>) -> Arc<Self> {
        assert!(
            !replies.is_empty(),
            "a scripted model needs at least one reply"
        );
        Arc::new(Self {
            replies,
            fail: false,
            calls: AtomicUsize::new(0),
            move_card: StdMutex::new(None),
        })
    }

    pub(crate) fn failing() -> Arc<Self> {
        Arc::new(Self {
            replies: Vec::new(),
            fail: true,
            calls: AtomicUsize::new(0),
            move_card: StdMutex::new(None),
        })
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ChatModel<()> for ScriptedModel {
    async fn invoke(&self, _state: &(), request: ModelRequest) -> TaResult<ModelResponse> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(
            request.tools.is_empty(),
            "a builder pass must expose NO tools — a tool here is a loop, and a loop is a dispatch"
        );
        // Simulate an operator drag arriving while the model is thinking.
        let mover = self.move_card.lock().unwrap().clone();
        if let Some((weak, task_id)) = mover
            && let Some(runtime) = weak.upgrade()
        {
            let mut card = runtime
                .tasks()
                .list(runtime.id())
                .await
                .unwrap()
                .into_iter()
                .find(|t| t.id == task_id)
                .unwrap();
            card.column = COLUMN_TODO.to_string();
            card.updated_at_millis += 1;
            runtime.tasks().upsert(runtime.id(), &card).await.unwrap();
        }
        if self.fail {
            return Err(InferenceError::Model("the brain is down".to_string()));
        }
        let reply = self.replies[index.min(self.replies.len() - 1)].clone();
        Ok(ModelResponse::assistant(reply))
    }
}

impl HarnessModel for ScriptedModel {
    fn telemetry_provider_id(&self) -> String {
        "managed".to_string()
    }
}

// ---------------------------------------------------------------------------
// A native tool-calling model — the create-time copilot's agent path (issue #840)
// ---------------------------------------------------------------------------

/// One scripted turn of the native model: a text reply and/or a batch of tool
/// calls. A step with `calls` continues the agent's turn (the loop runs the
/// tools and asks again); a step with no calls ends it.
pub(crate) struct NativeStep {
    text: String,
    calls: Vec<(String, Value)>,
}

impl NativeStep {
    /// A turn that calls one tool with the given arguments.
    pub(crate) fn call(tool: &str, args: Value) -> Self {
        Self {
            text: String::new(),
            calls: vec![(tool.to_string(), args)],
        }
    }

    /// A final turn: text, no tool calls — ends the agent's turn.
    pub(crate) fn done(text: &str) -> Self {
        Self {
            text: text.to_string(),
            calls: Vec::new(),
        }
    }
}

/// A model that advertises NATIVE tool-calling and replays a script of turns —
/// the shape openhuman's [`NativeToolDispatcher`] drives. Each `invoke` pops the
/// next step and emits its `message.tool_calls`; once the script runs out it
/// repeats the last step (a repeated tool-call step is how a cap-hit is
/// exercised). Optionally carries per-call `usage` + backend-charged USD (stashed
/// in `raw` under `openhuman_usage_meta`, exactly as the managed backend does) so
/// the metering path is testable.
pub(crate) struct NativeCopilotModel {
    steps: StdMutex<VecDeque<NativeStep>>,
    /// The last step, repeated once the script is exhausted (drives cap-hits).
    repeat: StdMutex<NativeStep>,
    calls: AtomicUsize,
    usage: Option<Usage>,
    charged_usd: f64,
    profile: ModelProfile,
    /// The `request.messages` seen on each `invoke`, in call order — so a test can
    /// assert whether a later turn's first invoke replayed an earlier turn's
    /// transcript (issue #1042).
    seen_messages: StdMutex<Vec<Vec<Message>>>,
    /// The `request.tools` names seen on each `invoke`, in call order — so a test
    /// can assert the model actually got offered a given tool (issue #1931
    /// regression: `propose_company_workflow` was silently withheld by the
    /// vendored toolpacks registry, and the model dutifully called a tool it was
    /// never advertised).
    seen_tool_names: StdMutex<Vec<Vec<String>>>,
}

impl NativeCopilotModel {
    pub(crate) fn scripting(steps: Vec<NativeStep>) -> Arc<Self> {
        assert!(
            !steps.is_empty(),
            "a scripted model needs at least one step"
        );
        let deque: VecDeque<NativeStep> = steps.into();
        // The tail step is what repeats when the script runs dry — clone its
        // shape (text + calls) as the repeat template.
        let last = deque.back().expect("non-empty");
        let repeat = NativeStep {
            text: last.text.clone(),
            calls: last.calls.clone(),
        };
        Arc::new(Self {
            steps: StdMutex::new(deque),
            repeat: StdMutex::new(repeat),
            calls: AtomicUsize::new(0),
            usage: None,
            charged_usd: 0.0,
            profile: ModelProfile {
                tool_calling: true,
                ..ModelProfile::default()
            },
            seen_messages: StdMutex::new(Vec::new()),
            seen_tool_names: StdMutex::new(Vec::new()),
        })
    }

    /// Attaches per-call token usage and a backend-charged USD amount to every
    /// reply — the managed-backend metering signal.
    pub(crate) fn with_charge(self: Arc<Self>, input: u64, output: u64, usd: f64) -> Arc<Self> {
        // The model is built before it is shared; mutate through a fresh Arc.
        let mut model = Arc::try_unwrap(self).ok().expect("uniquely held at setup");
        model.usage = Some(Usage {
            input_tokens: input,
            output_tokens: output,
            ..Usage::default()
        });
        model.charged_usd = usd;
        Arc::new(model)
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The `request.messages` recorded for each invoke so far, in call order.
    pub(crate) fn seen_messages(&self) -> Vec<Vec<Message>> {
        self.seen_messages.lock().unwrap().clone()
    }

    /// The `request.tools` names recorded for each invoke so far, in call order.
    pub(crate) fn seen_tool_names(&self) -> Vec<Vec<String>> {
        self.seen_tool_names.lock().unwrap().clone()
    }

    pub(crate) fn next_step(&self) -> NativeStep {
        let mut steps = self.steps.lock().unwrap();
        if let Some(step) = steps.pop_front() {
            step
        } else {
            let repeat = self.repeat.lock().unwrap();
            NativeStep {
                text: repeat.text.clone(),
                calls: repeat.calls.clone(),
            }
        }
    }
}

#[async_trait]
impl ChatModel<()> for NativeCopilotModel {
    fn profile(&self) -> Option<&ModelProfile> {
        Some(&self.profile)
    }

    async fn invoke(&self, _state: &(), request: ModelRequest) -> TaResult<ModelResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen_tool_names.lock().unwrap().push(
            request
                .tools
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>(),
        );
        self.seen_messages.lock().unwrap().push(request.messages);
        let step = self.next_step();
        let tool_calls: Vec<ToolCall> = step
            .calls
            .iter()
            .enumerate()
            .map(|(idx, (name, args))| ToolCall {
                id: format!("call-{idx}"),
                name: name.clone(),
                arguments: args.clone(),
                invalid: None,
            })
            .collect();

        let mut response = ModelResponse::assistant(step.text);
        response.message.tool_calls = tool_calls.clone();
        response.finish_reason = Some(
            if tool_calls.is_empty() {
                "stop"
            } else {
                "tool_calls"
            }
            .to_string(),
        );
        if let Some(usage) = self.usage.as_ref() {
            response.usage = Some(*usage);
            response.message.usage = Some(*usage);
        }
        if self.charged_usd > 0.0 {
            response.raw = Some(json!({
                "openhuman_usage_meta": { "charged_amount_usd": self.charged_usd, "context_window": 0 }
            }));
        }
        Ok(response)
    }
}

impl HarnessModel for NativeCopilotModel {
    fn telemetry_provider_id(&self) -> String {
        "managed".to_string()
    }
}

/// A usage meter that keeps every recorded sample, so a test can assert what the
/// copilot's turn metered (or that a zero-usage turn metered nothing).
#[derive(Default)]
pub(crate) struct RecordingUsageMeter {
    samples: StdMutex<Vec<UsageSample>>,
}

impl RecordingUsageMeter {
    pub(crate) fn samples(&self) -> Vec<UsageSample> {
        self.samples.lock().unwrap().clone()
    }
}

#[async_trait]
impl UsageMeter for RecordingUsageMeter {
    async fn record(&self, _company: &CompanyId, sample: &UsageSample) -> crate::Result<()> {
        self.samples.lock().unwrap().push(sample.clone());
        Ok(())
    }

    async fn query(
        &self,
        _company: &CompanyId,
        _since_millis: u64,
    ) -> crate::Result<Vec<UsageSample>> {
        Ok(self.samples())
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

pub(crate) const MANIFEST: &str = r#"
[company]
name = "Acme"

[[agent]]
id = "maya"
role = "Writer"
tools = ["docs", "web"]

[policy]
mode = "full"

[tools]
allow = ["docs", "web"]
"#;

pub(crate) fn manifest() -> CompanyManifest {
    toml::from_str(MANIFEST).expect("the fixture manifest parses")
}

pub(crate) struct EmptyUsageMeter;

#[async_trait]
impl UsageMeter for EmptyUsageMeter {
    async fn record(&self, _company: &CompanyId, _sample: &UsageSample) -> crate::Result<()> {
        Ok(())
    }

    async fn query(
        &self,
        _company: &CompanyId,
        _since_millis: u64,
    ) -> crate::Result<Vec<UsageSample>> {
        Ok(Vec::new())
    }
}

/// A valid answer: a two-node scheduled graph whose agent is on the roster.
pub(crate) const VALID_GRAPH: &str = r#"```json
{
  "automatable": true,
  "summary": "Email the weekly digest every Monday",
  "workflow": {
    "id": "the-model-should-not-pick-this",
    "name": "Weekly digest",
    "description": "Draft and send the weekly digest.",
    "nodes": [
      { "id": "start", "kind": "trigger", "name": "Every Monday", "schedule": "0 9 * * 1" },
      { "id": "draft", "kind": "agent", "name": "Draft it", "agent": "maya", "summary": "write the digest" }
    ],
    "edges": [{ "from": "start", "to": "draft" }]
  }
}
```"#;
