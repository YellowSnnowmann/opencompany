//! Ledger lifecycle tests: declare, record, close, delete, retire, read, and
//! the board/derived-file surfaces (split out of `ledgers_tests.rs`, issue
//! tracked in `ledgers.rs`'s own module doc).

use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::company::runtime::CompanyRuntime;
use crate::ledger::LedgerAuthor;
use crate::ports::types::CompanyId;

async fn ledgers() -> (Ledgers, CompanyRuntime, tempfile::TempDir) {
    let (runtime, home) = runtime().await;
    let ctx = Ledgers::from(&runtime);
    (ctx, runtime, home)
}

async fn runtime() -> (CompanyRuntime, tempfile::TempDir) {
    let home = tempfile::tempdir().expect("tempdir");
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [policy]
        mode = "supervised"
        "#,
    )
    .expect("manifest");
    let runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(CompanyId::new("acme"))
        .build()
        .await
        .expect("runtime");
    (runtime, home)
}

fn hazards() -> serde_json::Value {
    json!({
        "slug": "hazards",
        "title": "Hazards",
        "purpose": "What could go wrong.",
        "derived": "derived/hazards.md",
        "fields": [
            { "name": "id", "role": "id" },
            { "name": "risk", "role": "title" },
            { "name": "status", "role": "status" },
            { "name": "reason", "role": "prose" }
        ],
        "statuses": [
            { "name": "open" },
            { "name": "closed", "closed": true, "needs_reason": true }
        ],
        "sections": [
            { "heading": "Live", "statuses": ["open"], "order": "recent" },
            { "heading": "Closed", "statuses": ["closed"] }
        ],
        "checks": ["known-status", "closed-needs-reason"]
    })
}

fn fields(pairs: &[(&str, &str)]) -> BTreeMap<String, Option<String>> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Some((*v).to_string())))
        .collect()
}

fn agent() -> LedgerAuthor {
    LedgerAuthor::agent("ceo")
}

fn person() -> LedgerAuthor {
    LedgerAuthor::human("u-1", "Dana")
}

/// A company starts with the three built-ins and nothing else.
#[tokio::test]
async fn a_fresh_company_has_the_built_ins_and_the_baseline() {
    let (ctx, _runtime, _home) = ledgers().await;
    let registry = registry(&ctx).await.expect("registry");
    // The built-ins first, in registry order, then whatever the global
    // baseline seeds (`crate::globals::ledgers`) — a company that starts with
    // nothing to record its risks, promises or learnings on gets one only if
    // some turn thinks to invent it.
    let slugs = registry.slugs();
    assert_eq!(&slugs[..3], ["tasks", "goals", "decisions"]);
    for global in crate::globals::ledgers() {
        assert!(slugs.contains(&global.slug), "`{}` is missing", global.slug);
    }
    assert!(registry.faults().is_empty());
}

/// An agent may declare an axis nobody anticipated — the whole point.
#[tokio::test]
async fn an_agent_may_declare_a_ledger_and_record_into_it() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    assert_eq!(spec.slug, "hazards");

    let entry = record(
        &ctx,
        &spec,
        &agent(),
        "vendor-slip",
        fields(&[("risk", "the vendor misses the date"), ("status", "open")]),
    )
    .await
    .expect("recorded");
    assert_eq!(entry.get("risk"), "the vendor misses the date");
    assert_eq!(entry.opened_by.kind, crate::ledger::AuthorKind::Agent);

    let registry = registry(&ctx).await.expect("registry");
    assert!(registry.find("hazards").is_some());
}

/// Recording twice against one id is an amendment, not a second row.
#[tokio::test]
async fn recording_again_amends_the_same_row() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(&ctx, &spec, &agent(), "r1", fields(&[("risk", "first")]))
        .await
        .expect("recorded");
    let amended = record(&ctx, &spec, &agent(), "r1", fields(&[("risk", "second")]))
        .await
        .expect("amended");
    assert_eq!(amended.events, 2);
    let read = read(&ctx, &spec, &Query::default()).await.expect("read");
    assert_eq!(read.entries.len(), 1);
    assert_eq!(read.entries[0].get("risk"), "second");
}

