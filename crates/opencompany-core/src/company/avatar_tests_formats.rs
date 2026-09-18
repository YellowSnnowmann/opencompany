//! Decoded-size validation tests: per-format dimension reading, the
//! decompression-bomb and aspect-ratio guards, and the animation-cost
//! caps (split out of `avatar_tests.rs`).

use super::*;

// ——— decoded-size validation ——————————————————————————————

/// A PNG whose header announces the given size — the signature and IHDR
/// that carry width and height, plus the IHDR fields that follow them.
pub(super) fn png(w: u32, h: u32) -> Vec<u8> {
    let mut v = PNG_SIGNATURE.to_vec();
    v.extend_from_slice(&13u32.to_be_bytes());
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&[8, 6, 0, 0, 0]);
    v
}

/// A GIF whose logical screen announces the given size.
pub(super) fn gif(w: u16, h: u16) -> Vec<u8> {
    let mut v = GIF_SIGNATURE_89.to_vec();
    v.extend_from_slice(&w.to_le_bytes());
    v.extend_from_slice(&h.to_le_bytes());
    v
}

/// A GIF with the given logical screen and one Image Descriptor per entry
/// in `frames` — enough of the block stream for the frame walker to count
/// decoded pixels, with no color tables and empty raster data. The LZW
/// bytes are never decoded by the check being exercised, so empty sub-block
/// data is exactly what the parse needs.
fn gif_animated(logical: (u16, u16), frames: &[(u16, u16)]) -> Vec<u8> {
    let mut v = GIF_SIGNATURE_89.to_vec();
    v.extend_from_slice(&logical.0.to_le_bytes());
    v.extend_from_slice(&logical.1.to_le_bytes());
    // No global color table: flags 0, background 0, aspect 0.
    v.extend_from_slice(&[0x00, 0x00, 0x00]);
    for &(w, h) in frames {
        v.push(0x2C);
        v.extend_from_slice(&[0x00, 0x00]); // left
        v.extend_from_slice(&[0x00, 0x00]); // top
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.push(0x00); // no local color table
        v.push(0x02); // LZW minimum code size
        v.push(0x00); // zero-length raster data sub-block (the terminator)
    }
    v.push(0x3B); // trailer
    v
}

/// A minimal JPEG whose SOF0 announces the given size, preceded by an APP0
/// segment so the size is found by walking the marker list, not assumed at
/// an offset.
fn jpeg(w: u16, h: u16) -> Vec<u8> {
    let mut v = b"\xff\xd8".to_vec();
    // APP0 (JFIF), length 16, then a 14-byte payload.
    v.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
    v.extend_from_slice(b"JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00");
    // SOF0, length 16: precision(1) + h(2) + w(2) + 3 components × 3.
    v.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x10, 0x08]);
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&[0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    v
}

/// A WebP whose VP8X canvas chunk announces the given size.
fn webp_vp8x(w: u32, h: u32) -> Vec<u8> {
    let (wm1, hm1) = (w - 1, h - 1);
    let mut v = b"RIFF".to_vec();
    v.extend_from_slice(&22u32.to_le_bytes());
    v.extend_from_slice(b"WEBPVP8X");
    v.extend_from_slice(&10u32.to_le_bytes());
    v.extend_from_slice(&[
        0x00,
        0x00,
        0x00,
        0x00, //
        (wm1 & 0xFF) as u8,
        ((wm1 >> 8) & 0xFF) as u8,
        ((wm1 >> 16) & 0xFF) as u8,
        (hm1 & 0xFF) as u8,
        ((hm1 >> 8) & 0xFF) as u8,
        ((hm1 >> 16) & 0xFF) as u8,
    ]);
    v
}

