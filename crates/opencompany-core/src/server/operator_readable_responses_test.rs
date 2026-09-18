use super::readable_responses;
use crate::ports::types::OutboundMessage;

fn reply(text: &str) -> OutboundMessage {
    OutboundMessage {
        channel: "engineering".to_string(),
        agent: Some("software_engineer".to_string()),
        text: text.to_string(),
        steps: Vec::new(),
        reply_to: None,
        task_id: None,
        outputs: Vec::new(),
        message_id: None,
        mentions: Vec::new(),
    }
}

/// **A row must read the same live as it does after a reload.**
///
/// The history projection cleaned deliberation grammar; the POST did not.
/// So a turn arriving live showed `!support #lazy-load ^3 agreed` and the
/// same turn after a refresh showed `agreed` — one message, two renderings,
/// separated by a page reload.
#[test]
fn a_live_reply_reads_as_the_reloaded_one_will() {
    let cleaned = readable_responses(vec![
        reply("!support #lazy-load ^3 agreed, and it is reversible"),
        reply("here is the summary you asked for"),
    ]);

    assert_eq!(
        cleaned[0].text, "agreed, and it is reversible",
        "the grammar is gone on the live path too"
    );
    assert_eq!(
        cleaned[1].text, "here is the summary you asked for",
        "and an ordinary reply is untouched"
    );
}
