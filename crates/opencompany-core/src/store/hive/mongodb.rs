//! [`MongoStore`] as a [`HiveStore`]: a `hive_state` document per company and
//! a `hive_messages` collection of rows.
//!
//! ## Why the commit's rows ride inside the swap
//!
//! MongoDB gives this backend no transaction it can require of its deployment
//! (a standalone server has none), so rows and state cannot be written at once.
//! Writing the rows *first* and swapping the state *second* — the order the
//! filesystem uses under its lock — is not enough here, because nothing
//! serializes two replicas: a writer that is about to lose the swap can upsert
//! its rows over sequences the winner has just committed, and the log then
//! holds the loser's text under the winner's state.
//!
//! So the swap is the only write a commit makes before it has won:
//!
//! 1. Read the state document. Apply the shared commit check.
//! 2. Upsert the rows the *previous* commit carried in `pending` (they are
//!    committed already, so this rewrites identical bytes at worst).
//! 3. Swap the document — `update_one({_id, revision: expected})`, or an
//!    `insert_one` for a first commit, which the `_id` index makes a
//!    compare-and-swap too — with this commit's rows as the new `pending`.
//! 4. Upsert those rows into `hive_messages`, best effort.
//!
//! A loser fails at step 3 having written only committed rows. A crash after
//! step 3 leaves rows only in `pending`, which [`load_hive`] overlays and the
//! next commit materializes at its step 2. Rows in `hive_messages` at or above
//! `next_sequence` — a recreated company's leftovers, or anything else — are
//! clipped on load and replaced by the commit that claims their sequence.
//!
//! The state document is keyed `_id = company_id`, so the one-document-per-
//! company invariant is the always-present `_id` index rather than one
//! `ensure_indexes` could fail to build; rows are keyed `_id = {c, s}` for the
//! same reason, and `(company_id, sequence)` is indexed for the range read.
//! Company ids arrive already tenant-namespaced in shared-single-DB mode, so
//! neither key can collide across tenants.
//!
//! [`load_hive`]: HiveStore::load_hive

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::stream::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};

use crate::Result;
use crate::error::OpenCompanyError;
use crate::ports::hive::{
    CommitCheck, HiveCommit, HiveMessageRow, HiveSnapshot, HiveStateDoc, HiveStore, check_commit,
    load_bound,
};
use crate::ports::now_millis;
use crate::ports::types::CompanyId;
use crate::store::mongodb::{MongoStore, get_i64, get_str, is_duplicate_key, mongo_err};

/// One state document per company, `_id = company_id`.
pub(crate) const HIVE_STATE: &str = "hive_state";
/// One document per message row, `_id = {c: company_id, s: sequence}`.
pub(crate) const HIVE_MESSAGES: &str = "hive_messages";

/// The state document as read back: the port's document plus the rows its
/// commit carried.
struct Stored {
    state: HiveStateDoc,
    pending: Vec<HiveMessageRow>,
}

#[async_trait]
impl HiveStore for MongoStore {
    async fn load_hive(
        &self,
        company: &CompanyId,
        before: Option<u64>,
    ) -> Result<Option<HiveSnapshot>> {
        let Some(stored) = read_state(self, company).await? else {
            return Ok(None);
        };
        let bound = load_bound(stored.state.next_sequence, before);
        let mut rows: BTreeMap<u64, HiveMessageRow> = BTreeMap::new();
        let mut cursor = self
            .collection(HIVE_MESSAGES)
            .find(doc! {"company_id": company.as_ref(), "sequence": {"$lt": to_i64(bound)?}})
            .sort(doc! {"sequence": 1})
            .await
            .map_err(mongo_err)?;
        while let Some(document) = cursor.try_next().await.map_err(mongo_err)? {
            let row = decode_row(&document)?;
            rows.insert(row.sequence, row);
        }
        // The latest commit's rows may not have reached the collection yet.
        for row in stored.pending {
            if row.sequence < bound {
                rows.insert(row.sequence, row);
            }
        }
        Ok(Some(HiveSnapshot {
            state: stored.state,
            messages: rows.into_values().collect(),
        }))
    }

