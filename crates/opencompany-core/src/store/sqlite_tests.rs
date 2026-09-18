use super::*;
use crate::store::conformance;

pub(super) fn store() -> Arc<SqliteStore> {
    Arc::new(SqliteStore::open_in_memory().expect("open in-memory sqlite"))
}

/// [`SqliteStore::apply_pragmas`] settles for the connection the ports use.
///
/// Asserted against a FILE-backed database on purpose. Every other test here
/// runs in memory, where `journal_mode=WAL` legitimately answers `memory` —
/// so an in-memory assertion could not distinguish "WAL was applied" from
/// "the pragma was never issued", which is the regression worth catching.
/// `synchronous=NORMAL` is only sound under WAL, so the two are checked
/// together rather than separately.
#[tokio::test]
async fn file_backed_store_runs_in_wal_with_relaxed_sync() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = SqliteStore::open(dir.path().join("opencompany.db")).expect("open sqlite file");

    let conn = store.conn();
    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("read journal_mode");
    // SQLite reports the mode lowercased regardless of how it was set.
    assert_eq!(journal.to_ascii_lowercase(), "wal");

    // 1 == NORMAL. The numeric form is what `PRAGMA synchronous` reads back;
    // there is no symbolic getter.
    let synchronous: i64 = conn
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .expect("read synchronous");
    assert_eq!(synchronous, 1, "expected synchronous=NORMAL");

    let foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .expect("read foreign_keys");
    assert_eq!(foreign_keys, 1, "expected foreign_keys=ON");
}

#[tokio::test]
async fn conformance_isolation_by_company() {
    let s = store();
    conformance::assert_isolation_by_company(s.clone(), s.clone(), s.clone(), s).await;
}

#[tokio::test]
async fn conformance_paused_ordinary_save_preserves_activation_gate() {
    let s = store();
    conformance::assert_paused_ordinary_save_preserves_activation_gate(s).await;
}

#[tokio::test]
async fn conformance_append_only_event_and_ledger() {
    let s = store();
    conformance::assert_append_only_event_and_ledger(s.clone(), s).await;
}

#[tokio::test]
async fn conformance_monotonic_event_seq() {
    let s = store();
    conformance::assert_monotonic_event_seq(s).await;
}

#[tokio::test]
async fn conformance_event_subscription_surfaces_gap() {
    conformance::assert_event_subscription_surfaces_gap(store()).await;
}

#[tokio::test]
async fn conformance_event_read_before() {
    conformance::assert_event_read_before(store()).await;
}

#[tokio::test]
async fn conformance_event_retention() {
    let s = store();
    conformance::assert_event_retention(s).await;
}

#[tokio::test]
async fn conformance_export_totality() {
    let s = store();
    conformance::assert_export_totality(s.clone(), s.clone(), s.clone(), s).await;
}

#[tokio::test]
async fn conformance_inbox_store() {
    conformance::assert_inbox_store(store()).await;
}

/// Issue #1505. The port holds this company's inference credential, its MCP
/// OAuth tokens and its SMTP password, and had no conformance case on any
/// backend until this one.
#[tokio::test]
async fn conformance_secret_store() {
    conformance::assert_secret_store(store()).await;
}

#[tokio::test]
async fn conformance_task_store() {
    conformance::assert_task_store(store()).await;
}

#[tokio::test]
async fn conformance_user_store() {
    conformance::assert_user_store(store()).await;
}

#[tokio::test]
async fn conformance_session_store() {
    conformance::assert_session_store(store()).await;
}

#[tokio::test]
async fn conformance_login_code_store() {
    conformance::assert_login_code_store(store()).await;
}

#[tokio::test]
async fn conformance_fact_store() {
    conformance::assert_fact_store(store()).await;
    conformance::assert_artifact_store(store()).await;
}

#[tokio::test]
async fn conformance_workflow_revision_store() {
    conformance::assert_workflow_revision_store(store()).await;
}