/// #2048 review: `read_ledger` used to fold the ledger once for its rows and
/// again, independently, inside `summary()` for the open/closed count sent
/// alongside them. A write landing in the gap between those two folds could
/// flip a row's status after the rows were read but before the count was, so
/// the response shipped a badge that disagreed with the rows beside it.
///
/// This reproduces that gap deterministically — recording a close at exactly
/// the point the old handler's second, independent fold used to run — rather
/// than relying on scheduler luck to land a race. It shows two things: a
/// second fold at that point genuinely disagrees with the rows already
/// returned (the defect is real, not hypothetical), and `read`'s own
/// `open`/`closed` — computed from the very fold that produced `entries`,
/// never a later one — hold steady across the write instead.
#[tokio::test]
async fn a_second_independent_fold_would_disagree_with_the_rows_read_already_returned() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(
        &ctx,
        &spec,
        &agent(),
        "r1",
        fields(&[("risk", "a"), ("status", "open")]),
    )
    .await
    .expect("recorded");

    // One fold, as the fixed `read` performs it: rows and counts share a
    // single snapshot, so both describe the ledger as of the same instant.
    let read = read(&ctx, &spec, &Query::default()).await.expect("read");
    assert_eq!(read.entries.len(), 1);
    assert_eq!(read.open, 1);
    assert_eq!(read.closed, 0);

    // The write that used to land in the window between the rows' fold and
    // the count's second, independent fold.
    record(
        &ctx,
        &spec,
        &agent(),
        "r1",
        fields(&[("status", "closed"), ("reason", "handled")]),
    )
    .await
    .expect("closed");

    // A second, independent fold taken right here — what `summary()` used to
    // run after `read()` already returned — now disagrees with the rows
    // `read` already handed back above: this is the exact mismatch a
    // two-fold response would have shipped to the console.
    let refolded = entries(&ctx, &spec).await.expect("entries");
    assert_eq!(
        refolded.open_count(&spec),
        0,
        "the row already closed by the time a second fold ran"
    );
    assert_ne!(
        refolded.open_count(&spec),
        read.open,
        "a second, independent fold disagrees with the snapshot `read` already returned"
    );

    // `read`'s own numbers, by contrast, are unaffected by the write that
    // came after it: they were taken from one snapshot and remain
    // internally consistent with the rows in that same `read`.
    assert_eq!(
        read.open,
        read.entries
            .iter()
            .filter(|entry| !spec.is_closed(&entry.status(&spec)))
            .count()
    );
    assert_eq!(read.closed, 0);
}

/// Refused at the **write**, not reported at the read: by the time somebody
/// reads it, the person who knew why has moved on.
#[tokio::test]
async fn closing_without_a_reason_is_refused() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(&ctx, &spec, &agent(), "r1", fields(&[("risk", "a")]))
        .await
        .expect("recorded");
    let error = record(&ctx, &spec, &agent(), "r1", fields(&[("status", "closed")]))
        .await
        .expect_err("no reason");
    assert!(format!("{error}").contains("reason"), "{error}");

    close(
        &ctx,
        &spec,
        &agent(),
        "r1",
        "closed",
        "the vendor delivered",
    )
    .await
    .expect("closed with a reason");
}

/// A row that already explained itself must not be refused for saying it twice.
#[tokio::test]
async fn a_reason_already_on_the_row_satisfies_the_close() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(
        &ctx,
        &spec,
        &agent(),
        "r1",
        fields(&[("risk", "a"), ("reason", "the vendor delivered")]),
    )
    .await
    .expect("recorded");
    record(&ctx, &spec, &agent(), "r1", fields(&[("status", "closed")]))
        .await
        .expect("the reason is already there");
}

#[tokio::test]
async fn an_undeclared_status_is_refused_and_names_the_real_ones() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    let error = record(
        &ctx,
        &spec,
        &agent(),
        "r1",
        fields(&[("status", "resolved")]),
    )
    .await
    .expect_err("unknown status");
    let message = format!("{error}");
    assert!(message.contains("resolved"), "{message}");
    assert!(message.contains("closed"), "{message}");
}

/// `close` refuses a status that closes nothing — the mistake a caller reaching
/// for "close" actually makes.
#[tokio::test]
async fn close_refuses_a_status_that_does_not_close() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    let error = close(&ctx, &spec, &agent(), "r1", "open", "done")
        .await
        .expect_err("open does not close");
    assert!(format!("{error}").contains("closed"), "{error}");
}

/// The rule, in the one place it lives. An agent's whole relationship with a
/// ledger is additive; deleting is not, and it is a person's call.
#[tokio::test]
async fn only_a_person_may_delete_a_row() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(&ctx, &spec, &agent(), "r1", fields(&[("risk", "a")]))
        .await
        .expect("recorded");

    let error = delete_entry(&ctx, &spec, &agent(), "r1")
        .await
        .expect_err("an agent may not delete");
    let message = format!("{error}");
    assert!(message.contains("only a person"), "{message}");
    assert!(message.contains("Close the row instead"), "{message}");
    // Refused, not silently ignored.
    assert!(
        read(&ctx, &spec, &Query::default())
            .await
            .expect("read")
            .entries
            .iter()
            .any(|entry| entry.id == "r1")
    );

    assert!(
        delete_entry(&ctx, &spec, &person(), "r1")
            .await
            .expect("a person may")
    );
    assert!(
        read(&ctx, &spec, &Query::default())
            .await
            .expect("read")
            .entries
            .is_empty()
    );
}

