//! A dependency-light 5-field cron matcher.
//!
//! Parses a standard `minute hour day-of-month month day-of-week` expression
//! into a [`CronExpr`], then answers two questions against a wall-clock-free
//! [`CivilTime`]:
//!
//! - [`CronExpr::matches`] — does this minute satisfy the schedule?
//! - [`CronExpr::next_after`] — what is the next minute that does?
//!
//! No `chrono`/`cron` dependency: civil-time conversion is hand-rolled
//! (days-from-epoch, Howard Hinnant's algorithms) so the whole matcher is std
//! only and trivially testable with a fake clock. Every field supports `*`,
//! comma lists, `a-b` ranges, `*/step` (and `a-b/step`), and — for the month
//! and weekday fields — the usual three-letter names (`JAN`…`DEC`,
//! `SUN`…`SAT`). Day-of-week accepts both `0` and `7` for Sunday.
//!
//! Day-of-month and day-of-week combine with cron's historical "or" rule: when
//! *both* fields are restricted (neither is `*`), a day matches if it satisfies
//! *either* field; otherwise the two combine with the usual "and".

use crate::Result;
use crate::error::OpenCompanyError;

/// Milliseconds in one minute.
const MINUTE_MS: u64 = 60_000;
/// Milliseconds in one day.
const DAY_MS: u64 = 86_400_000;
/// Upper bound on the minute-by-minute search in [`CronExpr::next_after`].
///
/// Four years (with a leap day) is the widest gap any valid 5-field expression
/// can have between fire times (e.g. `0 0 29 2 *`), so a match is guaranteed
/// inside this window when one exists.
const NEXT_SEARCH_LIMIT_MINUTES: u64 = 4 * 366 * 24 * 60;

/// The set of permitted values for one cron field, as a 64-bit mask.
///
/// Bit `v` is set when value `v` is allowed. Every cron field value fits in
/// `0..=59`, well inside 64 bits. `restricted` records whether the source field
/// was anything other than `*`, which the day-of-month/day-of-week "or" rule
/// depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FieldSet {
    mask: u64,
    restricted: bool,
}

impl FieldSet {
    /// Whether `value` is a member of this set.
    fn contains(&self, value: u32) -> bool {
        value < 64 && self.mask & (1u64 << value) != 0
    }

    /// The permitted values, ascending. Used only by [`CronExpr::describe`],
    /// which reads the parsed masks rather than re-reading the source text —
    /// so the description can never disagree with what [`CronExpr::matches`]
    /// will actually do.
    fn values(&self) -> Vec<u32> {
        (0..64).filter(|v| self.contains(*v)).collect()
    }

    /// The single permitted value, when the set holds exactly one.
    fn single(&self) -> Option<u32> {
        let values = self.values();
        match values.len() {
            1 => Some(values[0]),
            _ => None,
        }
    }
}

/// A parsed 5-field cron expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CronExpr {
    minute: FieldSet,
    hour: FieldSet,
    dom: FieldSet,
    month: FieldSet,
    dow: FieldSet,
}

