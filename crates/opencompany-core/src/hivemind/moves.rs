//! The per-member move grammar: which trace kinds a seat may open a line with.
//!
//! A room whose members may all make every move is a room that votes. The live
//! evidence is unambiguous: on a nine-episode run of `companies/hive_math_lab`
//! every episode was three independent `!propose`s of the same number followed
//! by a `!commit`, and quorum carried because in `tinyhivemind` a `!propose`
//! counts as its own author's support. No `!support`, `!object`, `!evidence`,
//! `!question` or `!pin` was ever deposited. Three agents agreeing in parallel
//! is not deliberation; it is three answers with a quorum rule stapled on.
//!
//! The repair is structural rather than exhortative. A desk assigns moves —
//! one proposer, one challenger who may only object or refute, an archivist who
//! may only supply evidence — and the driver **enforces** the assignment, so a
//! member that cannot propose has to do something with the floor other than
//! restate the answer.
//!
//! What the table gates is the *deliberation* kinds only. `commit`, `question`
//! and `defer` ([`UNGATED_KINDS`]) belong to every seat however narrow its
//! entry: recording a topic the room has already carried re-derives nothing,
//! and a member with nothing to add must always have something to say that is
//! not prose. See [`UNGATED_KINDS`] for the live run that made this a rule.
//!
//! Enforcement is deliberately two-stage and never fatal: a first violation is
//! a one-line correction and a second attempt, and a second violation is
//! journaled with its leading `!` removed. A demoted line still says whatever
//! the member wanted to say — it simply deposits no trace, because
//! `tinyhivemind_hive::trace::resolve` only reads a marker at the start of a
//! line. It can therefore never be folded as support for anything, which is the
//! one outcome that would let a barred move still carry a topic.

use std::collections::BTreeMap;

/// Every kind a `[group_chat.hive].moves` entry may name.
///
/// `pin` covers `!unpin` as well: both write the same desk pinboard, and a
/// member entitled to put something on the board is entitled to take it off
/// again. Splitting them would be a permission nobody could use correctly.
pub const MOVE_KINDS: &[&str] = &[
    "propose", "support", "object", "refute", "evidence", "question", "defer", "commit", "pin",
];

/// The permission kind a marker line asks for, if it asks for one.
///
/// `None` for a line with no leading marker, and for a marker this host does
/// not police — a line the fold itself discards is not a move anybody made, and
/// demoting it would be a correction with no subject.
#[must_use]
pub fn line_kind(line: &str) -> Option<&'static str> {
    let word = line.trim_start().strip_prefix('!')?;
    let word = word.split_whitespace().next()?;
    match word {
        "unpin" => Some("pin"),
        other => MOVE_KINDS.iter().copied().find(|kind| *kind == other),
    }
}

/// The kinds no `moves` table can take away from a seat.
///
/// Each one is a move whose absence costs the room a turn and buys it nothing:
///
/// - `commit` is **bookkeeping, not authorship**. The library authorizes a
///   commit turn by setting the phase, and it hands the floor to whoever the
///   attention market picks — not to whoever the manifest thought would be
///   holding it. The live evidence: a six-member desk reached quorum at the
///   third evidential `!support`, the phase flipped to `Commit`, and the fold
///   then gave the floor to three seats a `moves` table had barred from
///   `commit`. Each was shown the no-move block, wrote prose, and the room
///   reported `Exhausted` on an answer it had already decided. Recording a
///   topic the room has already carried re-derives nothing, so there is no
///   seat too cheap to do it.
/// - `question` and `defer` are the two honest things a member with nothing to
///   add can say. A seat barred from both has only silence or a guess, and
///   prose costs the room a turn while depositing no trace.
///
/// A `moves` entry naming one of these is therefore accepted and ignored: it
/// describes what the seat could already do, so it is a no-op rather than an
/// error.
pub const UNGATED_KINDS: &[&str] = &["question", "defer", "commit"];

/// The moves `member` may open a line with, in [`MOVE_KINDS`] order.
///
/// A member the map does not name may make every move, which is what makes an
/// omitted `moves` table a no-op for every manifest written before it existed.
/// An entry naming no kind at all is read as "every move" for the same reason a
/// silent member is: an empty list is far more likely to be a table written and
/// never filled in than a deliberate vow of silence, and the alternative reading
/// hands somebody the floor with nothing legal to say.
///
/// [`UNGATED_KINDS`] are always in the result, whether the table named them or
/// not: `moves` gates the deliberation kinds — what a seat may put *on* the
/// floor — and never the room's bookkeeping or its two ways of saying "not me".
#[must_use]
pub fn allowed_for(moves: &BTreeMap<String, Vec<String>>, member: &str) -> Vec<&'static str> {
    match moves.get(member) {
        Some(declared) if !declared.is_empty() => MOVE_KINDS
            .iter()
            .copied()
            .filter(|kind| {
                UNGATED_KINDS.contains(kind) || declared.iter().any(|named| named == kind)
            })
            .collect(),
        _ => MOVE_KINDS.to_vec(),
    }
}

