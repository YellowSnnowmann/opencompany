use super::*;

#[test]
fn a_parked_effect_tells_the_client_how_to_resolve_it() {
    let note = parked_notification("sess-1", "appr-7", "Send mail to 4 people");
    assert_eq!(note["method"], "session/update");
    // A notification: an `id` would have a conforming client try to reply.
    assert!(note.get("id").is_none());

    let approval = &note["params"]["update"]["_meta"]["opencompany/approval"];
    assert_eq!(approval["id"], "appr-7");
    assert_eq!(approval["summary"], "Send mail to 4 people");
    // A third-party client has no reason to know this host's REST shape, so
    // it is told rather than assumed.
    assert!(
        approval["resolve"]
            .as_str()
            .unwrap()
            .contains("/approvals/")
    );
}
