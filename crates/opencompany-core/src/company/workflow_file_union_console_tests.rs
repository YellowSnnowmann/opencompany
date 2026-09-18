//! workflow_file: seed/overlay union loading (issue #168), console-message parity, and issue #1016's per-kind config gate.

use super::*;

// --- seed ∪ overlay union (issue #168) ----------------------------------

use crate::ports::types::OverlayWorkflow;

/// A minimal valid graph body with the given id and display name.
fn body(id: &str, name: &str) -> String {
    format!(
        r#"
id = "{id}"
name = "{name}"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "done"
"#
    )
}

fn overlay(id: &str, name: &str) -> OverlayWorkflow {
    OverlayWorkflow {
        id: id.to_string(),
        toml: body(id, name),
    }
}

/// Writes a seed graph to `<dir>/workflows/<id>.toml`.
fn seed(dir: &Path, id: &str, name: &str) {
    let workflows = dir.join("workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join(format!("{id}.toml")), body(id, name)).unwrap();
}

/// The hosted shape: no source directory at all, so the overlay is the only
/// source. This is the read half of the #168 fix.
#[test]
fn load_union_falls_back_to_the_overlay_with_no_source_dir() {
    let overlays = vec![overlay("hosted", "Hosted flow")];
    let file = load_workflow_union(None, &overlays, "hosted")
        .expect("loads")
        .expect("present");
    assert_eq!(file.id, "hosted");
    assert_eq!(file.name, "Hosted flow");
    assert_eq!(file.nodes.len(), 2);
}

/// A source directory that simply has no file for the id also falls through.
#[test]
fn load_union_falls_back_when_the_seed_file_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), "other", "Other");
    let overlays = vec![overlay("mine", "Mine")];
    let file = load_workflow_union(Some(dir.path()), &overlays, "mine")
        .expect("loads")
        .expect("present");
    assert_eq!(file.name, "Mine");
}

/// Documented precedence: the committed seed file wins over an overlay body
/// with the same id. The overlay is shadowed, not destroyed.
#[test]
fn load_union_prefers_the_seed_file_on_an_id_collision() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), "dup", "From seed");
    let overlays = vec![overlay("dup", "From overlay")];
    let file = load_workflow_union(Some(dir.path()), &overlays, "dup")
        .expect("loads")
        .expect("present");
    assert_eq!(file.name, "From seed");
}

/// An id neither source has is `Ok(None)` — the caller's clean 404, not an
/// error.
#[test]
fn load_union_of_an_unknown_id_is_none() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), "known", "Known");
    assert!(
        load_workflow_union(Some(dir.path()), &[], "ghost")
            .expect("no error")
            .is_none()
    );
    assert!(
        load_workflow_union(None, &[], "ghost")
            .expect("no error")
            .is_none()
    );
}

/// A malformed overlay body surfaces as an error labelled with its id — the
/// same shape a malformed on-disk file gets.
#[test]
fn load_union_of_a_malformed_overlay_is_an_error() {
    let overlays = vec![OverlayWorkflow {
        id: "broken".to_string(),
        toml: "id = \"broken\"\nname = \"Broken\"\n".to_string(),
    }];
    let err = load_workflow_union(None, &overlays, "broken").unwrap_err();
    assert!(err.to_string().contains("trigger"), "{err}");
    assert!(err.to_string().contains("broken.toml"), "{err}");
}

/// The list union dedupes by id with the seed winning, and keeps a stable
/// order (seed scan first, then overlays by id).
#[test]
fn list_union_dedupes_with_source_winning() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), "dup", "From seed");
    seed(dir.path(), "aaa", "Seed A");
    let overlays = vec![
        overlay("zzz", "Overlay Z"),
        overlay("dup", "From overlay"),
        overlay("mmm", "Overlay M"),
    ];
    let files = list_workflows_union(Some(dir.path()), &overlays);
    let ids: Vec<&str> = files.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["aaa", "dup", "mmm", "zzz"]);
    let dup = files.iter().find(|f| f.id == "dup").unwrap();
    assert_eq!(dup.name, "From seed", "the seed file must win");
}

