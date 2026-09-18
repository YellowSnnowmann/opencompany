use super::tests::*;
use super::*;
use crate::store::FsOps;

// -- write behaviour ----------------------------------------------------

#[tokio::test]
async fn a_write_with_the_current_revision_lands() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceWriteTool::new(ws(store.clone(), id.clone()));
    let result = tool
        .execute(json!({
            "path": "standards/engineering-standards.md",
            "content": "# Engineering\nShip on Fridays.",
            "expected_updated_at": 2_000,
        }))
        .await
        .unwrap();
    assert!(!result.is_error, "{}", text(&result));

    let (_, body) = store.read(&id, "n-eng").await.unwrap().unwrap();
    assert_eq!(body, "# Engineering\nShip on Fridays.");
}

/// Models stringify numbers constantly. `"2000"` must land exactly as
/// `2000` does — the old `as_u64`-only read rejected it with "is required",
/// which reads as "you forgot the argument" for an argument the agent did
/// supply, and costs a turn to recover from.
#[tokio::test]
async fn a_revision_is_accepted_as_a_number_or_a_string() {
    for revision in [json!(2_000), json!("2000"), json!(" 2000 ")] {
        let (_dir, store) = seeded("acme").await;
        let id = CompanyId::new("acme");
        let tool = WorkspaceWriteTool::new(ws(store.clone(), id.clone()));
        let result = tool
            .execute(json!({
                "id": "n-eng",
                "content": "# Engineering\nShip on Fridays.",
                "expected_updated_at": revision,
            }))
            .await
            .unwrap();
        assert!(
            !result.is_error,
            "revision {revision} was rejected: {}",
            text(&result)
        );

        let (_, body) = store.read(&id, "n-eng").await.unwrap().unwrap();
        assert_eq!(body, "# Engineering\nShip on Fridays.", "for {revision}");
    }
}

/// A string that is not a revision is still a missing revision — the
/// fallback widens the accepted spelling, never the guard itself.
#[tokio::test]
async fn a_non_numeric_revision_string_is_still_refused() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceWriteTool::new(ws(store.clone(), id.clone()));
    let result = tool
        .execute(json!({
            "id": "n-eng",
            "content": "clobbered",
            "expected_updated_at": "latest",
        }))
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(text(&result).contains("expected_updated_at"));

    let (_, body) = store.read(&id, "n-eng").await.unwrap().unwrap();
    assert_eq!(body, "# Engineering\nReview every PR.");
}

#[tokio::test]
async fn a_stale_revision_is_refused_and_names_the_current_one() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceWriteTool::new(ws(store.clone(), id.clone()));
    let result = tool
        .execute(json!({
            "id": "n-eng",
            "content": "clobbered",
            "expected_updated_at": 1,
        }))
        .await
        .unwrap();
    assert!(result.is_error);
    let out = text(&result);
    assert!(out.contains("changed since you read it"), "{out}");
    assert!(
        out.contains("2000"),
        "must name the current revision: {out}"
    );

    let (_, body) = store.read(&id, "n-eng").await.unwrap().unwrap();
    assert_eq!(
        body, "# Engineering\nReview every PR.",
        "note was clobbered"
    );
}

/// Required, not optional: without the token a hallucinated path under
/// `full` policy mode would overwrite an operator's note unchallenged.
#[tokio::test]
async fn a_write_without_a_revision_is_refused() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceWriteTool::new(ws(store.clone(), id.clone()));
    let result = tool
        .execute(json!({"id": "n-eng", "content": "blind"}))
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(text(&result).contains("expected_updated_at"));

    let (_, body) = store.read(&id, "n-eng").await.unwrap().unwrap();
    assert_eq!(body, "# Engineering\nReview every PR.");
}

/// Create stays operator-only: there is no revision for a note that does
/// not exist, so a write cannot conjure one.
#[tokio::test]
async fn a_write_cannot_create_a_note() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceWriteTool::new(ws(store.clone(), id.clone()));
    let result = tool
        .execute(json!({
            "path": "standards/brand new.md",
            "content": "hello",
            "expected_updated_at": 0,
        }))
        .await
        .unwrap();
    assert!(result.is_error);
    assert_eq!(
        store.tree(&id).await.unwrap().len(),
        3,
        "nothing was created"
    );
}