/// A second delete of the same id must stay the quiet `Ok(false)`
/// `purge_entry` already reports for "nothing there" — never an error, and
/// never a second entry in the log. Nothing before this called `delete_entry`
/// twice on one id.
#[tokio::test]
async fn deleting_an_already_deleted_row_reports_false_not_an_error() {
    let (ctx, runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(&ctx, &spec, &agent(), "r1", fields(&[("risk", "a")]))
        .await
        .expect("recorded");

    assert!(
        delete_entry(&ctx, &spec, &person(), "r1")
            .await
            .expect("the first delete removes the row")
    );
    assert!(
        !delete_entry(&ctx, &spec, &person(), "r1")
            .await
            .expect("deleting an already-deleted row is not an error"),
        "a second delete of the same id must report false, not recreate anything"
    );

    let events = runtime
        .ledgers()
        .events(runtime.id(), "hazards")
        .await
        .expect("events");
    assert!(
        events.is_empty(),
        "the row's event was purged by the first delete; a no-op second \
         delete must not resurrect it: {events:?}"
    );
}

/// The runtime is not exempt either: a sweep that could delete rows is the same
/// loss with nobody to ask about it.
#[tokio::test]
async fn the_runtime_itself_may_not_delete_a_row() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    let error = delete_entry(&ctx, &spec, &LedgerAuthor::system("sweep"), "r1")
        .await
        .expect_err("system is not a person");
    assert!(format!("{error}").contains("only a person"), "{error}");
}