/// With no source directory, the list is exactly the overlay set.
#[test]
fn list_union_with_no_source_dir_is_the_overlay_set() {
    let overlays = vec![overlay("b", "B"), overlay("a", "A")];
    let files = list_workflows_union(None, &overlays);
    let ids: Vec<&str> = files.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b"]);
}

/// One malformed overlay skips only itself — the same tolerance the seed
/// scan has, so a single bad graph never empties the picker.
#[test]
fn list_union_skips_a_malformed_overlay() {
    let overlays = vec![
        overlay("good", "Good"),
        OverlayWorkflow {
            id: "bad".to_string(),
            toml: "id = \"bad\"\nname =".to_string(),
        },
    ];
    let files = list_workflows_union(None, &overlays);
    let ids: Vec<&str> = files.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["good"]);
}

/// A malformed overlay whose id collides with a global workflow must not
/// let the global slip into the list: `load_workflow_with_globals`
/// resolves the company's (broken) definition first and errors, so a list
/// entry backed by the global instead would be one the loader can never
/// actually return.
#[test]
fn list_with_globals_reserves_a_malformed_overlays_id_against_the_global() {
    let taken = crate::globals::workflows()[0].id.clone();
    let overlays = vec![OverlayWorkflow {
        id: taken.clone(),
        toml: "id = \"broken\"\nname =".to_string(),
    }];

    let listed = list_workflows_with_globals(None, &overlays, &[]);
    assert!(
        listed.iter().all(|f| f.id != taken),
        "the global must not appear in place of the company's malformed definition: {listed:?}"
    );

    let loaded = load_workflow_with_globals(None, &overlays, &[], &taken);
    assert!(
        loaded.is_err(),
        "the loader must surface the malformed overlay's error, not the global"
    );
}

// -----------------------------------------------------------------------
// Console drift guard (issue #260)
// -----------------------------------------------------------------------
//
// The console pre-flights the destination and schedule rules client-side so
// a wrong target is caught without a round trip. That is worth keeping — it
// is the difference between instant feedback and a save that bounces — but
// it makes one rule live in two hand-written places, free to drift. Issue
// #260 reports the drift that already happened: two different messages for
// the same rule.
//
// These tests are the coupling. Each shared fragment is asserted TWICE —
// once against this module's live `validate()` output, so a server rewording
// fails here, and once against the console source, so a console rewording
// fails here too. Neither side can be reworded alone.
//
// This is a tripwire, not a proof. The fragment only has to APPEAR in the
// console source, so a stale copy left in a comment would false-pass, and
// nothing here checks that the console's rule FIRES in the same cases the
// host's does. What it does buy is that the specific failure #260 describes
// — one side reworded, the other silently asserting the old contract — can
// no longer happen quietly. Closing the rest means option 3 from the issue:
// the host exposing the destination contract as data.

/// The console's workflow creator, read at compile time so a file move is a
/// build error naming the path rather than a silently-skipped test.
const CONSOLE_DIALOG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../frontend/src/views/WorkflowCreateDialog.tsx"
));
const CONSOLE_DIALOG_PATH: &str = "frontend/src/views/WorkflowCreateDialog.tsx";

/// The console's workflow API module, which declares the picker's
/// destination kinds.
const CONSOLE_API: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../frontend/src/api/workflows.ts"
));
const CONSOLE_API_PATH: &str = "frontend/src/api/workflows.ts";

/// How an `email` destination with a non-address target ends, on both sides.
const EMAIL_TARGET_TAIL: &str = "is not an email address — give the recipient's full address.";
/// How a `channel` destination with no target ends, on both sides.
const CHANNEL_TARGET_TAIL: &str = "name the channel to post the report to.";

/// A graph that trips both destination target rules at once.
const BAD_DESTINATIONS: &str = r#"
    id = "wf"
    name = "WF"

    [[node]]
    id = "start"
    kind = "trigger"
    name = "Start"

    [[node]]
    id = "mailer"
    kind = "output"
    name = "Mailer"
    [node.destination]
    kind = "email"
    target = "nope"

    [[node]]
    id = "poster"
    kind = "output"
    name = "Poster"
    [node.destination]
    kind = "channel"
"#;