#[tokio::test]
async fn a_write_cannot_target_a_folder() {
    let (_dir, store) = seeded("acme").await;
    let tool = WorkspaceWriteTool::new(ws(store, CompanyId::new("acme")));
    let result = tool
        .execute(json!({
            "path": "standards",
            "content": "x",
            "expected_updated_at": 1_000,
        }))
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(text(&result).contains("is a folder"));
}

/// The truncate-then-overwrite data-loss path: a note too large to read in
/// full must not be overwritable from the partial view the agent saw.
#[tokio::test]
async fn an_oversized_note_is_read_truncated_and_refused_for_writing() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let id = CompanyId::new("acme");
    let big = "x".repeat(MAX_CONTENT_BYTES + 4_096);
    store
        .create(&id, &file("n-big", "big.md", None), Some(&big))
        .await
        .unwrap();

    let read = WorkspaceReadTool::new(ws(store.clone(), id.clone()));
    let out = text(&read.execute(json!({"path": "big.md"})).await.unwrap());
    assert!(out.contains("bytes truncated"), "{out}");
    assert!(out.contains("CANNOT be overwritten"), "{out}");

    let rev = store
        .read(&id, "n-big")
        .await
        .unwrap()
        .unwrap()
        .0
        .updated_at_millis;
    let write = WorkspaceWriteTool::new(ws(store.clone(), id.clone()));
    let result = write
        .execute(json!({
            "path": "big.md",
            "content": "truncated copy",
            "expected_updated_at": rev,
        }))
        .await
        .unwrap();
    assert!(result.is_error, "{}", text(&result));
    assert!(text(&result).contains("larger than"), "{}", text(&result));

    let (_, body) = store.read(&id, "n-big").await.unwrap().unwrap();
    assert_eq!(body.len(), big.len(), "the oversized note was clobbered");
}

/// How a [`FixedTree`] answers the body read, when a test asks it to fail.
#[derive(Clone, Copy)]
pub(super) enum ReadFault {
    /// The node was there in the tree and is gone by the time the body read
    /// runs — raced with an operator delete.
    Vanished,
    /// The store itself failed. A factory rather than a value because
    /// [`crate::error::OpenCompanyError`] is not `Clone` (it carries a
    /// `std::io::Error`).
    Failed(fn() -> crate::error::OpenCompanyError),
}

/// A store that answers `tree()` from a fixed node list, and can be told to
/// fail either of the two calls a read makes.
///
/// The listing bounds have to be exercised against a tree big enough to hit
/// them and containing nodes no real backend will create for us — a
/// dangling parent, to raise `unaddressable`. `FsOps` refuses both, so the
/// only way to reach that rendering is to hand the index the tree directly.
///
/// Issue #887 added the faults for the same reason one level along: a
/// store-level I/O failure, and a node that vanishes between the tree read
/// and the body read, are exactly what a healthy filesystem will not do on
/// request — and they are two of `workspace_read`'s five failure exits.
pub(super) struct FixedTree {
    nodes: Vec<WorkspaceNode>,
    tree_fault: Option<fn() -> crate::error::OpenCompanyError>,
    read_fault: Option<ReadFault>,
}

impl FixedTree {
    pub(super) fn new(nodes: Vec<WorkspaceNode>) -> Self {
        Self {
            nodes,
            tree_fault: None,
            read_fault: None,
        }
    }

    /// `tree()` — and therefore every tool's index read — fails.
    pub(super) fn failing_tree(
        nodes: Vec<WorkspaceNode>,
        fault: fn() -> crate::error::OpenCompanyError,
    ) -> Self {
        Self {
            tree_fault: Some(fault),
            ..Self::new(nodes)
        }
    }

    /// The tree resolves normally; the body read is the one that fails.
    pub(super) fn failing_read(nodes: Vec<WorkspaceNode>, fault: ReadFault) -> Self {
        Self {
            read_fault: Some(fault),
            ..Self::new(nodes)
        }
    }
}

