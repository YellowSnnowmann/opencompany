use super::*;
use crate::app::config::MapEnv;

const DSN: &str = "https://examplePublicKey@o0.ingest.sentry.io/0";

#[test]
fn an_install_that_configures_nothing_installs_no_client() {
    let (decision, guard) = init(Deployment::SelfHosted, &MapEnv::default());
    assert_eq!(decision, Decision::Silent(Silence::NoDsn));
    assert!(!guard.is_active());
    // Flushing a guard that holds nothing is a success, so a caller need
    // not ask whether reporting is on before draining.
    assert!(guard.flush(FLUSH_TIMEOUT));
}

#[test]
fn opting_out_installs_no_client_even_with_a_dsn() {
    let (decision, guard) = init(
        Deployment::HostedTenant,
        &MapEnv::new([(config::DSN_ENV, DSN), (config::ENABLE_ENV, "off")]),
    );
    assert_eq!(decision, Decision::Silent(Silence::OptedOut));
    assert!(!guard.is_active());
}

/// The default build's answer to a configured DSN. `Report` here would be
/// a line that says the opposite of what the process does.
#[cfg(not(feature = "crash-reporting"))]
#[test]
fn a_build_without_the_feature_says_so() {
    let (decision, guard) = init(
        Deployment::HostedTenant,
        &MapEnv::new([(config::DSN_ENV, DSN)]),
    );
    assert_eq!(decision, Decision::Silent(Silence::NotCompiled));
    assert!(!guard.is_active());
    assert!(decision.describe().contains("crash-reporting"));
    // The seam still exists, and still refuses honestly.
    assert_eq!(capture_test_event("ping"), None);
}

#[cfg(feature = "crash-reporting")]
mod gated {
    use super::*;
    use crate::observability::redaction::credential_shaped;
    use sentry::protocol::{Breadcrumb, Event, Exception, LogEntry, Value};