#[tokio::test]
async fn only_a_person_may_retire_a_ledger_and_the_rows_survive_it() {
    let (ctx, runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(&ctx, &spec, &agent(), "r1", fields(&[("risk", "a")]))
        .await
        .expect("recorded");

    assert!(retire(&ctx, &agent(), "hazards", false).await.is_err());
    retire(&ctx, &person(), "hazards", false)
        .await
        .expect("a person may");
    assert!(
        registry(&ctx)
            .await
            .expect("registry")
            .find("hazards")
            .is_none()
    );

    // Retiring a ledger nobody reads is worth doing; deleting what it recorded
    // is a separate, explicit act.
    let events = runtime
        .ledgers()
        .events(runtime.id(), "hazards")
        .await
        .expect("events");
    assert_eq!(events.len(), 1, "the log survives the retirement");
}

/// A second `retire` of the same slug must stay a clean [`NotFound`], not
/// panic, not silently succeed, and not touch the rows the first retirement
/// already left alone.
///
/// [`NotFound`]: crate::error::OpenCompanyError::NotFound
#[tokio::test]
async fn retiring_an_already_retired_ledger_is_refused_not_silently_repeated() {
    let (ctx, runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(&ctx, &spec, &agent(), "r1", fields(&[("risk", "a")]))
        .await
        .expect("recorded");

    retire(&ctx, &person(), "hazards", false)
        .await
        .expect("the first retirement succeeds");

    let error = retire(&ctx, &person(), "hazards", false)
        .await
        .expect_err("a second retirement of the same slug must be refused");
    assert!(
        matches!(error, OpenCompanyError::NotFound(_)),
        "a repeat retirement must be reported as not-found, not any other \
         error shape: {error:?}"
    );

    // The rows the first retirement left in place are still there — the
    // refused second call did not fall through to a purge.
    let events = runtime
        .ledgers()
        .events(runtime.id(), "hazards")
        .await
        .expect("events");
    assert_eq!(
        events.len(),
        1,
        "the refused repeat retirement must not have purged anything"
    );
}

#[tokio::test]
async fn a_built_in_cannot_be_retired() {
    let (ctx, _runtime, _home) = ledgers().await;
    let error = retire(&ctx, &person(), "goals", false)
        .await
        .expect_err("built in");
    assert!(
        format!("{error}").contains("ships with the runtime"),
        "{error}"
    );
}

/// The board keeps its own store, its own routes and its own dispatch edge, so
/// `record_entry` must refuse it — and say what does write it.
#[tokio::test]
async fn the_board_is_readable_through_the_ledger_surface_and_not_writable_by_it() {
    let (ctx, _runtime, _home) = ledgers().await;
    let registry = registry(&ctx).await.expect("registry");
    let tasks = registry.find("tasks").expect("built in");

    let error = record(&ctx, tasks, &agent(), "t1", fields(&[("title", "x")]))
        .await
        .expect_err("native");
    assert!(format!("{error}").contains("spawn_task"), "{error}");

    // Reading it works the same as reading any other ledger.
    let read = read(&ctx, tasks, &Query::default()).await.expect("read");
    assert!(read.entries.is_empty(), "a fresh company has no cards");

    // And so does deleting: a card is deleted through the board.
    let error = delete_entry(&ctx, tasks, &person(), "t1")
        .await
        .expect_err("native");
    assert!(format!("{error}").contains("elsewhere"), "{error}");
}

/// Every write re-renders, so `derived/` is never a stale copy of something.
#[tokio::test]
async fn a_write_publishes_the_derived_file() {
    let (ctx, runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(
        &ctx,
        &spec,
        &agent(),
        "vendor-slip",
        fields(&[("risk", "the vendor misses the date"), ("status", "open")]),
    )
    .await
    .expect("recorded");

    let tree = runtime.workspace().tree(runtime.id()).await.expect("tree");
    let folder = tree
        .iter()
        .find(|node| node.name == "derived")
        .expect("the derived folder exists");
    let file = tree
        .iter()
        .find(|node| {
            node.parent_id.as_deref() == Some(folder.id.as_str()) && node.name == "hazards.md"
        })
        .expect("the ledger's file exists");
    let (_, body) = runtime
        .workspace()
        .read(runtime.id(), &file.id)
        .await
        .expect("read")
        .expect("present");
    assert!(body.contains("vendor-slip"), "{body}");
    assert!(body.contains("Do not edit this file"), "{body}");
}

/// A ledger is visible in `derived/` from the moment it exists, not from its
/// first row — a folder that gains a file only on first write reads as though
/// the ledger was never created.
#[tokio::test]
async fn declaring_a_ledger_publishes_its_empty_file() {
    let (ctx, runtime, _home) = ledgers().await;
    define(&ctx, &hazards()).await.expect("declared");
    let tree = runtime.workspace().tree(runtime.id()).await.expect("tree");
    assert!(tree.iter().any(|node| node.name == "hazards.md"));
}

/// A read that returned twenty rows must be distinguishable from one that
/// returned all of them.
#[tokio::test]
async fn a_read_is_bounded_and_says_how_many_matched() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    for n in 0..40 {
        record(
            &ctx,
            &spec,
            &agent(),
            &format!("r{n}"),
            fields(&[("risk", "a"), ("status", "open")]),
        )
        .await
        .expect("recorded");
    }
    let read = read(&ctx, &spec, &Query::default()).await.expect("read");
    assert_eq!(
        read.entries.len(),
        crate::ledger::budget::DEFAULT_READ_LIMIT
    );
    assert_eq!(read.matched, 40);

    let huge = read2(&ctx, &spec, 10_000).await;
    assert_eq!(
        huge.entries.len(),
        crate::ledger::budget::MAX_READ_LIMIT.min(40)
    );
}

pub(super) async fn read2(ctx: &Ledgers, spec: &crate::ledger::LedgerSpec, limit: usize) -> Read {
    read(
        ctx,
        spec,
        &Query {
            limit: Some(limit),
            ..Query::default()
        },
    )
    .await
    .expect("read")
}

#[tokio::test]
async fn a_read_narrows_by_status_entry_and_text() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(
        &ctx,
        &spec,
        &agent(),
        "vendor",
        fields(&[("risk", "supplier misses the date"), ("status", "open")]),
    )
    .await
    .expect("recorded");
    record(
        &ctx,
        &spec,
        &agent(),
        "hiring",
        fields(&[("risk", "the role stays open")]),
    )
    .await
    .expect("recorded");
    close(&ctx, &spec, &agent(), "hiring", "closed", "role filled")
        .await
        .expect("recorded");

    let open = read(
        &ctx,
        &spec,
        &Query {
            status: Some("open".into()),
            ..Query::default()
        },
    )
    .await
    .expect("read");
    assert_eq!(open.entries.len(), 1);
    assert_eq!(open.entries[0].id, "vendor");

    let one = read(
        &ctx,
        &spec,
        &Query {
            entry: Some("hiring".into()),
            ..Query::default()
        },
    )
    .await
    .expect("read");
    assert_eq!(one.entries.len(), 1);

    let found = read(
        &ctx,
        &spec,
        &Query {
            text: Some("SUPPLIER".into()),
            ..Query::default()
        },
    )
    .await
    .expect("read");
    assert_eq!(found.entries.len(), 1);
}
