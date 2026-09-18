use super::*;

#[tokio::test]
async fn unscoped_reads_as_unknown() {
    assert_eq!(current(), RunOrigin::Unknown);
}

#[tokio::test]
async fn a_dispatched_origin_is_readable_inside_its_scope() {
    let dispatched = RunOrigin::Dispatched {
        agent: "researcher".to_string(),
        source: DispatchSource::Task,
        scope: None,
    };
    let claim = claim(dispatched.clone());
    let observed = claim.scoped(async { current() }).await;
    assert_eq!(observed, dispatched);
}

#[tokio::test]
async fn an_origin_does_not_escape_its_scope() {
    let claim = claim(RunOrigin::Dispatched {
        agent: "researcher".to_string(),
        source: DispatchSource::Task,
        scope: None,
    });
    claim.scoped(async {}).await;
    // The claim is still alive (not dropped) but its `scope` future has
    // ended — the ambient origin outside that future must not carry it.
    assert_eq!(current(), RunOrigin::Unknown);
}

#[tokio::test]
async fn nested_scopes_restore_the_outer_origin_on_the_way_out() {
    let outer = claim(RunOrigin::Dispatched {
        agent: "researcher".to_string(),
        source: DispatchSource::Task,
        scope: None,
    });
    outer
        .scoped(async {
            let inner = claim(RunOrigin::Dispatched {
                agent: "engineer".to_string(),
                source: DispatchSource::Schedule,
                scope: None,
            });
            inner.scoped(async {}).await;
            assert_eq!(
                current(),
                RunOrigin::Dispatched {
                    agent: "researcher".to_string(),
                    source: DispatchSource::Task,
                    scope: None,
                }
            );
        })
        .await;
}

/// A delegate that runs on a **new task** (a `tokio::spawn` this module
/// never re-scoped) must not inherit the parent's origin — the same
/// fail-closed default as never scoping one at all. Same-task delegation
/// (the ordinary hand-off path, a direct nested `.await` with no spawn in
/// between) is covered by `a_dispatched_origin_is_readable_inside_its_scope`
/// above: a plain nested call reads the same ambient origin its caller
/// did, with no extra code, which is what "inherit rather than re-derive"
/// means at `src/runtime/delegation.rs`.
#[tokio::test]
async fn a_spawned_task_does_not_inherit_the_parent_origin() {
    let parent = claim(RunOrigin::Dispatched {
        agent: "researcher".to_string(),
        source: DispatchSource::Workflow {
            workflow_id: "wf-1".to_string(),
        },
        scope: None,
    });
    let observed = parent
        .scoped(async { tokio::spawn(async { current() }).await.unwrap() })
        .await;
    assert_eq!(observed, RunOrigin::Unknown);
}
