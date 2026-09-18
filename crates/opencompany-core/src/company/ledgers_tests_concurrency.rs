//! Concurrency and admission tests: purge races against in-flight writes,
//! the briefing/republish surfaces, pagination clamping, and slug-admission
//! races (split out of `ledgers_tests.rs`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;
use tokio::sync::Notify;

use super::tests_lifecycle::read2;
use super::tests_validation::findings;
use super::*;
use crate::company::runtime::CompanyRuntime;
use crate::ledger::LedgerAuthor;
use crate::ports::ledgers::LedgerStore;
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

/// A [`LedgerStore`] that pauses inside `events` exactly once, after the read
/// has already happened, so a test can hold a caller mid-check while another
/// task mutates the store underneath it.
struct PausingStore {
    inner: Arc<dyn LedgerStore>,
    armed: Arc<AtomicBool>,
    paused: Arc<Notify>,
    resume: Arc<Notify>,
}

#[async_trait::async_trait]
impl LedgerStore for PausingStore {
    async fn list_specs(&self, company: &CompanyId) -> Result<Vec<LedgerSpec>> {
        self.inner.list_specs(company).await
    }

    async fn put_spec(&self, company: &CompanyId, spec: &LedgerSpec) -> Result<()> {
        self.inner.put_spec(company, spec).await
    }

    async fn delete_spec(&self, company: &CompanyId, slug: &str) -> Result<bool> {
        self.inner.delete_spec(company, slug).await
    }

    async fn append(&self, company: &CompanyId, event: &LedgerEvent) -> Result<()> {
        self.inner.append(company, event).await
    }

    async fn events(&self, company: &CompanyId, ledger: &str) -> Result<Vec<LedgerEvent>> {
        let read = self.inner.events(company, ledger).await;
        if self.armed.swap(false, Ordering::SeqCst) {
            self.paused.notify_one();
            self.resume.notified().await;
        }
        read
    }

    async fn purge_entry(&self, company: &CompanyId, ledger: &str, entry: &str) -> Result<bool> {
        self.inner.purge_entry(company, ledger, entry).await
    }

    async fn purge_ledger(&self, company: &CompanyId, ledger: &str) -> Result<bool> {
        self.inner.purge_ledger(company, ledger).await
    }
}

/// The required-field check reads the stored row, then the write appends. A
/// purge landing in that gap used to remove the earlier events the check had
/// just relied on, so the append that followed recreated the row with only
/// its own partial fields — reporting success on exactly the row the check
/// exists to keep out.
///
/// [`PausingStore`] holds the amendment inside that exact gap — after its
/// required-field check has read the row, before it appends — so the purge
/// gets a deterministic window to land in, rather than relying on real
/// thread timing to hit a gap this narrow. The lock under test still does
/// its own real work here: it is what makes the purge task block instead of
/// running in that window once the fix is in place.
#[tokio::test]
async fn a_purge_racing_the_required_field_check_cannot_recreate_a_row_missing_it() {
    let (runtime, _home) = runtime().await;

    let armed = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let store: Arc<dyn LedgerStore> = Arc::new(PausingStore {
        inner: runtime.ledgers().clone(),
        armed: armed.clone(),
        paused: paused.clone(),
        resume: resume.clone(),
    });
    let ctx = Ledgers::new(runtime.id().clone(), store);

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

    // Seeding is done: arm the pause for the amendment's own read.
    armed.store(true, Ordering::SeqCst);

    let amend_ctx = ctx.clone();
    let amend_spec = spec.clone();
    let amender = tokio::spawn(async move {
        record(
            &amend_ctx,
            &amend_spec,
            &agent(),
            "f1",
            fields(&[("status", "noted")]),
        )
        .await
    });

    paused.notified().await;

    let purge_ctx = ctx.clone();
    let purge_spec = spec.clone();
    let purger =
        tokio::spawn(async move { delete_entry(&purge_ctx, &purge_spec, &person(), "f1").await });

    // Under the fix the purge blocks on the same lock the amendment is
    // holding; this just gives it the chance to run first when it is not
    // blocked, which is the whole bug.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    resume.notify_one();

    let amend_result = tokio::time::timeout(std::time::Duration::from_secs(5), amender)
        .await
        .expect("amendment did not finish")
        .expect("amendment task panicked");
    let purge_result = tokio::time::timeout(std::time::Duration::from_secs(5), purger)
        .await
        .expect("purge did not finish")
        .expect("purge task panicked");
    purge_result.expect("purge does not error");

    if let Ok(entry) = amend_result {
        assert!(
            !entry.get("evidence").trim().is_empty(),
            "amendment reported success on a row missing a required field: {entry:?}"
        );
    }
}

