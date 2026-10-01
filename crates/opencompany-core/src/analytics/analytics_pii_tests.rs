//! A no-PII property over **every** event shape, by exhaustive enumeration.
//!
//! `analytics_payload_tests.rs` renders a handful of hostile samples. This is the
//! complement: it walks every `Event` variant crossed with every value of every
//! enum an event can carry, and asserts that every string in every resulting
//! payload is a compiled vocabulary word, the `profileId`, a platform constant,
//! or the `__timestamp` shape — and contains nothing from the machine it ran on.
//!
//! No property-testing dependency: the input space is small and closed (that is
//! the point of the vocabulary), so enumerating it is stronger than sampling it.
//! The `exhaustive_*` helpers match without a wildcard, so adding a variant to
//! any of these enums fails to compile here until it is enumerated.

use super::*;
use crate::analytics::types::{OpaqueId, provider_slug, sample_kind_slug};
use crate::app::deployment::Deployment;
use crate::metering::ModelSlug;
use crate::ports::brain::{Cognition, UsageMetering};
use crate::ports::usage::{SampleKind, UsageSample};

/// The fake client id the leak search looks for. Never a real one.
const FAKE_CLIENT_ID: &str = "fake-pii-client-id-91c2";

const TRIGGERS: [Trigger; 5] = [
    Trigger::OperatorMessage,
    Trigger::TaskDispatch,
    Trigger::ApprovalContinuation,
    Trigger::AgentReply,
    Trigger::Other,
];
const OUTCOMES: [Outcome; 2] = [Outcome::Ok, Outcome::Failed];
const FAILURES: [FailureCode; 9] = [
    FailureCode::Store,
    FailureCode::Manifest,
    FailureCode::Refused,
    FailureCode::NotFound,
    FailureCode::Cognition,
    FailureCode::Workflow,
    FailureCode::Config,
    FailureCode::Upstream,
    FailureCode::Other,
];
const KINDS: [SampleKind; 11] = [
    SampleKind::Inference,
    SampleKind::OauthCall,
    SampleKind::SearchCall,
    SampleKind::PlanningCall,
    SampleKind::JudgeCall,
    SampleKind::TriageCall,
    SampleKind::SetupCall,
    SampleKind::AuthoringCall,
    SampleKind::SelectorCall,
    SampleKind::TitleCall,
    SampleKind::ExtractionCall,
];
const DEPLOYMENTS: [Deployment; 3] = [
    Deployment::Desktop,
    Deployment::SelfHosted,
    Deployment::HostedTenant,
];
const METERINGS: [UsageMetering; 3] = [
    UsageMetering::PerTurn,
    UsageMetering::PerCycle,
    UsageMetering::None,
];

// No wildcard arms: a new variant stops these compiling, which is the prompt to
// extend the arrays above.
fn exhaustive_trigger(t: Trigger) {
    match t {
        Trigger::OperatorMessage
        | Trigger::TaskDispatch
        | Trigger::ApprovalContinuation
        | Trigger::AgentReply
        | Trigger::Other => {}
    }
}
fn exhaustive_failure(f: FailureCode) {
    match f {
        FailureCode::Store
        | FailureCode::Manifest
        | FailureCode::Refused
        | FailureCode::NotFound
        | FailureCode::Cognition
        | FailureCode::Workflow
        | FailureCode::Config
        | FailureCode::Upstream
        | FailureCode::Other => {}
    }
}
fn exhaustive_kind(k: SampleKind) {
    match k {
        SampleKind::Inference
        | SampleKind::OauthCall
        | SampleKind::SearchCall
        | SampleKind::PlanningCall
        | SampleKind::JudgeCall
        | SampleKind::TriageCall
        | SampleKind::SetupCall
        | SampleKind::AuthoringCall
        | SampleKind::SelectorCall
        | SampleKind::TitleCall
        | SampleKind::ExtractionCall => {}
    }
}
fn exhaustive_event(e: &Event) {
    match e {
        Event::InstanceStarted { .. } | Event::TurnFinished { .. } | Event::TurnMetered { .. } => {}
    }
}

/// Every event the enumerated enum values can produce.
fn every_event() -> Vec<Event> {
    TRIGGERS.into_iter().for_each(exhaustive_trigger);
    FAILURES.into_iter().for_each(exhaustive_failure);
    KINDS.into_iter().for_each(exhaustive_kind);

    let mut events = Vec::new();
    for storage in ["fs", "sqlite", "mongodb"] {
        for setup_complete in [false, true] {
            events.push(Event::InstanceStarted {
                companies: 0,
                storage,
                setup_complete,
            });
        }
    }
    for trigger in TRIGGERS {
        for outcome in OUTCOMES {
            for failure in std::iter::once(None).chain(FAILURES.into_iter().map(Some)) {
                events.push(Event::TurnFinished {
                    trigger,
                    outcome,
                    failure,
                    duration_ms: 7,
                    effects_executed: 1,
                    approvals_parked: 1,
                });
            }
        }
    }
    let base = UsageSample {
        at_millis: 1,
        agent: "a private agent name".into(),
        provider: String::new(),
        input_tokens: 1,
        output_tokens: 1,
        cached_input_tokens: 0,
        cost_usd: 0.5,
        kind: SampleKind::Inference,
        run_id: None,
        model: None,
    };
    for kind in KINDS {
        for provider in ["openrouter", "mcp:private-server", "never heard of it"] {
            for model in [None, Some(ModelSlug::classify("a private fine-tune"))] {
                for run_id in [None, Some("a private run id".to_string())] {
                    events.push(Event::metered(&UsageSample {
                        kind,
                        provider: provider.into(),
                        model,
                        run_id,
                        ..base.clone()
                    }));
                }
            }
        }
    }
    events.iter().for_each(exhaustive_event);
    events
}

