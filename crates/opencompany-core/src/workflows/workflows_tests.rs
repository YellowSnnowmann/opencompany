use super::*;

/// The tinyflows engine is linked and its API answers — the P0 link proof.
#[test]
fn tinyflows_engine_is_linked() {
    assert_eq!(tinyflows_engine_name(), "tinyflows");
}
