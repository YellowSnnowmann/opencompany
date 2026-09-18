//! Serialization coverage for the operator's folded-aside DTO.

use crate::server::chat_history::{AsideConversation, AsideLine};

/// The wire shape the console binds to.
#[test]
fn the_folded_aside_reaches_the_wire_as_camel_case() {
    let aside = AsideConversation {
        members: vec!["exchanges".to_owned(), "refunds".to_owned()],
        lines: vec![AsideLine {
            author_id: "exchanges".to_owned(),
            text: "the difference is -$16.63".to_owned(),
        }],
    };
    let dto = super::AsideConversationDto {
        members: aside.members,
        lines: aside
            .lines
            .into_iter()
            .map(|line| super::AsideLineDto {
                author_id: line.author_id,
                text: line.text,
            })
            .collect(),
    };
    let wire = serde_json::to_value(&dto).expect("the DTO serializes");

    assert!(
        wire.get("members").is_some(),
        "author first, then who they addressed: {wire}"
    );
    let line = &wire
        .get("lines")
        .and_then(|lines| lines.as_array())
        .expect("lines")[0];
    assert_eq!(
        line.get("authorId").and_then(|author| author.as_str()),
        Some("exchanges"),
        "camelCase, as the console reads it: {wire}"
    );
    assert_eq!(
        line.get("text").and_then(|text| text.as_str()),
        Some("the difference is -$16.63"),
        "and the marker head never reaches the browser"
    );
}
