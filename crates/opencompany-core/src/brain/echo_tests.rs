use super::*;
use crate::ports::types::{
    ApprovalId, CompanyId, ContextOp, ContextOpResult, EffectDisposition, ToolCall, ToolResult,
};

/// A minimal host that records emitted effects and auto-executes them.
#[derive(Default)]
struct RecordingHost {
    effects: std::sync::Mutex<Vec<Effect>>,
}

#[async_trait]
impl CycleHost for RecordingHost {
    async fn call_tool(&self, _call: ToolCall) -> Result<ToolResult> {
        Ok(ToolResult {
            ok: true,
            output: serde_json::Value::Null,
        })
    }

    async fn context_op(&self, _op: ContextOp) -> Result<ContextOpResult> {
        Ok(ContextOpResult::Text(String::new()))
    }

    async fn emit_effect(&self, effect: Effect) -> Result<EffectDisposition> {
        self.effects.lock().unwrap().push(effect);
        Ok(EffectDisposition::Executed)
    }

    async fn park_effect(&self, effect: Effect) -> Result<ApprovalId> {
        self.effects.lock().unwrap().push(effect);
        Ok(ApprovalId::new("appr-parked"))
    }
}

fn request(events: Vec<CompanyEvent>) -> CycleRequest {
    CycleRequest {
        cycle_id: "cycle-1".to_string(),
        company_id: CompanyId::new("acme"),
        events,
        event_seqs: Vec::new(),
        policy: None,
    }
}

#[tokio::test]
async fn echoes_operator_message_and_records_trace() {
    let brain = EchoBrain::new();
    let host = RecordingHost::default();
    let result = brain
        .run_cycle(
            request(vec![CompanyEvent::OperatorMessage {
                mentions: Vec::new(),
                parent: None,
                text: "hi".into(),
                by: None,
                chat: None,
                deliverable: None,
                attachments: Vec::new(),
            }]),
            &host,
        )
        .await
        .unwrap();

    assert_eq!(result.channel_responses.len(), 1);
    assert_eq!(result.channel_responses[0].channel, "operator");
    assert_eq!(result.channel_responses[0].text, "You said: hi");
    assert_eq!(result.new_traces.len(), 1);
    // The heartbeat effect flowed through the host.
    assert_eq!(host.effects.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn guarantees_a_response_with_no_events() {
    let brain = EchoBrain::new();
    let host = RecordingHost::default();
    let result = brain.run_cycle(request(Vec::new()), &host).await.unwrap();
    assert_eq!(result.channel_responses.len(), 1);
    assert_eq!(result.channel_responses[0].text, "Acknowledged.");
}
