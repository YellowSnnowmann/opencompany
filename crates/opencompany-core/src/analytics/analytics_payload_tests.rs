//! The guarantee, tested: no analytics payload can carry caller-supplied text.
//!
//! These run at **default features**, in the build every lane compiles, because
//! the leak they guard against would not be introduced in the gated transport —
//! it would be introduced at a call site, in a payload field, on any build.

use super::*;
use crate::analytics::types::{OpaqueId, provider_slug, sample_kind_slug};
use crate::app::deployment::Deployment;
use crate::error::OpenCompanyError;
use crate::metering::ModelSlug;
use crate::ports::brain::{Cognition, UsageMetering};
use crate::ports::usage::{SampleKind, UsageSample};

/// Strings that must never appear in a payload, standing in for the four kinds
/// of content #1739 names: a customer's brand, an operator's own text, a host
/// path, and an address.
const HOSTILE: &[&str] = &[
    "AcmeCorp Holdings",
    "please summarise the merger memo",
    "/Users/someone/companies/acme/secrets",
    "founder@acme.example",
    "sk-not-a-real-key",
    "project-titan",
];

fn envelope() -> Envelope {
    Envelope::new(
        OpaqueId::instance("0123456789abcdef0123456789abcdef"),
        Deployment::HostedTenant,
        Cognition {
            path: "harness",
            provider: "openrouter",
            model: None,
            metering: UsageMetering::PerTurn,
        },
    )
}

/// Every event kind, each built from the most hostile input its constructor
/// will accept.
fn hostile_events() -> Vec<Event> {
    let sample = UsageSample {
        at_millis: 1,
        agent: "AcmeCorp Holdings".into(),
        provider: "mcp:project-titan".into(),
        input_tokens: 10,
        output_tokens: 4,
        cached_input_tokens: 0,
        cost_usd: 0.25,
        kind: SampleKind::Inference,
        run_id: Some("/Users/someone/companies/acme/secrets".into()),
        // A BYOK tenant names its model whatever it likes, and what it likes is
        // often its own brand. `ModelSlug::classify` folds it to `other` at the
        // harness; this sample proves the analytics payload cannot undo that.
        model: Some(ModelSlug::classify("AcmeCorp Holdings project-titan")),
    };

    let err = OpenCompanyError::Store(
        "could not write /Users/someone/companies/acme/secrets for founder@acme.example".into(),
    );

    // A second sample whose provider is neither MCP-prefixed nor a known slug:
    // it must reach the `other` fallback rather than the `mcp` branch. Without
    // it the whole fallback path went untested by the two assertions below —
    // found by mutating `provider_slug`'s `_` arm into a `Box::leak`, which the
    // MCP-prefixed sample alone did not notice.
    let unknown_provider = UsageSample {
        provider: "AcmeCorp Holdings".into(),
        kind: SampleKind::OauthCall,
        model: None,
        ..sample.clone()
    };

    // A third sample on the happy path: a model the vocabulary *can* name. The
    // two above only ever reach `other`, so on their own they would let the
    // `model` property be deleted outright without a payload changing.
    let named_model = UsageSample {
        model: Some(ModelSlug::classify("anthropic/claude-sonnet-4-6")),
        ..sample.clone()
    };

    vec![
        Event::InstanceStarted {
            companies: 3,
            storage: "mongodb",
            setup_complete: true,
        },
        Event::TurnFinished {
            trigger: Trigger::OperatorMessage,
            outcome: Outcome::Failed,
            failure: Some(FailureCode::of(&err)),
            duration_ms: 1_234,
            effects_executed: 2,
            approvals_parked: 1,
        },
        Event::metered(&sample),
        Event::metered(&unknown_provider),
        Event::metered(&named_model),
    ]
}

/// **Issue #1739's third acceptance criterion**, from the outside: render every
/// event from hostile inputs and assert none of that text survives.
///
/// It is the blunt half of the guarantee. The structural half is that
/// [`PropValue`] has no `String` variant at all — but a blunt test is what
/// fails loudly if someone adds one, so both are here.
#[test]
fn a_payload_carries_no_caller_supplied_text() {
    let envelope = envelope();
    for event in hostile_events() {
        // Case-insensitively: a classifier that lowercases what it passes
        // through has still passed it through, and an exact-case search would
        // read that as clean. Found by mutation — the first version of this
        // test missed exactly that.
        let rendered = payload(&envelope, &event).to_string().to_ascii_lowercase();
        for needle in HOSTILE {
            assert!(
                !rendered.contains(&needle.to_ascii_lowercase()),
                "the {} payload leaked {needle:?}: {rendered}",
                event.name()
            );
        }
    }
}