#[async_trait]
impl WorkspaceStore for FixedTree {
    async fn tree(&self, _company: &CompanyId) -> crate::Result<Vec<WorkspaceNode>> {
        match self.tree_fault {
            Some(make) => Err(make()),
            None => Ok(self.nodes.clone()),
        }
    }
    async fn read(
        &self,
        _company: &CompanyId,
        _id: &str,
    ) -> crate::Result<Option<(WorkspaceNode, String)>> {
        match self.read_fault {
            None => unreachable!("the listing never reads a body"),
            Some(ReadFault::Vanished) => Ok(None),
            Some(ReadFault::Failed(make)) => Err(make()),
        }
    }
    async fn read_capped(
        &self,
        company: &CompanyId,
        id: &str,
        max_bytes: u64,
    ) -> crate::Result<Option<(WorkspaceNode, String, u64)>> {
        crate::ports::workspace::read_capped_by_reading(self, company, id, max_bytes).await
    }
    async fn write_with_revision(
        &self,
        _company: &CompanyId,
        _id: &str,
        _content: &str,
        _author: WorkspaceOrigin,
        _expected_updated_at: Option<u64>,
    ) -> crate::Result<WorkspaceNode> {
        unreachable!("the listing never writes")
    }
    async fn create(
        &self,
        _company: &CompanyId,
        _node: &WorkspaceNode,
        _content: Option<&str>,
    ) -> crate::Result<()> {
        unreachable!("the listing never creates")
    }
    async fn adopt_or_create_folder(
        &self,
        _company: &CompanyId,
        _parent: Option<&str>,
        _name: &str,
        _origin: WorkspaceOrigin,
    ) -> crate::Result<crate::ports::workspace::FolderClaim> {
        unreachable!("the listing never claims a folder")
    }
    async fn rename_move(
        &self,
        _company: &CompanyId,
        _id: &str,
        _name: Option<&str>,
        _parent_id: Option<Option<&str>>,
    ) -> crate::Result<WorkspaceNode> {
        unreachable!("the listing never renames")
    }
    async fn swap_files(
        &self,
        _company: &CompanyId,
        _expected_id: Option<&str>,
        _replacement_id: &str,
        _name: &str,
    ) -> crate::Result<Option<WorkspaceNode>> {
        unreachable!("the listing never swaps files")
    }
    async fn delete(&self, _company: &CompanyId, _id: &str) -> crate::Result<bool> {
        unreachable!("the listing never deletes")
    }
    async fn create_binary(
        &self,
        _company: &CompanyId,
        _node: &WorkspaceNode,
        _bytes: &[u8],
    ) -> crate::Result<WorkspaceNode> {
        unreachable!("the listing never creates")
    }
    async fn write_binary(
        &self,
        _company: &CompanyId,
        _id: &str,
        _bytes: &[u8],
        _mime: Option<&str>,
        _author: WorkspaceOrigin,
    ) -> crate::Result<WorkspaceNode> {
        unreachable!("the listing never writes")
    }
    async fn read_bytes(
        &self,
        _company: &CompanyId,
        _id: &str,
    ) -> crate::Result<Option<(WorkspaceNode, crate::ports::workspace::BlobStream)>> {
        unreachable!("the listing never reads a payload")
    }
    async fn is_empty(&self, _company: &CompanyId) -> crate::Result<bool> {
        Ok(self.nodes.is_empty())
    }
}

