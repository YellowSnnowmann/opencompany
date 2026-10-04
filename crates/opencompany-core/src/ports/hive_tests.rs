//! The commit rule every [`HiveStore`] backend applies before writing.

use super::*;

fn doc(revision: &str, next_sequence: u64) -> HiveStateDoc {
    HiveStateDoc {
        revision: revision.into(),
        next_sequence,
        body: serde_json::json!({"rev": revision}),
    }
}

fn rows(sequences: &[u64]) -> Vec<HiveMessageRow> {
    sequences
        .iter()
        .map(|&sequence| HiveMessageRow {
            sequence,
            body: serde_json::json!({"n": sequence}),
        })
        .collect()
}

fn refused(result: Result<CommitCheck>) -> String {
    match result {
        Err(OpenCompanyError::InvalidRequest(message)) => message,
        other => panic!("expected an invalid-request refusal, got {other:?}"),
    }
}

#[test]
fn a_first_commit_against_no_document_proceeds() {
    assert_eq!(
        check_commit(None, None, &doc("r1", 2), &rows(&[0, 1])).unwrap(),
        CommitCheck::Proceed
    );
}

#[test]
fn a_matching_revision_proceeds_with_rows_from_the_stored_next_sequence() {
    assert_eq!(
        check_commit(Some(("r1", 2)), Some("r1"), &doc("r2", 4), &rows(&[2, 3])).unwrap(),
        CommitCheck::Proceed
    );
}

#[test]
fn a_state_only_commit_proceeds() {
    assert_eq!(
        check_commit(Some(("r1", 2)), Some("r1"), &doc("r2", 2), &[]).unwrap(),
        CommitCheck::Proceed
    );
}

#[test]
fn an_expected_revision_that_is_not_stored_conflicts_with_the_stored_one() {
    assert_eq!(
        check_commit(Some(("r2", 4)), Some("r1"), &doc("r3", 5), &rows(&[4])).unwrap(),
        CommitCheck::Conflict(Some("r2".into()))
    );
}

#[test]
fn creating_a_document_that_already_exists_conflicts() {
    assert_eq!(
        check_commit(Some(("r1", 2)), None, &doc("r9", 1), &rows(&[0])).unwrap(),
        CommitCheck::Conflict(Some("r1".into()))
    );
}

#[test]
fn expecting_a_document_that_does_not_exist_conflicts_with_none() {
    assert_eq!(
        check_commit(None, Some("r1"), &doc("r2", 1), &rows(&[0])).unwrap(),
        CommitCheck::Conflict(None)
    );
}

/// A stale writer is told it is stale before it is told its rows no longer
/// fit: reloading fixes the first, and only the first.
#[test]
fn a_conflict_outranks_a_malformed_commit() {
    assert_eq!(
        check_commit(Some(("r2", 9)), Some("r1"), &doc("r3", 5), &rows(&[4])).unwrap(),
        CommitCheck::Conflict(Some("r2".into()))
    );
}

#[test]
fn a_row_below_the_stored_next_sequence_is_refused() {
    let message = refused(check_commit(
        Some(("r1", 2)),
        Some("r1"),
        &doc("r2", 3),
        &rows(&[1, 2]),
    ));
    assert!(message.contains("sequence 1"), "{message}");
}

#[test]
fn a_row_at_or_above_the_new_next_sequence_is_refused() {
    let message = refused(check_commit(
        Some(("r1", 2)),
        Some("r1"),
        &doc("r2", 3),
        &rows(&[2, 3]),
    ));
    assert!(message.contains("sequence 3"), "{message}");
}

#[test]
fn rows_that_do_not_ascend_strictly_are_refused() {
    refused(check_commit(None, None, &doc("r1", 3), &rows(&[1, 0])));
    refused(check_commit(None, None, &doc("r1", 3), &rows(&[1, 1])));
}

#[test]
fn a_next_sequence_that_goes_backwards_is_refused() {
    let message = refused(check_commit(
        Some(("r1", 5)),
        Some("r1"),
        &doc("r2", 4),
        &[],
    ));
    assert!(message.contains("next_sequence"), "{message}");
}

#[test]
fn a_commit_that_keeps_the_revision_is_refused() {
    let message = refused(check_commit(
        Some(("r1", 2)),
        Some("r1"),
        &doc("r1", 2),
        &[],
    ));
    assert!(message.contains("revision"), "{message}");
}

#[test]
fn an_empty_revision_is_refused() {
    refused(check_commit(None, None, &doc("", 0), &[]));
}

#[test]
fn the_load_bound_clips_the_callers_bound_to_the_committed_range() {
    assert_eq!(load_bound(5, None), 5);
    assert_eq!(load_bound(5, Some(3)), 3);
    assert_eq!(load_bound(5, Some(9)), 5);
}
