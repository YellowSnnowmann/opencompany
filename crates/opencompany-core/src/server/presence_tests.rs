use super::*;

fn acme() -> CompanyId {
    CompanyId::new("acme")
}

#[test]
fn a_beat_makes_somebody_present() {
    let reg = PresenceRegistry::new();
    assert!(reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Online, 0));
    let live = reg.list(&acme(), 0);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].user_id, "u1");
    assert_eq!(live[0].status, PresenceStatus::Online);
}

/// The 3x margin doing its job: a beat missed entirely still leaves the
/// person online, so a dot does not flicker on one slow request.
#[test]
fn one_missed_beat_does_not_flap_somebody_offline() {
    let reg = PresenceRegistry::new();
    reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Online, 0);
    assert_eq!(
        reg.list(&acme(), PRESENCE_HEARTBEAT_MILLIS * 2).len(),
        1,
        "two beats' worth of silence is still within the lease"
    );
}

#[test]
fn a_lapsed_lease_stops_being_present() {
    let reg = PresenceRegistry::new();
    reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Online, 0);
    assert!(reg.list(&acme(), PRESENCE_TTL_MILLIS + 1).is_empty());
}

/// Crash recovery costs nothing: nobody has to notice the browser died.
#[test]
fn a_crashed_console_needs_no_cleanup() {
    let reg = PresenceRegistry::new();
    reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Online, 0);
    // No detach — the tab is simply gone.
    assert!(reg.list(&acme(), PRESENCE_TTL_MILLIS + 1).is_empty());
}

#[test]
fn a_clean_disconnect_drops_the_lease_at_once() {
    let reg = PresenceRegistry::new();
    reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Online, 0);
    assert_eq!(
        reg.detach(&acme(), "u1", "tab-1", 0),
        Some(PresenceStatus::Offline)
    );
    assert!(reg.list(&acme(), 0).is_empty());
    assert_eq!(
        reg.detach(&acme(), "u1", "tab-1", 0),
        None,
        "a second teardown changes nothing"
    );
}

/// The bug this module's header now calls out by name: the same person
/// with two tabs open must not have one tab's departure log the other one
/// out. Closing tab 1 must not touch tab 2's lease, and only closing the
/// last open tab is a real departure worth a frame.
#[test]
fn closing_one_tab_does_not_disconnect_another() {
    let reg = PresenceRegistry::new();
    reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Online, 0);
    reg.beat(&acme(), "u1", "tab-2", PresenceStatus::Online, 0);

    assert_eq!(
        reg.detach(&acme(), "u1", "tab-1", 0),
        None,
        "tab 2 is still open at the same status, so this is not a real departure"
    );
    let live = reg.list(&acme(), 0);
    assert_eq!(live.len(), 1, "still present through tab 2");
    assert_eq!(live[0].status, PresenceStatus::Online);

    assert_eq!(
        reg.detach(&acme(), "u1", "tab-2", 0),
        Some(PresenceStatus::Offline),
        "the last open tab leaving is a real departure"
    );
    assert!(reg.list(&acme(), 0).is_empty());
}

/// Two open tabs disagreeing about status — one idle, one active — must
/// read as the more present of the two: a person reading in one tab while
/// away in another is still here.
#[test]
fn the_most_present_tab_wins_the_aggregate_status() {
    let reg = PresenceRegistry::new();
    reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Away, 0);
    reg.beat(&acme(), "u1", "tab-2", PresenceStatus::Online, 0);
    let live = reg.list(&acme(), 0);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].status, PresenceStatus::Online);
}

/// The downgrade case: closing the console that was carrying the
/// aggregate must report the *new* aggregate (`Away`), not `Offline` —
/// the person is still here, just not at their most-present console
/// anymore, and every other viewer's dot should say so at once rather
/// than waiting for the away tab's next heartbeat to correct a false
/// "gone".
#[test]
fn closing_the_more_present_tab_reports_the_downgraded_status() {
    let reg = PresenceRegistry::new();
    reg.beat(&acme(), "u1", "tab-online", PresenceStatus::Online, 0);
    reg.beat(&acme(), "u1", "tab-away", PresenceStatus::Away, 0);

    assert_eq!(
        reg.detach(&acme(), "u1", "tab-online", 0),
        Some(PresenceStatus::Away),
        "the away tab is still open — the aggregate degrades, it does not vanish"
    );
    let live = reg.list(&acme(), 0);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].status, PresenceStatus::Away);
}