    async fn commit_hive(
        &self,
        company: &CompanyId,
        expected: Option<&str>,
        next: HiveStateDoc,
        appended: Vec<HiveMessageRow>,
    ) -> Result<HiveCommit> {
        let current = read_state(self, company).await?;
        let current_key = current
            .as_ref()
            .map(|stored| (stored.state.revision.as_str(), stored.state.next_sequence));
        if let CommitCheck::Conflict(current) = check_commit(current_key, expected, &next, &appended)? {
            return Ok(HiveCommit::Conflict { current });
        }
        // Step 2: the previous commit's rows must be in the collection before
        // this swap replaces the only other copy of them.
        if let Some(stored) = &current {
            materialize(self, company, &stored.pending).await?;
        }

        let pending = appended
            .iter()
            .map(encode_pending)
            .collect::<Result<Vec<Document>>>()?;
        let fields = doc! {
            "company_id": company.as_ref(),
            "revision": next.revision.as_str(),
            "next_sequence": to_i64(next.next_sequence)?,
            "body_json": serde_json::to_string(&next.body)?,
            "pending": pending,
            "updated_ms": now_millis() as i64,
        };
        let state = self.collection(HIVE_STATE);
        let swapped = match expected {
            None => {
                let mut document = doc! {"_id": company.as_ref()};
                document.extend(fields);
                match state.insert_one(document).await {
                    Ok(_) => true,
                    Err(error) if is_duplicate_key(&error) => false,
                    Err(error) => return Err(mongo_err(error)),
                }
            }
            Some(expected) => {
                state
                    .update_one(
                        doc! {"_id": company.as_ref(), "revision": expected},
                        doc! {"$set": fields},
                    )
                    .await
                    .map_err(mongo_err)?
                    .matched_count
                    == 1
            }
        };
        if !swapped {
            let current = read_state(self, company)
                .await?
                .map(|stored| stored.state.revision);
            return Ok(HiveCommit::Conflict { current });
        }

        // Step 4: best effort. The rows are committed (they are in `pending`),
        // so failing the call here would tell the caller a landed commit did
        // not land; the next commit writes them out before it swaps.
        if let Err(error) = materialize(self, company, &appended).await {
            tracing::warn!(
                company = %company.as_ref(), %error,
                "hive rows committed but not yet written to hive_messages; \
                 the next commit writes them"
            );
        }
        Ok(HiveCommit::Committed)
    }

    /// State first, so the company reads as absent the moment the purge
    /// starts; rows a crash leaves behind sit above any recreated document's
    /// `next_sequence` until a commit replaces them.
    async fn purge_hive(&self, company: &CompanyId) -> Result<()> {
        self.collection(HIVE_STATE)
            .delete_one(doc! {"_id": company.as_ref()})
            .await
            .map_err(mongo_err)?;
        self.collection(HIVE_MESSAGES)
            .delete_many(doc! {"company_id": company.as_ref()})
            .await
            .map_err(mongo_err)?;
        Ok(())
    }
}

async fn read_state(store: &MongoStore, company: &CompanyId) -> Result<Option<Stored>> {
    let Some(document) = store
        .collection(HIVE_STATE)
        .find_one(doc! {"_id": company.as_ref()})
        .await
        .map_err(mongo_err)?
    else {
        return Ok(None);
    };
    let pending = match document.get_array("pending") {
        Ok(items) => items
            .iter()
            .map(|item| match item {
                Bson::Document(row) => decode_row(row),
                other => Err(mongo_err(format!("hive pending row is not a document: {other}"))),
            })
            .collect::<Result<Vec<_>>>()?,
        Err(_) => Vec::new(),
    };
    Ok(Some(Stored {
        state: HiveStateDoc {
            revision: get_str(&document, "revision")?,
            next_sequence: from_i64(get_i64(&document, "next_sequence")?)?,
            body: serde_json::from_str(&get_str(&document, "body_json")?)?,
        },
        pending,
    }))
}

/// Upserts `rows` into `hive_messages`, replacing whatever holds each key —
/// which is how an orphan above the old `next_sequence` is overwritten.
async fn materialize(store: &MongoStore, company: &CompanyId, rows: &[HiveMessageRow]) -> Result<()> {
    let messages = store.collection(HIVE_MESSAGES);
    for row in rows {
        let sequence = to_i64(row.sequence)?;
        messages
            .replace_one(
                doc! {"_id": {"c": company.as_ref(), "s": sequence}},
                doc! {
                    "company_id": company.as_ref(),
                    "sequence": sequence,
                    "body_json": serde_json::to_string(&row.body)?,
                },
            )
            .upsert(true)
            .await
            .map_err(mongo_err)?;
    }
    Ok(())
}

fn encode_pending(row: &HiveMessageRow) -> Result<Document> {
    Ok(doc! {
        "sequence": to_i64(row.sequence)?,
        "body_json": serde_json::to_string(&row.body)?,
    })
}

fn decode_row(document: &Document) -> Result<HiveMessageRow> {
    Ok(HiveMessageRow {
        sequence: from_i64(get_i64(document, "sequence")?)?,
        body: serde_json::from_str(&get_str(document, "body_json")?)?,
    })
}

/// BSON integers are signed; a sequence past `i64::MAX` is refused rather
/// than wrapped into a negative that would sort first.
fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| OpenCompanyError::InvalidRequest(format!("hive sequence {value} is too large")))
}

fn from_i64(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| mongo_err(format!("negative hive sequence {value}")))
}
