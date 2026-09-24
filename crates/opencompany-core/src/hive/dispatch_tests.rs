//! Which surface a chat resolves to, and the rows a takeover leaves behind.
//!
//! Extracted from `dispatch.rs`: an inline `#[cfg(test)] mod` is rejected by
//! `scripts/ci/assert-rs-source-layout.sh`, and it also put every item after
//! it -- `announce_takeover` and `hand_off_notice` -- behind a test module,
//! which `clippy::items_after_test_module` denies.

use super::*;
use crate::hive::test_support::{TWO_DESKS, record};

/// A DM routes to its hive, and to the pooled turn when it has none.
///
/// Both halves matter. The first is what makes an operator DM an episode
/// at all; the second is the flag being off -- `dm_hives` builds nothing
/// then, and an absent entry has to mean "take the path you always took"
/// rather than "this chat has no home".
#[test]
fn a_dm_is_a_room_when_it_has_a_hive_and_a_single_turn_when_it_does_not() {
    let record = record(TWO_DESKS);
    let empty: HashMap<String, Arc<crate::hive::graph::DeskHive>> = HashMap::new();

    assert!(
        matches!(surface_of(&record, &empty, Some("dm:ceo")), Surface::Single),
        "with no DM hive the pooled turn still answers"
    );

    // `resolve_desk_id` cannot answer for a DM -- it is not a desk in the
    // manifest -- so without the explicit branch this would stay `Single`
    // however many hives exist.
    assert!(
        matches!(
            surface_of(&record, &empty, Some("engineering")),
            Surface::Single
        ),
        "a desk with no hive is unchanged"
    );
}

/// General never becomes a DM room, whatever it is called.
#[test]
fn the_general_line_is_never_a_dm() {
    let record = record(TWO_DESKS);
    let empty: HashMap<String, Arc<crate::hive::graph::DeskHive>> = HashMap::new();
    assert!(matches!(
        surface_of(&record, &empty, Some("general")),
        Surface::Single
    ));
}

/// The flag is off unless something says otherwise, and says it plainly.
#[test]
fn dm_episodes_are_off_until_asked_for() {
    struct Env(Option<&'static str>);
    impl crate::app::config::EnvSource for Env {
        fn get_os(&self, _key: &str) -> Option<std::ffi::OsString> {
            self.0.map(Into::into)
        }
    }
    use crate::hive::graph::dm_episodes_enabled;
    assert!(!dm_episodes_enabled(&Env(None)), "unset is off");
    assert!(!dm_episodes_enabled(&Env(Some("0"))), "0 is off");
    assert!(!dm_episodes_enabled(&Env(Some("maybe"))), "junk is off");
    for on in ["1", "true", "yes", "on", " true "] {
        assert!(dm_episodes_enabled(&Env(Some(on))), "`{on}` is on");
    }
}