/// Every literal any classifier or enum in this module can produce.
///
/// The vocabulary is enumerated by hand **on purpose**: it is the list a
/// reviewer reads to answer "what can this product send?", and a list derived
/// from the code would answer that question with itself.
fn vocabulary() -> Vec<&'static str> {
    let mut words = vec![
        // Deployment kinds.
        "desktop",
        "self-hosted",
        "hosted-tenant",
        // Outcomes and triggers.
        "ok",
        "failed",
        "operator-message",
        "task-dispatch",
        "approval-continuation",
        "agent-reply",
        // Failures.
        "none",
        "store",
        "manifest",
        "refused",
        "not-found",
        "cognition",
        "workflow",
        "config",
        "upstream",
        // Metering.
        "per-turn",
        "per-cycle",
        // Cognition paths (`ports::brain::Cognition::path`).
        "harness",
        "hosted",
        "echo",
        "sidecar",
        "custom",
        // Storage kinds.
        "fs",
        "sqlite",
        "mongodb",
        // The catch-all.
        types::OTHER,
    ];
    // Provider slugs and sample kinds, taken from the classifiers themselves so
    // a new one cannot be added without appearing here.
    for provider in [
        "openrouter",
        "subscription",
        "managed",
        "ollama",
        "byok",
        "openai_compatible",
        "echo",
        "hosted",
        "github",
        "google",
        "slack",
        "notion",
        "composio",
        "unknown",
        "mcp:anything",
        "something nobody anticipated",
    ] {
        words.push(provider_slug(provider));
    }
    for kind in [
        SampleKind::Inference,
        SampleKind::OauthCall,
        SampleKind::SearchCall,
        SampleKind::PlanningCall,
        SampleKind::JudgeCall,
        SampleKind::TriageCall,
        SampleKind::SetupCall,
        SampleKind::AuthoringCall,
        SampleKind::SelectorCall,
    ] {
        words.push(sample_kind_slug(kind));
    }
    // Model slugs, taken from `ModelSlug::classify` for the same reason the
    // provider slugs are taken from `provider_slug`: this module owns no model
    // vocabulary of its own — it forwards the one `crate::metering::model`
    // already folded a raw name onto, and a second hand-written copy here would
    // be a second place for the two to disagree.
    for model in [
        "anthropic/claude-sonnet-4-6",
        "chat-v1",
        "AcmeCorp Holdings project-titan",
    ] {
        words.push(ModelSlug::classify(model).as_str());
    }
    words
}

/// The structural claim, asserted rather than described: **every string value in
/// every payload is either the opaque identity, a platform fact fixed at compile
/// time, or a word from the vocabulary above.**
///
/// This is the test that fails the moment someone adds a `String`-carrying
/// property. A free-form value has, by definition, no entry in a hand-written
/// list.
#[test]
fn every_string_in_a_payload_comes_from_the_compiled_vocabulary() {
    let envelope = envelope();
    let vocabulary = vocabulary();

    // The three values that are strings but are not vocabulary: the opaque id,
    // and the two platform facts `std::env::consts` supplies. All three are
    // fixed for the life of the process and none originates with a user.
    let allowed_platform = [
        envelope.id.as_str().to_string(),
        envelope.app_version.to_string(),
        envelope.os.to_string(),
        envelope.arch.to_string(),
    ];

    for event in hostile_events() {
        let rendered = payload(&envelope, &event);
        assert_eq!(rendered["type"], "track", "the OpenPanel operation");
        assert_eq!(rendered["payload"]["name"], event.name());
        assert_eq!(rendered["payload"]["profileId"], envelope.id.as_str());

        let properties = rendered["payload"]["properties"]
            .as_object()
            .expect("properties is an object");
        assert!(!properties.is_empty(), "an event with no properties");

        // Walked over the **whole** body rather than only the property bag.
        // OpenPanel's shape moved the identity and the event name out of the
        // properties and up beside them (`profileId`, `name`), so a check that
        // still looked only at `properties` would have stopped covering two of
        // the three strings that were always the point.
        let mut strings = Vec::new();
        collect_strings(&rendered, String::new(), &mut strings);
        assert!(!strings.is_empty(), "a payload with no strings at all");

        for (path, text) in strings {
            // The one string that is neither a literal nor an id: the event
            // time OpenPanel reads out of `__timestamp`. Asserted by **shape**
            // rather than waved through, so "every string here is an id, a
            // platform constant or a hand-written word" keeps its one exception
            // pinned to a fixed grammar that no runtime value could satisfy by
            // accident.
            if path == "payload.properties.__timestamp" {
                assert!(
                    is_rfc3339_utc_seconds(&text),
                    "the event time must be RFC-3339 UTC to the second, and nothing \
                     else, or it is a free-form string in a payload: {text:?}"
                );
                continue;
            }
            let known = vocabulary.contains(&text.as_str())
                || allowed_platform.iter().any(|allowed| allowed == &text)
                || text == "track"
                || text == event.name();
            assert!(
                known,
                "the field {path:?} carried the string {text:?}, which is not in this \
                 module's compiled vocabulary. Either it is a leak, or a new literal was \
                 added without recording it in `vocabulary()`."
            );
        }
    }
}

/// Exactly `YYYY-MM-DDTHH:MM:SSZ`, and nothing else.
///
/// Written out rather than approximated with a `len()` check, because the
/// property it is standing in for is "this field cannot carry text", and a
/// looser check would let one through.
fn is_rfc3339_utc_seconds(text: &str) -> bool {
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

/// Every string **value** in `value`, with a dotted path to each, so a failure
/// names the field rather than only the leaked text. Keys are not collected:
/// they are `&'static str` literals written in this repository by construction,
/// which is the same argument [`PropValue`] rests on.
fn collect_strings(value: &serde_json::Value, path: String, out: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::String(text) => out.push((path, text.clone())),
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                collect_strings(child, child_path, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_strings(child, format!("{path}[{index}]"), out);
            }
        }
        _ => {}
    }
}
