use super::publish_test_helpers_tests::*;
use super::*;
use serde_json::json;

// ── Queue semantics ───────────────────────────────────────────────────────

#[test]
fn the_queue_drains_fifo_and_empties() {
    let queue = PendingPublishQueue::default();
    let publish = |source: &str| PendingPublish {
        agent: "maya".to_string(),
        source: source.to_string(),
        title: source.to_string(),
        kind: ArtifactKind::Text,
        note: None,
        payload: PublishPayload::Text("b".to_string()),
    };
    queue.push(publish("a.md"));
    queue.push(publish("b.md"));
    assert_eq!(queue.sources(), ["a.md", "b.md"]);
    assert_eq!(queue.queued(), 2);

    let drained = queue.drain();
    assert_eq!(
        drained
            .iter()
            .map(|p| p.source.as_str())
            .collect::<Vec<_>>(),
        ["a.md", "b.md"]
    );
    assert_eq!(queue.queued(), 0, "drain empties");
    assert!(queue.drain().is_empty(), "a second drain yields nothing");
}

/// `clear` is what stops an operator chat turn earlier in the same cycle — or
/// an abandoned redirect re-run — from having its staged file attributed to
/// this card.
#[test]
fn clear_drops_what_a_prior_turn_staged() {
    let queue = PendingPublishQueue::default();
    queue.push(PendingPublish {
        agent: "maya".to_string(),
        source: "leftover.md".to_string(),
        title: "leftover".to_string(),
        kind: ArtifactKind::Text,
        note: None,
        payload: PublishPayload::Text("b".to_string()),
    });
    queue.clear();
    assert_eq!(queue.queued(), 0);
    assert!(queue.sources().is_empty());
}

/// The queue handle is shared, not copied — the tool built into the agent and
/// the brain that drains it must see one queue.
///
/// Since #445 that sharing has a second half: the **destination** travels with
/// the clone too. `build_agent` hands the tool one clone and nothing else, so a
/// destination that did not survive cloning would leave every built tool
/// permanently unclaimed and unable to publish at all.
#[tokio::test]
async fn a_cloned_handle_sees_the_same_queue() {
    let dir = workspace(&[("spec.md", b"# Spec")]);
    let queue = PendingPublishQueue::default();
    let tool = PublishArtifactTool::new(dir.path(), "maya", queue.clone());

    let _claim = queue.claim(PublishDestination::Task);
    run(&tool, json!({ "path": "spec.md" })).await;

    assert_eq!(queue.queued(), 1, "the brain's handle sees the tool's push");
}

// ── The scan ──────────────────────────────────────────────────────────────

#[test]
fn the_scan_sees_new_and_modified_files_but_not_deletions() {
    let dir = workspace(&[("keep.md", b"one"), ("gone.md", b"two")]);
    let before = WorkspaceSnapshot::take(dir.path());
    assert_eq!(before.len(), 2);

    std::fs::write(dir.path().join("keep.md"), b"one, revised").unwrap();
    std::fs::write(dir.path().join("fresh.md"), b"new").unwrap();
    std::fs::remove_file(dir.path().join("gone.md")).unwrap();

    let changed = before.changed_since(dir.path()).files;
    assert_eq!(
        changed,
        ["fresh.md", "keep.md"],
        "a deleted file is not a deliverable somebody forgot to publish"
    );
}

/// A same-timestamp rewrite still counts, because size is compared too. Coarse
/// filesystem clocks are common enough that mtime alone would miss real edits.
#[test]
fn a_same_instant_rewrite_of_a_different_length_is_still_a_change() {
    let dir = workspace(&[("spec.md", b"short")]);
    let before = WorkspaceSnapshot::take(dir.path());
    let path = dir.path().join("spec.md");
    let stat = std::fs::metadata(&path).unwrap();
    std::fs::write(&path, b"a considerably longer body").unwrap();
    // Force the mtime back so only the size differs.
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(stat.modified().unwrap()).unwrap();
    drop(file);

    assert_eq!(before.changed_since(dir.path()).files, ["spec.md"]);
}