/// A WebP whose VP8 (lossy) chunk announces the given size.
fn webp_vp8(w: u16, h: u16) -> Vec<u8> {
    let mut v = b"RIFF".to_vec();
    // 12 (container) + 8 (chunk header) + 10 (frame) = 30.
    v.extend_from_slice(&30u32.to_le_bytes());
    v.extend_from_slice(b"WEBPVP8 ");
    v.extend_from_slice(&10u32.to_le_bytes());
    // Frame tag + start code (RFC 6386), then width and height each as
    // their own little-endian 16-bit field, scale bits clear.
    v.extend_from_slice(&[0x9D, 0x01, 0x2A, 0x9D, 0x01, 0x2A]);
    v.push((w & 0xFF) as u8);
    v.push(((w >> 8) & 0x3F) as u8);
    v.push((h & 0xFF) as u8);
    v.push(((h >> 8) & 0x3F) as u8);
    v
}

/// A WebP whose VP8L (lossless) chunk announces the given size, with no
/// alpha (alpha_is_used = 0, version = 0).
///
/// Setting `alpha` toggles the alpha_is_used hint, so a regression test can
/// verify that the alpha and version bits are excluded from the height.
fn webp_vp8l(w: u32, h: u32, alpha: bool) -> Vec<u8> {
    let (wm1, hm1) = (w - 1, h - 1);
    let mut v = b"RIFF".to_vec();
    // 12 (container) + 8 (chunk header) + 5 (header) = 25.
    v.extend_from_slice(&25u32.to_le_bytes());
    v.extend_from_slice(b"WEBPVP8L");
    v.extend_from_slice(&5u32.to_le_bytes());
    // Signature (0x2F) + 14-bit (w−1) + 14-bit (h−1) + alpha + version(3).
    let payload = wm1 | (hm1 << 14) | ((alpha as u32) << 28);
    v.push(0x2F);
    v.extend_from_slice(&payload.to_le_bytes()[..4]);
    v
}

/// An animated WebP with the given VP8X canvas and one ANMF frame per entry
/// in `frames` — enough of the chunk stream for the frame walker to count
/// decoded pixels. Each ANMF carries its own `(width−1, height−1)` at the
/// fixed 24-bit offsets the walker reads, and a throwaway VP8 sub-chunk the
/// walker never looks inside.
fn webp_animated(canvas: (u32, u32), frames: &[(u32, u32)]) -> Vec<u8> {
    let (cw, ch) = canvas;
    let mut v = b"RIFF".to_vec();
    v.extend_from_slice(&0u32.to_le_bytes()); // size, fixed below
    v.extend_from_slice(b"WEBP");
    // VP8X canvas with the animation flag (bit 1) set.
    v.extend_from_slice(b"VP8X");
    v.extend_from_slice(&10u32.to_le_bytes());
    v.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]);
    v.extend_from_slice(&(cw - 1).to_le_bytes()[..3]);
    v.extend_from_slice(&(ch - 1).to_le_bytes()[..3]);
    // ANIM chunk: background(3) + loop count(2).
    v.extend_from_slice(b"ANIM");
    v.extend_from_slice(&6u32.to_le_bytes());
    v.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    for &(fw, fh) in frames {
        let (fw1, fh1) = (fw - 1, fh - 1);
        // Frame header X(3) Y(3) W−1(3) H−1(3) duration(3) flags(1) — 16
        // bytes — then the frame's own VP8 sub-chunk (8 + 10).
        v.extend_from_slice(b"ANMF");
        v.extend_from_slice(&34u32.to_le_bytes());
        v.extend_from_slice(&[0x00, 0x00, 0x00]); // X
        v.extend_from_slice(&[0x00, 0x00, 0x00]); // Y
        v.extend_from_slice(&fw1.to_le_bytes()[..3]);
        v.extend_from_slice(&fh1.to_le_bytes()[..3]);
        v.extend_from_slice(&[0x0A, 0x00, 0x00]); // 10 ms
        v.push(0x00);
        v.extend_from_slice(b"VP8 ");
        v.extend_from_slice(&10u32.to_le_bytes());
        v.extend_from_slice(&[0x9D, 0x01, 0x2A, 0x9D, 0x01, 0x2A]);
        v.push((fw1 & 0xFF) as u8);
        v.push(((fw1 >> 8) & 0x3F) as u8);
        v.push((fh1 & 0xFF) as u8);
        v.push(((fh1 >> 8) & 0x3F) as u8);
    }
    let riff_size = (v.len() - 8) as u32;
    v[4..8].copy_from_slice(&riff_size.to_le_bytes());
    v
}