impl CronExpr {
    /// Parses a standard 5-field cron expression.
    ///
    /// Returns [`OpenCompanyError::InvalidRequest`] with a prosumer-readable
    /// message when the expression does not have exactly five fields or a field
    /// is malformed or out of range.
    pub fn parse(expr: &str) -> Result<Self> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(OpenCompanyError::InvalidRequest(format!(
                "cron `{expr}` needs 5 fields (minute hour day month weekday), found {}",
                fields.len()
            )));
        }
        Ok(Self {
            minute: parse_field(fields[0], 0, 59, &[])?,
            hour: parse_field(fields[1], 0, 23, &[])?,
            dom: parse_field(fields[2], 1, 31, &[])?,
            month: parse_field(fields[3], 1, 12, MONTHS)?,
            dow: parse_dow(fields[4])?,
        })
    }

    /// Whether `t` (truncated to the minute) satisfies this schedule.
    pub fn matches(&self, t: &CivilTime) -> bool {
        if !self.minute.contains(t.minute)
            || !self.hour.contains(t.hour)
            || !self.month.contains(t.month)
        {
            return false;
        }
        // Cron's day rule: OR the two day fields when both are restricted.
        let dom_hit = self.dom.contains(t.day);
        let dow_hit = self.dow.contains(t.weekday);
        if self.dom.restricted && self.dow.restricted {
            dom_hit || dow_hit
        } else {
            dom_hit && dow_hit
        }
    }

    /// The next minute strictly after `t` that satisfies this schedule.
    ///
    /// Searches minute-by-minute over a bounded window (four leap years), so it
    /// returns `None` only for an expression that can never fire (impossible for
    /// a value that parsed successfully). Purely arithmetic — no wall clock.
    pub fn next_after(&self, t: &CivilTime) -> Option<CivilTime> {
        let start = t.floor_to_minute_millis();
        let mut cursor = start + MINUTE_MS;
        for _ in 0..NEXT_SEARCH_LIMIT_MINUTES {
            let candidate = CivilTime::from_unix_millis(cursor);
            if self.matches(&candidate) {
                return Some(candidate);
            }
            cursor += MINUTE_MS;
        }
        None
    }

    /// The most recent epoch **minute** strictly before `before_minute` that
    /// satisfies this schedule, scanning back at most `limit` minutes; `None`
    /// when no match falls inside that window.
    ///
    /// The bounded reverse mirror of [`next_after`](Self::next_after), working in
    /// epoch minutes (`unix_millis / 60_000`) rather than [`CivilTime`] because
    /// its one caller — the restart catch-up (issue #241) — compares against the
    /// `latest_fire` anchor, which is a minute. The `limit` bound is what keeps a
    /// long downtime from walking the schedule back years: the catch-up passes a
    /// one-week window, so at most `limit` minutes are probed and the scan is
    /// `O(limit)`, never unbounded. Purely arithmetic — no wall clock.
    pub fn prev_match_before(&self, before_minute: u64, limit: u64) -> Option<u64> {
        let mut minute = before_minute;
        for _ in 0..limit {
            if minute == 0 {
                break; // epoch floor: nothing before minute 0
            }
            minute -= 1;
            if self.matches(&CivilTime::from_unix_millis(minute * MINUTE_MS)) {
                return Some(minute);
            }
        }
        None
    }

    /// A plain-English gloss of this schedule, or `None` when the shape is
    /// gnarlier than a one-liner can honestly state.
    ///
    /// Issue #262: cron is easy to write and hard to read — `0 9 * * *` and
    /// `9 0 * * *` are both valid and only one is 9am — and the dialect is UTC,
    /// so an author in IST who wants a 9am report gets one at 14:30 local.
    /// Neither mistake is *invalid*, so validation can never catch them; the
    /// only defence is echoing the parsed meaning back.
    ///
    /// Derived from the parsed field masks, never from the source string, so
    /// this cannot drift from [`matches`](Self::matches). `None` is a first-class
    /// answer, not a failure: it means "I will not paraphrase this", and the
    /// caller still has the next fire times, which say the same thing without
    /// any risk of a confidently wrong gloss. Widening the covered shapes is
    /// safe by construction — anything not recognised keeps returning `None`.
    ///
    /// Recognised shapes (each optionally narrowed by a weekday field):
    ///
    /// | Expression | Description |
    /// |---|---|
    /// | `* * * * *` | `Every minute` |
    /// | `*/15 * * * *` | `Every 15 minutes` |
    /// | `5 * * * *` | `Every hour at :05` |
    /// | `0 9 * * *` | `Every day at 09:00 UTC` |
    /// | `0 9 * * MON` | `Every Mon at 09:00 UTC` |
    /// | `0 9,17 * * 1-5` | `At 09:00 and 17:00 UTC on Mon-Fri` |
    pub fn describe(&self) -> Option<String> {
        // A restricted month or day-of-month ("the 3rd of every other month")
        // takes more words to state than the expression takes to read, so it is
        // left to the next-fire times.
        if self.month.restricted || self.dom.restricted {
            return None;
        }
        let days = self.describe_weekdays()?;
        // `on Mon-Fri` / `` — appended to whichever body is built below.
        let on_days = days
            .as_deref()
            .map(|d| format!(" on {d}"))
            .unwrap_or_default();

        // An unrestricted hour means the schedule repeats within every hour.
        if !self.hour.restricted {
            if !self.minute.restricted {
                return Some(format!("Every minute{on_days}"));
            }
            if let Some(step) = self.minute_step() {
                return Some(format!("Every {step} minutes{on_days}"));
            }
            let minute = self.minute.single()?;
            return Some(format!("Every hour at :{minute:02}{on_days}"));
        }

        // A restricted hour with a wildcard (or multi-valued) minute fires many
        // times inside the named hours — not a clean "at HH:MM".
        let minute = self.minute.single()?;
        let hours = self.hour.values();
        // Beyond a handful of hours the list stops reading as a sentence.
        if hours.len() > 4 {
            return None;
        }
        let times: Vec<String> = hours
            .iter()
            .map(|hour| format!("{hour:02}:{minute:02}"))
            .collect();
        if let [only] = times.as_slice() {
            // `Every day at 09:00 UTC` / `Every Mon at 09:00 UTC` — the weekday
            // replaces "day" here rather than trailing, which is how the phrase
            // is said out loud.
            let subject = days.unwrap_or_else(|| "day".to_string());
            return Some(format!("Every {subject} at {only} UTC"));
        }
        Some(format!("At {} UTC{on_days}", join_and(&times)))
    }

    /// The weekday phrase for this expression: `None` = every day (the field is
    /// `*`, or names all seven days), `Some("Mon-Fri")` for a contiguous run,
    /// `Some("Mon, Wed")` for a list. The outer `Option` is `None` when the
    /// field is too long to read as a phrase.
    fn describe_weekdays(&self) -> Option<Option<String>> {
        if !self.dow.restricted {
            return Some(None);
        }
        let days = self.dow.values();
        if days.len() >= WEEKDAYS.len() {
            return Some(None);
        }
        let names: Vec<&'static str> = days
            .iter()
            .map(|d| title_case_weekday(WEEKDAYS[*d as usize]))
            .collect();
        // A contiguous run reads as a range; anything else as a list. More than
        // three scattered days is a list nobody parses at a glance.
        let contiguous = days.windows(2).all(|w| w[1] == w[0] + 1);
        if contiguous && days.len() > 2 {
            return Some(Some(format!("{}-{}", names[0], names[names.len() - 1])));
        }
        if names.len() > 3 {
            return None;
        }
        Some(Some(names.join(", ")))
    }

    /// The step of an evenly-spaced minute field starting at `0`, when it has
    /// one — `*/15` → `Some(15)`.
    ///
    /// Requires the step to divide 60: `*/7` yields 0,7,…,56, and the 56 → 0
    /// wrap is a 4-minute gap, so calling it "every 7 minutes" would be a lie
    /// once an hour.
    fn minute_step(&self) -> Option<u32> {
        let values = self.minute.values();
        if values.len() < 2 || values[0] != 0 {
            return None;
        }
        let step = values[1];
        if 60 % step != 0 || values.len() as u32 != 60 / step {
            return None;
        }
        values
            .windows(2)
            .all(|w| w[1] - w[0] == step)
            .then_some(step)
    }
}

