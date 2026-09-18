use super::*;

#[test]
fn base64_matches_the_standard_alphabet_and_padding() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    // The bytes that expose an alphabet typo — the last two entries and the
    // high bits — which ASCII test vectors never reach.
    assert_eq!(base64_encode(&[0xff, 0xef, 0xbe]), "/+++");
}

/// The signature check is what stops `~/.face` — which conventionally has no
/// extension — being reported under a type the host will refuse.
#[test]
fn only_the_four_accepted_formats_are_recognised() {
    assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
    assert_eq!(sniff(b"\xff\xd8\xffrest"), Some("image/jpeg"));
    assert_eq!(sniff(b"GIF89a"), Some("image/gif"));
    assert_eq!(sniff(b"RIFF\x20\x00\x00\x00WEBP"), Some("image/webp"));
    assert_eq!(sniff(b"<svg><script/></svg>"), None);
    assert_eq!(sniff(b"RIFF\x20\x00\x00\x00WAVE"), None);
    assert_eq!(sniff(b""), None);
}

/// A picture that is missing, oversized or not an image is `None` rather
/// than an error: a prefill that cannot happen is a form somebody fills in.
#[test]
fn an_unreadable_picture_is_simply_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(encode_picture(&dir.path().join("nothing")), None);
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, b"not an image").unwrap();
    assert_eq!(encode_picture(&text), None);
}

/// The happy path, end to end: bytes on disk become a data URL the console
/// can turn into a `File`.
#[test]
fn a_png_becomes_a_data_url() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".face");
    std::fs::write(&path, b"\x89PNG\r\n\x1a\nbody").unwrap();
    let url = encode_picture(&path).expect("a data url");
    assert!(url.starts_with("data:image/png;base64,"), "{url}");
}

/// Reading identity must never panic or block, whatever this machine is.
#[test]
fn reading_this_machine_answers_something() {
    let _ = device_identity();
}

/// The `JPEGPhoto:` label contains a hex digit — `E` — so the payload must
/// be the hex *after* the label. Filtering the whole response prepends that
/// digit to an even-length payload, making the count odd and the picture
/// absent on every macOS machine. This is the regression for that bug.
#[test]
fn jpegphoto_decodes_the_hex_after_the_label() {
    let payload = b"\xff\xd8\xff\xe0"; // a JPEG signature, four bytes
    let hex: String = payload.iter().map(|b| format!("{b:02X}")).collect();
    let out = format!("JPEGPhoto: {hex}");
    let url = decode_jpegphoto(&out).expect("a data url");
    assert_eq!(
        url,
        format!("data:image/jpeg;base64,{}", base64_encode(payload)),
        "{out}"
    );
}

/// Long values make `dscl` put the label on its own line and wrap the hex
/// beneath it — the same shape the `RealName` reader handles.
#[test]
fn jpegphoto_handles_a_wrapped_payload() {
    let out = "JPEGPhoto:\n  FFD8\n  FFE0\n";
    let url = decode_jpegphoto(out).expect("a data url");
    assert_eq!(url, "data:image/jpeg;base64,/9j/4A==");
}

/// A payload whose bytes are not one of the accepted images is absent, like
/// every other picture source on this file.
#[test]
fn jpegphoto_that_is_not_an_image_is_absent() {
    assert_eq!(decode_jpegphoto("JPEGPhoto: 4141"), None);
    assert_eq!(decode_jpegphoto("JPEGPhoto:"), None);
}

/// `whoami /user` is localized and its column layout varies; the SID is
/// found by shape — a `S-1-5-21-…` digit token — never by column or
/// position. This is the parse behind scoping the Windows picture lookup
/// to the current account rather than every SID on the machine.
#[test]
fn whoami_user_sid_is_found_by_shape() {
    assert_eq!(
        parse_whoami_user(
            "USER INFORMATION\n\
             ----------------\n\
             \n\
             User Name        SID\n\
             ================ =================================\n\
             workstation\\alice S-1-5-21-1004336348-1177238915-682003330-1001\n",
        ),
        Some("S-1-5-21-1004336348-1177238915-682003330-1001".to_string())
    );
    // Nothing SID-shaped means no suggestion — the correct answer when the
    // account cannot be identified.
    assert_eq!(parse_whoami_user("workstation\\alice"), None);
    assert_eq!(parse_whoami_user(""), None);
    // `S-` alone, or `S-123` with no second dash, is not a SID.
    assert_eq!(parse_whoami_user("S-"), None);
    assert_eq!(parse_whoami_user("S-123"), None);
}