#[test]
fn destination_messages_match_the_console() {
    let raw: RawWorkflow = toml::from_str(BAD_DESTINATIONS).expect("the fixture is valid TOML");
    // Destination checks are unconditional, so the load-path (`false`) form
    // surfaces them exactly as `parse_workflow` does.
    let problems = validate(&raw, false).join("\n");

    for tail in [EMAIL_TARGET_TAIL, CHANNEL_TARGET_TAIL] {
        assert!(
            problems.contains(tail),
            "the host stopped saying `{tail}` — if that rewording is deliberate, \
             update this const AND the matching message in {CONSOLE_DIALOG_PATH}, \
             so an author who trips the pre-flight and an author who trips the 400 \
             are still told the same thing.\nhost said:\n{problems}"
        );
        assert!(
            CONSOLE_DIALOG.contains(tail),
            "{CONSOLE_DIALOG_PATH} no longer says `{tail}` — the console's \
             client-side pre-flight has drifted from the host's rule (issue #260). \
             Reword both sides together, or drop the pre-flight and surface the \
             host's message on the failed save."
        );
    }
}

/// The picker's destination kinds, extracted from the console's own
/// `DESTINATION_KINDS` block. A kind added on one side alone is either a
/// picker option the host rejects or one the host accepts and the author
/// can never choose.
#[test]
fn destination_kinds_match_the_console() {
    let start = CONSOLE_API
        .find("export const DESTINATION_KINDS")
        .unwrap_or_else(|| {
            panic!(
                "`DESTINATION_KINDS` is gone from {CONSOLE_API_PATH} — it is what this test reads"
            )
        });
    // Slice from the array opener, NOT from the declaration: the type
    // annotation in between ends `WorkflowDestination["kind"];`, which
    // contains a literal `"];` and would close the block before the first
    // entry.
    let block = &CONSOLE_API[start..];
    let open = block.find("= [").unwrap_or_else(|| {
        panic!("`DESTINATION_KINDS` in {CONSOLE_API_PATH} is no longer an array literal")
    });
    let block = &block[open..];
    let end = block
        .find("];")
        .unwrap_or_else(|| panic!("`DESTINATION_KINDS` in {CONSOLE_API_PATH} has no `];`"));
    let block = &block[..end];

    // Scan for `value: "…"` entries. The type annotation on the same
    // declaration carries a bare `value:` with no string, so keying on the
    // opening quote is what keeps it out.
    let needle = "value: \"";
    let mut console = std::collections::BTreeSet::new();
    let mut rest = block;
    while let Some(at) = rest.find(needle) {
        rest = &rest[at + needle.len()..];
        let close = rest
            .find('"')
            .unwrap_or_else(|| panic!("unterminated `value:` in {CONSOLE_API_PATH}"));
        console.insert(&rest[..close]);
        rest = &rest[close..];
    }

    let host: std::collections::BTreeSet<&str> =
        WORKFLOW_DESTINATION_KINDS.iter().copied().collect();
    assert_eq!(
        console, host,
        "the console's DESTINATION_KINDS picker ({CONSOLE_API_PATH}) and the host's \
         WORKFLOW_DESTINATION_KINDS disagree — one side offers a kind the other \
         does not know (issue #260)"
    );
}

/// The console's `looksLikeCron` pre-flight counts whitespace-separated
/// fields and accepts exactly five, so it is only correct while the host's
/// parser draws the line in the same place. Relaxing the host to accept a
/// 6-field (seconds) expression without touching the console would make the
/// console reject input the host now takes — the drift direction #260 says
/// bites, because the console is the stricter side by construction.
#[test]
fn cron_arity_matches_the_console_preflight() {
    use crate::runtime::cron::CronExpr;
    assert!(CronExpr::parse("0 9 * * MON").is_ok(), "5 fields");
    assert!(CronExpr::parse("0 9 * *").is_err(), "4 fields");
    assert!(CronExpr::parse("0 0 9 * * MON").is_err(), "6 fields");
    assert!(
        CONSOLE_DIALOG.contains("function looksLikeCron"),
        "{CONSOLE_DIALOG_PATH} no longer defines `looksLikeCron` — this test \
         exists to pin the arity that helper assumes"
    );
}