#[test]
fn the_scan_skips_the_directories_an_exec_sandbox_fills() {
    let dir = workspace(&[
        ("spec.md", b"one"),
        (".git/objects/ab/cdef", b"blob"),
        ("node_modules/left-pad/index.js", b"module"),
        ("target/debug/build.log", b"log"),
    ]);
    let snapshot = WorkspaceSnapshot::take(dir.path());
    assert_eq!(snapshot.len(), 1, "only the agent's own file");
    assert!(!snapshot.truncated());

    // …and they are skipped on the diff side too, so a build never nudges.
    let before = WorkspaceSnapshot::take(dir.path());
    std::fs::write(dir.path().join("target/debug/build.log"), b"rebuilt").unwrap();
    assert!(before.changed_since(dir.path()).files.is_empty());
}

/// **The false-positive test that matters most.** The agent's `workspace_dir`
/// is also where OpenHuman writes its own session transcripts, audit trail and
/// checkpoints — on *every* run, by the harness rather than the agent. If the
/// scan counted them, the nudge would fire after every single dispatch, asking
/// an agent whether its own transcript is a deliverable.
///
/// Found the hard way: before these exclusions, every existing dispatch test
/// grew a second model turn.
#[test]
fn the_scan_ignores_what_the_runtime_itself_writes() {
    let dir = workspace(&[("spec.md", b"one")]);
    let before = WorkspaceSnapshot::take(dir.path());

    // Exactly what a real run leaves behind beside the agent's own work.
    for path in [
        "sessions/2026_08_05/1785952277_chief.md",
        "session_raw/1785952277_chief.jsonl",
        "artifacts/some-id/content",
        "checkpoints/state.json",
        "tinyagents_store/journal/session.1785953147_ceo.messages.jsonl",
        ".openhuman/subagent_checkpoints/a.json",
        ".runs/run-1.json",
        "audit.log",
        ".env",
    ] {
        let full = dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, b"runtime bookkeeping").unwrap();
    }

    assert!(
        before.changed_since(dir.path()).files.is_empty(),
        "the runtime's own files must never look like unpublished agent work"
    );

    // The agent's actual file is still seen, so the exclusions did not blind it.
    std::fs::write(dir.path().join("spec.md"), b"one, revised").unwrap();
    assert_eq!(before.changed_since(dir.path()).files, ["spec.md"]);
}

/// The entry cap. A truncated scan may only under-report — it feeds a warning,
/// never a promotion, so missing something is the acceptable failure.
#[test]
fn the_scan_stops_at_its_entry_cap() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..(MAX_SCAN_ENTRIES + 50) {
        std::fs::write(dir.path().join(format!("f{i}.txt")), b"x").unwrap();
    }
    let snapshot = WorkspaceSnapshot::take(dir.path());
    assert!(snapshot.truncated());
    assert!(snapshot.len() <= MAX_SCAN_ENTRIES);
}

#[test]
fn a_workspace_that_does_not_exist_yet_has_changed_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let never = dir.path().join("no-such-agent/workspace");
    let snapshot = WorkspaceSnapshot::take(&never);
    assert!(snapshot.is_empty());
    assert!(snapshot.changed_since(&never).files.is_empty());
}

#[test]
fn unpublished_is_changed_minus_staged() {
    let changed = vec!["a.md".to_string(), "b.md".to_string(), "c.md".to_string()];
    assert_eq!(
        unpublished(&changed, &["b.md".to_string()]),
        ["a.md", "c.md"]
    );
    assert!(unpublished(&changed, &changed).is_empty(), "all published");
    assert!(unpublished(&[], &[]).is_empty(), "nothing written");
}

#[test]
fn a_long_file_list_is_bounded_and_says_so() {
    let many: Vec<String> = (0..MAX_NAMED_FILES + 7)
        .map(|i| format!("f{i}.txt"))
        .collect();
    let rendered = name_files(&many);
    assert!(rendered.contains("and 7 more"), "{rendered}");
    assert!(!rendered.contains(&format!("f{}.txt", MAX_NAMED_FILES + 1)));
    // Under the bound, nothing is added.
    assert_eq!(name_files(&["a.md".to_string()]), "a.md");
}
