use super::*;
use serde_json::json;

fn session(id: &str, company: &str) -> AcpSession {
    AcpSession {
        id: id.to_string(),
        company: CompanyId::new(company),
        chat: "General".to_string(),
        agent_id: None,
    }
}

#[test]
fn session_scoped_mcp_servers_are_refused_with_a_reason() {
    // Silently dropping them leaves a client believing its tools were
    // installed, and the model then gets asked why it never called them.
    let refusal = refuse_unsupported(&[json!({ "name": "x" })], &[]).unwrap();
    assert_eq!(refusal, NewSessionRefusal::McpServers);
    // Specific enough to act on: a client told only "invalid params"
    // retries with the same request.
    assert!(refusal.message().contains("mcp/servers"));
}

#[test]
fn additional_directories_are_refused_too() {
    let refusal = refuse_unsupported(&[], &[json!("/tmp")]).unwrap();
    assert_eq!(refusal, NewSessionRefusal::AdditionalDirectories);
}

#[test]
fn an_ordinary_request_is_accepted() {
    assert!(refuse_unsupported(&[], &[]).is_none());
}

#[test]
fn the_client_is_told_its_cwd_was_not_used() {
    // Accepted and ignored is only honest if it is also reported. A client
    // that believes its own path was honoured will resolve file paths
    // against a directory that does not exist on this machine.
    let meta = cwd_meta("/data/harness/acme/ceo/workspace");
    assert_eq!(meta["opencompany/cwdIgnored"], true);
    assert_eq!(
        meta["opencompany/workspace"],
        "/data/harness/acme/ceo/workspace"
    );
}

#[test]
fn sessions_are_scoped_to_their_connection() {
    // Two clients must not see each other's sessions — and a reconnecting
    // one must not resume into another's.
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    registry
        .open("conn-b", "bob", session("s2", "globex"), 0)
        .unwrap();

    assert!(registry.get("conn-a", "alice", "s1", 0).is_some());
    assert!(
        registry.get("conn-b", "alice", "s1", 0).is_none(),
        "no cross-connection reads"
    );
    assert_eq!(registry.list("conn-a", "alice", 0).unwrap().len(), 1);
}

#[test]
fn a_caller_cannot_open_a_session_on_a_connection_it_does_not_own() {
    // The defect this registry exists to close: a caller-supplied
    // `connectionId` is otherwise just a guessable string, and whoever
    // guesses it could open sessions into somebody else's connection.
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();

    let refusal = registry
        .open("conn-a", "mallory", session("s2", "acme"), 0)
        .unwrap_err();
    assert_eq!(refusal, OpenSessionRefusal::NotOwned);
    assert_eq!(
        registry.list("conn-a", "alice", 0).unwrap().len(),
        1,
        "the attempted takeover left alice's session set untouched"
    );
    assert!(
        registry.list("conn-a", "mallory", 0).is_none(),
        "mallory never owned the connection, so it stays invisible to her too"
    );
}

#[test]
fn a_caller_cannot_read_or_act_on_a_connection_it_does_not_own() {
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();

    assert!(registry.get("conn-a", "mallory", "s1", 0).is_none());
    assert!(registry.list("conn-a", "mallory", 0).is_none());
    assert!(!registry.remove("conn-a", "mallory", "s1"));
    assert!(
        registry
            .close_connection("conn-a", "mallory", |_| true)
            .is_empty()
    );
    // None of mallory's attempts touched alice's session.
    assert!(registry.get("conn-a", "alice", "s1", 0).is_some());
}

#[test]
fn a_session_over_the_per_connection_cap_is_refused() {
    let registry = SessionRegistry::new();
    for i in 0..MAX_SESSIONS_PER_CONNECTION {
        registry
            .open("conn-a", "alice", session(&format!("s{i}"), "acme"), 0)
            .unwrap();
    }
    let refusal = registry
        .open("conn-a", "alice", session("s-over", "acme"), 0)
        .unwrap_err();
    assert_eq!(refusal, OpenSessionRefusal::PerConnectionCap);
    assert_eq!(
        registry.list("conn-a", "alice", 0).unwrap().len(),
        MAX_SESSIONS_PER_CONNECTION
    );
}

#[test]
fn a_session_over_the_host_wide_cap_is_refused_even_on_a_fresh_connection() {
    let registry = SessionRegistry::new();
    // Fan the total cap out across many connections rather than one, so
    // this proves the cap is host-wide and not just per-connection.
    let mut opened = 0;
    for i in 0..MAX_SESSIONS_TOTAL {
        let conn = format!("conn-{i}");
        registry
            .open(&conn, "alice", session("s0", "acme"), 0)
            .unwrap();
        opened += 1;
    }
    assert_eq!(opened, MAX_SESSIONS_TOTAL);
    let refusal = registry
        .open("conn-fresh", "alice", session("s0", "acme"), 0)
        .unwrap_err();
    assert_eq!(refusal, OpenSessionRefusal::TotalCap);
}