/// `MON` → `Mon`. The parser's tables are upper-case; a description is prose.
fn title_case_weekday(name: &str) -> &'static str {
    match name {
        "SUN" => "Sun",
        "MON" => "Mon",
        "TUE" => "Tue",
        "WED" => "Wed",
        "THU" => "Thu",
        "FRI" => "Fri",
        _ => "Sat",
    }
}

/// `["09:00", "17:00"]` → `"09:00 and 17:00"`; three or more use commas.
fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [only] => only.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

/// Three-letter month names, index 0 = `JAN` (value 1).
const MONTHS: &[&str] = &[
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];
/// Three-letter weekday names, index 0 = `SUN` (value 0).
const WEEKDAYS: &[&str] = &["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// Parses the day-of-week field, normalizing `7` (and `SUN`) to `0`.
fn parse_dow(spec: &str) -> Result<FieldSet> {
    // Accept 0..=7 on input, then fold bit 7 down onto bit 0 (both = Sunday) so
    // matching against a `0..=6` civil weekday works uniformly.
    let mut set = parse_field(spec, 0, 7, WEEKDAYS)?;
    if set.mask & (1u64 << 7) != 0 {
        set.mask &= !(1u64 << 7);
        set.mask |= 1u64;
    }
    Ok(set)
}

/// Parses one cron field into a [`FieldSet`] bounded by `[min, max]`.
///
/// `names` maps three-letter aliases to values (`names[i]` = `min + i`); it is
/// empty for the purely numeric fields.
fn parse_field(spec: &str, min: u32, max: u32, names: &[&str]) -> Result<FieldSet> {
    let mut mask = 0u64;
    // A field is "unrestricted" only when it is exactly `*`; any list, range, or
    // step restricts it. This flag drives the day-of-month/day-of-week or-rule.
    let restricted = spec.trim() != "*";
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(field_error(spec, "an empty item"));
        }
        // Split off an optional `/step`.
        let (range_spec, step) = match part.split_once('/') {
            Some((range, step_str)) => {
                let step: u32 = step_str
                    .parse()
                    .map_err(|_| field_error(spec, "a non-numeric step"))?;
                if step == 0 {
                    return Err(field_error(spec, "a zero step"));
                }
                (range, step)
            }
            None => (part, 1),
        };

        let (lo, hi) = if range_spec == "*" {
            (min, max)
        } else if let Some((a, b)) = range_spec.split_once('-') {
            (
                resolve_value(a, min, max, names, spec)?,
                resolve_value(b, min, max, names, spec)?,
            )
        } else {
            let value = resolve_value(range_spec, min, max, names, spec)?;
            (value, value)
        };
        if lo > hi {
            return Err(field_error(spec, "a descending range"));
        }
        let mut value = lo;
        while value <= hi {
            mask |= 1u64 << value;
            value += step;
        }
    }
    if mask == 0 {
        return Err(field_error(spec, "no values"));
    }
    Ok(FieldSet { mask, restricted })
}