#[tokio::test]
async fn conformance_workflow_run_output_store() {
    conformance::assert_workflow_run_output_store(store()).await;
}

#[tokio::test]
async fn conformance_context_chunk_stamps() {
    conformance::assert_context_chunk_stamps(store()).await;
}

// Exercises this backend's single-connection `peek_many` override against
// the same positional contract the default implementation gives.
#[tokio::test]
async fn conformance_context_peek_many() {
    conformance::assert_context_peek_many_answers_positionally(store()).await;
}

#[tokio::test]
async fn conformance_context_multibyte_bodies() {
    conformance::assert_multibyte_bodies_survive_search_and_ranged_peek(store()).await;
}

#[tokio::test]
async fn conformance_context_identical_body_two_labels() {
    conformance::assert_identical_body_two_labels(store()).await;
}

#[tokio::test]
async fn conformance_context_delete_label_scoped() {
    conformance::assert_delete_label_scoped(store()).await;
}

#[tokio::test]
async fn conformance_context_delete_label_survives_a_concurrent_identical_put() {
    conformance::assert_delete_label_survives_a_concurrent_identical_put(store()).await;
}

/// The same search semantics as every other backend.
#[tokio::test]
async fn conformance_context_search_ranking() {
    conformance::assert_context_search_ranking(store()).await;
}

/// The migration path a fresh database never exercises.
///
/// [`MIGRATIONS`] is all `CREATE TABLE IF NOT EXISTS`, which is a no-op
/// against a database that already has `context_chunks` — so on an existing
/// deployment only [`add_column_if_missing`] can add `stored_ms`. Without
/// it, every read of the column would fail with "no such column" and the
/// whole Brain list/stats surface would 500.
#[tokio::test]
async fn legacy_context_chunks_table_gains_stored_ms_on_open() {
    // A database as it looked before the column existed, with a row in it.
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE context_chunks (
                 company_id TEXT NOT NULL,
                 addr       TEXT NOT NULL,
                 label      TEXT NOT NULL,
                 body       TEXT NOT NULL,
                 len        INTEGER NOT NULL,
                 PRIMARY KEY (company_id, addr)
             );
             INSERT INTO context_chunks (company_id, addr, label, body, len)
             VALUES ('acme', 'legacy-addr', 'agent/ceo', 'remembered before stamps', 24);",
    )
    .expect("seed a pre-`stored_ms` database");

    let store = SqliteStore::from_conn(conn).expect("migrations run on a legacy database");
    let id = CompanyId::new("acme");

    // The legacy row survives the migration and reports an *unknown* store
    // time — not the epoch, and not a misleading "migrated just now".
    // Fully qualified: `SqliteStore` implements both `ContextStore::list`
    // and `CompanyStore::list`, and method resolution matches on the name
    // alone — arity does not disambiguate, so the bare call is `E0034`.
    let metas = ContextStore::list(&store, &id, "")
        .await
        .expect("list after migration");
    assert_eq!(metas.len(), 1, "the legacy row must not be dropped");
    assert_eq!(metas[0].label, "agent/ceo");
    assert_eq!(
        metas[0].stored_at_millis, 0,
        "a row written before stamps existed has no store time to report"
    );

    // A write after the migration is stamped for real.
    let before = now_millis();
    store
        .put(
            &id,
            ContextChunk {
                label: "agent/ops".to_string(),
                body: "remembered after the migration".to_string(),
            },
        )
        .await
        .expect("put into a migrated database");

    let metas = ContextStore::list(&store, &id, "")
        .await
        .expect("list after put");
    assert_eq!(metas.len(), 2);
    let fresh = metas
        .iter()
        .find(|m| m.label == "agent/ops")
        .expect("the new chunk is listed");
    assert!(
        fresh.stored_at_millis >= before,
        "a post-migration write must carry a real stamp, got {}",
        fresh.stored_at_millis
    );

    // Reopening an already-migrated database must not try to add the column
    // a second time — the `ALTER` would fail with "duplicate column name".
    add_column_if_missing(
        &store.conn(),
        "context_chunks",
        "stored_ms",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .expect("adding an existing column is a no-op, not an error");
}

