//! Field-validation tests: required fields, status refusal, writers lists
//! and the write/read parity checks (split out of `ledgers_tests.rs`).

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

/// A ledger whose required fields the read path polices, so a write that skips
/// one is exactly the write that folds into an unreadable row.
fn decisions() -> serde_json::Value {
    json!({
        "slug": "choices",
        "title": "Choices",
        "purpose": "What the work chose.",
        "derived": "derived/choices.md",
        "fields": [
            { "name": "id", "role": "id" },
            { "name": "decision", "role": "title", "required": true },
            { "name": "status", "role": "status", "required": true },
            { "name": "constraint", "role": "prose", "required": true },
            { "name": "reason", "role": "prose" }
        ],
        "statuses": [
            { "name": "proposed" },
            { "name": "settled" },
            { "name": "superseded", "closed": true, "needs_reason": true }
        ],
        "checks": ["required-field", "known-status", "closed-needs-reason"]
    })
}

/// The same shape with no `checks` at all — what `define_ledger` produces when
/// a caller omits them, where nothing reported the corruption either.
fn decisions_unchecked() -> serde_json::Value {
    let mut spec = decisions();
    spec["slug"] = json!("unchecked");
    spec["derived"] = json!("derived/unchecked.md");
    spec.as_object_mut().expect("object").remove("checks");
    spec
}

#[tokio::test]
async fn a_write_missing_a_required_field_is_refused_not_folded_unreadable() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &decisions()).await.expect("declared");

    let error = record(
        &ctx,
        &spec,
        &agent(),
        "palette",
        fields(&[
            ("constraint", "the accessibility bar"),
            ("status", "proposed"),
        ]),
    )
    .await
    .expect_err("a row the ledger cannot read back must be refused");

    let message = error.to_string();
    assert!(message.contains("`decision`"), "names the field: {message}");
    assert!(message.contains("choices"), "names the ledger: {message}");

    // Refused, not merely reported: nothing landed to be read back.
    let after = read(&ctx, &spec, &Query::default()).await.expect("read");
    assert!(after.entries.is_empty(), "{:?}", after.entries);
    assert!(after.faults.is_empty(), "{:?}", after.faults);
}

/// The write refuses exactly what the read would have called unreadable. Two
/// implementations of "required" would drift; this pins them to each other.
#[tokio::test]
async fn the_write_refuses_what_the_read_would_report_unreadable() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &decisions()).await.expect("declared");
    let missing = fields(&[("constraint", "the accessibility bar")]);

    let refused = record(&ctx, &spec, &agent(), "palette", missing.clone())
        .await
        .expect_err("refused");

    let folded = crate::ledger::engine::fold(
        &spec,
        &[crate::ledger::LedgerEvent {
            ledger: spec.slug.clone(),
            id: "palette".into(),
            author: agent(),
            at_millis: 0,
            fields: missing,
        }],
    );
    for name in ["decision", "status"] {
        assert!(
            folded.faults.iter().any(|fault| fault.contains(name)),
            "read reports `{name}` unreadable: {:?}",
            folded.faults
        );
        assert!(
            refused.to_string().contains(name),
            "write refuses for `{name}`: {refused}"
        );
    }
}

/// A required field already on the row does not have to be resent: the check
/// judges the merged row, so amending one field is a complete write.
#[tokio::test]
async fn an_amendment_need_not_resend_fields_the_row_already_holds() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &decisions()).await.expect("declared");
    record(
        &ctx,
        &spec,
        &agent(),
        "palette",
        fields(&[
            ("decision", "a four-tone ramp"),
            ("status", "proposed"),
            ("constraint", "the accessibility bar"),
        ]),
    )
    .await
    .expect("recorded");

    record(
        &ctx,
        &spec,
        &agent(),
        "palette",
        fields(&[("status", "settled")]),
    )
    .await
    .expect("an amendment carrying only what changed is complete");

    let after = read(&ctx, &spec, &Query::default()).await.expect("read");
    assert!(after.faults.is_empty(), "{:?}", after.faults);
    assert_eq!(after.entries[0].get("decision"), "a four-tone ramp");
}

