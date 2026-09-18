use super::*;

/// Builds a `CivilTime` from a `YYYY-MM-DD HH:MM` UTC literal via its unix
/// millis, exercising the same path the scheduler uses.
fn at(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> CivilTime {
    let ms = ((days_from_civil(year, month, day) as u64) * DAY_MS)
        + (hour as u64) * 3_600_000
        + (minute as u64) * MINUTE_MS;
    CivilTime::from_unix_millis(ms)
}

#[test]
fn epoch_is_a_thursday() {
    let t = CivilTime::from_unix_millis(0);
    assert_eq!((t.year, t.month, t.day), (1970, 1, 1));
    assert_eq!(t.weekday, 4); // Thursday
    assert_eq!((t.hour, t.minute), (0, 0));
}

#[test]
fn known_weekdays_round_trip() {
    // 2026-07-10 is a Friday (weekday 5).
    let friday = at(2026, 7, 10, 12, 0);
    assert_eq!(friday.weekday, 5);
    // 2000-01-01 was a Saturday (weekday 6).
    assert_eq!(at(2000, 1, 1, 0, 0).weekday, 6);
    // 2024-02-29 (leap day) was a Thursday (weekday 4).
    assert_eq!(at(2024, 2, 29, 0, 0).weekday, 4);
}

#[test]
fn every_field_wildcard_matches_any_minute() {
    let expr = CronExpr::parse("* * * * *").unwrap();
    assert!(expr.matches(&at(2026, 7, 10, 3, 27)));
    assert!(expr.matches(&at(1999, 12, 31, 23, 59)));
}

#[test]
fn step_minute_matches_quarter_hours() {
    let expr = CronExpr::parse("*/15 * * * *").unwrap();
    for minute in [0, 15, 30, 45] {
        assert!(expr.matches(&at(2026, 7, 10, 9, minute)), "minute {minute}");
    }
    for minute in [1, 14, 16, 29, 44, 59] {
        assert!(
            !expr.matches(&at(2026, 7, 10, 9, minute)),
            "minute {minute}"
        );
    }
}

#[test]
fn named_weekday_matches_only_that_day() {
    // 09:00 every Monday. 2026-07-13 is a Monday; 2026-07-14 a Tuesday.
    let expr = CronExpr::parse("0 9 * * MON").unwrap();
    assert!(expr.matches(&at(2026, 7, 13, 9, 0)));
    assert!(!expr.matches(&at(2026, 7, 14, 9, 0)));
    assert!(!expr.matches(&at(2026, 7, 13, 10, 0)));
}

#[test]
fn sunday_accepts_zero_and_seven() {
    let zero = CronExpr::parse("0 0 * * 0").unwrap();
    let seven = CronExpr::parse("0 0 * * 7").unwrap();
    // 2026-07-12 is a Sunday.
    let sunday = at(2026, 7, 12, 0, 0);
    assert!(zero.matches(&sunday));
    assert!(seven.matches(&sunday));
    assert_eq!(zero, seven);
}

#[test]
fn day_of_month_field() {
    let expr = CronExpr::parse("0 0 1 * *").unwrap();
    assert!(expr.matches(&at(2026, 3, 1, 0, 0)));
    assert!(!expr.matches(&at(2026, 3, 2, 0, 0)));
}

#[test]
fn named_month_range_and_business_hours() {
    // 09:00 and 17:00 on weekdays (Mon–Fri).
    let expr = CronExpr::parse("0 9,17 * * 1-5").unwrap();
    assert!(expr.matches(&at(2026, 7, 10, 9, 0))); // Friday 09:00
    assert!(expr.matches(&at(2026, 7, 10, 17, 0))); // Friday 17:00
    assert!(!expr.matches(&at(2026, 7, 11, 9, 0))); // Saturday
    assert!(!expr.matches(&at(2026, 7, 10, 12, 0))); // midday, not on the hour list

    let named = CronExpr::parse("0 0 * JAN-MAR *").unwrap();
    assert!(named.matches(&at(2026, 2, 15, 0, 0)));
    assert!(!named.matches(&at(2026, 4, 1, 0, 0)));
}

#[test]
fn day_or_rule_when_both_restricted() {
    // "Fire on the 1st OR on any Monday." 2026-07-13 is a Monday but not the
    // 1st; 2026-07-01 is the 1st (a Wednesday).
    let expr = CronExpr::parse("0 0 1 * MON").unwrap();
    assert!(expr.matches(&at(2026, 7, 13, 0, 0))); // Monday
    assert!(expr.matches(&at(2026, 7, 1, 0, 0))); // the 1st
    assert!(!expr.matches(&at(2026, 7, 8, 0, 0))); // neither
}

#[test]
fn next_after_finds_the_following_tick() {
    let expr = CronExpr::parse("*/15 * * * *").unwrap();
    let next = expr.next_after(&at(2026, 7, 10, 9, 7)).unwrap();
    assert_eq!((next.hour, next.minute), (9, 15));

    let weekly = CronExpr::parse("0 9 * * MON").unwrap();
    // From Friday, the next Monday 09:00 is 2026-07-13.
    let next = weekly.next_after(&at(2026, 7, 10, 9, 0)).unwrap();
    assert_eq!((next.year, next.month, next.day), (2026, 7, 13));
    assert_eq!((next.hour, next.minute), (9, 0));
}

/// The epoch minute of a UTC civil minute, the unit `prev_match_before`
/// speaks in.
fn minute_of(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
    ((days_from_civil(year, month, day) as u64) * DAY_MS
        + (hour as u64) * 3_600_000
        + (minute as u64) * MINUTE_MS)
        / MINUTE_MS
}

#[test]
fn prev_match_before_finds_the_preceding_tick() {
    let expr = CronExpr::parse("*/15 * * * *").unwrap();
    // Strictly before :07 the previous quarter-hour is :00.
    let from = minute_of(2026, 7, 10, 9, 7);
    assert_eq!(
        expr.prev_match_before(from, 60),
        Some(minute_of(2026, 7, 10, 9, 0))
    );
    // Strictly before an exact match (:15) returns the one before it (:00),
    // never :15 itself — the scan is exclusive of `before_minute`.
    let on_match = minute_of(2026, 7, 10, 9, 15);
    assert_eq!(
        expr.prev_match_before(on_match, 60),
        Some(minute_of(2026, 7, 10, 9, 0))
    );
}

#[test]
fn prev_match_before_respects_the_window_bound() {
    // Weekly Monday 09:00. From the following Wednesday the previous fire is
    // ~2 days back (2880 minutes).
    let weekly = CronExpr::parse("0 9 * * MON").unwrap();
    let wednesday = minute_of(2026, 7, 15, 9, 0);
    let monday = minute_of(2026, 7, 13, 9, 0);
    // A window that reaches the Monday finds it…
    assert_eq!(weekly.prev_match_before(wednesday, 10_080), Some(monday));
    // …but a window too short to reach it finds nothing rather than walking
    // further back.
    assert_eq!(weekly.prev_match_before(wednesday, 60), None);
}

#[test]
fn next_after_crosses_the_leap_day() {
    let expr = CronExpr::parse("0 0 29 2 *").unwrap();
    // From 2025 (not a leap year) the next 29 Feb is 2028.
    let next = expr.next_after(&at(2025, 3, 1, 0, 0)).unwrap();
    assert_eq!((next.year, next.month, next.day), (2028, 2, 29));
}

/// The whole point of issue #262: `0 9 * * *` and `9 0 * * *` are both
/// valid, differ by two characters, and mean nine hours apart. Validation
/// cannot help — neither is wrong — so the description is the only thing
/// standing between an author and a report that arrives at the wrong time.
#[test]
fn describes_the_nine_am_versus_nine_past_midnight_confusion() {
    assert_eq!(
        CronExpr::parse("0 9 * * *").unwrap().describe().as_deref(),
        Some("Every day at 09:00 UTC")
    );
    assert_eq!(
        CronExpr::parse("9 0 * * *").unwrap().describe().as_deref(),
        Some("Every day at 00:09 UTC")
    );
}

#[test]
fn describes_the_common_shapes() {
    let describe = |expr: &str| CronExpr::parse(expr).unwrap().describe();
    assert_eq!(describe("* * * * *").as_deref(), Some("Every minute"));
    assert_eq!(
        describe("*/15 * * * *").as_deref(),
        Some("Every 15 minutes")
    );
    assert_eq!(describe("5 * * * *").as_deref(), Some("Every hour at :05"));
    assert_eq!(
        describe("0 9 * * MON").as_deref(),
        Some("Every Mon at 09:00 UTC")
    );
    assert_eq!(
        describe("0 9,17 * * 1-5").as_deref(),
        Some("At 09:00 and 17:00 UTC on Mon-Fri")
    );
    assert_eq!(
        describe("*/30 * * * 1-5").as_deref(),
        Some("Every 30 minutes on Mon-Fri")
    );
    assert_eq!(
        describe("0 8,12,17 * * *").as_deref(),
        Some("At 08:00, 12:00 and 17:00 UTC")
    );
    // Every preset the console's schedule picker offers must describe —
    // an author who picks "Daily — 09:00 UTC" from a menu should not then
    // see a blank preview.
    assert_eq!(describe("0 * * * *").as_deref(), Some("Every hour at :00"));
}

/// `describe` returning `None` is a designed answer, not a bug: the caller
/// still shows the next fire times, which state the schedule exactly. A
/// wrong paraphrase would be worse than none.
#[test]
fn declines_to_describe_gnarlier_shapes() {
    let describe = |expr: &str| CronExpr::parse(expr).unwrap().describe();
    assert_eq!(describe("0 0 1 * *"), None); // day-of-month restricted
    assert_eq!(describe("0 0 * JAN-MAR *"), None); // month restricted
    assert_eq!(describe("* 9 * * *"), None); // every minute of one hour
    assert_eq!(describe("0 1,3,5,7,9 * * *"), None); // too many hours to list
    // `*/7` is 0,7,…,56 — the wrap back to 0 is a 4-minute gap, so "every
    // 7 minutes" would be wrong once an hour.
    assert_eq!(describe("*/7 * * * *"), None);
}

/// The preview hands the console epoch millis so the UTC reading and the
/// viewer's local reading are two renderings of ONE instant.
#[test]
fn unix_millis_round_trips_a_fire_time() {
    let expr = CronExpr::parse("0 9 * * *").unwrap();
    let next = expr.next_after(&at(2026, 8, 2, 12, 0)).unwrap();
    let millis = next.unix_millis();
    let back = CivilTime::from_unix_millis(millis);
    assert_eq!(back, next);
    assert_eq!((back.year, back.month, back.day), (2026, 8, 3));
    assert_eq!((back.hour, back.minute), (9, 0));
}

#[test]
fn rejects_malformed_expressions() {
    assert!(CronExpr::parse("* * * *").is_err()); // 4 fields
    assert!(CronExpr::parse("* * * * * *").is_err()); // 6 fields
    assert!(CronExpr::parse("60 * * * *").is_err()); // minute out of range
    assert!(CronExpr::parse("* 24 * * *").is_err()); // hour out of range
    assert!(CronExpr::parse("*/0 * * * *").is_err()); // zero step
    assert!(CronExpr::parse("5-1 * * * *").is_err()); // descending range
    assert!(CronExpr::parse("* * * FOO *").is_err()); // bad month name
}
