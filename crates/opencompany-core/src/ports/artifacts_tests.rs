use super::*;

fn version(n: u32, body: &str, author: ArtifactAuthor) -> ArtifactVersion {
    ArtifactVersion {
        version: n,
        body: body.to_string(),
        author,
        author_id: "ceo".to_string(),
        created_at_millis: n as u64,
        step_seq: None,
        note: None,
        run_id: None,
        workspace_node_id: None,
    }
}

/// Issue #242: the attempt that wrote a revision is recorded **on that
/// revision**, so a card dispatched twice keeps both links rather than the
/// second attempt erasing the first's. And because artifacts are stored as a
/// JSON blob on every backend, a record written before the field existed
/// loads with `None` — no schema migration, no backfill.
#[test]
fn each_revision_remembers_the_attempt_that_wrote_it() {
    let mut a = ArtifactRecord::new("a1", "t-1", "Draft", ArtifactKind::Text, "one", "ceo", 1);
    a.stamp_run("run-1");
    a.push_version("two", ArtifactAuthor::Agent, "ceo", 2, None);
    a.stamp_run("run-2");
    // An operator edit names no attempt — nobody's run produced it.
    a.push_version("three", ArtifactAuthor::Operator, "operator", 3, None);

    assert_eq!(a.version(1).unwrap().run_id.as_deref(), Some("run-1"));
    assert_eq!(
        a.version(2).unwrap().run_id.as_deref(),
        Some("run-2"),
        "the second attempt must not overwrite the first attempt's link"
    );
    assert_eq!(a.version(3).unwrap().run_id, None);

    let json = serde_json::to_string(&a).expect("serialize");
    assert_eq!(a, serde_json::from_str(&json).expect("round trip"));
    assert!(json.contains(r#""runId":"run-1""#), "{json}");

    // A pre-#242 blob is exactly what an unstamped record serializes to —
    // the field is skipped when absent — so it must still load, with `None`.
    let unstamped = ArtifactRecord::new("a2", "t-1", "Draft", ArtifactKind::Text, "one", "ceo", 1);
    let legacy = serde_json::to_string(&unstamped).expect("serialize");
    assert!(!legacy.contains("runId"), "{legacy}");
    let loaded: ArtifactRecord =
        serde_json::from_str(&legacy).expect("a pre-#242 artifact blob must still load");
    assert!(loaded.versions.iter().all(|v| v.run_id.is_none()));
}

/// Stamping an artifact with no revisions is a no-op rather than a panic —
/// the accessors already tolerate an empty record, and so must this.
#[test]
fn stamping_an_empty_record_is_a_no_op() {
    let mut a = ArtifactRecord::new("a1", "t-1", "Draft", ArtifactKind::Text, "one", "ceo", 1);
    a.versions.clear();
    a.stamp_run("run-1");
    a.stamp_workspace_node("n-1");
    assert!(a.versions.is_empty());
}

/// Issue #552: the workspace node a revision was mirrored into is recorded
/// **on that revision**, for the same reason `run_id` is one field up.
///
/// The case that forces it: an operator deletes a published node (deletions
/// stick), and the next publish materializes a fresh one. v1 must keep
/// naming the node that actually held it — a record-level field would
/// rewrite history and claim it had always lived in the new node.
#[test]
fn each_revision_remembers_the_node_it_was_mirrored_into() {
    let mut a = ArtifactRecord::new("a1", "t-1", "Spec", ArtifactKind::Markdown, "v1", "ceo", 1);
    a.stamp_workspace_node("node-old");
    assert_eq!(a.workspace_node_id(), Some("node-old"));

    // Re-publish after the operator deleted `node-old`: a fresh node, and
    // the old version keeps pointing at the one that held it.
    a.push_version("v2", ArtifactAuthor::Agent, "ceo", 2, None);
    a.stamp_workspace_node("node-new");
    assert_eq!(
        a.version(1).unwrap().workspace_node_id.as_deref(),
        Some("node-old")
    );
    assert_eq!(
        a.version(2).unwrap().workspace_node_id.as_deref(),
        Some("node-new")
    );
    assert_eq!(
        a.workspace_node_id(),
        Some("node-new"),
        "the record-level answer is the newest revision's node, which is where the body lives"
    );

    let json = serde_json::to_string(&a).expect("serialize");
    assert!(json.contains(r#""workspaceNodeId":"node-old""#), "{json}");
    assert_eq!(a, serde_json::from_str(&json).expect("round trip"));
}

/// A pre-#552 blob is exactly what an unmirrored record serializes to — the
/// field is skipped when absent — so it must still load, with `None`. All
/// three backends store an artifact as an opaque JSON blob, so this
/// `#[serde(default)]` is the whole of the migration.
#[test]
fn a_pre_mirror_record_loads_with_no_workspace_node() {
    let legacy = r##"{
        "id": "a1",
        "taskId": "t-1",
        "title": "Launch spec",
        "kind": "markdown",
        "versions": [
            {
                "version": 1,
                "body": "# Spec",
                "author": "agent",
                "authorId": "maya",
                "createdAtMillis": 5
            }
        ],
        "createdAtMillis": 5,
        "updatedAtMillis": 5,
        "source": "specs/launch.md"
    }"##;
    let loaded: ArtifactRecord = serde_json::from_str(legacy).expect("a pre-#552 record parses");
    assert_eq!(loaded.workspace_node_id(), None);
    // Round-tripping one must not mint a node id for it.
    let json = serde_json::to_string(&loaded).unwrap();
    assert!(!json.contains("workspaceNodeId"), "{json}");
}

#[test]
fn push_version_numbers_after_the_current_max() {
    let mut a = ArtifactRecord::new("a1", "t-1", "Draft", ArtifactKind::Text, "one", "ceo", 1);
    assert_eq!(a.latest().unwrap().version, 1);
    let n = a.push_version("two", ArtifactAuthor::Operator, "operator", 2, None);
    assert_eq!(n, 2);
    assert_eq!(a.updated_at_millis, 2);
    assert_eq!(a.version(1).unwrap().body, "one");
    assert_eq!(a.latest().unwrap().body, "two");
}

/// Numbering must come from the max, not `len()`: a record that lost a row
/// would otherwise reuse a number that already means a different revision.
#[test]
fn push_version_does_not_reuse_a_number_after_a_gap() {
    let mut a = ArtifactRecord::new("a1", "t-1", "Draft", ArtifactKind::Text, "one", "ceo", 1);
    a.push_version("two", ArtifactAuthor::Agent, "ceo", 2, None);
    a.push_version("three", ArtifactAuthor::Agent, "ceo", 3, None);
    a.versions.remove(1); // v2 lost; len() is now 2
    assert_eq!(
        a.push_version("four", ArtifactAuthor::Agent, "ceo", 4, None),
        4
    );
}

#[test]
fn no_human_edit_means_no_diff() {
    let mut a = ArtifactRecord::new("a1", "t-1", "Draft", ArtifactKind::Text, "one", "ceo", 1);
    a.push_version("one revised", ArtifactAuthor::Agent, "ceo", 2, None);
    assert!(a.human_edit_diff().is_none());
}

/// The diff anchors on the last *agent* version before the human's, not on
/// v1 — otherwise a redirect (agent re-runs, then a human tweaks the second
/// draft) would diff against the abandoned first draft.
#[test]
fn human_edit_diff_anchors_on_the_agent_version_it_edited() {
    let mut a = ArtifactRecord::new(
        "a1",
        "t-1",
        "Draft",
        ArtifactKind::Text,
        "first draft",
        "ceo",
        1,
    );
    a.push_version("second draft", ArtifactAuthor::Agent, "ceo", 2, None);
    a.push_version(
        "second draft, edited",
        ArtifactAuthor::Operator,
        "operator",
        3,
        None,
    );

    let diff = a.human_edit_diff().expect("a human edited");
    assert_eq!(
        diff.from_version, 2,
        "must anchor on the edited draft, not v1"
    );
    assert_eq!(diff.to_version, 3);
}

#[test]
fn diff_reports_added_removed_and_churn() {
    let from = version(1, "alpha\nbeta\ngamma", ArtifactAuthor::Agent);
    let to = version(2, "alpha\nBETA\ngamma", ArtifactAuthor::Operator);
    let diff = ArtifactDiff::between(&from, &to);

    assert_eq!(diff.added, 1);
    assert_eq!(diff.removed, 1);
    // One of three original lines changed.
    assert!(
        (diff.churn - 1.0 / 3.0).abs() < 1e-9,
        "churn {}",
        diff.churn
    );
    // Unchanged lines survive as context.
    assert_eq!(
        diff.lines.iter().filter(|l| l.op == DiffOp::Equal).count(),
        2
    );
}

#[test]
fn an_untouched_body_diffs_to_all_equal() {
    let from = version(1, "same\nlines", ArtifactAuthor::Agent);
    let to = version(2, "same\nlines", ArtifactAuthor::Operator);
    let diff = ArtifactDiff::between(&from, &to);
    assert_eq!(diff.added, 0);
    assert_eq!(diff.removed, 0);
    assert_eq!(diff.churn, 0.0);
    assert!(diff.lines.iter().all(|l| l.op == DiffOp::Equal));
}

/// A wholesale rewrite is the signal the epic cares about most, so pin that
/// it reads as maximum churn rather than something merely large.
#[test]
fn a_full_rewrite_is_maximum_churn() {
    let from = version(1, "alpha\nbeta", ArtifactAuthor::Agent);
    let to = version(2, "totally\ndifferent", ArtifactAuthor::Operator);
    let diff = ArtifactDiff::between(&from, &to);
    assert_eq!(diff.churn, 1.0);
}

#[test]
fn an_empty_original_does_not_divide_by_zero() {
    let empty = version(1, "", ArtifactAuthor::Agent);
    let filled = version(2, "now it says something", ArtifactAuthor::Operator);
    assert_eq!(ArtifactDiff::between(&empty, &filled).churn, 1.0);
    assert_eq!(ArtifactDiff::between(&empty, &empty).churn, 0.0);
}

/// Past the ceiling the diff degrades to a whole-body replace instead of
/// allocating an n×m table; it must stay truthful, not silently empty.
#[test]
fn an_oversized_body_degrades_to_a_whole_body_replace() {
    let big = (0..MAX_DIFF_LINES + 1)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let from = version(1, &big, ArtifactAuthor::Agent);
    let to = version(2, "one line", ArtifactAuthor::Operator);
    let diff = ArtifactDiff::between(&from, &to);
    assert_eq!(diff.removed, MAX_DIFF_LINES + 1);
    assert_eq!(diff.added, 1);
    assert!(diff.lines.iter().all(|l| l.op != DiffOp::Equal));
}

#[test]
fn a_record_round_trips_through_json() {
    let mut a = ArtifactRecord::new(
        "a1",
        "t-1",
        "Launch post",
        ArtifactKind::Markdown,
        "# Draft",
        "ceo",
        1,
    );
    a.push_version(
        "# Draft, edited",
        ArtifactAuthor::Operator,
        "operator",
        2,
        Some("operator edit before approval".to_string()),
    );
    let json = serde_json::to_string(&a).unwrap();
    // camelCase on the wire, matching every other console-facing record.
    assert!(json.contains("\"taskId\""));
    assert!(json.contains("\"createdAtMillis\""));
    let back: ArtifactRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back, a);
}

/// `step_seq` is the only cross-reference to #185 and is optional, so an
/// artifact must load and serialize without it.
#[test]
fn step_seq_is_optional_and_omitted_when_absent() {
    let a = ArtifactRecord::new("a1", "t-1", "D", ArtifactKind::Text, "b", "ceo", 1);
    let json = serde_json::to_string(&a).unwrap();
    assert!(!json.contains("stepSeq"));
    let back: ArtifactRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back.latest().unwrap().step_seq, None);
}

/// Issue #244: `source` round-trips as camelCase and is omitted entirely
/// when absent, so a record with no path carries no empty scaffolding.
#[test]
fn source_round_trips_and_is_omitted_when_absent() {
    let published = ArtifactRecord::new(
        "a1",
        "t-1",
        "Launch spec",
        ArtifactKind::Markdown,
        "# Spec",
        "ceo",
        1,
    )
    .with_source("specs/launch.md");
    let json = serde_json::to_string(&published).unwrap();
    assert!(json.contains(r#""source":"specs/launch.md""#), "{json}");
    let back: ArtifactRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back, published);
    assert_eq!(back.source.as_deref(), Some("specs/launch.md"));

    let unsourced = ArtifactRecord::new("a2", "t-1", "D", ArtifactKind::Text, "b", "ceo", 1);
    let json = serde_json::to_string(&unsourced).unwrap();
    assert!(!json.contains("source"), "{json}");
}

/// The legacy case, pinned against a literal blob in exactly the shape
/// already on disk: an artifact written before #244 — a captured chat
/// reply, possibly a refusal — must still load, list and diff. `None` is
/// what marks it as legacy, so nothing may invent a value for it.
///
/// Mirrors [`a_pre_run_id_record_still_loads`]-style coverage for the
/// `run_id` field, for the same reason: all three backends store this
/// record as an opaque JSON blob, so a `#[serde(default)]` field is the
/// whole migration.
#[test]
fn a_pre_publish_record_loads_with_no_source() {
    let legacy = r#"{
        "id": "a1",
        "taskId": "t-1",
        "title": "Draft the spec",
        "kind": "text",
        "versions": [
            {
                "version": 1,
                "body": "I can't do this, I'm blocked on the API key.",
                "author": "agent",
                "authorId": "maya",
                "createdAtMillis": 5
            }
        ],
        "createdAtMillis": 5,
        "updatedAtMillis": 5
    }"#;
    let loaded: ArtifactRecord = serde_json::from_str(legacy).expect("a legacy record parses");
    assert_eq!(
        loaded.source, None,
        "absence of a source is what marks a record as legacy capture"
    );
    // It still behaves as an artifact: readable, versionable, diffable.
    assert_eq!(loaded.latest().unwrap().version, 1);
    let mut edited = loaded;
    edited.push_version("The spec.", ArtifactAuthor::Operator, "operator", 6, None);
    assert!(edited.human_edit_diff().is_some());
    // Round-tripping a legacy record must not mint a `source` for it.
    let json = serde_json::to_string(&edited).unwrap();
    assert!(!json.contains("source"), "{json}");
}
