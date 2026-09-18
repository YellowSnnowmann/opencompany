use super::*;

fn entry(key: &str, kind: InflightKind) -> InflightEntry {
    InflightEntry {
        key: key.to_string(),
        task_id: matches!(kind, InflightKind::Task).then(|| key.to_string()),
        kind,
        title: "Ship the thing".to_string(),
        agent_id: "ceo".to_string(),
        started_at_millis: 0,
        pending_action: None,
    }
}

/// The registry answers "is anything running" across every company, and the
/// guard returned by `register` is what makes the answer self-correcting: it
/// deregisters on drop, on every exit path, so a crashed or early-returning
/// turn cannot leave the workload permanently claiming to be busy and its
/// tenant permanently un-parkable.
#[test]
fn any_inflight_tracks_registration_and_clears_on_drop() {
    let registry = InflightRegistry::new();
    let company = CompanyId::new("acme");
    assert!(!registry.any_inflight(), "an empty registry is not busy");

    {
        let _guard = registry.register(&company, entry("run-1", InflightKind::Task));
        assert!(registry.any_inflight(), "a registered run makes it busy");
    }

    assert!(
        !registry.any_inflight(),
        "the guard must clear the entry on drop, or the tenant never parks again"
    );
}

#[test]
fn control_is_one_shot_and_last_write_wins() {
    let control = SteerControl::new();
    assert!(!control.requested());
    control.request(SteerAction::Pause);
    control.request(SteerAction::Cancel); // last write wins
    assert!(control.requested());
    assert_eq!(control.take(), Some(SteerAction::Cancel));
    assert!(!control.requested(), "take clears the action");
    assert_eq!(control.take(), None);
}

#[test]
fn register_lists_then_guard_drop_deregisters() {
    let company = CompanyId::new("acme");
    let reg = InflightRegistry::new();
    {
        let _guard = reg.register(&company, entry("t1", InflightKind::Task));
        let listed = reg.list(&company);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, "t1");
    }
    // Guard dropped → the row is gone, and the empty company map is pruned.
    assert!(reg.list(&company).is_empty());
}

#[test]
fn steer_sets_the_control_and_pending_badge() {
    let company = CompanyId::new("acme");
    let reg = InflightRegistry::new();
    let guard = reg.register(&company, entry("t1", InflightKind::Task));
    reg.steer(&company, "t1", SteerAction::Pause)
        .expect("steers");
    assert!(guard.control().requested(), "the turn's control is set");
    assert_eq!(
        reg.list(&company)[0].pending_action.as_deref(),
        Some("pause"),
        "the strip shows a pending badge"
    );
}

#[test]
fn steer_unknown_key_is_not_in_flight() {
    let company = CompanyId::new("acme");
    let reg = InflightRegistry::new();
    let err = reg
        .steer(&company, "ghost", SteerAction::Cancel)
        .expect_err("unknown key");
    assert_eq!(err, SteerError::NotInFlight);
}

#[test]
fn delegation_is_cancel_only() {
    let company = CompanyId::new("acme");
    let reg = InflightRegistry::new();
    let _guard = reg.register(&company, entry("d1", InflightKind::Delegation));
    assert_eq!(
        reg.steer(&company, "d1", SteerAction::Pause),
        Err(SteerError::Unsupported)
    );
    assert_eq!(
        reg.steer(
            &company,
            "d1",
            SteerAction::Redirect {
                instruction: "do X".into()
            }
        ),
        Err(SteerError::Unsupported)
    );
    // Cancel is allowed.
    reg.steer(&company, "d1", SteerAction::Cancel)
        .expect("cancel a delegation");
}

#[test]
fn deregister_on_error_path_via_guard_drop() {
    // Simulate a turn that errors: the guard is created, then an early error
    // return drops it. The registry must be clean afterward (RAII).
    let company = CompanyId::new("acme");
    let reg = InflightRegistry::new();
    let run = || -> Result<(), &'static str> {
        let _guard = reg.register(&company, entry("t1", InflightKind::Task));
        Err("turn blew up")
    };
    assert!(run().is_err());
    assert!(
        reg.list(&company).is_empty(),
        "the guard deregistered even on the error path"
    );
}

#[test]
fn cap_redirect_is_identity_under_the_cap() {
    let exact: String = "a".repeat(MAX_REDIRECT_CHARS);
    assert_eq!(
        cap_redirect(&exact),
        exact,
        "an at-limit redirect is verbatim"
    );
    assert_eq!(cap_redirect("short"), "short");
    assert_eq!(cap_redirect(""), "");
}

#[test]
fn cap_redirect_marks_how_much_it_dropped() {
    let long: String = "a".repeat(MAX_REDIRECT_CHARS + 500);
    let capped = cap_redirect(&long);
    assert!(
        capped.chars().count() <= MAX_REDIRECT_CHARS,
        "the cap reserves room for its own marker, got {} chars",
        capped.chars().count()
    );
    // The marker names the real loss: kept + dropped is the whole input.
    let (head, tail) = capped.rsplit_once(" […").expect("a cut redirect is marked");
    let dropped: usize = tail
        .trim_end_matches(" chars truncated]")
        .parse()
        .expect("the marker names a count");
    assert_eq!(
        head.chars().count() + dropped,
        long.chars().count(),
        "shown + dropped accounts for every input character"
    );
}

#[test]
fn cap_redirect_is_codepoint_safe() {
    // A multi-byte codepoint straddling the limit must be cut on a character
    // boundary, not panic the way a byte slice would.
    let long: String = "é".repeat(MAX_REDIRECT_CHARS + 50);
    let capped = cap_redirect(&long);
    assert!(capped.chars().count() <= MAX_REDIRECT_CHARS);
    assert!(capped.ends_with("chars truncated]"));
    let head: String = capped.chars().take_while(|c| *c == 'é').collect();
    // The surviving head is whole `é`s — 2 bytes each, no split codepoint.
    assert_eq!(head.len(), head.chars().count() * 2);
}
