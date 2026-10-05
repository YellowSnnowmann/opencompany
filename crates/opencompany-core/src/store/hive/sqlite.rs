//! [`SqliteStore`] as a [`HiveStore`]: two tables, written in one transaction.
//!
//! - `hive_state` — one row per company: revision, `next_sequence`, body.
//! - `hive_messages` — one row per `(company_id, sequence)`.
//!
//! A commit reads the state row, applies the shared commit check, upserts its
//! rows and swaps the state row inside a single `IMMEDIATE` transaction, so a
//! commit lands whole or not at all and the crash window the `next_sequence`
//! rule exists for cannot open here. The load still clips to `next_sequence`,
//! and rows are written `INSERT OR REPLACE`, so the port's contract holds even
//! for rows that reached the table some other way.
//!
//! Tests live with the store's own (`store/sqlite_hive_tests.rs`), where the
//! `sqlite` CI lane's `store::sqlite` filter selects them.

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Result;
use crate::error::OpenCompanyError;
use crate::ports::hive::{
    CommitCheck, HiveCommit, HiveMessageRow, HiveSnapshot, HiveStateDoc, HiveStore, check_commit,
    load_bound,
};
use crate::ports::now_millis;
use crate::ports::types::CompanyId;
use crate::store::sqlite::{SqliteStore, sql_err};

/// The hive tables, run with the store's own migrations on every open.
/// Idempotent, like the rest of them.
pub(crate) const HIVE_MIGRATIONS: &str = r#"
CREATE TABLE IF NOT EXISTS hive_state (
    company_id    TEXT PRIMARY KEY,
    revision      TEXT NOT NULL,
    next_sequence INTEGER NOT NULL,
    body_json     TEXT NOT NULL,
    updated_ms    INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS hive_messages (
    company_id TEXT NOT NULL,
    sequence   INTEGER NOT NULL,
    body_json  TEXT NOT NULL,
    PRIMARY KEY (company_id, sequence)
);
"#;

#[async_trait]
impl HiveStore for SqliteStore {
    async fn load_hive(
        &self,
        company: &CompanyId,
        before: Option<u64>,
    ) -> Result<Option<HiveSnapshot>> {
        let conn = self.conn();
        let Some(state) = read_state(&conn, company)? else {
            return Ok(None);
        };
        let bound = to_i64(load_bound(state.next_sequence, before))?;
        let mut stmt = conn
            .prepare(
                "SELECT sequence, body_json FROM hive_messages \
                 WHERE company_id = ?1 AND sequence < ?2 ORDER BY sequence ASC",
            )
            .map_err(sql_err)?;
        let rows = stmt
            .query_map(params![company.as_ref(), bound], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sql_err)?;
        let mut messages = Vec::new();
        for row in rows {
            let (sequence, body) = row.map_err(sql_err)?;
            messages.push(HiveMessageRow {
                sequence: from_i64(sequence)?,
                body: serde_json::from_str(&body)?,
            });
        }
        Ok(Some(HiveSnapshot { state, messages }))
    }

    async fn commit_hive(
        &self,
        company: &CompanyId,
        expected: Option<&str>,
        next: HiveStateDoc,
        appended: Vec<HiveMessageRow>,
    ) -> Result<HiveCommit> {
        let mut conn = self.conn();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_err)?;
        let current = read_state(&tx, company)?;
        let current_key = current
            .as_ref()
            .map(|state| (state.revision.as_str(), state.next_sequence));
        if let CommitCheck::Conflict(current) =
            check_commit(current_key, expected, &next, &appended)?
        {
            return Ok(HiveCommit::Conflict { current });
        }
        for row in &appended {
            tx.execute(
                "INSERT OR REPLACE INTO hive_messages (company_id, sequence, body_json) \
                 VALUES (?1, ?2, ?3)",
                params![
                    company.as_ref(),
                    to_i64(row.sequence)?,
                    serde_json::to_string(&row.body)?
                ],
            )
            .map_err(sql_err)?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO hive_state \
             (company_id, revision, next_sequence, body_json, updated_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                company.as_ref(),
                next.revision,
                to_i64(next.next_sequence)?,
                serde_json::to_string(&next.body)?,
                now_millis() as i64
            ],
        )
        .map_err(sql_err)?;
        tx.commit().map_err(sql_err)?;
        Ok(HiveCommit::Committed)
    }

    async fn purge_hive(&self, company: &CompanyId) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_err)?;
        for table in ["hive_state", "hive_messages"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE company_id = ?1"),
                params![company.as_ref()],
            )
            .map_err(sql_err)?;
        }
        tx.commit().map_err(sql_err)
    }
}

/// The company's state row, `None` when it has none.
fn read_state(conn: &Connection, company: &CompanyId) -> Result<Option<HiveStateDoc>> {
    let row = conn
        .query_row(
            "SELECT revision, next_sequence, body_json FROM hive_state WHERE company_id = ?1",
            params![company.as_ref()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sql_err)?;
    row.map(|(revision, next_sequence, body)| {
        Ok(HiveStateDoc {
            revision,
            next_sequence: from_i64(next_sequence)?,
            body: serde_json::from_str(&body)?,
        })
    })
    .transpose()
}

/// SQLite integers are signed; a sequence past `i64::MAX` is refused rather
/// than wrapped into a negative that would sort first.
fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        OpenCompanyError::InvalidRequest(format!("hive sequence {value} is too large"))
    })
}

fn from_i64(value: i64) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| OpenCompanyError::Store(format!("negative hive sequence {value} in sqlite")))
}
