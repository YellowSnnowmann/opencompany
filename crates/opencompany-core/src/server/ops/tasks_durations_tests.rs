use super::*;

/// 2026-08-05 09:00:00 UTC.
const T0: u64 = 1_785_920_400_000;

fn entry(at: u64, kind: &str, waited: Option<u64>) -> TimelineEntry {
    TimelineEntry {
        seq: at,
        at_millis: at,
        kind: kind.to_string(),
        label: kind.to_string(),
        detail: None,
        cost_key: None,
        cost: None,
        waited_millis: waited,
    }
}

/// A re-dispatched card accumulates every worked window, not just the first.
#[test]
fn worked_accumulates_every_dispatch_window() {
    let d = TaskDurations::compute(
        &[
            entry(T0, "dispatched", None),
            entry(T0 + 60_000, "completed", None),
            entry(T0 + 600_000, "dispatched", None),
            entry(T0 + 720_000, "completed", None),
        ],
        None,
        T0,
    );
    assert_eq!(d.worked_millis, 180_000);
    assert!(!d.worked_live);
}

/// A `completed` with no open window is legacy data: skipped, not counted
/// from zero, which would report the whole epoch as worked time.
#[test]
fn a_completion_without_a_dispatch_is_not_counted_from_zero() {
    let d = TaskDurations::compute(&[entry(T0 + 60_000, "completed", None)], None, T0);
    assert_eq!(d.worked_millis, 0);
}

/// Overlapping waits are merged, not summed twice — the company waited once.
#[test]
fn overlapping_waits_are_merged() {
    let d = TaskDurations::compute(
        &[
            entry(T0 + 300_000, "approval", Some(300_000)), // [T0,     T0+5m]
            entry(T0 + 420_000, "approval", Some(300_000)), // [T0+2m,  T0+7m]
        ],
        None,
        T0,
    );
    assert_eq!(d.waiting_millis, 420_000, "the overlap was counted twice");
    assert!(!d.waiting_live);
}

/// A sign-off resolved instantly (`0`) or with no recoverable park instant
/// (`None`) is not a span. Counting either would invent waiting time.
#[test]
fn an_instant_or_unknown_sign_off_is_not_a_wait() {
    let d = TaskDurations::compute(
        &[
            entry(T0 + 60_000, "approval", Some(0)),
            entry(T0 + 120_000, "approval", None),
        ],
        None,
        T0,
    );
    assert_eq!(d.waiting_millis, 0);
}

/// The live halves run to `as_of`, and `*_at` extends them past it.
///
/// This is the property the console's 1s tick and the exporter both rely on:
/// a reader adds elapsed time to the live half and nothing else, because
/// every closed span already ended before `as_of_millis`.
#[test]
fn live_spans_extend_exactly_and_sealed_ones_do_not() {
    let live = TaskDurations::compute(
        &[entry(T0, "dispatched", None)],
        Some(T0 + 60_000),
        T0 + 300_000,
    );
    assert!(live.worked_live && live.waiting_live);
    assert_eq!(live.worked_millis, 300_000);
    assert_eq!(live.waiting_millis, 240_000);
    // One more minute of wall clock adds one minute to each live half.
    assert_eq!(live.worked_at(T0 + 360_000), 360_000);
    assert_eq!(live.waiting_at(T0 + 360_000), 300_000);

    let sealed = TaskDurations::compute(
        &[
            entry(T0, "dispatched", None),
            entry(T0 + 600_000, "completed", None),
        ],
        None,
        T0 + 600_000,
    );
    assert!(!sealed.worked_live && !sealed.waiting_live);
    // A finished task's totals do not move, however late it is read.
    assert_eq!(sealed.worked_at(T0 + 99_000_000), 600_000);
    assert_eq!(sealed.waiting_at(T0 + 99_000_000), 0);
}

/// A client clock behind the host's cannot subtract from a total.
#[test]
fn a_reader_clock_behind_the_host_does_not_go_backwards() {
    let d = TaskDurations::compute(&[entry(T0, "dispatched", None)], None, T0 + 300_000);
    assert_eq!(d.worked_at(T0), 300_000);
}
