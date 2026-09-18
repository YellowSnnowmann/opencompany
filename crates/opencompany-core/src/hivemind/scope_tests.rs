use super::*;

#[test]
fn context_at_or_below_the_trigger_is_always_admitted() {
    let scope = EpisodeScope::new(EventSeq::new(10));
    assert!(scope.admits(EventSeq::new(1)));
    assert!(scope.admits(EventSeq::new(10)));
}

#[test]
fn a_row_above_the_trigger_is_refused_until_this_scope_records_it() {
    let scope = EpisodeScope::new(EventSeq::new(10));
    assert!(!scope.admits(EventSeq::new(11)));
    scope.record(EventSeq::new(11));
    assert!(scope.admits(EventSeq::new(11)));
}

#[test]
fn recording_one_sequence_does_not_admit_another() {
    let scope = EpisodeScope::new(EventSeq::new(10));
    scope.record(EventSeq::new(11));
    assert!(!scope.admits(EventSeq::new(12)));
}