/// Issue #417's second head: the listing's own guidance was unreachable.
///
/// `MAX_LIST_ENTRIES` is 300 but an entry renders at ~90-105 bytes, so the
/// harness budget bit at roughly 176 — below the count bound, which means
/// the "… more entries not shown, narrow with `prefix`" marker was never
/// even generated, and the `unaddressable` notice below it was cut away
/// too. Both sat at the end of the body, which is the end an outer cut
/// takes first.
///
/// So the listing must stop on bytes, and both trailers must move above the
/// entries where no cut can reach them.
/// One pathological name must not hide the entries behind it.
///
/// A node name is operator-supplied and no backend length-caps it, so a
/// single deep path can render a line larger than the whole byte budget.
/// Unbounded, that line fails the budget check on the loop's first
/// iteration and `break`s — reporting `0 of N` for a workspace that is
/// almost entirely listable. Bounding the echoed path keeps every line
/// small enough that only the genuine tail is ever lost.
#[tokio::test]
async fn one_oversized_path_does_not_hide_the_entries_behind_it() {
    let deep = "d".repeat(MAX_ECHOED_PATH_BYTES * 4);
    let mut nodes = vec![file("n-deep", &deep, None)];
    for n in 0..12 {
        nodes.push(file(
            &format!("n-after-{n:02}"),
            &format!("after-{n:02}.md"),
            None,
        ));
    }

    let store: Arc<dyn WorkspaceStore> = Arc::new(FixedTree::new(nodes));
    let list = WorkspaceListTool::new(ws(store, CompanyId::new("acme")));
    let out = text(&list.execute(json!({})).await.unwrap());

    // Every entry survives — the oversized one is clamped, not fatal.
    let shown: usize = out.matches("\tid=").count();
    assert_eq!(
        shown,
        13,
        "one long name truncated the listing to {shown} of 13 entries: {}",
        &out[..out.len().min(400)]
    );

    // The clamp announces itself rather than presenting a shortened path
    // as if it were the whole thing.
    assert!(
        out.contains("… (+"),
        "the oversized path was shortened without saying so: {}",
        &out[..out.len().min(400)]
    );

    // The id is the addressable handle and is never clamped, so a bounded
    // entry is still usable.
    assert!(
        out.contains("id=n-deep"),
        "the clamped entry lost its id, so nothing can address it: {}",
        &out[..out.len().min(400)]
    );

    assert!(
        out.len() <= TOOL_RESULT_BUDGET_BYTES,
        "the listing rendered {} bytes, over the {TOOL_RESULT_BUDGET_BYTES}-byte budget",
        out.len(),
    );
}

#[tokio::test]
async fn a_long_listing_fits_the_budget_and_carries_its_guidance_in_the_header() {
    let mut nodes = vec![folder("f-standards", "standards", None)];
    for n in 0..MAX_LIST_ENTRIES {
        nodes.push(file(
            &format!("node-{n:04}-0000000000"),
            &format!("engineering-standards-v{n:03}.md"),
            Some("f-standards"),
        ));
    }
    // Two nodes whose ancestor chain dangles, so `unaddressable` is set.
    nodes.push(file("n-orphan-a", "orphan-a.md", Some("gone")));
    nodes.push(file("n-orphan-b", "orphan-b.md", Some("gone")));

    let store: Arc<dyn WorkspaceStore> = Arc::new(FixedTree::new(nodes));
    let list = WorkspaceListTool::new(ws(store, CompanyId::new("acme")));
    let out = text(&list.execute(json!({})).await.unwrap());

    // The whole listing reaches the model, so nothing below is cut off.
    assert!(
        out.len() <= TOOL_RESULT_BUDGET_BYTES,
        "the listing rendered {} bytes, over the {TOOL_RESULT_BUDGET_BYTES}-byte harness \
         budget — the outer cut would fire and take the last entries with it",
        out.len(),
    );

    // The byte bound is what stopped it, not the count bound: this tree has
    // 301 addressable entries and fewer are shown. If only the count bound
    // existed the marker below would never be generated at all.
    let shown: usize = out.matches("\tid=").count();
    assert!(
        shown > 0 && shown < MAX_LIST_ENTRIES,
        "expected a partial listing, got {shown} of {MAX_LIST_ENTRIES}"
    );
    assert!(
        out.contains(&format!("{shown} of {} entries", MAX_LIST_ENTRIES + 1)),
        "the header must count honestly: {}",
        &out[..out.len().min(400)]
    );

    // Everything the model has to act on precedes the first entry line, so
    // truncating the tail can never remove it.
    let first_entry = out.find("\tid=").expect("entries were rendered");
    let head = &out[..first_entry];
    assert!(
        head.contains("Narrow the listing with the `prefix` parameter"),
        "the narrowing guidance is not in the header: {head}"
    );
    assert!(
        head.contains("node(s) have no valid path and were omitted entirely"),
        "the unaddressable notice is not in the header: {head}"
    );
    assert!(
        head.contains("2 node(s)"),
        "the unaddressable count is wrong: {head}"
    );
}

