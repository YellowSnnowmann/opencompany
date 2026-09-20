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
/// Both paths go through `readable_moves`, which since plan hive-desks
/// Phase 4 is an identity: a seat speaks through a tool call, so there is no
/// move grammar left to rewrite, and a reply reads exactly as written on the
/// live path and after a refresh.
#[test]
fn a_live_reply_reads_as_the_reloaded_one_will() {
    let cleaned = readable_responses(vec![
        reply("agreed, and it is reversible"),
        reply("here is the summary you asked for"),
    ]);

    assert_eq!(cleaned[0].text, "agreed, and it is reversible");
    assert_eq!(
        cleaned[1].text, "here is the summary you asked for",
        "an ordinary reply is untouched"
    );
}
