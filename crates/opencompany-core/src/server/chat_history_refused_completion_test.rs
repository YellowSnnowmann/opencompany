//! **A completion the driver refused does not read as one that happened.**
//!
//! The host appends a seat's row before the driver rules on it, and `commit`
//! stamps `episode.kind` off the utterance -- so a `complete_episode` refused
//! for `AwaitingReply` is already on the desk, saying the seat finished. The
//! journal cannot take a row back; `UtteranceRefused` is the correction written
//! beside it, and the history plane is where the two are reconciled.
//!
//! Journalled rather than driven, the way every sibling here is: what is under
//! test is the read plane's reconciliation, and a scripted episode would prove
//! the driver's half instead. That half is
//! `a_completion_refused_while_owed_an_answer_is_journaled_as_refused`.

use super::*;
use crate::ports::types::{CompanyId, ReplyEpisode, UtteranceKind};

use super::referral_origin_test_support::*;

/// A seat's line, optionally claiming it ended the episode.
fn desk_row(who: &str, text: &str, completes: bool) -> CompanyEvent {
    CompanyEvent::AgentReply {
        chat_id: "engineering".to_string(),
        agent_id: who.to_string(),
        text: text.to_string(),
        steps: Vec::new(),
        task_id: None,
        outputs: Vec::new(),
        parent: None,
        mentions: Vec::new(),
        mention_depth: 0,
        audience: Vec::new(),
        episode: completes.then(|| ReplyEpisode {
            id: "ep-1".into(),
            revision: 1,
            kind: UtteranceKind::CompleteEpisode,
            to: Vec::new(),
            routed_by: None,
        }),
    }
}

/// The refusal drops the claim and keeps the words.
///
/// Both halves matter and they fail in opposite directions. Leaving the claim
/// makes a console fold the episode closed on a seat that never finished --
/// which is the bug, observed live. Dropping the row instead would lose a line
/// the seat really did say and an operator should read, so the row has to stay,
/// shorn of the one assertion that was refused.
#[tokio::test]
async fn a_refused_completion_keeps_its_words_and_loses_its_claim() {
    let home = tempfile::tempdir().expect("tempdir");
    let runtime = runtime(home.path()).await;
    let id = CompanyId::new("acme");
    let append = |event: CompanyEvent| {
        let runtime = Arc::clone(&runtime);
        let id = id.clone();
        async move { runtime.events().append(&id, event).await.expect("journal") }
    };

    // One seat finishes for real; the other is refused. Two rows, so the
    // correction has to name the right one rather than clear the column.
    let honest = append(desk_row("ceo", "shipping it.", true)).await;
    let refused = append(desk_row("engineer", "I'll wrap up once I hear back.", true)).await;
    append(CompanyEvent::UtteranceRefused {
        chat_id: "engineering".into(),
        episode_id: "ep-1".into(),
        seat: "engineer".into(),
        at: refused.value(),
        reason: "AwaitingReply { waiting_on: [\"ceo\"] }".into(),
    })
    .await;

    let history = history_for_desk(
        &runtime,
        "engineering",
        "engineering",
        &Viewer::Operator,
        None,
        50,
        true,
    )
    .await
    .expect("history");
    let row = |seq: EventSeq| {
        history
            .iter()
            .find(|message| message.id == seq.value().to_string())
            .unwrap_or_else(|| panic!("the desk shows {seq:?}"))
    };

    let refused_row = row(refused);
    assert!(
        refused_row.episode.is_none(),
        "a refused completion no longer claims to have ended the episode: {:?}",
        refused_row.episode
    );
    assert!(
        refused_row.text.contains("once I hear back"),
        "and the seat's own words are still on the desk: {:?}",
        refused_row.text
    );

    // The refusal named one row. A correction that cleared the column instead
    // would erase an episode that really did end, and nothing on screen would
    // say so.
    assert!(
        row(honest).episode.is_some(),
        "the completion nobody refused still ends its episode",
    );
}