/// The nonce off a read's BEGIN fence, so a test can demand the *matching*
/// END fence rather than any occurrence of the words.
fn fence_of(out: &str) -> String {
    let at = out
        .find("--- BEGIN WORKSPACE NOTE ")
        .expect("the read is fenced");
    out[at + "--- BEGIN WORKSPACE NOTE ".len()..]
        .split_whitespace()
        .next()
        .expect("the fence carries a nonce")
        .to_string()
}

/// Issue #417, the data-loss window itself.
///
/// A 20 KiB note sat between the module's old 64 KiB read cap and the
/// harness's 16 KiB budget. The module saw `dropped == 0`, emitted the
/// write-eligible branch — "call `workspace_write` … with the complete new
/// body" — and the harness then handed the model ~16 KiB of the note. An
/// agent doing exactly as instructed wrote back what it had seen, the
/// 64 KiB write gate accepted it, and the rest of the operator's note was
/// destroyed with nothing reporting a loss.
///
/// Two things have to hold for that to be closed, and neither implies the
/// other: the invitation must be absent (so a compliant agent is never told
/// to send a whole body it does not have), and the result must fit under
/// the harness budget (so the module's view and the model's view are the
/// same bytes, closing fence included).
#[tokio::test]
async fn a_note_the_harness_would_have_cut_is_read_only_and_never_invites_a_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let id = CompanyId::new("acme");
    let body = "x".repeat(20 * 1024);
    store
        .create(&id, &file("n-big", "big.md", None), Some(&body))
        .await
        .unwrap();

    let read = WorkspaceReadTool::new(ws(store, id));
    let out = text(&read.execute(json!({"path": "big.md"})).await.unwrap());

    // The agent is told it may not write, and is never handed the sentence
    // that caused the overwrite.
    assert!(out.contains("CANNOT be overwritten"), "{out}");
    assert!(
        !out.contains("complete new body"),
        "a partial read still invited a full-body overwrite: {out}"
    );

    // The whole result survives the harness, so the model sees the same
    // bytes this module believes it returned — terminator included.
    assert!(
        out.len() <= TOOL_RESULT_BUDGET_BYTES,
        "a read of a {} byte note rendered {} bytes, over the {TOOL_RESULT_BUDGET_BYTES}-byte \
         harness budget — the outer cut would fire and take the end with it",
        body.len(),
        out.len(),
    );
    let nonce = fence_of(&out);
    assert!(
        out.trim_end()
            .ends_with(&format!("--- END WORKSPACE NOTE {nonce} ---")),
        "the closing fence is not the last thing in the result: {out}"
    );

    // And the first line says how much of it arrived, rather than leaving
    // that to a marker at the very end.
    assert!(
        out.contains(&format!(
            "returned {MAX_CONTENT_BYTES} of {} bytes",
            body.len()
        )),
        "the header does not state what was returned: {out}"
    );
}

