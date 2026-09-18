use super::*;
use serde_json::json;

use crate::ports::EventSeq;
use crate::ports::users::UserRole;
use crate::server::graphql::auth::UserPrincipal;

use super::test_support::*;

/// An `@alice-smith` ACP prompt must badge alice exactly as a console
/// message would: the ACP surface is just another operator ingress, and the
/// durable notification is what lets an offline person see the mention at
/// all.
#[tokio::test]
async fn a_prompt_mention_files_the_durable_notification() {
    let home = tempfile::Builder::new()
        .prefix("oc-acp-mention-")
        .tempdir()
        .expect("tempdir");
    let state = acp_state(home.path()).await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).expect("company");

    // Two people: the operator driving the prompt, and the person it names.
    let admin = seed_user(&state, &company, "u-admin", "Admin Person").await;
    let alice = seed_user(&state, &company, "u-alice", "Alice Smith").await;
    let auth = GqlAuth::User(UserPrincipal {
        company: company.clone(),
        user_id: admin,
        email: "admin@example.test".to_string(),
        role: UserRole::Admin,
        must_change_password: false,
        session_token_hash: "hash".to_string(),
        credential: crate::ports::SessionKind::Browser,
    });

    state
        .acp_sessions()
        .open(
            "conn-1",
            &owner(&auth),
            crate::server::acp::AcpSession {
                id: "s-1".to_string(),
                company: company.clone(),
                chat: "engineering".to_string(),
                agent_id: None,
            },
            crate::ports::now_millis(),
        )
        .expect("open session");

    let result = prompt(
        &state,
        &auth,
        &json!({
            "sessionId": "s-1",
            "prompt": [
                { "type": "text", "text": "@alice-smith please review the invoice" },
            ],
            "_meta": { "opencompany/connectionId": "conn-1" },
        }),
    )
    .await;
    assert!(result.is_ok(), "prompt failed: {result:?}");

    // The durable half of the mention: the person named gets a row they can
    // badge, placed in the channel the prompt ran in.
    let rows = runtime
        .notifications()
        .list(&company, &alice)
        .await
        .expect("list");
    assert_eq!(rows.len(), 1, "an @alice prompt must badge alice");
    assert_eq!(rows[0].notification.kind, "mention");
    assert_eq!(rows[0].notification.context.as_deref(), Some("engineering"));
    // And the author is not badged for their own prompt.
    let admin_rows = runtime
        .notifications()
        .list(&company, "u-admin")
        .await
        .expect("list");
    assert!(admin_rows.is_empty(), "{admin_rows:?}");
}

/// B-101, on the ACP ingress (codex P2): a `@writer` that names both the
/// `writer` teammate and the `writer` desk must be refused and reported,
/// exactly as the REST chat path's `accept_chat_turn` does it. Before this
/// fix the ACP `session/prompt` handler called the plain `resolve_mentions`
/// and never posted the ambiguity note, so an ACP client (e.g. Zed) sending
/// an ambiguous `@name` pinged nobody with no durable refusal anywhere.
#[tokio::test]
async fn a_prompt_ambiguous_mention_is_reported_in_the_channel() {
    let home = tempfile::Builder::new()
        .prefix("oc-acp-ambiguous-")
        .tempdir()
        .expect("tempdir");
    let state = acp_state(home.path()).await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).expect("company");
    let admin = seed_user(&state, &company, "u-admin", "Admin Person").await;
    let auth = GqlAuth::User(UserPrincipal {
        company: company.clone(),
        user_id: admin,
        email: "admin@example.test".to_string(),
        role: UserRole::Admin,
        must_change_password: false,
        session_token_hash: "hash".to_string(),
        credential: crate::ports::SessionKind::Browser,
    });

    state
        .acp_sessions()
        .open(
            "conn-1",
            &owner(&auth),
            crate::server::acp::AcpSession {
                id: "s-1".to_string(),
                company: company.clone(),
                chat: "engineering".to_string(),
                agent_id: None,
            },
            crate::ports::now_millis(),
        )
        .expect("open session");

    let result = prompt(
        &state,
        &auth,
        &json!({
            "sessionId": "s-1",
            "prompt": [
                { "type": "text", "text": "@writer can you draft the autumn brief?" },
            ],
            "_meta": { "opencompany/connectionId": "conn-1" },
        }),
    )
    .await;
    assert!(result.is_ok(), "prompt failed: {result:?}");

    // The durable refusal notice: the same `AgentReply` the REST chat path
    // posts, attributed to the runtime and landing in the channel the
    // prompt ran in.
    let events = runtime
        .events()
        .read_from(&company, EventSeq::new(0), usize::MAX)
        .await
        .expect("read events");
    let advisories: Vec<(String, String)> = events
        .into_iter()
        .filter_map(|stored| match stored.event {
            CompanyEvent::AgentReply {
                agent_id,
                chat_id,
                text,
                ..
            } if agent_id == crate::ports::SYSTEM_AUTHOR => Some((chat_id, text)),
            _ => None,
        })
        .collect();
    assert_eq!(
        advisories.len(),
        1,
        "exactly one ambiguity note, cross-ingress: {advisories:?}"
    );
    let (chat, text) = &advisories[0];
    assert_eq!(chat, "engineering");
    assert!(text.contains("@writer"), "names the literal typed: {text}");
    assert!(
        text.contains("pinged nobody"),
        "states what happened: {text}"
    );
}

