use super::*;

// As `SteerStopHook`'s own test records: `TurnState.cost` names
// `openhuman::agent::cost::TurnCost`, whose module is crate-private to
// openhuman, so a `TurnState` cannot be constructed from here and the
// hook's Continue/Stop decision cannot be unit-tested. It is proven
// end-to-end instead, by `harness::spend_halt_turn_tests`, which drives a
// real turn through `with_stop_hooks` against a scripted provider.
//
// What is testable here is the wiring either side of that decision: the
// name the delegate lends it, and that the flag starts clear and is shared
// rather than copied.
#[test]
fn the_hook_borrows_the_delegates_name() {
    let hook = SpendStopHook::new(1.0);
    assert_eq!(
        hook.name(),
        BudgetStopHook::new(1.0).name(),
        "a second spelling would make one halt look like two in the trace"
    );
}

#[test]
fn the_flag_starts_clear_and_is_shared_not_copied() {
    let hook = SpendStopHook::new(1.0);
    let flag = hook.halted();
    assert!(
        !flag.load(Ordering::SeqCst),
        "a turn that has not run cannot have been halted"
    );
    // The handle the turn keeps must observe what the hook stores, or the
    // halt is recorded into a copy nobody reads — the exact failure this
    // module exists to prevent.
    hook.halted.store(true, Ordering::SeqCst);
    assert!(flag.load(Ordering::SeqCst));
}