/// Clearing a required field is the same corruption arriving by another route.
#[tokio::test]
async fn clearing_a_required_field_is_refused() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &decisions()).await.expect("declared");
    record(
        &ctx,
        &spec,
        &agent(),
        "palette",
        fields(&[
            ("decision", "a four-tone ramp"),
            ("status", "proposed"),
            ("constraint", "the accessibility bar"),
        ]),
    )
    .await
    .expect("recorded");

    let mut clearing = BTreeMap::new();
    clearing.insert("decision".to_string(), None);
    let error = record(&ctx, &spec, &agent(), "palette", clearing)
        .await
        .expect_err("clearing a required field must be refused");
    assert!(error.to_string().contains("`decision`"), "{error}");
}

/// A ledger declared without `checks` still refuses the write. `required` is
/// the ledger's schema; `checks` only chooses what a read reports — and a
/// declaration that omitted them accepted the corruption *and* stayed silent
/// about it, which is the worse of the two failures.
#[tokio::test]
async fn a_ledger_declaring_no_checks_still_refuses_a_missing_required_field() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &decisions_unchecked())
        .await
        .expect("declared");
    assert!(spec.checks.is_empty(), "fixture declares no checks");

    let error = record(
        &ctx,
        &spec,
        &agent(),
        "palette",
        fields(&[("constraint", "the accessibility bar")]),
    )
    .await
    .expect_err("refused even with nothing set to report it");
    assert!(error.to_string().contains("`decision`"), "{error}");
}

/// Closing an id that does not exist opened a fresh row instead: closed, empty,
/// and named by the typo that produced it.
#[tokio::test]
async fn closing_an_unknown_id_is_refused_rather_than_opening_a_closed_row() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");

    let error = close(&ctx, &spec, &agent(), "typo", "closed", "role filled")
        .await
        .expect_err("closing a row that does not exist must be refused");
    let message = error.to_string();
    assert!(message.contains("typo"), "names the id: {message}");
    assert!(message.contains("hazards"), "names the ledger: {message}");

    let after = read(&ctx, &spec, &Query::default()).await.expect("read");
    assert!(after.entries.is_empty(), "no row was opened: {:?}", after);
}

/// `tasks` is the one built-in the runtime renders itself, and the only ledger
/// that can be native at all — `define` refuses the source for anything a
/// company declares — so this is the whole class, not one example of it.
async fn tasks_spec(ctx: &Ledgers) -> crate::ledger::LedgerSpec {
    registry(ctx)
        .await
        .expect("registry")
        .require("tasks")
        .expect("tasks is a built-in")
        .clone()
}

/// Closing an id on a ledger this tool does not write must say so, rather than
/// report on a row. "There is no such row" sends the caller looking for a row;
/// the ledger's own `written_by` sends them to the tool that owns the write.
#[tokio::test]
async fn closing_a_native_ledger_names_the_owning_tool_not_a_missing_row() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = tasks_spec(&ctx).await;

    let error = close(&ctx, &spec, &agent(), "never-existed", "done", "finished")
        .await
        .expect_err("a native ledger is not written here");

    let message = error.to_string();
    assert!(
        !message.contains("there is no"),
        "must not report on a row the caller cannot write anyway: {message}"
    );
    assert!(
        message.contains("record_entry"),
        "names the tool that does not own this write: {message}"
    );
    assert!(
        message.contains(spec.written_by.split_whitespace().next().unwrap_or("task")),
        "keeps the ledger's own written_by guidance: {message}"
    );
}

/// The same precedence one level down: a caller who may not write the ledger
/// hears that, not that the status they chose does not close a row on it.
#[tokio::test]
async fn a_native_ledger_outranks_the_closing_status_check() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = tasks_spec(&ctx).await;

    let error = close(&ctx, &spec, &agent(), "any", "not-a-closing-status", "why")
        .await
        .expect_err("a native ledger is not written here");
    assert!(
        error.to_string().contains("record_entry"),
        "the write guard outranks the status vocabulary: {error}"
    );
}