/// Issue #1300's open-time heals on a mixed-version database: the
/// backfill gives a row an older binary wrote its label claim (so the
/// label-scoped delete can reach it), and the orphan sweep drops an index
/// row whose body row an older binary's address-level delete removed.
#[tokio::test]
async fn legacy_context_rows_gain_label_claims_and_orphans_are_swept_on_open() {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE context_chunks (
                 company_id TEXT NOT NULL,
                 addr       TEXT NOT NULL,
                 label      TEXT NOT NULL,
                 body       TEXT NOT NULL,
                 len        INTEGER NOT NULL,
                 stored_ms  INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (company_id, addr)
             );
             CREATE TABLE context_chunk_labels (
                 company_id TEXT NOT NULL,
                 addr       TEXT NOT NULL,
                 label      TEXT NOT NULL,
                 stored_ms  INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (company_id, addr, label)
             );
             -- A row an older binary wrote after the labels table existed: the
             -- body row is there, its claim is not.
             INSERT INTO context_chunks (company_id, addr, label, body, len, stored_ms)
             VALUES ('acme', 'old-binary-addr', 'agent/ceo', 'written by an old binary', 24, 7);
             -- And the reverse: a claim whose body row an older binary's
             -- address-level delete removed.
             INSERT INTO context_chunk_labels (company_id, addr, label, stored_ms)
             VALUES ('acme', 'reaped-addr', 'agent/ops', 9);",
    )
    .expect("seed a mixed-version database");

    let store = SqliteStore::from_conn(conn).expect("migrations run");
    let id = CompanyId::new("acme");

    let metas = ContextStore::list(&store, &id, "")
        .await
        .expect("list after the heals");
    assert_eq!(
        metas.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(),
        ["agent/ceo"],
        "the old binary's row gains its claim; the orphaned claim is swept"
    );
    assert_eq!(
        metas[0].stored_at_millis, 7,
        "the backfilled claim carries the body row's stamp"
    );

    // The backfilled claim is label-deletable, and takes the body with it
    // as the last claim.
    let addr = ChunkAddr::new("old-binary-addr".to_string());
    assert!(
        store
            .delete_label(&id, &addr, "agent/ceo")
            .await
            .expect("label-scoped delete on a backfilled claim")
    );
    assert!(
        ContextStore::list(&store, &id, "")
            .await
            .unwrap()
            .is_empty(),
        "no claim may remain"
    );
    assert!(
        store.peek(&id, &addr, None).await.is_err(),
        "the body row went with its last claim"
    );
}

