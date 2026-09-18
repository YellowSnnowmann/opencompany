//! Locking and `check_model_id` tests: the per-company index lock, and
//! the model-id validation rules (split out of `store_tests.rs`).

use super::*;

// ---- index_lock (keys rework, issue #2306, round-3b lock coordination) -

#[tokio::test]
async fn index_lock_serialises_two_holders_on_the_same_company() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let company = CompanyId::new("acme-co");
    let inside = Arc::new(AtomicBool::new(false));
    let overlapped = Arc::new(AtomicBool::new(false));

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let company = company.clone();
        let inside = inside.clone();
        let overlapped = overlapped.clone();
        tasks.push(tokio::spawn(async move {
            let _guard = index_lock(&company).await;
            if inside.swap(true, Ordering::SeqCst) {
                overlapped.store(true, Ordering::SeqCst);
            }
            tokio::task::yield_now().await;
            inside.store(false, Ordering::SeqCst);
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert!(
        !overlapped.load(Ordering::SeqCst),
        "two holders of the same company's lock ran inside the guarded section at once"
    );
}

#[tokio::test]
async fn index_lock_does_not_block_a_different_company() {
    let a = CompanyId::new("acme-co");
    let b = CompanyId::new("other-co");
    let _guard_a = index_lock(&a).await;
    // A different company's lock must not wait on this one.
    tokio::time::timeout(std::time::Duration::from_secs(2), index_lock(&b))
        .await
        .expect("a different company's lock must not wait on this one");
}

/// Round-3a review P2-2's exact scenario, reproduced directly: "a
/// concurrent PATCH that pins `acme` and a DELETE of `acme` can both pass
/// their checks." Two different guarded mutations — not two holders of
/// the same shape, which [`index_lock_serialises_two_holders_on_the_same_company`]
/// already covers — each doing a check, a yield (so a race would need to
/// interleave right here to go unnoticed), then a write. If the lock
/// wired into both call sites actually serialises them, the delete's read
/// of the row always happens either wholly before or wholly after the
/// pin's check-and-write, never in between it.
#[tokio::test]
async fn a_pin_and_a_delete_of_the_same_provider_never_interleave() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let company = CompanyId::new("acme-co");
    let row_present = Arc::new(AtomicBool::new(true));
    let pin_saw_row_gone_mid_write = Arc::new(AtomicBool::new(false));

    let pin_row_present = row_present.clone();
    let pin_saw_gone = pin_saw_row_gone_mid_write.clone();
    let pin_company = company.clone();
    let pin = tokio::spawn(async move {
        let _guard = index_lock(&pin_company).await;
        // Check: the pin validates the row exists, exactly as
        // `server::ops::team_agent::edit_agent` does under this lock.
        let existed = pin_row_present.load(Ordering::SeqCst);
        tokio::task::yield_now().await;
        // Write: only meaningful if the row was still there when checked —
        // a delete that ran inside this critical section would make this
        // pin write against a row it never actually validated.
        if existed && !pin_row_present.load(Ordering::SeqCst) {
            pin_saw_gone.store(true, Ordering::SeqCst);
        }
    });

    let delete_row_present = row_present.clone();
    let delete_company = company.clone();
    let delete = tokio::spawn(async move {
        let _guard = index_lock(&delete_company).await;
        // Check: the delete reads `usedBy`, exactly as
        // `server::ops::inference::providers::delete_provider` does under
        // this lock.
        let _used_by_snapshot = delete_row_present.load(Ordering::SeqCst);
        tokio::task::yield_now().await;
        // Write: the row goes.
        delete_row_present.store(false, Ordering::SeqCst);
    });

    pin.await.unwrap();
    delete.await.unwrap();
    assert!(
        !pin_saw_row_gone_mid_write.load(Ordering::SeqCst),
        "the pin's check and write must never straddle the delete's write — the lock \
         wired into both handlers should have serialised them"
    );
}

// ---- check_model_id (keys rework, issue #2306, slice 2c) ---------------

#[test]
fn a_model_id_is_trimmed() {
    assert_eq!(
        check_model_id("  acme/test-model \n").unwrap(),
        "acme/test-model"
    );
}

#[test]
fn an_empty_model_id_is_refused() {
    for raw in ["", "   "] {
        let err = check_model_id(raw).unwrap_err();
        assert!(err.to_string().contains("Choose a model"), "{err}");
    }
}

#[test]
fn a_model_id_with_a_control_character_is_refused() {
    let err = check_model_id("test\u{0007}model").unwrap_err();
    assert!(err.to_string().contains("control characters"), "{err}");
}

#[test]
fn a_model_id_with_inner_whitespace_is_refused() {
    let err = check_model_id("test model").unwrap_err();
    assert!(err.to_string().contains("spaces"), "{err}");
}

#[test]
fn a_model_id_is_bounded_in_chars_not_bytes() {
    assert!(check_model_id(&"é".repeat(256)).is_ok());
    let err = check_model_id(&"é".repeat(257)).unwrap_err();
    assert!(err.to_string().contains("256"), "{err}");
}

#[test]
fn every_tier_name_is_refused_as_a_model_id() {
    for tier in crate::company::INFERENCE_TIERS {
        let err = check_model_id(tier).unwrap_err();
        assert!(err.to_string().contains("workload name"), "{err}");
    }
    let err = check_model_id(" chat-v1 ").unwrap_err();
    assert!(err.to_string().contains("workload name"), "{err}");
}

#[test]
fn model_ids_of_every_shape_pass() {
    for id in [
        "acme/test-model",
        "acme/test-model:free",
        "test-model:8b",
        "test-model",
        "test.deployment-1",
    ] {
        assert_eq!(check_model_id(id).unwrap(), id);
    }
}