/// A runtime being replaced refuses the prompt *before* it is journaled
/// (codex P2): a message appended and then rejected would stay in the
/// transcript with nothing that will ever answer it — the ordering the
/// REST chat path holds via `accept_chat_turn`.
#[tokio::test]
async fn a_quiesced_runtime_refuses_prompt_before_journaling() {
    let home = tempfile::Builder::new()
        .prefix("oc-acp-quiesce-")
        .tempdir()
        .expect("tempdir");
    let state = acp_state(home.path()).await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).expect("company");
    let admin = seed_user(&state, &company, "u-admin", "Admin Person").await;
    let auth = GqlAuth::User(UserPrincipal {
        company: company.clone(),
        user_id: admin,
        email: "admin@example.test".to_string(),
        role: UserRole::Admin,
        must_change_password: false,
        session_token_hash: "hash".to_string(),
        credential: crate::ports::SessionKind::Browser,
    });

    state
        .acp_sessions()
        .open(
            "conn-1",
            &owner(&auth),
            crate::server::acp::AcpSession {
                id: "s-1".to_string(),
                company: company.clone(),
                chat: "engineering".to_string(),
                agent_id: None,
            },
            crate::ports::now_millis(),
        )
        .expect("open session");

    runtime.quiesce().await;

    let result = prompt(
        &state,
        &auth,
        &json!({
            "sessionId": "s-1",
            "prompt": [
                { "type": "text", "text": "please review the invoice" },
            ],
            "_meta": { "opencompany/connectionId": "conn-1" },
        }),
    )
    .await;
    assert!(result.is_err(), "a quiesced runtime must refuse the prompt");

    // And nothing was journaled: the refusal happened before the append.
    let events = runtime
        .events()
        .read_from(&company, EventSeq::new(0), usize::MAX)
        .await
        .expect("read events");
    assert!(
        events
            .iter()
            .all(|stored| !matches!(&stored.event, CompanyEvent::OperatorMessage { .. })),
        "a refused prompt must not leave a message in the journal: {events:?}"
    );
}

/// Issue #1781 review (Codex P1): `prompt` used to journal straight to
/// `runtime.events()`, never through the REST `/chat` route's
/// `chat_and_emit`, so a session opened with `_meta.opencompany.chat =
/// "operator"` could post into the durable, supposedly read-only Operator
/// system feed. `acme` (from `acp_state`) has no real `operator` desk or
/// teammate, so this is the ordinary, non-grandfathered case REST already
/// refuses — the ACP surface must refuse it identically.
#[tokio::test]
async fn an_acp_prompt_addressed_to_the_operator_channel_is_refused() {
    let home = tempfile::Builder::new()
        .prefix("oc-acp-operator-guard-")
        .tempdir()
        .expect("tempdir");
    let state = acp_state(home.path()).await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).expect("company");
    let admin = seed_user(&state, &company, "u-admin", "Admin Person").await;
    let auth = GqlAuth::User(UserPrincipal {
        company: company.clone(),
        user_id: admin,
        email: "admin@example.test".to_string(),
        role: UserRole::Admin,
        must_change_password: false,
        session_token_hash: "hash".to_string(),
        credential: crate::ports::SessionKind::Browser,
    });

    // Mirrors what `open_session` stores for an unpinned session whose
    // client requested `_meta.opencompany.chat = "operator"`
    // (`AcpSession::thread_key` passes an unpinned request through
    // verbatim).
    state
        .acp_sessions()
        .open(
            "conn-1",
            &owner(&auth),
            crate::server::acp::AcpSession {
                id: "s-1".to_string(),
                company: company.clone(),
                chat: "operator".to_string(),
                agent_id: None,
            },
            crate::ports::now_millis(),
        )
        .expect("open session");

    let result = prompt(
        &state,
        &auth,
        &json!({
            "sessionId": "s-1",
            "prompt": [
                { "type": "text", "text": "hello from a session pinned to the feed" },
            ],
            "_meta": { "opencompany/connectionId": "conn-1" },
        }),
    )
    .await;
    assert!(
        result.is_err(),
        "a prompt addressed to the read-only Operator channel must be refused"
    );

    // And nothing was journaled: the refusal happened before the append,
    // same ordering the quiesced-runtime test above proves.
    let events = runtime
        .events()
        .read_from(&company, EventSeq::new(0), usize::MAX)
        .await
        .expect("read events");
    assert!(
        events
            .iter()
            .all(|stored| !matches!(&stored.event, CompanyEvent::OperatorMessage { .. })),
        "a refused prompt must not leave a message in the journal: {events:?}"
    );
}
