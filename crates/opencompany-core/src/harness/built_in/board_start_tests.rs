//! Tests for the board-start handle.

use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct Recorder {
    started: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl BoardStarter for Recorder {
    async fn start(&self, _observed: &TaskRecord, card: &TaskRecord) -> crate::Result<bool> {
        self.started.lock().expect("recorder").push(card.id.clone());
        Ok(true)
    }
}

/// An unfilled handle answers `None` rather than panicking, because a build with
/// no starter is ordinary — every test runtime, and any embedder that wired none.
/// The tool refuses in its own turn on that answer.
#[test]
fn an_unfilled_handle_is_empty() {
    assert!(BoardStarterHandle::default().get().is_none());
}

/// Every clone sees one cell, which is what makes filling it after the deps are
/// cloned into the brain and its tools work at all.
#[test]
fn a_clone_sees_the_fill() {
    let handle = BoardStarterHandle::default();
    let on_deps = handle.clone();
    let in_tool = handle.clone();
    assert!(in_tool.get().is_none());

    let starter: Arc<dyn BoardStarter> = Arc::new(Recorder::default());
    on_deps.set(&starter);

    assert!(
        in_tool.get().is_some(),
        "the fill must reach the tool's clone"
    );
    assert!(handle.get().is_some());
}

/// The cell is weak, so it does not keep the owner alive — and reports empty once
/// the owner is dropped instead of handing back a dangling capability.
#[test]
fn the_handle_does_not_own_the_starter() {
    let handle = BoardStarterHandle::default();
    {
        let starter: Arc<dyn BoardStarter> = Arc::new(Recorder::default());
        handle.set(&starter);
        assert!(handle.get().is_some());
    }
    assert!(
        handle.get().is_none(),
        "a dropped owner must read as unwired, never as a live capability"
    );
}

/// A rebuild repoints the handle rather than being ignored.
///
/// This cell is keyed on the company and outlives the runtime it names, so a
/// write-once cell would leave a rebuilt company pointing at its predecessor's
/// shim — whose `Weak` is dead — and `start_task` would read unwired forever after
/// the first swap. `WorkflowRunnerHandle` can be write-once precisely because it
/// does not outlive its owner; this one does.
#[test]
fn a_second_fill_repoints_the_handle() {
    let handle = BoardStarterHandle::default();
    let first: Arc<dyn BoardStarter> = Arc::new(Recorder::default());
    handle.set(&first);

    let second: Arc<dyn BoardStarter> = Arc::new(Recorder::default());
    handle.set(&second);
    drop(first);

    assert!(
        handle.get().is_some(),
        "dropping the replaced owner must not unwire the handle"
    );
}

/// One handle per company, and never shared between them.
#[test]
fn the_registry_keys_handles_by_company() {
    use crate::ports::types::CompanyId;

    let acme = for_company(&CompanyId::new("acme-board-start"));
    let again = for_company(&CompanyId::new("acme-board-start"));
    let other = for_company(&CompanyId::new("other-board-start"));

    let starter: Arc<dyn BoardStarter> = Arc::new(Recorder::default());
    acme.set(&starter);

    assert!(again.get().is_some(), "the same company gets the same cell");
    assert!(
        other.get().is_none(),
        "another company must not see this one's capability"
    );
}
