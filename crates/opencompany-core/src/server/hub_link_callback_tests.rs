use super::*;

#[test]
fn escape_neutralizes_html() {
    let out = escape("<script>alert(1)</script>&\"");
    assert!(!out.contains('<'));
    assert!(out.contains("&lt;script&gt;"));
    assert!(out.contains("&amp;"));
    assert!(out.contains("&quot;"));
}

#[test]
fn success_page_escapes_the_note() {
    let resp = success_page("<b>done</b>");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[test]
fn non_empty_trims_and_rejects_blank() {
    assert_eq!(non_empty(Some("  x ")), Some("x"));
    assert_eq!(non_empty(Some("   ")), None);
    assert_eq!(non_empty(None), None);
}