/// Issue #983: a database created while `runs.task_id` was `NOT NULL` must
/// end up able to hold a card-less run.
///
/// `MIGRATIONS` is all `CREATE TABLE IF NOT EXISTS`, so the relaxed DDL is a
/// no-op against an existing deployment — and SQLite cannot drop a column
/// constraint in place. Without the rebuild the *first chat turn* on any
/// upgraded self-hosted install fails its insert, which is a failure mode
/// that appears only in production and never in a test that starts fresh.
#[tokio::test]
async fn a_legacy_runs_table_learns_to_hold_a_card_less_run() {
    use crate::ports::runs::{NewRun, RunStore};

    // The table exactly as it shipped before #983, with an attempt in it.
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE runs (
                 company_id TEXT NOT NULL,
                 id         TEXT NOT NULL,
                 task_id    TEXT NOT NULL,
                 status     TEXT NOT NULL,
                 attempt    INTEGER NOT NULL,
                 created_ms INTEGER NOT NULL,
                 run_json   TEXT NOT NULL,
                 PRIMARY KEY (company_id, id)
             );
             INSERT INTO runs (company_id, id, task_id, status, attempt, created_ms, run_json)
             VALUES ('acme', 'old-run', 'card-7', 'succeeded', 1, 1700000000000,
                     '{\"id\":\"old-run\",\"company\":\"acme\",\"taskId\":\"card-7\",\
                       \"agentId\":\"ceo\",\"attempt\":1,\"status\":\"succeeded\",\
                       \"createdAtMillis\":1700000000000}');",
    )
    .expect("seed a pre-#983 database");

    let store = SqliteStore::from_conn(conn).expect("migrations run on a legacy database");
    let id = CompanyId::new("acme");

    // The rebuild copied the history rather than dropping it.
    let old = store
        .get_run(&id, "old-run")
        .await
        .expect("read after the rebuild")
        .expect("the legacy attempt survived");
    assert_eq!(old.task_id.as_deref(), Some("card-7"));
    assert_eq!(old.attempt, 1);

    // …and the constraint is gone, so a chat turn can be recorded at all.
    let turn = store
        .create_run(&id, NewRun::for_chat("turn-1", "general", "general"))
        .await
        .expect("a card-less run must be insertable after the rebuild");
    assert_eq!(turn.task_id, None);
    assert_eq!(
        store.get_run(&id, "turn-1").await.unwrap().as_ref(),
        Some(&turn)
    );

    // The per-card filter still works over the rebuilt indexes, and the
    // card-less row does not answer it.
    let for_card = store
        .list_runs(&id, &crate::ports::runs::RunFilter::for_task("card-7"))
        .await
        .expect("list by card");
    assert_eq!(
        for_card.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["old-run"]
    );

    // Re-running the migration is a no-op: the constraint check is what
    // stops it rewriting the whole run history on every single open.
    relax_runs_task_id_nullability(&store.conn())
        .expect("the rebuild is idempotent once the constraint is gone");
    assert_eq!(
        store
            .list_runs(&id, &Default::default())
            .await
            .unwrap()
            .len(),
        2,
        "a second pass duplicated or dropped rows"
    );
}

/// Issue #1573: a database created before the `agent_id` mirror column must
/// end up able to answer "what has this desk run" over its **whole**
/// history, not just the attempts written since the upgrade.
///
/// The column reaches an existing deployment through an additive `ALTER`,
/// which leaves every stored row `NULL` — and a `NULL` never matches
/// `agent_id = ?`. So the rows that would silently disappear from the new
/// filter are precisely the ones an operator opening a teammate for the
/// first time most wants to see. That is a wrong answer rather than a
/// missing feature: an empty history reads as "this teammate has never
/// run".
///
/// Seeded with the **post-#983** shape on purpose, so this exercises the
/// plain `ALTER` + backfill path rather than riding along on the table
/// rebuild that `a_legacy_runs_table_learns_to_hold_a_card_less_run`
/// already covers.
#[tokio::test]
async fn a_legacy_runs_table_learns_which_desk_ran_each_attempt() {
    use crate::ports::runs::{NewRun, RunFilter, RunStore};

    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE runs (
                 company_id TEXT NOT NULL,
                 id         TEXT NOT NULL,
                 task_id    TEXT,
                 status     TEXT NOT NULL,
                 attempt    INTEGER NOT NULL,
                 created_ms INTEGER NOT NULL,
                 run_json   TEXT NOT NULL,
                 PRIMARY KEY (company_id, id)
             );
             INSERT INTO runs (company_id, id, task_id, status, attempt, created_ms, run_json)
             VALUES ('acme', 'old-run', 'card-7', 'succeeded', 1, 1700000000000,
                     '{\"id\":\"old-run\",\"company\":\"acme\",\"taskId\":\"card-7\",\
                       \"agentId\":\"engineer\",\"attempt\":1,\"status\":\"succeeded\",\
                       \"createdAtMillis\":1700000000000}');",
    )
    .expect("seed a pre-#1573 database");

    let store = SqliteStore::from_conn(conn).expect("migrations run on a legacy database");
    let id = CompanyId::new("acme");

    // The desk was only ever inside the blob; the backfill is what makes it
    // a predicate.
    assert_eq!(
        store
            .list_runs(&id, &RunFilter::for_agent("engineer"))
            .await
            .expect("list by desk")
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["old-run"],
        "an attempt written before the column existed is still this desk's"
    );
    assert!(
        store
            .list_runs(&id, &RunFilter::for_agent("ceo"))
            .await
            .expect("list by desk")
            .is_empty(),
        "and it is not somebody else's"
    );

    // A run written after the upgrade is filed by the same predicate.
    store
        .create_run(&id, NewRun::for_chat("turn-1", "general", "engineer"))
        .await
        .expect("mint a run on the migrated table");
    let mut ids = store
        .list_runs(&id, &RunFilter::for_agent("engineer"))
        .await
        .expect("list by desk")
        .into_iter()
        .map(|r| r.id)
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, ["old-run", "turn-1"]);

    // The heal is idempotent — it runs on every open, and must not rewrite
    // the run history each time. Re-running it leaves the same answer, and
    // nothing is left `NULL` for it to touch.
    heal_runs_agent_id(&store.conn()).expect("the heal is idempotent");
    assert_eq!(
        store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM runs WHERE agent_id IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .expect("count"),
        0,
        "the backfill left nothing behind for a later open to find"
    );
}