/// An APNG with the given IHDR canvas and one fcTL frame per entry in
/// `frames` — enough of the chunk stream for the frame walker to count
/// decoded pixels, with CRCs the walker ignores.
fn apng_animated(canvas: (u32, u32), frames: &[(u32, u32)]) -> Vec<u8> {
    let mut v = PNG_SIGNATURE.to_vec();
    // IHDR: the canvas.
    v.extend_from_slice(&13u32.to_be_bytes());
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&canvas.0.to_be_bytes());
    v.extend_from_slice(&canvas.1.to_be_bytes());
    v.extend_from_slice(&[8, 6, 0, 0, 0]);
    v.extend_from_slice(&[0, 0, 0, 0]); // CRC (ignored)
    // acTL: frame count + plays.
    v.extend_from_slice(&8u32.to_be_bytes());
    v.extend_from_slice(b"acTL");
    v.extend_from_slice(&(frames.len() as u32).to_be_bytes());
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(&[0, 0, 0, 0]); // CRC
    // One fcTL per frame: sequence, width, height, offsets, delays, ops.
    for (seq, &(fw, fh)) in frames.iter().enumerate() {
        v.extend_from_slice(&26u32.to_be_bytes());
        v.extend_from_slice(b"fcTL");
        v.extend_from_slice(&(seq as u32).to_be_bytes());
        v.extend_from_slice(&fw.to_be_bytes());
        v.extend_from_slice(&fh.to_be_bytes());
        v.extend_from_slice(&0u32.to_be_bytes()); // x offset
        v.extend_from_slice(&0u32.to_be_bytes()); // y offset
        v.extend_from_slice(&[0, 0, 0, 0]); // delay_num, delay_den
        v.extend_from_slice(&[0, 0]); // dispose_op, blend_op
        v.extend_from_slice(&[0, 0, 0, 0]); // CRC
    }
    // An IDAT so the file reads as a complete PNG; the walker never reaches it.
    v.extend_from_slice(&0u32.to_be_bytes());
    v.extend_from_slice(b"IDAT");
    v.extend_from_slice(&[0, 0, 0, 0]); // CRC
    v
}

#[test]
fn reads_the_size_each_format_announces() {
    assert_eq!(image_dimensions(&png(1, 1)).unwrap(), (1, 1));
    assert_eq!(image_dimensions(&png(192, 192)).unwrap(), (192, 192));
    assert_eq!(image_dimensions(&png(65535, 1)).unwrap(), (65535, 1));
    assert_eq!(image_dimensions(&gif(640, 480)).unwrap(), (640, 480));
    assert_eq!(image_dimensions(&jpeg(320, 240)).unwrap(), (320, 240));
    // A real mascot shape: a 192×192 VP8X canvas with an animated-style
    // VP8 frame following it (the canvas is what a decoder allocates).
    let mut extended = webp_vp8x(192, 192);
    extended.extend_from_slice(b"ANIM");
    extended.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(image_dimensions(&extended).unwrap(), (192, 192));
    assert_eq!(image_dimensions(&webp_vp8(192, 192)).unwrap(), (192, 192));
    assert_eq!(
        image_dimensions(&webp_vp8l(192, 192, false)).unwrap(),
        (192, 192)
    );
    assert_eq!(
        image_dimensions(&webp_vp8l(192, 192, true)).unwrap(),
        (192, 192)
    );
}

/// The VP8 height is a full 14 bits, not 10: the high six bits live in the
/// second size word's top bits (`data[9]`), and a parse that dropped them
/// measured a 4096×16383 frame as 4096×1023 — under the dimension cap, so
/// the decompression-bomb check let a 67-megapixel frame through.
#[test]
fn vp8_height_uses_all_fourteen_bits() {
    let (w, h) = (MAX_AVATAR_DIMENSION, 16383);
    let tall = webp_vp8(w as u16, h as u16);
    assert_eq!(image_dimensions(&tall).unwrap(), (w, h));
    assert!(
        check_image_dimensions(&tall).is_err(),
        "a 4096×16383 frame must be refused, not measured as 4096×1023"
    );
}

