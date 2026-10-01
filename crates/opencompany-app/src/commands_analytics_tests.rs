use opencompany::app::config::MapEnv;

use super::*;

fn no_env() -> MapEnv {
    MapEnv::new(Vec::<(&str, &str)>::new())
}

#[test]
fn a_fresh_install_is_on_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let view = read_preference(dir.path(), &no_env(), None);
    assert!(view.enabled);
    assert_eq!(view.source, "default");
    assert!(!view.restart_required, "nothing is running to restart");
}

#[test]
fn opting_out_flips_the_gate_and_is_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let gate = ConsentGate::new(true);
    apply_preference(dir.path(), &gate, false).unwrap();
    assert!(!gate.is_enabled());

    let view = read_preference(dir.path(), &no_env(), None);
    assert!(!view.enabled);
    assert_eq!(view.source, "setting");
}

#[test]
fn opting_back_in_is_saved_and_re_enables_the_gate() {
    let dir = tempfile::tempdir().unwrap();
    let gate = ConsentGate::new(true);
    apply_preference(dir.path(), &gate, false).unwrap();
    apply_preference(dir.path(), &gate, true).unwrap();
    assert!(gate.is_enabled());
    assert!(read_preference(dir.path(), &no_env(), None).enabled);
}

/// A full or read-only disk must not keep events flowing after an opt-out.
#[test]
fn an_opt_out_holds_even_when_the_write_fails() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("not-a-directory");
    std::fs::write(&blocker, b"x").unwrap();
    let gate = ConsentGate::new(true);
    assert!(apply_preference(&blocker, &gate, false).is_err());
    assert!(!gate.is_enabled());
}

#[test]
fn an_operator_off_is_reported_as_the_env() {
    let dir = tempfile::tempdir().unwrap();
    let env = MapEnv::new([("OPENCOMPANY_ANALYTICS", "off")]);
    let view = read_preference(dir.path(), &env, None);
    assert!(!view.enabled);
    assert_eq!(view.source, "env");
}

/// Wanting it on while this launch is not reporting is the one case that needs
/// a restart, and the page has to be told.
#[test]
fn an_opt_in_from_a_silent_launch_asks_for_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut status = AnalyticsStatus::not_wired();
    status.decision = "off";
    let view = read_preference(dir.path(), &no_env(), Some(status.clone()));
    assert!(view.restart_required);

    status.decision = "reporting";
    assert!(!read_preference(dir.path(), &no_env(), Some(status)).restart_required);
}
