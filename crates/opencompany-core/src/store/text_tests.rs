use super::*;

// The inverted range below is the point of the last assertion: `peek` takes
// its range from a caller, and a caller that computed one backwards must get
// an empty read rather than a panic in a tenant container.
#[expect(
    clippy::reversed_empty_ranges,
    reason = "the reversed range is the input under test"
)]
#[test]
fn ranges_widen_to_char_boundaries_instead_of_panicking() {
    // "é" is two bytes; a range that splits it would panic on a raw slice.
    let body = "aébc";
    assert_eq!(slice_on_char_boundaries(body, 0..2), "aé");
    assert_eq!(slice_on_char_boundaries(body, 1..2), "é");
    // Past the end clamps rather than panicking.
    assert_eq!(slice_on_char_boundaries(body, 0..999), body);
    // An inverted range yields nothing, not a panic.
    assert_eq!(slice_on_char_boundaries(body, 3..1), "");
    // A zero-length range asks for no bytes and gets none, even where the
    // outward widening would otherwise have grown it into a whole "é".
    assert_eq!(slice_on_char_boundaries(body, 2..2), "");
    assert_eq!(slice_on_char_boundaries(body, 0..0), "");
    assert_eq!(slice_on_char_boundaries(body, 99..99), "");
}

#[test]
fn boundaries_floor_down_and_ceil_up() {
    let body = "aébc";
    // Byte 2 is mid-"é": floor lands before it, ceil after it.
    assert_eq!(floor_boundary(body, 2), 1);
    assert_eq!(ceil_boundary(body, 2), 3);
    // A boundary stays where it is in both directions.
    assert_eq!(floor_boundary(body, 3), 3);
    assert_eq!(ceil_boundary(body, 3), 3);
}
