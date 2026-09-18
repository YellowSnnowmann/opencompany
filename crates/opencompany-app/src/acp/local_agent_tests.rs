use super::*;

fn agent(root: &Path) -> LocalAcpAgent {
    LocalAcpAgent::new("claude", None, HashMap::new(), root.to_path_buf())
        .expect("`claude` is a catalogue harness")
}

#[test]
fn a_remembered_session_comes_back_for_the_same_pair() {
    // The whole point: the in-memory `sessions` map dies with the process,
    // and every runtime rebuild builds a fresh agent. Without a record on
    // disk, a teammate's conversation silently started over on a restart —
    // and nothing on the operator's screen said so.
    let dir = tempfile::tempdir().unwrap();
    let agent = agent(dir.path());
    let acme = CompanyId::new("acme");
    std::fs::create_dir_all(dir.path().join("acme").join("ceo")).unwrap();

    assert!(agent.read_session_record(&acme, "ceo").is_none());
    agent.write_session_record(&acme, "ceo", "sess-1", None);
    assert_eq!(
        agent
            .read_session_record(&acme, "ceo")
            .map(|r| r.session_id),
        Some("sess-1".to_string())
    );

    // Per pair, never shared: two teammates resuming one conversation is
    // the same defect as two desks sharing a session key.
    assert!(agent.read_session_record(&acme, "cto").is_none());
    assert!(
        agent
            .read_session_record(&CompanyId::new("globex"), "ceo")
            .is_none()
    );
}

#[test]
fn a_record_from_another_harness_is_not_offered() {
    // A `claude-agent-acp` session id means nothing to `codex-acp`, and
    // the record outlives a teammate being rebound between them. Loading
    // it would spend a round trip to be told "resource not found".
    let dir = tempfile::tempdir().unwrap();
    let acme = CompanyId::new("acme");
    std::fs::create_dir_all(dir.path().join("acme").join("ceo")).unwrap();

    agent(dir.path()).write_session_record(&acme, "ceo", "sess-1", None);

    let codex = LocalAcpAgent::new("codex", None, HashMap::new(), dir.path().to_path_buf())
        .expect("`codex` is a catalogue harness");
    assert!(codex.read_session_record(&acme, "ceo").is_none());
}

#[test]
fn a_new_session_replaces_the_one_that_would_not_load() {
    // Why a failed `session/load` does NOT delete the record (PR #1904
    // review): a load can fail because the adapter *exited*, which says
    // nothing about whether the session still exists — and the
    // `session/new` that follows is about to fail against the same dead
    // client. Deleting there would throw away the only pointer to a
    // recoverable conversation.
    //
    // What makes keeping it safe is this: a session that really is gone
    // gets overwritten the moment a replacement is opened, so the stale
    // id cannot be retried forever.
    let dir = tempfile::tempdir().unwrap();
    let agent = agent(dir.path());
    let acme = CompanyId::new("acme");
    std::fs::create_dir_all(dir.path().join("acme").join("ceo")).unwrap();

    agent.write_session_record(&acme, "ceo", "sess-dead", None);
    agent.write_session_record(&acme, "ceo", "sess-new", None);
    assert_eq!(
        agent
            .read_session_record(&acme, "ceo")
            .map(|r| r.session_id),
        Some("sess-new".to_string()),
        "the replacement is what the next start resumes"
    );
}

#[test]
fn a_record_remembers_which_model_its_session_is_on() {
    // Why the model is recorded at all (PR #1904 review): a resumed
    // session restores its own *session-scoped* model config, so without
    // this there is no way to notice the config no longer matches what
    // the company asks for.
    let dir = tempfile::tempdir().unwrap();
    let agent = agent(dir.path());
    let acme = CompanyId::new("acme");
    std::fs::create_dir_all(dir.path().join("acme").join("ceo")).unwrap();

    agent.write_session_record(&acme, "ceo", "sess-1", Some("claude-opus-4-6"));
    let record = agent.read_session_record(&acme, "ceo").expect("written");
    assert_eq!(record.model.as_deref(), Some("claude-opus-4-6"));

    // A record written before this field existed reads as "no model ever
    // chosen" — the same value a never-configured session has, so an old
    // record resumes exactly as it did.
    std::fs::write(
        agent.session_record_path(&acme, "ceo"),
        r#"{"harness":"claude","sessionId":"sess-old"}"#,
    )
    .unwrap();
    let legacy = agent.read_session_record(&acme, "ceo").expect("still read");
    assert_eq!(legacy.session_id, "sess-old");
    assert_eq!(legacy.model, None);
}