#[test]
fn an_idle_session_past_its_ttl_is_swept() {
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();

    assert_eq!(
        registry.sweep_expired(SESSION_TTL_MILLIS),
        0,
        "exactly at the boundary is not yet expired"
    );
    assert_eq!(registry.sweep_expired(SESSION_TTL_MILLIS + 1), 1);
    assert!(
        registry
            .get("conn-a", "alice", "s1", SESSION_TTL_MILLIS + 1)
            .is_none()
    );
}

#[test]
fn using_a_session_renews_its_idle_ttl() {
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    // A `get` at half the TTL — a live prompt — renews the clock.
    assert!(
        registry
            .get("conn-a", "alice", "s1", SESSION_TTL_MILLIS / 2)
            .is_some()
    );
    assert_eq!(
        registry.sweep_expired(SESSION_TTL_MILLIS),
        0,
        "renewed at TTL/2, so a full TTL later it is not yet idle that long"
    );
    assert!(
        registry
            .get("conn-a", "alice", "s1", SESSION_TTL_MILLIS)
            .is_some()
    );
}

#[test]
fn peek_does_not_renew_the_idle_ttl() {
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    // A `peek` at half the TTL — a pre-authorization check — must not
    // extend the clock the way `get` does.
    assert!(
        registry
            .peek("conn-a", "alice", "s1", SESSION_TTL_MILLIS / 2)
            .is_some()
    );
    assert_eq!(
        registry.sweep_expired(SESSION_TTL_MILLIS + 1),
        1,
        "peek must not have renewed the session past its original TTL"
    );
}

#[test]
fn peek_refuses_a_connection_or_owner_it_does_not_hold() {
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    assert!(registry.peek("conn-b", "alice", "s1", 0).is_none());
    assert!(registry.peek("conn-a", "mallory", "s1", 0).is_none());
    assert!(registry.peek("conn-a", "alice", "s1", 0).is_some());
}

#[test]
fn touch_renews_the_idle_ttl_that_peek_left_alone() {
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    assert!(registry.peek("conn-a", "alice", "s1", 0).is_some());
    registry.touch("conn-a", "alice", "s1", SESSION_TTL_MILLIS / 2);
    assert_eq!(
        registry.sweep_expired(SESSION_TTL_MILLIS),
        0,
        "touch renewed at TTL/2, so a full TTL later it is not yet idle that long"
    );
}

#[test]
fn peek_cannot_revive_a_session_already_past_its_ttl() {
    // Landing in the gap between two sweeper ticks must not let a lookup
    // (and the touch a caller runs after it authorizes) revive a session
    // that is already stale — the sweep is a cleanup convenience, not the
    // only place staleness is enforced.
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    assert!(
        registry
            .peek("conn-a", "alice", "s1", SESSION_TTL_MILLIS + 1)
            .is_none(),
        "a peek past the TTL must refuse, not hand back a session to authorize and touch"
    );
    // And it evicted the stale entry rather than merely refusing this call.
    assert!(registry.list("conn-a", "alice", 0).is_none());
}

#[test]
fn get_cannot_revive_a_session_already_past_its_ttl() {
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    assert!(
        registry
            .get("conn-a", "alice", "s1", SESSION_TTL_MILLIS + 1)
            .is_none()
    );
}

#[test]
fn list_does_not_advertise_a_session_already_past_its_ttl() {
    // `session/list` must not tell a caller a session is there when
    // `peek` and `get` already refuse it as expired, landing in the same
    // gap between two hourly sweeper ticks — otherwise a client is told a
    // session is resumable and then has `session/prompt` immediately
    // report it unknown.
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    registry
        .open("conn-a", "alice", session("s2", "acme"), SESSION_TTL_MILLIS)
        .unwrap();
    let listed = registry
        .list("conn-a", "alice", SESSION_TTL_MILLIS + 1)
        .expect("the connection itself is not expired, only one session on it");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "s2");
}

#[test]
fn touch_is_a_silent_no_op_for_a_connection_or_owner_it_does_not_hold() {
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    // Neither call may panic, and neither may renew alice's session.
    registry.touch("conn-b", "alice", "s1", SESSION_TTL_MILLIS / 2);
    registry.touch("conn-a", "mallory", "s1", SESSION_TTL_MILLIS / 2);
    assert_eq!(registry.sweep_expired(SESSION_TTL_MILLIS + 1), 1);
}

#[test]
fn sweeping_prunes_the_connection_once_every_session_on_it_expires() {
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    registry.sweep_expired(SESSION_TTL_MILLIS + 1);
    let by_connection = registry
        .by_connection
        .lock()
        .expect("session registry poisoned");
    assert!(!by_connection.contains_key("conn-a"));
}