/// `close`'s own existence check is subject to the same race: read before the
/// lock and appended after it, a purge landing in the gap could remove the
/// row the check saw and let the append that followed recreate it — closed,
/// carrying none of the fields the deleted row held, on a ledger that
/// declares no field required.
///
/// Same [`PausingStore`] technique, now pausing `close`'s existence read.
/// Under the fix that read runs inside the same locked section as the
/// append, so the purge cannot land until `close` has already finished with
/// the row it actually saw.
#[tokio::test]
async fn a_purge_racing_close_cannot_reopen_the_deleted_row() {
    let (runtime, _home) = runtime().await;

    let armed = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let store: Arc<dyn LedgerStore> = Arc::new(PausingStore {
        inner: runtime.ledgers().clone(),
        armed: armed.clone(),
        paused: paused.clone(),
        resume: resume.clone(),
    });
    let ctx = Ledgers::new(runtime.id().clone(), store);

    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(&ctx, &spec, &agent(), "h1", fields(&[("risk", "a leak")]))
        .await
        .expect("recorded");

    armed.store(true, Ordering::SeqCst);

    let close_ctx = ctx.clone();
    let close_spec = spec.clone();
    let closer = tokio::spawn(async move {
        close(&close_ctx, &close_spec, &agent(), "h1", "closed", "handled").await
    });

    paused.notified().await;

    let purge_ctx = ctx.clone();
    let purge_spec = spec.clone();
    let purger =
        tokio::spawn(async move { delete_entry(&purge_ctx, &purge_spec, &person(), "h1").await });

    // Under the fix the purge blocks on the same lock `close` is holding;
    // this just gives it the chance to run first when it is not blocked,
    // which is the whole bug.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    resume.notify_one();

    let close_result = tokio::time::timeout(std::time::Duration::from_secs(5), closer)
        .await
        .expect("close did not finish")
        .expect("close task panicked");
    let purge_result = tokio::time::timeout(std::time::Duration::from_secs(5), purger)
        .await
        .expect("purge did not finish")
        .expect("purge task panicked");
    purge_result.expect("purge does not error");

    if let Ok(entry) = close_result {
        assert!(
            !entry.get("risk").trim().is_empty(),
            "closed a ghost row missing the fields the deleted row held: {entry:?}"
        );
    }

    let after = read(&ctx, &spec, &Query::default()).await.expect("read");
    assert!(
        after.entries.is_empty(),
        "a deleted row was reopened: {:?}",
        after
    );
}

/// The briefing is what a turn carries: every ledger named, every open row
/// identified, and the call that fetches the rest on each one.
#[tokio::test]
async fn the_briefing_names_every_ledger_and_how_to_read_more() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    record(
        &ctx,
        &spec,
        &agent(),
        "vendor-slip",
        fields(&[("risk", "a"), ("status", "open")]),
    )
    .await
    .expect("recorded");

    let registry = registry(&ctx).await.expect("registry");
    let briefing = briefing(&ctx, &registry).await.expect("briefing");
    for slug in ["tasks", "goals", "decisions", "hazards"] {
        assert!(briefing.contains(slug), "`{slug}` is missing: {briefing}");
    }
    assert!(briefing.contains("vendor-slip"), "{briefing}");
    assert!(briefing.contains("read_ledger"), "{briefing}");
}

#[tokio::test]
async fn republish_writes_every_ledgers_file() {
    let (ctx, runtime, _home) = ledgers().await;
    define(&ctx, &hazards()).await.expect("declared");
    let written = republish_all(&ctx).await.expect("republished");
    // Three built-ins, the baseline's own, and the one just declared.
    assert_eq!(written, 4 + crate::globals::ledgers().len());
    let tree = runtime.workspace().tree(runtime.id()).await.expect("tree");
    for name in ["tasks.md", "goals.md", "decisions.md", "hazards.md"] {
        assert!(
            tree.iter().any(|node| node.name == name),
            "`{name}` was not written"
        );
    }
}

/// `limit` reaches [`read`] straight off a tool argument, so `0` is as
/// reachable as any other number, and so is one past every bound. Neither may
/// become an empty page or a panic: the clamp answers both, and `matched`
/// still reports how many there really were.
#[tokio::test]
async fn a_limit_of_zero_or_past_every_bound_clamps_rather_than_emptying_the_page() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");
    for n in 0..3 {
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

    let none = read2(&ctx, &spec, 0).await;
    assert_eq!(
        none.entries.len(),
        1,
        "a zero limit must clamp to a row, not return a page that reads as an empty ledger"
    );
    assert_eq!(
        none.matched, 3,
        "the count must stay honest whatever the page was truncated to"
    );

    let past = read2(&ctx, &spec, usize::MAX).await;
    assert_eq!(
        past.entries.len(),
        3,
        "a limit past every bound returns what there is: {past:?}"
    );
    assert_eq!(past.matched, 3);
}

