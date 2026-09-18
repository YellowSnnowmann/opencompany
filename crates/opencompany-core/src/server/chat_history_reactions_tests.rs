use super::*;

fn user(id: &str) -> Option<Actor> {
    Some(Actor {
        kind: ActorKind::User,
        id: id.to_string(),
    })
}

pub(super) fn at(seq: u64, event: CompanyEvent) -> StoredEvent {
    StoredEvent {
        seq: EventSeq::new(seq),
        company: crate::ports::types::CompanyId::new("acme"),
        event,
        at_millis: 1_700_000_000_000 + seq,
    }
}

fn reaction(seq: u64, message: u64, emoji: &str, on: bool, by: Option<Actor>) -> StoredEvent {
    at(
        seq,
        CompanyEvent::ReactionToggled {
            message_seq: EventSeq::new(message),
            emoji: emoji.to_string(),
            on,
            by,
        },
    )
}

pub(super) fn labels() -> HashMap<String, String> {
    HashMap::from([
        ("u1".to_string(), "Ada".to_string()),
        ("u2".to_string(), "Grace".to_string()),
    ])
}

/// Two people reacting with the same emoji are two rows, not a count of
/// two, and only the reader's own row is `mine` — which is the whole reason
/// the durable record is per-person.
#[test]
fn reactions_fold_into_one_row_per_person() {
    let log = vec![
        reaction(10, 4, "👍", true, user("u1")),
        reaction(11, 4, "👍", true, user("u2")),
    ];
    let folded = fold_reactions(&log, &Viewer::User("u1".to_string()), &labels());
    let rows = folded.get("4").expect("message 4 has reactions");
    assert_eq!(
        rows,
        &vec![
            ReactionView {
                emoji: "👍".to_string(),
                by_label: "Ada".to_string(),
                mine: true,
            },
            ReactionView {
                emoji: "👍".to_string(),
                by_label: "Grace".to_string(),
                mine: false,
            },
        ]
    );

    // The same log read by the other person flips only `mine`.
    let folded = fold_reactions(&log, &Viewer::User("u2".to_string()), &labels());
    let mine: Vec<bool> = folded["4"].iter().map(|r| r.mine).collect();
    assert_eq!(mine, vec![false, true]);
}

/// The last event per (message, person, emoji) wins, so a clear removes the
/// row and a repeated set leaves exactly one — which is what makes the
/// route's explicit `on` flag idempotent rather than a toggle that drifts.
#[test]
fn reactions_fold_to_the_last_event_per_person_and_emoji() {
    let log = vec![
        reaction(10, 4, "👍", true, user("u1")),
        reaction(11, 4, "👍", true, user("u1")),
        reaction(12, 4, "🎉", true, user("u1")),
        reaction(13, 4, "🎉", false, user("u1")),
    ];
    let folded = fold_reactions(&log, &Viewer::User("u1".to_string()), &labels());
    let emojis: Vec<&str> = folded["4"].iter().map(|r| r.emoji.as_str()).collect();
    assert_eq!(emojis, vec!["👍"], "a cleared reaction leaves no row");
}

/// A reaction made with a machine credential reads back as the operator's,
/// exactly as an unattributed message does — the same collapse `project`
/// makes for authorship, so the two surfaces cannot disagree about who a
/// credential is.
#[test]
fn an_unattributed_reaction_belongs_to_the_operator() {
    let log = vec![reaction(10, 4, "👀", true, None)];
    let folded = fold_reactions(&log, &Viewer::Operator, &labels());
    assert_eq!(folded["4"][0].by_label, "operator");
    assert!(folded["4"][0].mine);
    // …and is nobody's own when a signed-in person reads it.
    let folded = fold_reactions(&log, &Viewer::User("u1".to_string()), &labels());
    assert!(!folded["4"][0].mine);
}