/// A real 1920×1080 lossy WebP: width and height each occupy their own
/// little-endian 16-bit field (RFC 6386 §9.1), the height bytes being
/// `0x38, 0x04`. A parse that packed the two together read that height as
/// 4320 and refused a perfectly ordinary landscape upload.
#[test]
fn vp8_height_is_its_own_two_byte_field() {
    let mut v = b"RIFF".to_vec();
    v.extend_from_slice(&30u32.to_le_bytes());
    v.extend_from_slice(b"WEBPVP8 ");
    v.extend_from_slice(&10u32.to_le_bytes());
    v.extend_from_slice(&[0x9D, 0x01, 0x2A, 0x9D, 0x01, 0x2A]);
    // w = 1920 (0x0780), h = 1080 (0x0438), both scale bits clear.
    v.extend_from_slice(&[0x80, 0x07, 0x38, 0x04]);
    assert_eq!(image_dimensions(&v).unwrap(), (1920, 1080));
    assert!(
        check_image_dimensions(&v).is_ok(),
        "a 1920×1080 landscape WebP must be accepted"
    );
}

/// The VP8L (lossless) height is 14 bits; a parse that fails to mask out
/// the alpha_is_used and version bits reads a 192×192 lossless image with
/// alpha as 192×16576 and refuses the upload.
#[test]
fn vp8l_height_masks_alpha_and_version_bits() {
    // The flag is a non-normative hint; a real lossless file may have it
    // set, and version must be 0 for valid files.
    assert_eq!(
        image_dimensions(&webp_vp8l(192, 192, true)).unwrap(),
        (192, 192)
    );
    assert!(
        check_image_dimensions(&webp_vp8l(192, 192, true)).is_ok(),
        "a 192×192 lossless VP8L with alpha_is_used=1 must be accepted"
    );
    // A small VP8L with all version bits set (version = 7) must still
    // decode to the correct size — the spec requires version=0 but the
    // dimension parser must not read those bits as height.
    let bad_version = b"RIFF\x19\x00\x00\x00WEBPVP8L\x05\x00\x00\x00\x2F\x00\x00\x00\xE0";
    assert_eq!(
        image_dimensions(bad_version).unwrap(),
        (1, 1),
        "version bits must not corrupt the height"
    );
}

#[test]
fn size_check_accepts_a_reasonable_image() {
    for ok in [
        png(192, 192),
        gif(4096, 4096),
        jpeg(4032, 3024),
        webp_vp8x(4096, 4096),
    ] {
        check_image_dimensions(&ok).expect("a normal image must pass");
    }
}

/// The decompression bomb the caps exist for: a header claiming a huge
/// frame in a payload small enough to pass the 4 MiB ceiling.
#[test]
fn size_check_refuses_a_decompression_bomb() {
    for bomb in [
        png(65535, 65535),
        png(MAX_AVATAR_DIMENSION + 1, 1),
        gif(65535, 65535),
        jpeg(65535, 65535),
        webp_vp8x(65535, 65535),
        webp_vp8(65535, 65535),
    ] {
        let err = check_image_dimensions(&bomb).unwrap_err().to_string();
        assert!(
            err.contains("pixels") && err.contains("avatar has to fit"),
            "a bomb must be refused by name: {err}"
        );
    }
}

/// Both caps work together: an extreme aspect ratio whose edges each fit
/// within the dimension cap is still refused by total area.
#[test]
fn size_check_refuses_an_extreme_aspect_ratio() {
    let wide = png(MAX_AVATAR_DIMENSION * 2, MAX_AVATAR_DIMENSION / 2);
    assert!(
        check_image_dimensions(&wide).is_err(),
        "edges within the dimension cap must still respect the area cap"
    );
}