/// The worst case the reservation has to cover: a body at exactly the cap,
/// so nothing is dropped and the *whole* framing is emitted — write-
/// eligibility line, fence preamble, both markers — around a path long
/// enough to need clamping.
///
/// This is the case [`READ_OVERHEAD_BYTES`] exists for. If the reservation
/// were removed (or the cap raised to the budget), a full read would land
/// over the budget and the harness would shave the closing fence off the
/// end of the very reads the module says nothing was dropped from.
#[tokio::test]
async fn a_full_read_at_the_cap_still_fits_under_the_harness_budget() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let id = CompanyId::new("acme");
    // A path far longer than anything the console produces, to prove the
    // reservation covers the header and not just the body.
    let outer = "L".repeat(200);
    let inner = "M".repeat(200);
    let leaf = format!("{}.md", "N".repeat(200));
    store
        .create(&id, &folder("f-outer", &outer, None), None)
        .await
        .unwrap();
    store
        .create(&id, &folder("f-inner", &inner, Some("f-outer")), None)
        .await
        .unwrap();
    let body = "z".repeat(MAX_CONTENT_BYTES);
    store
        .create(&id, &file("n-max", &leaf, Some("f-inner")), Some(&body))
        .await
        .unwrap();

    let read = WorkspaceReadTool::new(ws(store, id));
    let out = text(&read.execute(json!({"id": "n-max"})).await.unwrap());

    // Nothing was dropped, so this is the write-eligible branch — the one
    // whose promise has to be true.
    assert!(out.contains("complete new body"), "{out}");
    assert!(
        out.contains(&format!("{MAX_CONTENT_BYTES} bytes")),
        "the header should report the note's full size: {out}"
    );
    assert!(
        out.len() <= TOOL_RESULT_BUDGET_BYTES,
        "a full read at the cap rendered {} bytes, over the \
         {TOOL_RESULT_BUDGET_BYTES}-byte harness budget: the framing needs more than the \
         {READ_OVERHEAD_BYTES} bytes reserved for it",
        out.len(),
    );
    let nonce = fence_of(&out);
    assert!(
        out.trim_end()
            .ends_with(&format!("--- END WORKSPACE NOTE {nonce} ---")),
        "the closing fence is not the last thing in the result: {out}"
    );
}

/// The write gate at its boundary: one byte over the read cap is refused.
///
/// The existing oversized test uses cap + 4 KiB, which passes even if the
/// gate is off by kilobytes. This pins the gate to the same number the read
/// clamps at, which is the whole point of deriving both from one constant.
#[tokio::test]
async fn a_write_is_refused_on_a_note_one_byte_over_the_read_cap() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let id = CompanyId::new("acme");
    let body = "x".repeat(MAX_CONTENT_BYTES + 1);
    store
        .create(&id, &file("n-edge", "edge.md", None), Some(&body))
        .await
        .unwrap();
    let rev = store
        .read(&id, "n-edge")
        .await
        .unwrap()
        .unwrap()
        .0
        .updated_at_millis;

    let write = WorkspaceWriteTool::new(ws(store.clone(), id.clone()));
    let result = write
        .execute(json!({
            "path": "edge.md",
            "content": "what the agent saw",
            "expected_updated_at": rev,
        }))
        .await
        .unwrap();
    assert!(result.is_error, "{}", text(&result));
    assert!(text(&result).contains("larger than"), "{}", text(&result));

    let (_, after) = store.read(&id, "n-edge").await.unwrap().unwrap();
    assert_eq!(after.len(), body.len(), "the note was clobbered");

    // Not vacuous in the other direction: at exactly the cap the same write
    // is allowed, so the refusal above is the boundary and not a blanket.
    let ok_body = "x".repeat(MAX_CONTENT_BYTES);
    store
        .create(&id, &file("n-ok", "ok.md", None), Some(&ok_body))
        .await
        .unwrap();
    let rev = store
        .read(&id, "n-ok")
        .await
        .unwrap()
        .unwrap()
        .0
        .updated_at_millis;
    let result = write
        .execute(json!({
            "path": "ok.md",
            "content": "a complete rewrite",
            "expected_updated_at": rev,
        }))
        .await
        .unwrap();
    assert!(!result.is_error, "{}", text(&result));
}

#[tokio::test]
async fn an_oversized_new_body_is_refused() {
    let (_dir, store) = seeded("acme").await;
    let tool = WorkspaceWriteTool::new(ws(store, CompanyId::new("acme")));
    let result = tool
        .execute(json!({
            "id": "n-eng",
            "content": "y".repeat(MAX_WRITE_BYTES + 1),
            "expected_updated_at": 2_000,
        }))
        .await
        .unwrap();
    assert!(result.is_error);
    assert!(text(&result).contains("over the"));
}
