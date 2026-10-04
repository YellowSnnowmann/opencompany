//! [`MemoryHiveStore`] against the shared hive conformance suite.

use std::sync::Arc;

use super::*;
use crate::store::hive::conformance;

#[tokio::test]
async fn conformance_hive_store() {
    conformance::assert_hive_store(Arc::new(MemoryHiveStore::new())).await;
}

#[tokio::test]
async fn conformance_hive_commit_race() {
    conformance::assert_hive_commit_race(Arc::new(MemoryHiveStore::new())).await;
}
