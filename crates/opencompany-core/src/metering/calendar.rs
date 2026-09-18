//! Minimal proleptic-Gregorian date math over epoch millis — no `chrono`/`time`
//! dependency. Uses Howard Hinnant's public-domain `civil_from_days` /
//! `days_from_civil` algorithms so the metering projections can label daily
//! buckets and find the current-month boundary offline.
//!
//! All arithmetic is UTC. "Epoch day" is the count of whole days since
//! 1970-01-01.

/// Milliseconds in one UTC day.
pub const MILLIS_PER_DAY: u64 = 86_400_000;

/// The epoch day (days since 1970-01-01, UTC) an instant falls on.
pub fn epoch_day(at_millis: u64) -> i64 {
    (at_millis / MILLIS_PER_DAY) as i64
}

/// The `(year, month, day)` of an epoch day, using Hinnant's `civil_from_days`.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

/// The epoch day for a civil `(year, month, day)`, using Hinnant's
/// `days_from_civil`.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let m = m as i64;
    let d = d as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// The ISO `YYYY-MM-DD` label for an epoch day.
pub fn iso_day(epoch_day: i64) -> String {
    let (y, m, d) = civil_from_days(epoch_day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The epoch-millis start (00:00 UTC on the 1st) of the month an instant falls
/// in.
pub fn month_start_millis(at_millis: u64) -> u64 {
    let (y, m, _) = civil_from_days(epoch_day(at_millis));
    let first = days_from_civil(y, m, 1);
    (first as u64) * MILLIS_PER_DAY
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;