/// The frame walker sums every Image Descriptor's area, not just the
/// logical screen's.
#[test]
fn gif_animation_cost_counts_every_frame() {
    assert_eq!(
        gif_animation_cost(&gif_animated((100, 100), &[(100, 100), (50, 50)])).unwrap(),
        Some(12_500)
    );
    // Frames may be sub-rectangles of the screen; each one is still paid for.
    assert_eq!(
        gif_animation_cost(&gif_animated((4096, 4096), &[(128, 128)])).unwrap(),
        Some(16_384)
    );
    // Not a GIF, and a GIF with no Image Descriptor: nothing to count.
    assert_eq!(gif_animation_cost(PNG_SIGNATURE).unwrap(), None);
    assert_eq!(
        gif_animation_cost(&gif_animated((16, 16), &[])).unwrap(),
        None
    );
}

/// A global color table is skipped only when the descriptor's flag says one
/// is present — a walker that always skipped the table it expected would
/// misread the first block after a table-less header, and one that never
/// skipped it would read the table's bytes as block kinds.
#[test]
fn gif_animation_cost_skips_a_global_color_table_when_one_is_declared() {
    // Header + packed flags with the GCT flag (0x80) and size 0 (two
    // entries), then the 2 × 3-byte table, then one 16×16 frame.
    let mut v = GIF_SIGNATURE_89.to_vec();
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(&[0x80, 0x00, 0x00]);
    v.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    v.push(0x2C);
    v.extend_from_slice(&[0, 0, 0, 0]);
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.push(0x00); // no local color table
    v.push(0x02); // LZW min code size
    v.push(0x00); // empty raster data
    v.push(0x3B); // trailer
    assert_eq!(gif_animation_cost(&v).unwrap(), Some(256));
}

/// A GIF can hide a flood of full-canvas frames under the byte ceiling: the
/// logical screen fits the dimension caps and a single `4096²` frame would
/// too, but ten of them repaint ten times the decoded pixels every cycle.
#[test]
fn size_check_refuses_a_gif_that_animates_beyond_the_cost_cap() {
    let busy = gif_animated((4096, 4096), &[(4096, 4096); 10]);
    let err = check_image_dimensions(&busy).unwrap_err().to_string();
    assert!(
        err.contains("animates") && err.contains("per cycle"),
        "an animation far over the decoded-pixel cap must be refused by name: {err}"
    );

    // The same form, kept human: a small face with plenty of frames.
    let calm = gif_animated((128, 128), &[(128, 128); 60]);
    check_image_dimensions(&calm).expect("60 frames at 128×128 must pass");
}

/// The animated-WebP walker sums every ANMF rectangle, not the canvas size.
#[test]
fn webp_animation_cost_counts_every_anmf_frame() {
    assert_eq!(
        webp_animation_cost(&webp_animated((100, 100), &[(100, 100), (50, 50)])).unwrap(),
        Some(12_500)
    );
    // Frames may be sub-rectangles of the canvas; each one is still paid for.
    assert_eq!(
        webp_animation_cost(&webp_animated((4096, 4096), &[(128, 128)])).unwrap(),
        Some(16_384)
    );
    // Not a WebP, and a WebP with no ANMF chunks: nothing to count.
    assert_eq!(webp_animation_cost(PNG_SIGNATURE).unwrap(), None);
    assert_eq!(webp_animation_cost(&webp_vp8x(16, 16)).unwrap(), None);
}

/// The APNG walker pays for the default image (the canvas, frame 0 of the
/// cycle) plus every fcTL rectangle.
#[test]
fn apng_animation_cost_counts_the_canvas_and_every_fctl_frame() {
    assert_eq!(
        apng_animation_cost(&apng_animated((100, 100), &[(100, 100), (50, 50)])).unwrap(),
        Some(22_500)
    );
    // A still PNG carries no acTL, so there is nothing animated to count.
    assert_eq!(apng_animation_cost(&png(16, 16)).unwrap(), None);
}