/// A value the caller got wrong outranks one they left out: told only that a
/// field is missing, a caller resends with the same rejected status and learns
/// the second half on a further round trip.
#[tokio::test]
async fn a_status_the_ledger_rejects_is_named_before_the_rows_gaps() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &decisions()).await.expect("declared");

    let error = record(
        &ctx,
        &spec,
        &agent(),
        "palette",
        fields(&[("status", "banana")]),
    )
    .await
    .expect_err("refused");

    let message = error.to_string();
    assert!(
        message.contains("banana"),
        "names the bad status: {message}"
    );
    assert!(
        !message.contains("leaves"),
        "the missing-field report must not shadow it: {message}"
    );
}

#[tokio::test]
async fn an_unknown_sort_is_refused_rather_than_defaulted() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    let error = read(
        &ctx,
        &spec,
        &Query {
            sort: Some("newest".into()),
            ..Query::default()
        },
    )
    .await
    .expect_err("unknown sort");
    assert!(format!("{error}").contains("recorded"), "{error}");
}

/// Holding `record_entry` is not permission to write everything: the set of
/// ledgers is not fixed when tools are wired.
#[tokio::test]
async fn a_writers_list_is_enforced_at_the_write() {
    let (ctx, _runtime, _home) = ledgers().await;
    let mut document = hazards();
    document["writers"] = json!(["cfo"]);
    let spec = define(&ctx, &document).await.expect("declared");

    let error = record(&ctx, &spec, &agent(), "r1", fields(&[("risk", "a")]))
        .await
        .expect_err("ceo is not a writer");
    assert!(format!("{error}").contains("cfo"), "{error}");

    record(
        &ctx,
        &spec,
        &LedgerAuthor::agent("cfo"),
        "r1",
        fields(&[("risk", "a")]),
    )
    .await
    .expect("cfo may");
}

#[tokio::test]
async fn a_declaration_that_collides_is_refused() {
    let (ctx, _runtime, _home) = ledgers().await;
    define(&ctx, &hazards()).await.expect("declared");
    let error = define(&ctx, &hazards())
        .await
        .expect_err("already a ledger");
    assert!(format!("{error}").contains("hazards"), "{error}");

    let mut shadow = hazards();
    shadow["slug"] = json!("goals");
    shadow["derived"] = json!("derived/other.md");
    let error = define(&ctx, &shadow).await.expect_err("built in");
    assert!(format!("{error}").contains("built-in"), "{error}");
}

/// Over-long text is truncated rather than rejected: losing the tail of a long
/// note is a smaller failure than losing the whole write.
#[tokio::test]
async fn an_over_long_value_is_truncated_rather_than_refused() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    let entry = record(
        &ctx,
        &spec,
        &agent(),
        "r1",
        fields(&[("risk", &"x".repeat(20_000))]),
    )
    .await
    .expect("recorded");
    assert_eq!(
        entry.get("risk").chars().count(),
        crate::ledger::MAX_FIELD_CHARS
    );
}

/// A blank value clears the field rather than storing a present-but-empty one,
/// which would render an empty bullet under every row that ever set it.
#[tokio::test]
async fn a_blank_value_clears_the_field() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(&ctx, &spec, &agent(), "r1", fields(&[("risk", "a")]))
        .await
        .expect("recorded");
    let cleared = record(&ctx, &spec, &agent(), "r1", fields(&[("risk", "   ")]))
        .await
        .expect("recorded");
    assert_eq!(cleared.get("risk"), "");
}

