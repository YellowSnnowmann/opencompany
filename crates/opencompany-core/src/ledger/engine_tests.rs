//! What the fold guarantees, and what the bounds hold.

use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::ledger::spec::parse;
use crate::ledger::types::{AuthorKind, LedgerAuthor};

fn spec() -> LedgerSpec {
    parse(
        &json!({
            "slug": "risks",
            "title": "Risks",
            "purpose": "What could go wrong.",
            "derived": "derived/RISKS.md",
            "fields": [
                { "name": "id", "role": "id", "required": true },
                { "name": "risk", "role": "title", "required": true },
                { "name": "status", "role": "status" },
                { "name": "mitigation", "role": "prose" },
                { "name": "reason", "role": "prose" }
            ],
            "statuses": [
                { "name": "open" },
                { "name": "watching" },
                { "name": "closed", "closed": true, "needs_reason": true }
            ],
            "sections": [
                { "heading": "Live", "blurb": "Most recent first.", "statuses": ["open", "watching"], "order": "recent" },
                { "heading": "Closed", "statuses": ["closed"], "cap": 5 }
            ],
            "checks": ["required-field", "known-status", "closed-needs-reason"]
        }),
        false,
    )
    .expect("valid")
}

fn event(id: &str, pairs: &[(&str, Option<&str>)]) -> LedgerEvent {
    let mut fields = BTreeMap::new();
    for (name, value) in pairs {
        fields.insert(
            (*name).to_string(),
            value.map(std::string::ToString::to_string),
        );
    }
    LedgerEvent {
        ledger: "risks".to_string(),
        id: id.to_string(),
        author: LedgerAuthor::agent("ops"),
        at_millis: 1_000,
        fields,
    }
}

#[test]
fn events_merge_into_one_entry_rather_than_replacing_it() {
    let spec = spec();
    let entries = fold(
        &spec,
        &[
            event(
                "vendor-slip",
                &[
                    ("risk", Some("the vendor misses the date")),
                    ("status", Some("open")),
                ],
            ),
            event(
                "vendor-slip",
                &[("mitigation", Some("second supplier lined up"))],
            ),
        ],
    );
    assert_eq!(entries.entries.len(), 1, "one id is one row");
    let entry = entries.find("vendor-slip").expect("folded");
    assert_eq!(entry.title(&spec), "the vendor misses the date");
    assert_eq!(entry.get("mitigation"), "second supplier lined up");
    assert_eq!(entry.events, 2);
}

/// The one thing a merge cannot otherwise express.
#[test]
fn a_null_clears_a_field() {
    let spec = spec();
    let entries = fold(
        &spec,
        &[
            event("r1", &[("risk", Some("a")), ("mitigation", Some("b"))]),
            event("r1", &[("mitigation", None)]),
        ],
    );
    let entry = entries.find("r1").expect("folded");
    assert_eq!(entry.get("mitigation"), "");
    assert_eq!(entry.get("risk"), "a");
}

/// A field the declaration never anticipated is kept and reported, not dropped.
/// A recorded fact that vanishes because a schema did not expect it is worse
/// than one in the wrong place — only the second is visible.
#[test]
fn an_undeclared_field_is_kept() {
    let spec = spec();
    let entries = fold(
        &spec,
        &[event(
            "r1",
            &[("risk", Some("a")), ("severity", Some("high"))],
        )],
    );
    assert_eq!(entries.find("r1").expect("folded").get("severity"), "high");
}

/// Recording against an existing row raises it — with the tool every writer
/// already holds and no new authority. Newest-created would bury the row that
/// is still the most important.
#[test]
fn recording_against_an_old_row_moves_it_back_to_the_top() {
    let spec = spec();
    let entries = fold(
        &spec,
        &[
            event("old", &[("risk", Some("old")), ("status", Some("open"))]),
            event("new", &[("risk", Some("new")), ("status", Some("open"))]),
            event("old", &[("mitigation", Some("still the one that matters"))]),
        ],
    );
    let recent = ordered(&entries.entries, Order::Recent);
    assert_eq!(recent[0].id, "old");
    // The fold itself still says first-seen order, so nothing that reads
    // entries had its meaning changed underneath it.
    assert_eq!(entries.entries[0].id, "old");
    assert_eq!(entries.entries[1].id, "new");
    let recorded = ordered(&entries.entries, Order::Recorded);
    assert_eq!(recorded[0].id, "old");
    assert_eq!(recorded[1].id, "new");
}

#[test]
fn the_first_and_last_author_are_both_kept() {
    let spec = spec();
    let mut second = event(
        "r1",
        &[("status", Some("closed")), ("reason", Some("passed"))],
    );
    second.author = LedgerAuthor::human("u-1", "Dana");
    let entries = fold(&spec, &[event("r1", &[("risk", Some("a"))]), second]);
    let entry = entries.find("r1").expect("folded");
    assert_eq!(entry.opened_by.kind, AuthorKind::Agent);
    assert_eq!(entry.updated_by.kind, AuthorKind::Human);
    assert_eq!(entry.updated_by.byline(), "Dana (human)");
}

#[test]
fn the_declared_checks_report_rather_than_drop() {
    let spec = spec();
    let entries = fold(
        &spec,
        &[
            event("no-title", &[("status", Some("open"))]),
            event(
                "bad-status",
                &[("risk", Some("a")), ("status", Some("shipped"))],
            ),
            event(
                "silent-close",
                &[("risk", Some("a")), ("status", Some("closed"))],
            ),
        ],
    );
    assert_eq!(entries.entries.len(), 3, "every row survives its fault");
    let faults = entries.faults.join("\n");
    assert!(faults.contains("no-title"), "{faults}");
    assert!(faults.contains("shipped"), "{faults}");
    assert!(faults.contains("silent-close"), "{faults}");
}