/// The one-line correction a member gets after its first barred move.
///
/// One line, because the whole prompt already told it what its moves are and a
/// second paragraph explaining the rule again is how a correction turns into a
/// context-window tax. It names the move it may not make first: a model that
/// reads only the head of a correction still learns the thing it got wrong.
#[must_use]
pub fn correction(attempted: &str, allowed: &[&str]) -> String {
    let moves = allowed
        .iter()
        .map(|kind| format!("!{kind}"))
        .collect::<Vec<_>>()
        .join(", ");
    let moves = if moves.is_empty() {
        "none on this turn".to_owned()
    } else {
        moves
    };
    format!(
        "You may not `!{attempted}` on this desk; your moves are {moves}. Reply again with ONE \
         line beginning with one of those markers."
    )
}

/// The same line, stripped of the marker that made it a move.
///
/// One deliberation line, rendered for a person instead of for the fold.
///
/// The grammar is addressed to the mechanism: `!` says which move this is,
/// `#topic` names the option, `^N` and `>N` are sequence citations. All four
/// are load-bearing in the transcript and meaningless in a chat window — an
/// operator reading a desk was being shown `!support #lazy-load ^3 agreed`,
/// which is machine syntax rendered verbatim in a human channel.
///
/// So the head tokens are stripped and the member's own sentence is all that
/// remains — a teammate's line should read as a teammate talking. `None` for any line carrying no move, which is
/// every ordinary reply on every non-deliberating desk — those must pass
/// through byte-for-byte.
///
/// **A rendering only.** The stored line keeps its grammar: the fold reads
/// markers off the journal, and a projection that rewrote them would leave the
/// room unable to count its own transcript.
#[must_use]
pub fn readable(line: &str) -> Option<String> {
    let kind = line_kind(line)?;
    let rest = line.trim_start().strip_prefix('!')?;
    let rest = rest
        .split_once(char::is_whitespace)
        .map_or("", |(_, tail)| tail);

    // Only the citation tokens at the HEAD are grammar. The same characters
    // inside a sentence are the member's own words — "#2 in the list", "a > b"
    // — and rewriting those would edit what a teammate said.
    let mut topic = None;
    let mut rest = rest.trim_start();
    loop {
        let token = rest.split_whitespace().next().unwrap_or_default();
        let is_grammar = token.starts_with('#')
            || token.starts_with('^')
            || token.starts_with('>')
            || token.starts_with("!");
        if token.is_empty() || !is_grammar {
            break;
        }
        if let Some(name) = token.strip_prefix('#')
            && topic.is_none()
        {
            topic = Some(name.to_string());
        }
        rest = rest[token.len()..].trim_start();
    }

    // **No label, only the sentence.** The lead this once carried — "[supports
    // lazy-load]" — was the grammar in another costume: still the mechanism's
    // vocabulary, still addressed to the fold, still something an operator has
    // to learn before the channel reads as a conversation. A teammate's line
    // should look like a teammate talking.
    //
    // What is lost is that the channel no longer distinguishes a support from
    // an objection at a glance. That is recoverable from the prose, which says
    // so in words, and the fold keeps the marker on the stored row either way.
    //
    // A move with no sentence after it — `!question` and `!defer` are the two
    // honest things a member with nothing to add can say — would otherwise
    // render as an empty bubble, so those keep a plain phrase.
    if !rest.is_empty() {
        return Some(rest.to_string());
    }
    // `line_kind` folds `!unpin` onto `pin` — they write the same board, so the
    // fold treats them alike. A RENDERING must not: "Pinned for the room." is
    // the opposite of what an unpin did, and a bare one carries no sentence to
    // correct the impression.
    let bare = line.trim_start().strip_prefix('!').unwrap_or_default();
    let unpinning = bare.split_whitespace().next() == Some("unpin");
    Some(
        match kind {
            "pin" if unpinning => "Unpinned from the room's board.",
            "question" => "I have nothing further to ask.",
            "defer" => "This is not mine to answer.",
            "commit" => "Recorded.",
            "pin" => "Pinned for the room.",
            _ => return None,
        }
        .to_string(),
    )
}

/// The leading `!` and nothing else: the member's own words are kept verbatim,
/// so the transcript records what it wanted to say and a reader can see the
/// attempt. What it loses is the only thing at stake — `resolve` reads a
/// marker at the start of a line and nowhere else, so a demoted line folds to
/// no trace and cannot support, object to, or commit anything.
///
#[must_use]
pub fn demote(line: &str) -> String {
    line.trim_start().strip_prefix('!').map_or_else(
        || line.trim().to_owned(),
        |rest| {
            let demoted = rest.trim_start();
            if demoted.is_empty() {
                "(no answer)".to_owned()
            } else {
                demoted.to_owned()
            }
        },
    )
}

/// One line a member deposited that its seat is not entitled to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveViolation {
    /// The member that made it, twice.
    pub agent_id: String,
    /// The kind it reached for, without the `!`.
    pub attempted: String,
}