/// A ledger shaped like the global `learnings`: it declares the check *and*
/// marks a prose field required, which is the pair `hazards` deliberately
/// lacks.
pub(super) fn findings() -> serde_json::Value {
    json!({
        "slug": "findings",
        "title": "Findings",
        "purpose": "What we found out.",
        "derived": "derived/findings.md",
        "fields": [
            { "name": "id", "role": "id", "required": true },
            { "name": "finding", "role": "title", "required": true },
            { "name": "status", "role": "status", "required": true },
            { "name": "evidence", "role": "prose", "required": true,
              "description": "What actually happened, concretely." },
            { "name": "reason", "role": "prose" }
        ],
        "statuses": [
            { "name": "noted" },
            { "name": "adopted", "closed": true, "needs_reason": true }
        ],
        "sections": [
            { "heading": "Noted", "statuses": ["noted"], "order": "recent" }
        ],
        "checks": ["required-field", "known-status", "closed-needs-reason"]
    })
}

/// The write half of the contract the read half already stated. A row landing
/// without a field the ledger requires is refused, rather than stored and then
/// reported unreadable by every surface that opens the ledger.
#[tokio::test]
async fn a_row_missing_a_required_field_is_refused_at_the_write() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &findings()).await.expect("declared");
    let error = record(
        &ctx,
        &spec,
        &agent(),
        "f1",
        fields(&[("finding", "a row with no evidence"), ("status", "noted")]),
    )
    .await
    .expect_err("no evidence");
    let message = format!("{error}");
    assert!(message.contains("evidence"), "{message}");
    assert!(message.contains("f1"), "{message}");
    // The description is what tells whoever filled it in wrong what belongs
    // there, so the refusal carries it.
    assert!(message.contains("What actually happened"), "{message}");

    // Nothing was stored: a refused write must not leave the row behind.
    let read = read(&ctx, &spec, &Query::default()).await.expect("read");
    assert!(read.entries.is_empty(), "{:?}", read.entries);
}

/// The whole point, stated as one assertion: what the write accepts, the read
/// reads back clean. Before this, a row could be recorded and then reported by
/// the same ledger as one that could not be read.
#[tokio::test]
async fn what_the_write_accepts_the_read_reports_no_fault_on() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &findings()).await.expect("declared");
    record(
        &ctx,
        &spec,
        &agent(),
        "f1",
        fields(&[
            ("finding", "the vendor is slow"),
            ("status", "noted"),
            ("evidence", "three late deliveries in a row"),
        ]),
    )
    .await
    .expect("recorded");
    let read = read(&ctx, &spec, &Query::default()).await.expect("read");
    assert_eq!(read.entries.len(), 1);
    assert!(read.faults.is_empty(), "{:?}", read.faults);
}

/// Every write is a merge, so the check runs against the merged row. Moving a
/// row's status must not be refused for declining to repeat what it holds.
#[tokio::test]
async fn an_amendment_need_not_repeat_a_required_field_the_row_holds() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &findings()).await.expect("declared");
    record(
        &ctx,
        &spec,
        &agent(),
        "f1",
        fields(&[
            ("finding", "the vendor is slow"),
            ("status", "noted"),
            ("evidence", "three late deliveries"),
        ]),
    )
    .await
    .expect("recorded");
    let amended = record(&ctx, &spec, &agent(), "f1", fields(&[("status", "noted")]))
        .await
        .expect("an amendment carries only what changes");
    assert_eq!(amended.events, 2);

    close(
        &ctx,
        &spec,
        &agent(),
        "f1",
        "adopted",
        "folded into the standard",
    )
    .await
    .expect("closing carries neither the title nor the evidence again");
}

/// Clearing one is the same loss as never writing it, and a merge is the only
/// way to express a clear — so it is refused on the same ground. Blank text is
/// the other route to that clear: it normalizes to a null before the check
/// runs.
#[tokio::test]
async fn blanking_a_required_field_is_refused() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &findings()).await.expect("declared");
    record(
        &ctx,
        &spec,
        &agent(),
        "f1",
        fields(&[
            ("finding", "the vendor is slow"),
            ("status", "noted"),
            ("evidence", "three late deliveries"),
        ]),
    )
    .await
    .expect("recorded");
    let error = record(&ctx, &spec, &agent(), "f1", fields(&[("evidence", "  ")]))
        .await
        .expect_err("cleared");
    assert!(format!("{error}").contains("evidence"), "{error}");
}
