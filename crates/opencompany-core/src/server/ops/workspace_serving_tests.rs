use super::serving_for;

/// The classification table, stated once so the arms cannot drift apart.
#[test]
fn a_stored_mime_maps_to_exactly_one_serving() {
    let cases: &[(Option<&str>, &str, &str)] = &[
        // Renderable: the type survives and the browser shows it.
        (Some("image/png"), "image/png", "inline"),
        (Some("image/jpeg"), "image/jpeg", "inline"),
        (Some("image/gif"), "image/gif", "inline"),
        (Some("image/webp"), "image/webp", "inline"),
        (Some("application/pdf"), "application/pdf", "inline"),
        // Previewable but never a document.
        (Some("image/svg+xml"), "image/svg+xml", "attachment"),
        // Everything else is opaque bytes, whatever the caller called it.
        (Some("text/html"), "application/octet-stream", "attachment"),
        (
            Some("application/xhtml+xml"),
            "application/octet-stream",
            "attachment",
        ),
        (Some("text/plain"), "application/octet-stream", "attachment"),
        (
            Some("application/zip"),
            "application/octet-stream",
            "attachment",
        ),
        (None, "application/octet-stream", "attachment"),
    ];
    for (stored, content_type, disposition) in cases {
        let serving = serving_for(*stored);
        assert_eq!(
            (serving.content_type.as_str(), serving.disposition),
            (*content_type, *disposition),
            "stored mime {stored:?}"
        );
    }
}

/// A mime reaches this function from more than one writer, and only the
/// upload route normalises before storing — `capture_body` stores whatever
/// `mime_guess` produced, and a payload written straight through the port
/// stores whatever its caller passed. So the essence is matched here too,
/// or a parameterised or upper-cased `image/png` would be downgraded to a
/// download and the console's preview would break for it.
#[test]
fn the_essence_is_matched_not_the_raw_header_value() {
    for stored in [
        "image/png; charset=binary",
        "  image/png  ",
        "IMAGE/PNG",
        "Image/Png ;q=1",
    ] {
        let serving = serving_for(Some(stored));
        assert_eq!(serving.content_type, "image/png", "stored mime {stored:?}");
        assert_eq!(serving.disposition, "inline", "stored mime {stored:?}");
    }
}

/// The list is closed, not a blocklist: a type nobody has considered is
/// downloaded rather than rendered.
#[test]
fn an_unknown_type_falls_to_the_safe_arm() {
    let serving = serving_for(Some("application/x-invented-2031"));
    assert_eq!(serving.content_type, "application/octet-stream");
    assert_eq!(serving.disposition, "attachment");
}