/// The memory-growth finding: a client that keeps minting fresh
/// `consoleId`s (a bug in some other console, or a member hammering the
/// route) must not grow one person's lease set without bound. Past the
/// cap, a new console evicts the stalest one rather than being appended.
#[test]
fn a_flood_of_new_consoles_is_capped_rather_than_grown_forever() {
    let reg = PresenceRegistry::new();
    for i in 0..(MAX_CONSOLES_PER_PERSON * 3) {
        reg.beat(
            &acme(),
            "u1",
            &format!("console-{i}"),
            PresenceStatus::Online,
            i as u64,
        );
    }
    let count = reg
        .people
        .lock()
        .unwrap()
        .get(&(acme(), "u1".to_string()))
        .map(|consoles| consoles.len())
        .unwrap_or(0);
    assert!(
        count <= MAX_CONSOLES_PER_PERSON,
        "expected at most {MAX_CONSOLES_PER_PERSON} tracked consoles, found {count}"
    );
    // And the person is still (correctly) present — capping evicts the
    // stalest lease, it does not stop tracking the person altogether.
    assert_eq!(
        reg.list(&acme(), (MAX_CONSOLES_PER_PERSON * 3) as u64)
            .len(),
        1
    );
}

/// A console renewing its own existing lease must never be evicted by its
/// own renewal, even sitting exactly at the cap — the cap only ever makes
/// room for a *new* console id.
#[test]
fn renewing_an_existing_console_at_the_cap_never_evicts_itself() {
    let reg = PresenceRegistry::new();
    for i in 0..MAX_CONSOLES_PER_PERSON {
        reg.beat(
            &acme(),
            "u1",
            &format!("console-{i}"),
            PresenceStatus::Online,
            0,
        );
    }
    // Renew the very first console many times — it must still be present
    // afterward, not evicted as "stalest" by its own renewals.
    for tick in 1..10 {
        reg.beat(&acme(), "u1", "console-0", PresenceStatus::Online, tick);
    }
    let people = reg.people.lock().unwrap();
    let consoles = people.get(&(acme(), "u1".to_string())).unwrap();
    assert!(consoles.contains_key("console-0"));
    assert_eq!(consoles.len(), MAX_CONSOLES_PER_PERSON);
}

/// The bus stays quiet while nothing moves: a console beats every minute
/// whether or not anything changed, and republishing that to everyone would
/// be one frame per person per minute for no visible difference.
#[test]
fn an_unchanged_beat_reports_no_change() {
    let reg = PresenceRegistry::new();
    assert!(
        reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Online, 0),
        "arrival"
    );
    assert!(
        !reg.beat(
            &acme(),
            "u1",
            "tab-1",
            PresenceStatus::Online,
            PRESENCE_HEARTBEAT_MILLIS
        ),
        "a routine renewal is not news"
    );
    assert!(
        reg.beat(
            &acme(),
            "u1",
            "tab-1",
            PresenceStatus::Away,
            PRESENCE_HEARTBEAT_MILLIS
        ),
        "a status change is"
    );
}

#[test]
fn a_beat_after_the_lease_lapsed_is_an_arrival_again() {
    let reg = PresenceRegistry::new();
    reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Online, 0);
    assert!(
        reg.beat(
            &acme(),
            "u1",
            "tab-1",
            PresenceStatus::Online,
            PRESENCE_TTL_MILLIS + 1
        ),
        "otherwise somebody who lapsed reappears with no frame announcing it"
    );
}

/// One tenant must never see another's people, and the map is shared.
#[test]
fn presence_does_not_leak_between_companies() {
    let reg = PresenceRegistry::new();
    reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Online, 0);
    reg.beat(
        &CompanyId::new("other"),
        "u2",
        "tab-1",
        PresenceStatus::Online,
        0,
    );
    let live = reg.list(&acme(), 0);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].user_id, "u1");
}

#[test]
fn the_sweep_frees_only_what_list_already_ignores() {
    let reg = PresenceRegistry::new();
    reg.beat(&acme(), "u1", "tab-1", PresenceStatus::Online, 0);
    reg.beat(
        &acme(),
        "u2",
        "tab-1",
        PresenceStatus::Online,
        PRESENCE_TTL_MILLIS,
    );
    let now = PRESENCE_TTL_MILLIS + 1;
    let visible_before = reg.list(&acme(), now);
    assert_eq!(reg.sweep(now), 1);
    assert_eq!(reg.list(&acme(), now), visible_before);
}

/// A beat stamped slightly in the future — two hosts whose clocks disagree
/// — must not read as expired. Wrapping subtraction would make the age
/// enormous and mark a live person gone.
#[test]
fn a_beat_from_a_slightly_fast_clock_is_not_expired() {
    assert!(!expired(1_000, 0));
}

#[test]
fn every_status_has_a_stable_wire_word() {
    assert_eq!(PresenceStatus::Online.as_str(), "online");
    assert_eq!(PresenceStatus::Away.as_str(), "away");
    assert_eq!(PresenceStatus::Offline.as_str(), "offline");
    assert_eq!(
        serde_json::to_string(&PresenceStatus::Away).expect("serialize"),
        "\"away\""
    );
}

/// Forward-compatibility is not wanted here: an unknown status must be a
/// refusal, not a silently-stored string a reader cannot render.
#[test]
fn an_unknown_status_is_refused() {
    assert!(serde_json::from_str::<PresenceStatus>("\"invisible\"").is_err());
}