/// A valid-looking animation whose stream is truncated after frames have
/// begun must not fall back to the still-image cap. Browsers decode the
/// frames that are present, so accepting this would let a frame flood past
/// `MAX_AVATAR_ANIMATED_PIXELS` by omitting only a trailer or later chunk.
#[test]
fn size_check_refuses_truncated_animations() {
    let mut gif_bytes = gif_animated((4096, 4096), &[(4096, 4096); 9]);
    gif_bytes.pop(); // remove the trailer
    let gif_err = check_image_dimensions(&gif_bytes).unwrap_err().to_string();
    assert!(gif_err.contains("truncated animation"), "GIF: {gif_err}");

    let mut webp = webp_animated((4096, 4096), &[(4096, 4096); 9]);
    webp.truncate(webp.len() - 1); // cut off the final ANMF payload
    let webp_err = check_image_dimensions(&webp).unwrap_err().to_string();
    assert!(webp_err.contains("truncated animation"), "WebP: {webp_err}");

    let mut apng_bytes = apng_animated((4096, 4096), &[(4096, 4096); 9]);
    apng_bytes.truncate(apng_bytes.len() - 1); // cut off the final chunk
    let apng_err = check_image_dimensions(&apng_bytes).unwrap_err().to_string();
    assert!(apng_err.contains("truncated animation"), "APNG: {apng_err}");

    // A header-only GIF has never reached a frame, so it remains the
    // accepted still-image case used by the normal-size test above.
    check_image_dimensions(&gif(4096, 4096)).expect("header-only GIF is still");
}
/// An animated WebP can hide a flood of full-canvas frames under the byte
/// ceiling exactly like a GIF can; the per-cycle walk must refuse it too.
#[test]
fn size_check_refuses_an_animated_webp_beyond_the_cost_cap() {
    let busy = webp_animated((4096, 4096), &[(4096, 4096); 10]);
    let err = check_image_dimensions(&busy).unwrap_err().to_string();
    assert!(
        err.contains("animates") && err.contains("per cycle"),
        "an animated WebP far over the decoded-pixel cap must be refused by name: {err}"
    );

    let calm = webp_animated((128, 128), &[(128, 128); 60]);
    check_image_dimensions(&calm).expect("60 frames at 128×128 must pass");
}

/// The same flood through an APNG: the default image plus every fcTL frame
/// is the per-cycle cost, and it is bounded like the other two formats.
#[test]
fn size_check_refuses_an_animated_apng_beyond_the_cost_cap() {
    let busy = apng_animated((4096, 4096), &[(4096, 4096); 8]);
    let err = check_image_dimensions(&busy).unwrap_err().to_string();
    assert!(
        err.contains("animates") && err.contains("per cycle"),
        "an animated APNG far over the decoded-pixel cap must be refused by name: {err}"
    );

    let calm = apng_animated((128, 128), &[(128, 128); 60]);
    check_image_dimensions(&calm).expect("60 frames at 128×128 must pass");
}

/// A payload too short to announce a size is not an image: a truncated
/// avatar would not decode anywhere either.
#[test]
fn size_check_refuses_a_truncated_payload() {
    for truncated in [
        PNG_SIGNATURE,
        &b"GIF89a"[..],
        &b"\xff\xd8\xff\xe0\x00\x10"[..],
        &b"RIFF\x16\x00\x00\x00WEBPVP8X"[..],
    ] {
        assert!(
            check_image_dimensions(truncated).is_err(),
            "{:?}",
            &truncated[..truncated.len().min(16)]
        );
    }
}

/// A SOF segment whose declared length is too short to hold the size bytes
/// used to slip past the segment-end check and then read past the buffer
/// when the fixed height/width indexes were applied. It must be refused,
/// not panic the request task.
#[test]
fn size_check_refuses_an_undersized_sof() {
    let undersized = b"\xff\xd8\xff\xc0\x00\x02";
    assert!(image_dimensions(undersized).is_none());
    assert!(check_image_dimensions(undersized).is_err());
}
