use super::*;

fn runner(id: &str, scope: &str, connection: &str, seen: u64) -> RunnerStatus {
    RunnerStatus {
        runner_id: id.to_string(),
        owner: "owner-1".to_string(),
        scopes: vec![scope.to_string()],
        capabilities: RunnerCapabilities {
            harnesses: vec![HarnessOffer {
                id: "claude".to_string(),
                ready: true,
            }],
            max_parallel: 2,
        },
        last_seen_millis: seen,
        connection: connection.to_string(),
    }
}

#[test]
fn a_fresh_runner_can_take_work() {
    let registry = RunnerRegistry::new();
    assert_eq!(
        registry.admit(runner("r1", "acme::ceo", "c1", 0)).evicted,
        None
    );
    assert_eq!(registry.available_for("acme::ceo", 0).len(), 1);
}

#[test]
fn a_second_runner_on_one_scope_evicts_the_first() {
    // Two copies of a desktop must not both take a company's work. The
    // newer connection wins: it is the one someone just made, and the older
    // is most often a socket nobody has noticed is dead.
    let registry = RunnerRegistry::new();
    registry.admit(runner("r1", "acme::ceo", "conn-1", 0));

    let admission = registry.admit(runner("r2", "acme::ceo", "conn-2", 0));
    assert_eq!(admission.evicted.as_deref(), Some("conn-1"));

    let available = registry.available_for("acme::ceo", 0);
    assert_eq!(available.len(), 1, "exactly one holds the scope");
    assert_eq!(available[0].runner_id, "r2");
}

#[test]
fn runners_on_different_scopes_coexist() {
    let registry = RunnerRegistry::new();
    assert_eq!(
        registry.admit(runner("r1", "acme::ceo", "c1", 0)).evicted,
        None
    );
    assert_eq!(
        registry.admit(runner("r2", "globex::cto", "c2", 0)).evicted,
        None
    );
    assert_eq!(registry.list().len(), 2);
}

#[test]
fn a_runner_that_stops_reporting_stops_being_scheduled_to() {
    let registry = RunnerRegistry::new();
    registry.admit(runner("r1", "acme::ceo", "c1", 0));

    assert_eq!(
        registry
            .available_for("acme::ceo", PRESENCE_TTL_MILLIS - 1)
            .len(),
        1
    );
    assert!(
        registry
            .available_for("acme::ceo", PRESENCE_TTL_MILLIS)
            .is_empty(),
        "a stale runner must not be given work"
    );
}

#[test]
fn a_heartbeat_keeps_a_runner_live() {
    let registry = RunnerRegistry::new();
    registry.admit(runner("r1", "acme::ceo", "c1", 0));
    assert!(registry.beat("r1", PRESENCE_TTL_MILLIS - 1));
    assert_eq!(
        registry
            .available_for("acme::ceo", PRESENCE_TTL_MILLIS + 1)
            .len(),
        1
    );
}

#[test]
fn a_heartbeat_from_an_unknown_runner_does_not_attach_it() {
    // Otherwise presence is a way to attach without ever proving an
    // identity — a runner that reconnects must handshake again.
    let registry = RunnerRegistry::new();
    assert!(!registry.beat("never-seen", 0));
    assert!(registry.list().is_empty());
}

#[test]
fn a_runner_with_no_ready_harness_is_attached_but_not_scheduled_to() {
    // Attached and useless is a real state — every harness signed out — and
    // scheduling to it would fail every task. It stays listed so an
    // operator can see *why* nothing is running.
    let registry = RunnerRegistry::new();
    let mut idle = runner("r1", "acme::ceo", "c1", 0);
    idle.capabilities.harnesses[0].ready = false;
    registry.admit(idle);

    assert!(registry.available_for("acme::ceo", 0).is_empty());
    assert_eq!(registry.list().len(), 1, "still visible to an operator");
}

#[test]
fn the_list_keeps_stale_runners_so_an_operator_can_see_them() {
    let registry = RunnerRegistry::new();
    registry.admit(runner("r1", "acme::ceo", "c1", 0));

    let listed = registry.list();
    assert_eq!(listed.len(), 1);
    assert!(
        !listed[0].is_live(PRESENCE_TTL_MILLIS),
        "stale, and still shown"
    );
}

#[test]
fn a_sweep_drops_only_the_quiet_ones() {
    let registry = RunnerRegistry::new();
    registry.admit(runner("stale", "acme::ceo", "c1", 0));
    registry.admit(runner("fresh", "globex::cto", "c2", PRESENCE_TTL_MILLIS));

    assert_eq!(registry.sweep(PRESENCE_TTL_MILLIS), 1);
    assert_eq!(registry.list().len(), 1);
    assert_eq!(registry.list()[0].runner_id, "fresh");
}