/// A [`LedgerStore`] that yields inside `list_specs`, after the read and
/// before the caller can act on it.
///
/// The window a declaration races in is between reading the registry and
/// writing to it. Yielding there hands the runtime to whatever else is ready,
/// so on the single-threaded test runtime two concurrent declarations
/// deterministically interleave inside that window rather than doing so only
/// when thread timing happens to arrange it. A declaration that is properly
/// serialized never reaches the yield concurrently with another.
struct YieldingSpecStore {
    inner: Arc<dyn LedgerStore>,
}

#[async_trait::async_trait]
impl LedgerStore for YieldingSpecStore {
    async fn list_specs(&self, company: &CompanyId) -> Result<Vec<LedgerSpec>> {
        let read = self.inner.list_specs(company).await;
        tokio::task::yield_now().await;
        read
    }

    async fn put_spec(&self, company: &CompanyId, spec: &LedgerSpec) -> Result<()> {
        self.inner.put_spec(company, spec).await
    }

    async fn delete_spec(&self, company: &CompanyId, slug: &str) -> Result<bool> {
        self.inner.delete_spec(company, slug).await
    }

    async fn append(&self, company: &CompanyId, event: &LedgerEvent) -> Result<()> {
        self.inner.append(company, event).await
    }

    async fn events(&self, company: &CompanyId, ledger: &str) -> Result<Vec<LedgerEvent>> {
        self.inner.events(company, ledger).await
    }

    async fn purge_entry(&self, company: &CompanyId, ledger: &str, entry: &str) -> Result<bool> {
        self.inner.purge_entry(company, ledger, entry).await
    }

    async fn purge_ledger(&self, company: &CompanyId, ledger: &str) -> Result<bool> {
        self.inner.purge_ledger(company, ledger).await
    }
}

/// A ledger named twice at once must be declared once.
///
/// `record`, `close` and `retire` all take their lock before they read the
/// store, so their check and their write are one section. A declaration that
/// took none would list the specs, ask the registry whether the slug collides,
/// and only then write: two declarations of a slug that does not exist yet
/// both read a registry without it, both pass `admits`, and both write — so
/// the second silently replaces the first, and the caller that lost is told
/// its ledger was created.
///
/// [`YieldingSpecStore`] puts both inside that window deterministically.
/// Exactly one of the two must be refused.
#[tokio::test]
async fn two_declarations_of_one_new_slug_cannot_both_be_admitted() {
    let (runtime, _home) = runtime().await;

    let store: Arc<dyn LedgerStore> = Arc::new(YieldingSpecStore {
        inner: runtime.ledgers().clone(),
    });
    let ctx = Ledgers::new(runtime.id().clone(), store);

    let mut first = hazards();
    first["title"] = json!("Hazards, as the first caller named them");
    let mut second = hazards();
    second["title"] = json!("Hazards, as the second caller named them");

    let (first_result, second_result) = tokio::join!(define(&ctx, &first), define(&ctx, &second));

    assert!(
        first_result.is_err() ^ second_result.is_err(),
        "one of two declarations of `hazards` must be refused; both were admitted — first: \
         {first_result:?}, second: {second_result:?}"
    );

    let stored = registry(&ctx).await.expect("registry");
    let survivor = stored
        .require("hazards")
        .expect("one hazards ledger survives");
    let winner = if first_result.is_ok() {
        first_result
    } else {
        second_result
    };
    assert_eq!(
        survivor.title,
        winner.expect("the admitted declaration").title,
        "the stored ledger must be the one whose caller was told it was created"
    );
}

/// How many ledgers this company has declared, built-ins aside.
async fn declared_count(ctx: &Ledgers) -> usize {
    registry(ctx)
        .await
        .expect("registry")
        .specs()
        .iter()
        .filter(|spec| !spec.builtin)
        .count()
}

/// Builds a declaration that collides with nothing but its own slug.
fn ledger_named(slug: &str) -> serde_json::Value {
    let mut doc = hazards();
    doc["slug"] = json!(slug);
    doc["title"] = json!(slug);
    doc["derived"] = json!(format!("derived/{slug}.md"));
    doc
}