// --- issue #1016: per-kind config gate (transform / split_out / http url /
//     output_parser) --------------------------------------------------------

/// Parses a bare TOML config table for a node.
fn cfg(src: &str) -> toml::Value {
    toml::from_str::<toml::Value>(src).expect("valid config table")
}

fn problems(kind: WorkflowNodeKind, config: Option<&toml::Value>) -> Vec<WorkflowProblem> {
    required_config_problems(kind, "n", "node `n`", config)
}

#[test]
fn transform_without_set_is_rejected_naming_the_field() {
    let out = problems(WorkflowNodeKind::Transform, None);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].node_id.as_deref(), Some("n"));
    assert_eq!(out[0].field.as_deref(), Some("config.set"));
}

#[test]
fn transform_with_empty_set_is_rejected() {
    let config = cfg("[set]\n");
    assert_eq!(
        problems(WorkflowNodeKind::Transform, Some(&config))[0]
            .field
            .as_deref(),
        Some("config.set")
    );
}

#[test]
fn transform_with_non_string_set_value_is_rejected() {
    let config = cfg("[set]\ncount = 3\n");
    let out = problems(WorkflowNodeKind::Transform, Some(&config));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].field.as_deref(), Some("config.set"));
}

#[test]
fn transform_with_string_expressions_is_accepted() {
    let config = cfg("[set]\nname = \"=item.name\"\n");
    assert!(problems(WorkflowNodeKind::Transform, Some(&config)).is_empty());
}

#[test]
fn split_out_without_path_is_rejected_naming_the_field() {
    let out = problems(WorkflowNodeKind::SplitOut, None);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].field.as_deref(), Some("config.path"));
}

#[test]
fn split_out_with_path_is_accepted() {
    let config = cfg("path = \"items\"\n");
    assert!(problems(WorkflowNodeKind::SplitOut, Some(&config)).is_empty());
}

#[test]
fn http_request_url_must_be_a_real_url() {
    let good = cfg("method = \"GET\"\nurl = \"https://example.com/x\"\n");
    assert!(problems(WorkflowNodeKind::HttpRequest, Some(&good)).is_empty());

    // "" is rejected as before (regression guard) …
    let empty = cfg("method = \"GET\"\nurl = \"\"\n");
    assert_eq!(
        problems(WorkflowNodeKind::HttpRequest, Some(&empty))[0]
            .field
            .as_deref(),
        Some("config.url")
    );

    // … and now `ftp://x` and bare `garbage` are rejected too (new).
    for bad in ["ftp://x", "garbage", "https://"] {
        let config = cfg(&format!("method = \"GET\"\nurl = \"{bad}\"\n"));
        let out = problems(WorkflowNodeKind::HttpRequest, Some(&config));
        assert_eq!(out.len(), 1, "{bad}: {out:?}");
        assert_eq!(out[0].field.as_deref(), Some("config.url"), "{bad}");
    }
}

#[test]
fn output_parser_is_schema_less_by_default() {
    // A bare identity parser (no config at all) is accepted.
    assert!(problems(WorkflowNodeKind::OutputParser, None).is_empty());
    // A present but mistyped key is rejected, each naming its field.
    let config = cfg("auto_fix = \"yes\"\nconnection_ref = 5\n");
    let out = problems(WorkflowNodeKind::OutputParser, Some(&config));
    let fields: Vec<&str> = out.iter().filter_map(|p| p.field.as_deref()).collect();
    assert!(fields.contains(&"config.auto_fix"), "{fields:?}");
    assert!(fields.contains(&"config.connection_ref"), "{fields:?}");
}

#[test]
fn output_parser_with_well_typed_keys_is_accepted() {
    let config = cfg("auto_fix = true\nconnection_ref = \"conn\"\n[schema]\nname = \"string\"\n");
    assert!(problems(WorkflowNodeKind::OutputParser, Some(&config)).is_empty());
}

#[test]
fn merge_stays_config_free() {
    assert!(problems(WorkflowNodeKind::Merge, None).is_empty());
    let config = cfg("anything = \"goes\"\n");
    assert!(problems(WorkflowNodeKind::Merge, Some(&config)).is_empty());
}
