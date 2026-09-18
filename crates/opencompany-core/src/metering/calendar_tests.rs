use super::*;

#[test]
fn epoch_zero_is_1970_01_01() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(iso_day(0), "1970-01-01");
    assert_eq!(days_from_civil(1970, 1, 1), 0);
}

#[test]
fn known_dates_round_trip() {
    // 2026-07-16 — the project's "today".
    let d = days_from_civil(2026, 7, 16);
    assert_eq!(civil_from_days(d), (2026, 7, 16));
    assert_eq!(iso_day(d), "2026-07-16");
}

#[test]
fn iso_day_from_millis_matches() {
    // 2021-01-01T00:00:00Z = 1_609_459_200_000 ms.
    assert_eq!(iso_day(epoch_day(1_609_459_200_000)), "2021-01-01");
    // A time later that same day stays on the same bucket.
    assert_eq!(
        iso_day(epoch_day(1_609_459_200_000 + 23 * 3_600_000)),
        "2021-01-01"
    );
}

#[test]
fn month_start_snaps_to_the_first() {
    // 2026-07-16T12:00:00Z.
    let mid_month = (days_from_civil(2026, 7, 16) as u64) * MILLIS_PER_DAY + 12 * 3_600_000;
    let start = month_start_millis(mid_month);
    assert_eq!(iso_day(epoch_day(start)), "2026-07-01");
    // The start of the month maps to itself.
    assert_eq!(month_start_millis(start), start);
}

#[test]
fn leap_day_is_valid() {
    let d = days_from_civil(2024, 2, 29);
    assert_eq!(civil_from_days(d), (2024, 2, 29));
}