/// Resolves a single numeric-or-named token to a value inside `[min, max]`.
fn resolve_value(token: &str, min: u32, max: u32, names: &[&str], spec: &str) -> Result<u32> {
    let token = token.trim();
    let value = if let Some(idx) = names.iter().position(|n| n.eq_ignore_ascii_case(token)) {
        min + idx as u32
    } else {
        token
            .parse::<u32>()
            .map_err(|_| field_error(spec, "an unrecognized value"))?
    };
    if value < min || value > max {
        return Err(field_error(spec, "a value out of range"));
    }
    Ok(value)
}

/// Builds a uniform field-parse error.
fn field_error(spec: &str, why: &str) -> OpenCompanyError {
    OpenCompanyError::InvalidRequest(format!("cron field `{spec}` has {why}"))
}

/// A minute-granular civil (UTC) timestamp derived from unix milliseconds.
///
/// Carries the pre-computed weekday (`0` = Sunday) so the matcher never touches
/// a wall clock. Constructed with [`CivilTime::from_unix_millis`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilTime {
    /// Proleptic Gregorian year (e.g. `2026`).
    pub year: i64,
    /// Month of year, `1`–`12`.
    pub month: u32,
    /// Day of month, `1`–`31`.
    pub day: u32,
    /// Hour of day, `0`–`23`.
    pub hour: u32,
    /// Minute of hour, `0`–`59`.
    pub minute: u32,
    /// Day of week, `0` = Sunday … `6` = Saturday.
    pub weekday: u32,
}

impl CivilTime {
    /// Converts unix epoch milliseconds (UTC) into civil fields.
    pub fn from_unix_millis(ms: u64) -> Self {
        let days = (ms / DAY_MS) as i64;
        let rem = ms % DAY_MS;
        let hour = (rem / 3_600_000) as u32;
        let minute = ((rem % 3_600_000) / MINUTE_MS) as u32;
        // Epoch day 0 (1970-01-01) is a Thursday; `(days + 4) % 7` maps to the
        // Sunday-indexed weekday. `days` is non-negative for any real timestamp.
        let weekday = ((days % 7 + 4) % 7) as u32;
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            hour,
            minute,
            weekday,
        }
    }

    /// Unix milliseconds at the start of this civil minute — the inverse of
    /// [`from_unix_millis`](Self::from_unix_millis), which discards everything
    /// below the minute, so the round trip is lossless for any time this type
    /// can hold.
    ///
    /// Public so a caller that computed a fire time with
    /// [`CronExpr::next_after`] can hand it back as an instant: the cron
    /// preview (issue #262) sends epoch millis to the console precisely so the
    /// UTC reading and the viewer's local reading come from ONE number and
    /// cannot disagree.
    pub fn unix_millis(&self) -> u64 {
        self.floor_to_minute_millis()
    }

    /// Unix milliseconds at the start of this civil minute.
    fn floor_to_minute_millis(&self) -> u64 {
        let days = days_from_civil(self.year, self.month, self.day);
        (days as u64) * DAY_MS + (self.hour as u64) * 3_600_000 + (self.minute as u64) * MINUTE_MS
    }
}

/// Civil date from a days-since-epoch count (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

/// Days-since-epoch from a civil date (inverse of [`civil_from_days`]).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let m = month as i64;
    let d = day as i64;
    let y = if m <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
#[path = "cron_tests.rs"]
mod tests;
