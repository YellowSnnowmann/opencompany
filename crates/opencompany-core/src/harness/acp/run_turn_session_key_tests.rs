use super::run_turn_test_fixtures::*;
use super::*;

/// A runner with no desks declared — the shape every key assertion below
/// except the alias one is about.
fn keyer() -> AcpRunTurn {
    AcpRunTurn::new(Arc::new(Scripted::answering(a_working_turn())))
}

#[test]
fn a_session_key_separates_agents_and_companies() {
    let keyer = keyer();
    let acme = CompanyId::new("acme");
    let globex = CompanyId::new("globex");
    assert_ne!(
        keyer.session_key(&acme, "ceo", None),
        keyer.session_key(&acme, "cto", None)
    );
    assert_ne!(
        keyer.session_key(&acme, "ceo", None),
        keyer.session_key(&globex, "ceo", None)
    );
    // Stable across turns, or the second question in a thread arrives with
    // no memory of the first.
    assert_eq!(
        keyer.session_key(&acme, "ceo", None),
        keyer.session_key(&acme, "ceo", None)
    );
}

/// Issue #1890 H — the property this file's comments already claimed.
///
/// "Two desks do not share a conversation" was written on `session_key`
/// while the key held no channel at all, so every desk, DM and thread an
/// ACP teammate answered in was one durable conversation inside another
/// program.
#[test]
fn a_session_key_separates_conversations() {
    let keyer = keyer();
    let acme = CompanyId::new("acme");
    assert_ne!(
        keyer.session_key(&acme, "ceo", Some("engineering")),
        keyer.session_key(&acme, "ceo", Some("growth")),
        "two desks must not share one session"
    );
    assert_ne!(
        keyer.session_key(&acme, "ceo", Some("engineering")),
        keyer.session_key(&acme, "ceo", Some("dm:designer")),
        "nor a desk and a DM"
    );
    // Stable within a conversation, for the same reason it is stable across
    // turns at all.
    assert_eq!(
        keyer.session_key(&acme, "ceo", Some("engineering")),
        keyer.session_key(&acme, "ceo", Some("engineering"))
    );
}

/// The General desk is **one** conversation however it is spelled, folded
/// through the same rule every other reader of a chat id uses — otherwise
/// its four spellings would mint four sessions in an external process, and
/// an operator's own main line would forget itself depending on which id
/// the caller happened to address.
#[test]
fn every_spelling_of_the_general_desk_is_one_session() {
    let keyer = keyer();
    let acme = CompanyId::new("acme");
    let unaddressed = keyer.session_key(&acme, "ceo", None);
    for spelling in ["", "main", "General", "general", "MAIN"] {
        assert_eq!(
            keyer.session_key(&acme, "ceo", Some(spelling)),
            unaddressed,
            "{spelling:?} is the General desk"
        );
    }
}

/// A named desk's id and its display name are one session.
///
/// The key was `(company, agent)` before #1890 H, where no selector could
/// disagree with itself. Adding the chat introduced the possibility that a
/// client addressing one desk by id and another by name mints two sessions
/// in the external agent — and an ACP session is durable state in another
/// process with no lifecycle across the port, so the earlier one is not
/// merely re-created, its context is gone (codex on #1972).
#[test]
fn a_named_desks_two_spellings_are_one_session() {
    let keyer = keyer().with_desks(vec![("growth_desk".to_string(), "Growth".to_string())]);
    let acme = CompanyId::new("acme");
    assert_eq!(
        keyer.session_key(&acme, "ceo", Some("growth_desk")),
        keyer.session_key(&acme, "ceo", Some("Growth")),
        "one desk, one session, whichever spelling addressed it"
    );
    // …and an undeclared selector still keys on itself, which is what a
    // DM and an ad-hoc thread rely on.
    assert_ne!(
        keyer.session_key(&acme, "ceo", Some("growth_desk")),
        keyer.session_key(&acme, "ceo", Some("dm:designer"))
    );
}

/// The slot that serialises turns is **not** the session key.
///
/// Widening it alongside the session would let one teammate's desks prompt
/// an external process concurrently — a change to how that process is
/// driven, made as a side effect of a change about conversation scope.
#[test]
fn the_turn_slot_stays_per_teammate() {
    let keyer = keyer();
    let acme = CompanyId::new("acme");
    assert_eq!(
        AcpRunTurn::lock_key(&acme, "ceo"),
        AcpRunTurn::lock_key(&acme, "ceo"),
    );
    assert_ne!(
        AcpRunTurn::lock_key(&acme, "ceo"),
        AcpRunTurn::lock_key(&acme, "cto"),
    );
    // The point: two conversations of one teammate share a slot while
    // holding different sessions.
    assert_ne!(
        keyer.session_key(&acme, "ceo", Some("engineering")),
        keyer.session_key(&acme, "ceo", Some("growth")),
    );
}
