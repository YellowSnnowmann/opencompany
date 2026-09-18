use super::*;

#[test]
fn reads_the_workflow_out_of_a_copilot_thread() {
    assert_eq!(
        workflow_of_thread(Some("workflow-copilot:weekly_report")),
        Some("weekly_report")
    );
    assert!(is_copilot_thread(Some("workflow-copilot:weekly_report")));
}

#[test]
fn an_ordinary_thread_is_not_a_copilot_thread() {
    for chat in [None, Some("general"), Some("engineering"), Some("chief")] {
        assert_eq!(workflow_of_thread(chat), None, "{chat:?}");
        assert!(!is_copilot_thread(chat), "{chat:?}");
    }
}

/// A desk cannot be spelled with a `:`, so nothing that merely *contains*
/// the marker counts: the prefix has to start the thread id. Otherwise a
/// desk named after the copilot would inherit a confinement meant for a
/// workflow that does not exist.
#[test]
fn the_prefix_must_start_the_thread_id() {
    assert_eq!(workflow_of_thread(Some("desk-workflow-copilot:x")), None);
    assert_eq!(workflow_of_thread(Some(" workflow-copilot:x")), None);
}

/// A prefix with nothing after it names no workflow. Confining a turn to
/// "" would produce a boundary whose subject is blank, which reads to the
/// operator as a copilot that has lost the workflow it was opened on.
#[test]
fn a_workflowless_prefix_is_not_a_copilot_thread() {
    assert_eq!(workflow_of_thread(Some("workflow-copilot:")), None);
    assert_eq!(workflow_of_thread(Some("workflow-copilot:   ")), None);
}

/// Ids are trimmed, so a thread built with stray whitespace still names the
/// workflow it meant rather than a near-miss id nothing resolves.
#[test]
fn ids_are_trimmed() {
    assert_eq!(
        workflow_of_thread(Some("workflow-copilot: weekly_report ")),
        Some("weekly_report")
    );
}