#[test]
fn the_desired_model_is_the_override_then_the_harness_default() {
    // Deliberately blind to `self.env`: this answers what should be true
    // of the teammate, not who delivers it. A session that got its model
    // from the spawn env is on the same model as one that got it from
    // `session/set_config_option`, and the record must say so either way.
    let dir = tempfile::tempdir().unwrap();
    let mut overrides = HashMap::new();
    overrides.insert("cto".to_string(), "claude-haiku-4-5".to_string());
    let configured = LocalAcpAgent::new(
        "claude",
        Some("claude-sonnet-4-5"),
        overrides,
        dir.path().to_path_buf(),
    )
    .expect("`claude` is a catalogue harness");

    assert_eq!(
        configured.desired_model("cto").as_deref(),
        Some("claude-haiku-4-5"),
        "a teammate's own override wins"
    );
    assert_eq!(
        configured.desired_model("ceo").as_deref(),
        Some("claude-sonnet-4-5"),
        "and the harness default covers everyone else — even though the \
         spawn env is what actually carries it"
    );

    // Nothing declared anywhere: the adapter's own default, which no
    // `session/set_config_option` can name.
    let bare = agent(dir.path());
    assert_eq!(bare.desired_model("ceo"), None);
}

#[test]
fn the_record_lives_outside_the_agents_own_workspace() {
    // `workspace/` is the `cwd` handed to the adapter. A file written in
    // there is one the teammate can list, read and edit — and one it could
    // "tidy up" mid-session.
    let dir = tempfile::tempdir().unwrap();
    let agent = agent(dir.path());
    let acme = CompanyId::new("acme");

    let record = agent.session_record_path(&acme, "ceo");
    let cwd = agent.session_root(&acme, "ceo").unwrap();
    assert!(
        !record.starts_with(&cwd),
        "{} must not sit inside {}",
        record.display(),
        cwd.display()
    );
}

#[test]
fn an_unwritable_record_does_not_fail_the_turn() {
    // Bookkeeping must never cost an operator a working agent: the worst
    // a lost record can do is make the next cold start begin fresh, which
    // is what every start did before this existed.
    let dir = tempfile::tempdir().unwrap();
    let agent = agent(dir.path());
    // No `acme/ceo/` directory, so the write has nowhere to land.
    agent.write_session_record(&CompanyId::new("acme"), "ceo", "sess-1", None);
    assert!(
        agent
            .read_session_record(&CompanyId::new("acme"), "ceo")
            .is_none()
    );
}

#[test]
fn wire_text_is_bounded_before_it_reaches_the_timeline() {
    // A tool call's `title` and result are unvalidated text from an
    // external process, and they now travel to two places at once: the
    // durable step and every watching console.
    let long = "x".repeat(MAX_TITLE_CHARS * 2);
    let update = serde_json::json!({
        "sessionId": "s1",
        "update": {
            "sessionUpdate": "tool_call",
            "toolCallId": "c1",
            "title": long,
        }
    });
    let Some(AcpUpdate::ToolCall { title, .. }) = parse_update(&update) else {
        panic!("a tool call parses");
    };
    assert_eq!(
        title.chars().count(),
        MAX_TITLE_CHARS + 1,
        "bounded, plus the ellipsis"
    );

    // Multi-byte input must be cut on a character, never a byte — a byte
    // slice through a UTF-8 sequence panics.
    assert_eq!(bounded("héllo wörld", 4), "héll…");
    assert_eq!(
        bounded("short", 99),
        "short",
        "an unbounded string is untouched"
    );
}

#[test]
fn a_replayed_user_message_is_not_this_turns_execution_state() {
    // `session/load` replays the resumed conversation as ordinary
    // `session/update` notifications, including the operator's own past
    // messages. Mapping one would put a months-old message on this turn's
    // timeline.
    let update = serde_json::json!({
        "sessionId": "s1",
        "update": {
            "sessionUpdate": "user_message_chunk",
            "content": { "type": "text", "text": "what did I say before?" }
        }
    });
    assert!(parse_update(&update).is_none());
}

#[test]
fn an_observer_is_registered_for_one_turn_and_no_longer() {
    // The guard, not a matching pair of calls: the steered path *drops*
    // the prompt future for a turn that ignored its cancel, and a manual
    // deregistration would never run for exactly those turns.
    let live: Arc<StdMutex<HashMap<String, AcpObserver>>> = Arc::new(StdMutex::new(HashMap::new()));
    let seen = Arc::new(StdMutex::new(0usize));
    let counter = Arc::clone(&seen);
    let observer: AcpObserver = Arc::new(move |_| {
        *counter.lock().unwrap() += 1;
    });

    {
        let _guard = LiveTurn::register(&live, "s1", Some(&observer));
        assert!(live.lock().unwrap().contains_key("s1"));
        live.lock().unwrap()["s1"](&AcpUpdate::ThoughtChunk);
    }
    assert!(
        live.lock().unwrap().is_empty(),
        "the turn's observer goes when the turn does"
    );
    assert_eq!(*seen.lock().unwrap(), 1);

    // A turn nobody is watching registers nothing at all, so the sink does
    // no per-update work for it.
    assert!(LiveTurn::register(&live, "s1", None).is_none());
    assert!(live.lock().unwrap().is_empty());
}