/// **Issue #392 through the port**: the host-durable append really does
/// commit under `synchronous=FULL`, and really does put it back.
///
/// `assert_journal_store` cannot see this — a backend that ignored the
/// `Durability` argument entirely stores and orders every record identically
/// and passes the whole suite. So the raise and the restore are asserted
/// here, from inside and outside the closure.
///
/// The restore matters as much as the raise. `synchronous` is connection
/// state, not statement state: leaving `FULL` set would silently buy an
/// fsync for every later write on this connection — a permanent cost from a
/// per-record decision — and restoring only on success would leave it set
/// exactly when something has already gone wrong.
#[test]
fn the_host_durable_write_runs_under_full_sync_and_restores_normal() {
    /// `PRAGMA synchronous`: 1 = NORMAL, 2 = FULL.
    fn synchronous(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA synchronous", [], |r| r.get(0))
            .expect("read the pragma")
    }

    let store = SqliteStore::open_in_memory().expect("open");
    let conn = store.conn();
    assert_eq!(synchronous(&conn), 1, "the standing setting is NORMAL");

    let seen = with_full_sync(&conn, |c| Ok(synchronous(c))).expect("the write runs");
    assert_eq!(
        seen, 2,
        "the write must commit under FULL, or the host-durable level is a no-op"
    );
    assert_eq!(
        synchronous(&conn),
        1,
        "and the connection must be left as it was found"
    );

    // A failing write restores it too, and reports its own error rather than
    // the restore's.
    let err = with_full_sync(&conn, |_| {
        Err::<(), _>(OpenCompanyError::Store("the write failed".into()))
    })
    .expect_err("the write's error reaches the caller");
    assert!(err.to_string().contains("the write failed"));
    assert_eq!(
        synchronous(&conn),
        1,
        "a failed write must not strand the connection on FULL"
    );
}

#[tokio::test]
async fn conformance_journal_store() {
    conformance::assert_journal_store(store()).await;
}

#[tokio::test]
async fn conformance_journal_import() {
    conformance::assert_journal_import(store()).await;
}

#[tokio::test]
async fn conformance_run_store() {
    conformance::assert_run_store(store()).await;
}

#[tokio::test]
async fn conformance_run_reaper() {
    conformance::assert_run_reaper(store()).await;
}

#[tokio::test]
async fn conformance_deep_trace_store() {
    conformance::assert_deep_trace_store(store()).await;
}