/// The cap counts declarations, so two of *different* slugs contend too.
///
/// A lock taken per slug serializes only the callers naming one ledger. Both
/// of `admits`' rules range over every declaration — how many the company has,
/// and which derived file each writes — so two declarations of different slugs
/// read the same registry, both find room under the cap, and both write. The
/// company ends up past a cap it is the only thing enforcing.
///
/// With one slot left, exactly one must win.
#[tokio::test]
async fn two_declarations_of_different_slugs_cannot_both_take_the_last_slot() {
    let (runtime, _home) = runtime().await;
    let plain = Ledgers::new(runtime.id().clone(), runtime.ledgers().clone());

    let declared = declared_count(&plain).await;
    for n in declared..crate::ledger::registry::MAX_DECLARED - 1 {
        define(&plain, &ledger_named(&format!("filler-{n}")))
            .await
            .expect("filler declaration");
    }

    let store: Arc<dyn LedgerStore> = Arc::new(YieldingSpecStore {
        inner: runtime.ledgers().clone(),
    });
    let ctx = Ledgers::new(runtime.id().clone(), store);

    let (first_doc, second_doc) = (
        ledger_named("first-past-the-post"),
        ledger_named("second-past-the-post"),
    );
    let (first, second) = tokio::join!(define(&ctx, &first_doc), define(&ctx, &second_doc));

    assert!(
        first.is_ok() ^ second.is_ok(),
        "exactly one declaration may take the last slot — first: {first:?}, second: {second:?}"
    );
    assert_eq!(
        declared_count(&plain).await,
        crate::ledger::registry::MAX_DECLARED,
        "the cap is the only thing bounding declarations, so it must hold under a race"
    );
}

#[tokio::test]
async fn independently_constructed_contexts_share_declaration_admission() {
    let (runtime, _home) = runtime().await;
    let first_ctx = Ledgers::new(runtime.id().clone(), runtime.ledgers().clone());
    let second_ctx = Ledgers::new(runtime.id().clone(), runtime.ledgers().clone());
    let mut first = hazards();
    first["title"] = json!("First declaration");
    let mut second = hazards();
    second["title"] = json!("Second declaration");

    let (first_result, second_result) =
        tokio::join!(define(&first_ctx, &first), define(&second_ctx, &second));

    assert!(first_result.is_ok() ^ second_result.is_ok());
    let winner = first_result
        .or(second_result)
        .expect("one declaration wins");
    let stored = registry(&first_ctx).await.expect("registry");
    assert_eq!(stored.require("hazards").expect("stored ledger"), &winner);
}

/// A ledger with a `required` field, but no `Check::RequiredField` in its
/// `checks` list.
///
/// [`Field::required`] is documented to be enforced at the write "whether or
/// not the spec also declares `Check::RequiredField`" — `checks` only selects
/// what a *read* reports about rows that predate the requirement. This
/// fixture pins that: `checks` deliberately omits `required-field` so a test
/// against it cannot pass by accident of the read-time check firing instead.
fn hazards_with_a_required_field_and_no_required_field_check() -> serde_json::Value {
    json!({
        "slug": "hazards",
        "title": "Hazards",
        "purpose": "What could go wrong.",
        "derived": "derived/hazards.md",
        "fields": [
            { "name": "id", "role": "id" },
            { "name": "risk", "role": "title", "required": true },
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

/// `record` must refuse a row missing a `required` field even when the ledger
/// declares no `Check::RequiredField` — the field's own `required` flag is
/// the schema, and `checks` only selects what a read reports about rows that
/// predate the requirement (see [`crate::ledger::Field::required`]).
#[tokio::test]
async fn a_required_field_is_refused_at_write_with_no_required_field_check_declared() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(
        &ctx,
        &hazards_with_a_required_field_and_no_required_field_check(),
    )
    .await
    .expect("declared");
    assert!(
        !spec.checks.contains(&crate::ledger::Check::RequiredField),
        "the fixture's premise is a ledger with no required-field check declared"
    );

    let out = record(&ctx, &spec, &agent(), "r-1", fields(&[("status", "open")])).await;

    assert!(
        out.is_err(),
        "a row missing `risk`, a required field, must be refused even though `checks` does not \
         name `required-field`: {out:?}"
    );
}

/// `close` must refuse an id that names no existing row rather than
/// fabricating one that carries only the status and reason the caller
/// passed.
#[tokio::test]
async fn closing_an_id_that_does_not_exist_is_refused_not_fabricated() {
    let (ctx, _runtime, _home) = ledgers().await;
    let spec = define(&ctx, &hazards()).await.expect("declared");

    let out = close(&ctx, &spec, &agent(), "never-recorded", "closed", "n/a").await;
    assert!(
        out.is_err(),
        "closing an id that was never recorded must be refused, not create a new closed row: \
         {out:?}"
    );

    let read = read2(&ctx, &spec, usize::MAX).await;
    assert!(
        read.entries.is_empty(),
        "a refused close must not leave a fabricated row behind: {:?}",
        read.entries
    );
}