/// The id lives beside the fields, not inside them. A spec declaring its id
/// field required must not therefore report every one of its own rows
/// unreadable — the failure riemann hit with all eight open tasks filed under
/// "could not be read" while the log was perfectly valid.
#[test]
fn a_required_id_field_does_not_fault_every_well_formed_row() {
    let spec = spec();
    let entries = fold(
        &spec,
        &[event(
            "r1",
            &[("risk", Some("a")), ("status", Some("open"))],
        )],
    );
    assert!(entries.faults.is_empty(), "{:?}", entries.faults);
}

#[test]
fn an_event_naming_no_entry_is_reported_and_skipped() {
    let spec = spec();
    let entries = fold(&spec, &[event("  ", &[("risk", Some("a"))])]);
    assert!(entries.entries.is_empty());
    assert_eq!(entries.faults.len(), 1);
}

#[test]
fn render_sections_by_status_and_says_where_the_rest_is() {
    let spec = spec();
    let mut events = vec![event(
        "live",
        &[("risk", Some("live one")), ("status", Some("open"))],
    )];
    for n in 0..20 {
        events.push(event(
            &format!("closed-{n}"),
            &[
                ("risk", Some("settled")),
                ("status", Some("closed")),
                ("reason", Some("handled")),
            ],
        ));
    }
    let entries = fold(&spec, &events);
    let rendered = render(&spec, &entries);
    assert!(rendered.contains("## Live"));
    assert!(rendered.contains("## Closed"));
    assert!(rendered.contains("Do not edit this file"));
    // The section is capped at five and must say so rather than reading as
    // complete.
    assert!(rendered.contains("15 more not shown here"), "{rendered}");
    assert!(rendered.contains("read_ledger"), "{rendered}");
}

#[test]
fn an_empty_ledger_says_so() {
    let spec = spec();
    let rendered = render(&spec, &fold(&spec, &[]));
    assert!(rendered.contains("Nothing recorded yet"));
}

/// A ledger declaring no sections still renders its rows, rather than rendering
/// a header over nothing.
#[test]
fn a_sectionless_ledger_renders_everything_once() {
    let mut spec = spec();
    spec.sections.clear();
    let entries = fold(
        &spec,
        &[event(
            "r1",
            &[("risk", Some("a")), ("status", Some("open"))],
        )],
    );
    let rendered = render(&spec, &entries);
    assert!(rendered.contains("## Entries"), "{rendered}");
    assert!(rendered.contains("r1"), "{rendered}");
}

/// The ceiling. Sixty absurd rows and a hundred and eighty must render to
/// nearly the same file: a ceiling alone cannot catch a section that grows
/// slowly, and *past the bound, more rows must not mean more file* is the
/// property that actually matters.
#[test]
fn past_the_bound_more_rows_do_not_mean_more_file() {
    let spec = spec();
    let render_n = |count: usize| {
        let events: Vec<LedgerEvent> = (0..count)
            .map(|n| {
                event(
                    &format!("r{n}"),
                    &[
                        ("risk", Some(&"a".repeat(6_000))),
                        ("status", Some("open")),
                        ("mitigation", Some(&"b".repeat(6_000))),
                    ],
                )
            })
            .collect();
        render(&spec, &fold(&spec, &events)).len()
    };
    let small = render_n(60);
    let huge = render_n(180);
    assert!(
        small < 120_000,
        "sixty absurd rows rendered {small} characters"
    );
    assert!(
        huge.saturating_sub(small) < 200,
        "the file grew with row count past the bound: {small} → {huge}"
    );
}

/// An index keeps identity and drops reasoning — but a shortened list that
/// reads as complete is worse than a long one, so it must always say where the
/// rest is.
#[test]
fn an_index_keeps_every_open_row_and_bounds_the_archive() {
    let spec = spec();
    let mut events = Vec::new();
    for n in 0..12 {
        events.push(event(
            &format!("open-{n}"),
            &[("risk", Some("live")), ("status", Some("open"))],
        ));
    }
    for n in 0..12 {
        events.push(event(
            &format!("closed-{n}"),
            &[
                ("risk", Some("settled")),
                ("status", Some("closed")),
                ("reason", Some(&"why ".repeat(500))),
            ],
        ));
    }
    let entries = fold(&spec, &events);
    let index = index(&spec, &entries);
    for n in 0..12 {
        assert!(
            index.contains(&format!("open-{n}")),
            "every open row is carried"
        );
    }
    assert!(index.contains("7 more `closed`"), "{index}");
    assert!(index.contains("read_ledger"), "{index}");
    // The payload is what an index drops.
    assert!(
        !index.contains(&"why ".repeat(20)),
        "the index carries reasoning"
    );
    assert!(index.len() < render(&spec, &entries).len());
}

#[test]
fn an_empty_index_still_names_the_ledger() {
    let spec = spec();
    let index = index(&spec, &fold(&spec, &[]));
    assert!(index.contains("risks"));
    assert!(index.contains("Nothing recorded yet"));
}

#[test]
fn a_search_matches_any_field_and_the_id() {
    let spec = spec();
    let entries = fold(
        &spec,
        &[event(
            "vendor-slip",
            &[("risk", Some("supplier misses the date"))],
        )],
    );
    let entry = entries.find("vendor-slip").expect("folded");
    assert!(entry.matches("SUPPLIER"));
    assert!(entry.matches("vendor"));
    assert!(entry.matches(""));
    assert!(!entry.matches("invoice"));
}