fn envelopes() -> Vec<Envelope> {
    let mut out = Vec::new();
    for deployment in DEPLOYMENTS {
        for metering in METERINGS {
            for path in ["harness", "hosted", "echo", "sidecar", "custom"] {
                out.push(Envelope::new(
                    OpaqueId::instance("0123456789abcdef0123456789abcdef"),
                    deployment,
                    Cognition {
                        path,
                        provider: "openrouter",
                        model: None,
                        metering,
                    },
                ));
            }
        }
    }
    out
}

/// Every vocabulary word, built from the enums' own `as_str` so a new value
/// cannot be added without appearing here.
fn vocabulary() -> Vec<&'static str> {
    let mut words = vec![
        "track",
        "none",
        "fs",
        "sqlite",
        "mongodb",
        "harness",
        "hosted",
        "echo",
        "sidecar",
        "custom",
        types::OTHER,
    ];
    words.extend(TRIGGERS.map(Trigger::as_str));
    words.extend(OUTCOMES.map(Outcome::as_str));
    words.extend(FAILURES.map(FailureCode::as_str));
    words.extend(KINDS.map(sample_kind_slug));
    words.extend(DEPLOYMENTS.map(Deployment::as_str));
    words.extend(METERINGS.map(types::metering_slug));
    for provider in ["openrouter", "mcp:private-server", "never heard of it"] {
        words.push(provider_slug(provider));
    }
    words.push(ModelSlug::classify("a private fine-tune").as_str());
    words.extend(["instance_started", "turn_finished", "turn_metered"]);
    words
}

fn strings_of(value: &serde_json::Value, out: &mut Vec<(String, String)>, path: &str) {
    match value {
        serde_json::Value::String(text) => out.push((path.to_string(), text.clone())),
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                strings_of(child, out, &format!("{path}.{key}"));
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                strings_of(child, out, path);
            }
        }
        _ => {}
    }
}

/// Machine-local text that must never appear in a payload: the temp dir, the
/// user name and the hostname, each only when long enough that a substring match
/// is meaningful rather than a coincidence.
fn machine_secrets() -> Vec<String> {
    let mut found = vec![
        std::env::temp_dir().to_string_lossy().to_string(),
        FAKE_CLIENT_ID.to_string(),
    ];
    for var in ["USER", "USERNAME", "LOGNAME", "HOSTNAME", "HOST"] {
        if let Ok(value) = std::env::var(var) {
            found.push(value);
        }
    }
    if let Ok(host) = std::fs::read_to_string("/etc/hostname") {
        found.push(host.trim().to_string());
    }
    found.retain(|s| s.len() >= 4);
    found
}

/// The property: across the whole enumerated input space, no payload carries a
/// string that is not vocabulary, identity, a platform constant or the time
/// shape, and none contains machine-local text.
#[test]
fn analytics_no_pii_in_any_payload_over_every_event_and_enum_value() {
    let vocabulary = vocabulary();
    let secrets = machine_secrets();
    let events = every_event();
    let envelopes = envelopes();
    assert!(
        events.len() >= 238,
        "the enumeration shrank: {}",
        events.len()
    );

    for envelope in &envelopes {
        let platform = [
            envelope.id.as_str().to_string(),
            envelope.app_version.to_string(),
            envelope.os.to_string(),
            envelope.arch.to_string(),
        ];
        for event in &events {
            let body = payload(envelope, event);
            let mut strings = Vec::new();
            strings_of(&body, &mut strings, "");
            for (path, text) in strings {
                if path == ".payload.properties.__timestamp" {
                    assert!(
                        is_timestamp_shape(&text),
                        "{path} is not an RFC-3339 UTC timestamp: {text:?}"
                    );
                    continue;
                }
                assert!(
                    vocabulary.contains(&text.as_str()) || platform.contains(&text),
                    "{path} carried {text:?} for {event:?}, which is neither a compiled \
                     vocabulary word, the profileId nor a platform constant"
                );
            }
            let rendered = body.to_string();
            for secret in &secrets {
                assert!(
                    !rendered.contains(secret.as_str()),
                    "a payload contained machine-local text {secret:?}: {rendered}"
                );
            }
        }
    }
}

fn is_timestamp_shape(text: &str) -> bool {
    let shape = b"dddd-dd-ddTdd:dd:ddZ";
    text.len() == shape.len()
        && text
            .bytes()
            .zip(shape)
            .all(|(byte, expected)| match expected {
                b'd' => byte.is_ascii_digit(),
                other => byte == *other,
            })
}