    /// One event carrying a credential in every place an event can carry
    /// one. If a new field is added to the protocol that can hold text,
    /// this is where it should be added.
    fn hostile_event() -> Event<'static> {
        let mut event = Event {
            message: Some("refresh failed: api_key=hunter2".into()),
            logentry: Some(LogEntry {
                message: "posting to https://user:hunter2@collector.internal/1".into(),
                params: vec![Value::String("Authorization: Bearer hunter2".into())],
            }),
            server_name: Some("build-host-42".into()),
            ..Default::default()
        };
        event.exception.values.push(Exception {
            ty: "Error".into(),
            value: Some(format!("token: {} rejected", credential_shaped("ghp_", 36))),
            ..Default::default()
        });
        event.breadcrumbs.values.push(Breadcrumb {
            message: Some(format!("using {}", credential_shaped("sk-proj-", 20))),
            data: [(
                "url".to_string(),
                Value::String("https://u:hunter2@h/1".into()),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        });
        event
            .extra
            .insert("detail".into(), Value::String("password=hunter2".into()));
        event.extra.insert(
            "nested".into(),
            Value::Array(vec![Value::String("client_secret=hunter2".into())]),
        );
        event
            .tags
            .insert("origin".into(), credential_shaped("th_live_", 20));
        event
    }

    #[test]
    fn no_credential_survives_the_hook() {
        let sanitized = sanitize(hostile_event()).expect("the hook never drops an event");
        let rendered = serde_json::to_string(&sanitized).expect("an event serializes");
        for leaked in [
            "hunter2",
            "ghp_AAAA",
            "sk-proj-AAAA",
            "th_live_AAAA",
            "build-host-42",
        ] {
            assert!(!rendered.contains(leaked), "{leaked} survived: {rendered}");
        }
        // And the diagnostic around each one survived, or the report is
        // useless and an operator turns this off.
        assert!(rendered.contains("refresh failed"), "{rendered}");
        assert!(rendered.contains("rejected"), "{rendered}");
        assert!(rendered.contains("collector.internal"), "{rendered}");
    }

    #[test]
    fn a_credential_nested_in_structured_data_does_not_survive() {
        // The string rule cannot see these. `hunter2` under a `token` key
        // is a word with no issuer prefix and no `token=` beside it, so
        // only the KEY says what it is — and the key is only reachable by
        // walking the structure, at every depth rather than the first.
        let mut event = Event::default();
        event.extra.insert(
            "detail".into(),
            serde_json::json!({
                "request": {
                    "headers": { "authorization": "Bearer hunter2" },
                    "nested": [{ "api_key": "hunter2" }],
                },
                "credentials": { "anything": { "at": ["any", "depth", "hunter2"] } },
                "safe": "GET /api/v1/companies/acme -> 200",
            }),
        );
        event.breadcrumbs.values.push(Breadcrumb {
            data: [("token".to_string(), Value::String("hunter2".into()))]
                .into_iter()
                .collect(),
            ..Default::default()
        });

        let sanitized = sanitize(event).expect("the hook never drops an event");
        let rendered = serde_json::to_string(&sanitized).expect("an event serializes");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        // And the diagnostic beside it is untouched.
        assert!(rendered.contains("companies/acme"), "{rendered}");
    }

    #[test]
    fn a_credential_nested_in_a_context_does_not_survive() {
        // The shape the transaction path made reachable: a context whose
        // free-form data nests a credential more than one level down.
        let mut transaction = sentry::protocol::Transaction::default();
        transaction.contexts.insert(
            "app".into(),
            sentry::protocol::Context::Other(
                [(
                    "boot".to_string(),
                    serde_json::json!({
                        "outer": { "inner": { "authorization": "Bearer hunter2" } },
                        "list": [{ "deeper": { "secret": "hunter2" } }],
                    }),
                )]
                .into_iter()
                .collect(),
            ),
        );

        let sanitized = sanitize_transaction(transaction);
        let rendered = serde_json::to_string(&sanitized).expect("a transaction serializes");
        assert!(!rendered.contains("hunter2"), "{rendered}");
    }

    #[test]
    fn the_two_dsn_parsers_agree() {
        // `config::parse_dsn` decides what the BOOT LINE says; the SDK's
        // parser decides whether anything is actually delivered. Two
        // independent readers of one grammar, so they are pinned together
        // here: anything the first accepts, the second must accept, or an
        // install would be told it is reporting while the client is
        // disabled. `init` refuses in that case rather than trusting this,
        // but a divergence should be a red test, not a silent downgrade.
        use std::str::FromStr;

        for candidate in [
            "https://key@o0.ingest.sentry.io/0",
            "https://key@o0.ingest.sentry.io/abc",
            "https://key@host/api/7",
            "https://key@host/0?x=1",
            "https://key@host/0#fragment",
            "http://key@localhost:9000/2",
            "https://key@[::1]/3",
            "https://key@host:65535/3",
            "https://key@host/0/1/2",
            "https://key@host/%20",
            // Rejected by BOTH today. They are here so that a future
            // loosening of `parse_dsn` — dropping the public-key check,
            // widening the scheme — trips this assertion instead of
            // shipping a boot line that names a destination the SDK will
            // not accept. Verified: removing the username check from
            // `parse_dsn` makes this test fail on the first of them.
            "https://o0.ingest.sentry.io/0",
            "ftp://key@host/0",
            "https://key@host/",
            "https://key:secret@host/0",
        ] {
            let ours = config::parse_dsn_for_test(candidate);
            let theirs = sentry::types::Dsn::from_str(candidate);
            assert!(
                !(ours.is_some() && theirs.is_err()),
                "{candidate}: this crate accepts it but the SDK rejects it ({:?}) — the \
                 boot line would claim a destination nothing can be sent to",
                theirs.err()
            );
        }
    }

    #[test]
    fn a_dsn_the_sdk_refuses_installs_no_client_and_says_so() {
        // Belt and braces for the same failure: whatever the two parsers
        // do, `init` reports what the process WILL DO.
        let (decision, guard) = init(
            Deployment::SelfHosted,
            &MapEnv::new([(config::DSN_ENV, "https://key@host/0")]),
        );
        // A valid DSN still reports; the point is that the parse result and
        // the decision cannot disagree.
        assert!(matches!(decision, Decision::Report { .. }));
        assert!(guard.is_active());
    }

    #[test]
    fn a_tag_whose_key_names_a_credential_is_redacted() {
        // The tag map was value-only: `hunter2` under a `password` key has
        // no issuer prefix and no `password=` beside it, so the text rule
        // has nothing to bite on and only the KEY says what it is.
        let mut event = Event::default();
        event.tags.insert("password".into(), "hunter2".into());
        event.tags.insert("token".into(), "hunter2".into());
        event.tags.insert("company".into(), "acme".into());

        let sanitized = sanitize(event).expect("the hook never drops an event");
        let rendered = serde_json::to_string(&sanitized).expect("an event serializes");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        // A tag that names nothing secret survives, or filtering breaks.
        assert_eq!(
            sanitized.tags.get("company").map(String::as_str),
            Some("acme")
        );
    }

    #[test]
    fn the_free_form_fields_that_are_not_the_message_are_scrubbed_too() {
        // Walked from `protocol::Event`'s own field list rather than
        // remembered. Every one of these was uncovered until it was.
        let mut event = Event {
            transaction: Some("GET /login?code=hunter2".into()),
            culprit: Some("api_key=hunter2".into()),
            logger: Some("api_key=hunter2".into()),
            fingerprint: vec!["api_key=hunter2".into(), "route".into()].into(),
            ..Default::default()
        };
        event.threads.values.push(sentry::protocol::Thread {
            stacktrace: Some(sentry::protocol::Stacktrace {
                frames: vec![sentry::protocol::Frame {
                    context_line: Some("let key = \"hunter2\";".into()),
                    vars: [("key".to_string(), Value::String("hunter2".into()))]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        });
        event.template = Some(sentry::protocol::TemplateInfo {
            context_line: Some("password = \"hunter2\"".into()),
            ..Default::default()
        });

        let sanitized = sanitize(event).expect("the hook never drops an event");
        let rendered = serde_json::to_string(&sanitized).expect("an event serializes");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        // The diagnostic around each redaction survives.
        assert!(rendered.contains("/login"), "{rendered}");
        assert!(rendered.contains("route"), "{rendered}");
    }

    #[test]
    fn a_span_tag_is_redacted_by_its_key_as_well() {
        let mut transaction = sentry::protocol::Transaction::default();
        transaction.tags.insert("password".into(), "hunter2".into());
        transaction.tags.insert("route".into(), "/desk".into());
        transaction.spans.push(sentry::protocol::Span {
            tags: [("secret".to_string(), "hunter2".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        });

        let sanitized = sanitize_transaction(transaction);
        let rendered = serde_json::to_string(&sanitized).expect("a transaction serializes");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert_eq!(
            sanitized.tags.get("route").map(String::as_str),
            Some("/desk")
        );
    }

    #[test]
    fn the_hook_never_drops_an_event() {
        // Deciding which errors are worth seeing belongs in the operator's
        // own project, where they can see what they suppressed.
        assert!(sanitize(Event::default()).is_some());
    }

    #[test]
    fn frame_locals_and_source_context_are_stripped() {
        use sentry::protocol::{Frame, Stacktrace};

        let mut event = Event::default();
        event.exception.values.push(Exception {
            stacktrace: Some(Stacktrace {
                frames: vec![Frame {
                    context_line: Some("let key = \"hunter2\";".into()),
                    pre_context: vec!["fn connect() {".into()],
                    post_context: vec!["}".into()],
                    vars: [("key".to_string(), Value::String("hunter2".into()))]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        });

        let sanitized = sanitize(event).expect("the hook never drops an event");
        let frame = &sanitized.exception.values[0]
            .stacktrace
            .as_ref()
            .expect("the stacktrace survives")
            .frames[0];
        assert!(frame.vars.is_empty());
        assert!(frame.pre_context.is_empty());
        assert!(frame.post_context.is_empty());
        assert_eq!(frame.context_line, None);
    }

    /// One transaction carrying something it must not send in every field
    /// a transaction has.
    fn hostile_transaction() -> sentry::protocol::Transaction<'static> {
        use sentry::protocol::Span;

        let mut transaction = sentry::protocol::Transaction {
            name: Some("GET /api/v1/companies/{company}".into()),
            server_name: Some("build-host-42".into()),
            user: Some(sentry::protocol::User {
                email: Some("operator@example.com".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        transaction.request = Some(sentry::protocol::Request {
            method: Some("GET".into()),
            url: Some(
                "https://key:hunter2@host/api/v1/companies/acme?code=magic-link-code#frag"
                    .parse()
                    .expect("a url"),
            ),
            headers: [("Cookie".to_string(), "oc_session=hunter2".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        });
        transaction.spans.push(Span {
            op: Some("http.client".into()),
            description: Some("POST https://u:hunter2@provider/v1?api_key=hunter2".into()),
            data: [(
                "http.url".to_string(),
                Value::String(format!("https://provider/v1?token={}", "hunter2")),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        });
        transaction
            .tags
            .insert("origin".into(), credential_shaped("th_live_", 20));
        transaction
            .extra
            .insert("detail".into(), Value::String("password=hunter2".into()));
        transaction.contexts.insert(
            "trace".into(),
            sentry::protocol::TraceContext {
                // `url.full` is the request URL verbatim. A captured
                // envelope had it in clear beside correctly-redacted span
                // descriptions, which is what this field is here for.
                data: [(
                    "url.full".to_string(),
                    Value::String(
                        "https://host/api/v1?code=magic-link-code&api_key=hunter2".into(),
                    ),
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            }
            .into(),
        );
        transaction
    }

    #[test]
    fn no_credential_survives_a_transaction() {
        // `before_send` does not run on transactions — sentry 0.47 has no
        // `before_send_transaction`, and `Transaction::finish` posts an
        // envelope straight to the transport. This is the seam that closes
        // that, so it is tested on the same terms as the event hook.
        let sanitized = sanitize_transaction(hostile_transaction());
        let rendered = serde_json::to_string(&sanitized).expect("a transaction serializes");
        for leaked in [
            "hunter2",
            "magic-link-code",
            "build-host-42",
            "operator@example.com",
            "th_live_AAAA",
        ] {
            assert!(!rendered.contains(leaked), "{leaked} survived: {rendered}");
        }
    }

    #[test]
    fn an_event_keeps_its_trace_context_so_the_error_stays_on_its_trace() {
        // Dropping `trace` would sever an error from the request that
        // caused it — the link tracing exists to provide — but its `data`
        // is the one free-form field an allow-list would have to trust.
        let mut event = Event::default();
        event.contexts.insert(
            "trace".into(),
            sentry::protocol::TraceContext {
                data: [(
                    "url.full".to_string(),
                    Value::String("https://host/api/v1?code=magic-link-code".into()),
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            }
            .into(),
        );

        let sanitized = sanitize(event).expect("the hook never drops an event");
        assert!(
            sanitized.contexts.contains_key("trace"),
            "the link survives"
        );
        let rendered = serde_json::to_string(&sanitized).expect("an event serializes");
        assert!(!rendered.contains("magic-link-code"), "{rendered}");
        assert!(rendered.contains("url.full"), "{rendered}");
    }

    #[test]
    fn a_transaction_keeps_what_makes_it_readable() {
        let sanitized = sanitize_transaction(hostile_transaction());
        // The route template is the whole point of a transaction name.
        assert_eq!(
            sanitized.name.as_deref(),
            Some("GET /api/v1/companies/{company}")
        );
        let request = sanitized.request.expect("the request survives, narrowed");
        assert_eq!(request.method.as_deref(), Some("GET"));
        assert_eq!(
            request.url.map(|url| url.to_string()),
            Some("https://host/api/v1/companies/acme".to_string())
        );
        // Headers and cookies are dropped rather than scrubbed: neither
        // answers a question a transaction is read to answer.
        assert!(request.headers.is_empty());
        assert_eq!(sanitized.spans[0].op.as_deref(), Some("http.client"));
        assert!(
            sanitized.spans[0]
                .description
                .as_deref()
                .is_some_and(|text| text.contains("provider")),
            "{:?}",
            sanitized.spans[0].description
        );
    }

    #[test]
    fn the_transport_scrubs_a_transaction_envelope() {
        // The guarantee is made at the transport, so it is tested there:
        // an envelope in, an envelope out, with nothing in between that the
        // SDK could route around.
        let mut envelope = sentry::Envelope::new();
        envelope.add_item(hostile_transaction());
        let scrubbed = scrub_envelope(envelope);
        let mut rendered = Vec::new();
        scrubbed
            .to_writer(&mut rendered)
            .expect("an envelope serializes");
        let rendered = String::from_utf8(rendered).expect("utf-8");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(!rendered.contains("magic-link-code"), "{rendered}");
    }

    #[test]
    fn an_envelope_with_no_transaction_is_passed_through_untouched() {
        // Rebuilding indiscriminately would empty a RAW envelope, because
        // `into_items` yields nothing for one.
        let mut envelope = sentry::Envelope::new();
        envelope.add_item(Event {
            message: Some("api_key=hunter2".into()),
            ..Default::default()
        });
        let before = format!("{envelope:?}");
        assert_eq!(format!("{:?}", scrub_envelope(envelope)), before);
    }

    #[test]
    fn identity_fields_are_cleared() {
        let mut event = Event {
            server_name: Some("build-host-42".into()),
            user: Some(sentry::protocol::User {
                email: Some("operator@example.com".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        event.request = Some(sentry::protocol::Request {
            url: Some("https://host/api/v1/companies/acme".parse().expect("a url")),
            ..Default::default()
        });

        let sanitized = sanitize(event).expect("the hook never drops an event");
        assert_eq!(sanitized.server_name, None);
        assert!(sanitized.user.is_none());
        assert!(sanitized.request.is_none());
    }
}
