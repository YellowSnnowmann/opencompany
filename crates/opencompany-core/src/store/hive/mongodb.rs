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
        let _ = (company, before);
        todo!("load_hive")
    }

    async fn commit_hive(
        &self,
        company: &CompanyId,
        expected: Option<&str>,
        next: HiveStateDoc,
        appended: Vec<HiveMessageRow>,
    ) -> Result<HiveCommit> {
        let _ = (company, expected, next, appended);
        todo!("commit_hive")
    }

    async fn purge_hive(&self, company: &CompanyId) -> Result<()> {
        let _ = company;
        todo!("purge_hive")
    }
}