#[test]
fn closing_a_connection_drops_its_sessions_and_nothing_else() {
    // Without this a client that reconnects repeatedly accumulates sessions
    // nothing will ever close.
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    registry
        .open("conn-b", "bob", session("s2", "acme"), 0)
        .unwrap();

    let closed = registry.close_connection("conn-a", "alice", |_| true);
    assert_eq!(closed, vec!["s1".to_string()]);
    assert!(registry.get("conn-a", "alice", "s1", 0).is_none());
    assert!(
        registry.get("conn-b", "bob", "s2", 0).is_some(),
        "other connections survive"
    );
}

#[test]
fn close_connection_leaves_a_session_the_caller_is_not_authorized_for() {
    // `owner` names a tenant, not an authorization scope: two platform
    // credentials for the same tenant can carry different company
    // allow-lists, so a connection can hold sessions the *presented*
    // credential is not itself authorized to act on.
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "platform:acme", session("s-allowed", "acme"), 0)
        .unwrap();
    registry
        .open(
            "conn-a",
            "platform:acme",
            session("s-restricted", "globex"),
            0,
        )
        .unwrap();

    let closed = registry.close_connection("conn-a", "platform:acme", |company| {
        company.as_ref() == "acme"
    });
    assert_eq!(closed, vec!["s-allowed".to_string()]);
    assert!(
        registry
            .get("conn-a", "platform:acme", "s-restricted", 0)
            .is_some(),
        "a session for a company outside the caller's own allow-list must survive its disconnect"
    );
}

#[test]
fn disconnect_sweeps_every_session_it_opened_with_none_stranded() {
    let registry = SessionRegistry::new();
    for i in 0..5 {
        registry
            .open("conn-a", "alice", session(&format!("s{i}"), "acme"), 0)
            .unwrap();
    }
    let closed = registry.close_connection("conn-a", "alice", |_| true);
    assert_eq!(closed.len(), 5);
    for i in 0..5 {
        assert!(
            registry
                .get("conn-a", "alice", &format!("s{i}"), 0)
                .is_none(),
            "session s{i} was stranded"
        );
    }
    assert!(registry.list("conn-a", "alice", 0).is_none());
}

#[test]
fn deleting_a_session_removes_only_that_session() {
    // ACP's `session/delete`: one session goes, the connection and its
    // other sessions survive.
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    registry
        .open("conn-a", "alice", session("s2", "acme"), 0)
        .unwrap();

    assert!(registry.remove("conn-a", "alice", "s1"));
    assert!(registry.get("conn-a", "alice", "s1", 0).is_none());
    assert!(registry.get("conn-a", "alice", "s2", 0).is_some());
    assert_eq!(registry.list("conn-a", "alice", 0).unwrap().len(), 1);
}

#[test]
fn removing_a_connections_last_session_prunes_the_connection() {
    // `session/new` + `session/disconnect` over fresh caller-controlled
    // connection ids must not grow the host-wide registry by one empty map
    // per connection, forever.
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    registry
        .open("conn-b", "bob", session("s2", "acme"), 0)
        .unwrap();

    assert!(registry.remove("conn-a", "alice", "s1"));
    let by_connection = registry
        .by_connection
        .lock()
        .expect("session registry poisoned");
    assert!(
        !by_connection.contains_key("conn-a"),
        "the emptied connection key is pruned, not left as an empty map"
    );
    assert!(
        by_connection.contains_key("conn-b"),
        "a connection that still holds sessions survives"
    );
}

#[test]
fn deleting_a_never_existing_session_is_a_silent_no_op() {
    // ACP says deleting an already-deleted or never-existing session should
    // succeed silently — an opaque id leaking "I never had that" by an
    // error would tell a caller more than it needs to know.
    let registry = SessionRegistry::new();
    registry
        .open("conn-a", "alice", session("s1", "acme"), 0)
        .unwrap();
    assert!(!registry.remove("conn-a", "alice", "ghost"));
    assert!(!registry.remove("conn-b", "alice", "s1"));
    assert_eq!(registry.list("conn-a", "alice", 0).unwrap().len(), 1);
}

#[test]
fn a_session_names_its_company_and_desk() {
    // The triple is what makes an ACP session and the console's view of the
    // same desk one conversation rather than two.
    let s = session("s1", "acme");
    assert_eq!(s.company, CompanyId::new("acme"));
    assert_eq!(s.chat, "General");
    assert!(
        s.agent_id.is_none(),
        "unpinned routes through the desk lead"
    );
}

#[test]
fn a_pinned_sessions_thread_is_the_members_dm_channel() {
    // A pin is answered by its member, and `responder_for` resolves a chat
    // key to a member only through the `dm:<member>` shape — so that is the
    // thread key, not the desk the client asked for.
    let pin = Some("ceo".to_string());
    assert_eq!(AcpSession::thread_key("General", pin.as_deref()), "dm:ceo");
    assert_eq!(AcpSession::thread_key("General", None), "General");
}
