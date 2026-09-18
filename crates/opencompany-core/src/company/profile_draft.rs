//! Issue #1776 — drafting **one** teammate's mandate or persona, for an
//! operator who then keeps it or throws it away.
//!
//! A teammate's `description` (the mandate: one line on what it owns) and
//! `instructions` (the persona appended to its system prompt) are the two
//! fields that decide how it behaves, and the two an operator has the least
//! help writing. This module holds the shape of a draft; the model call that
//! produces one lives in [`crate::harness::profile_draft`], behind the
//! `openhuman` feature.
//!
//! ## Why this is not the roster designer's rule being relaxed
//!
//! [`crate::company::setup`] deliberately keeps the model out of a teammate's
//! standing instructions: it names a work *shape* from a closed enum and the
//! host owns every word. That rule is untouched, and it must stay that way —
//! it governs teammates that are **created** from a design pass, where the text
//! reaches a system prompt with nobody having read it, through a route any
//! member can call.
//!
//! A draft is the opposite case in the one way that matters. It is returned to
//! the operator and stored by **nothing**: the route that produces it never
//! writes, the console shows it beside the field rather than in it, and the
//! text only becomes a persona if a person takes it and then saves. That is the
//! same stance the workflow copilot's proposal protocol takes — the model's
//! output is data in a reply, and the operator's own action is what writes.
//!
//! So the boundary this module holds is narrow and specific: **a draft is
//! bounded like the field it is for, and it is never applied here.**

use crate::company::prompt::cap_persona_instructions;
use crate::company::setup::clamp_description;

/// Which authored field a draft is for.
///
/// Only the two prose fields. `name` and `role` are short identity values an
/// operator picks in seconds — and `role` is what delegation grounds on, so a
/// drafted one would change who the company routes work to, which is not a
/// thing to hand a model on a screen whose whole promise is "this changes
/// nothing until you save".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileField {
    /// The one-line mandate, shown on the roster card.
    Description,
    /// The persona appended to this teammate's system prompt.
    Instructions,
}

impl ProfileField {
    /// The wire spelling, which is also the `PATCH` field name — the console
    /// asks for a draft of the field it is about to fill.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Description => "description",
            Self::Instructions => "instructions",
        }
    }

    /// Reads a field off the wire. Anything else is `None`, so a request naming
    /// a field this pass does not draft is refused rather than silently
    /// answered about a different one.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "description" => Some(Self::Description),
            "instructions" => Some(Self::Instructions),
            _ => None,
        }
    }

    /// Brings a draft inside the bound the field itself obeys.
    ///
    /// Applied **host-side**, on the way out, so the console is not the only
    /// thing holding the limit — the same reason the roster pass clamps its own
    /// mandates rather than trusting the review screen to.
    ///
    /// The two bounds are different in kind, and each field gets its own:
    /// a mandate is clamped to [`MAX_DESCRIPTION`](crate::company::setup::MAX_DESCRIPTION)
    /// because the roster card has one line for it, while a persona is capped
    /// by prompt weight because it is read on every turn of that teammate.
    pub fn clamp(self, text: &str) -> String {
        match self {
            Self::Description => clamp_description(text),
            Self::Instructions => cap_persona_instructions(text.trim()),
        }
    }
}

/// Why no draft came back.
///
/// Several reasons rather than one, because **the operator's next move
/// differs** — the same split [`FallbackReason`](crate::company::setup::FallbackReason)
/// makes for the roster pass, and for the same reason: one sentence covering
/// all of them can only be vague enough to be useless. "Wire up a model",
/// "try again", "say more" and "wait for the period to reset" are four
/// different actions, and a reader who cannot tell which one they are in has
/// been told nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftRefusal {
    /// Nothing was reachable, so no call ran. Wire up a model.
    NoModel,
    /// A model is wired and the call did not land — a timeout, an unreachable
    /// provider. Retry, or check the provider; adding a key would fix nothing.
    ModelUnreachable,
    /// A model answered and the answer could not be used. Say more in the hint,
    /// or write the field by hand.
    Unreadable,
    /// The company has spent its plan-level token ceiling for the period
    /// (issue #188), so no call ran. Nothing the operator types will change
    /// that until the window resets or the ceiling is raised.
    BudgetExhausted,
}

impl DraftRefusal {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoModel => "no_model",
            Self::ModelUnreachable => "model_unreachable",
            Self::Unreadable => "unreadable",
            Self::BudgetExhausted => "budget_exhausted",
        }
    }
}

/// Who said one thing in a copilot conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnRole {
    /// The operator asking for something.
    Operator,
    /// The copilot's own earlier answer.
    Copilot,
}

impl TurnRole {
    /// Reads a role off the wire. Anything unrecognised is `None` — a turn
    /// whose speaker cannot be established is dropped rather than guessed at,
    /// because attributing the operator's words to the copilot (or the reverse)
    /// is how a conversation starts arguing with itself.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "operator" => Some(Self::Operator),
            "copilot" => Some(Self::Copilot),
            _ => None,
        }
    }
}

/// One thing said in a copilot conversation.
///
/// The console holds the transcript and sends it back each turn — the host
/// stores nothing. That is the whole of "in-session": closing the form ends the
/// conversation, and there is no journal to rehydrate from, no thread id to
/// collide, and no cleanup to get wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotTurn {
    pub role: TurnRole,
    pub text: String,
}

/// How many turns of a conversation are carried into the prompt.
///
/// A bound rather than a trim the console is trusted to do. Long conversations
/// are exactly the ones where an operator has been iterating hardest, so the
/// tail is what matters — the oldest turns are dropped first, and the grounding
/// block is re-sent every turn regardless, so nothing load-bearing ages out.
pub const MAX_TURNS: usize = 16;

/// The longest one turn may be.
///
/// Applies to both sides. An operator pasting a page into the box, or a copilot
/// that answered with an essay, must not be able to push the grounding out of a
/// later prompt.
pub const MAX_TURN_CHARS: usize = 2_000;

/// The longest sentence the reduced Add-teammate dialog may design from.
///
/// **Not [`MAX_DESCRIPTION`](crate::company::setup::MAX_DESCRIPTION), which is
/// a layout bound.** That constant exists because a roster card has one line
/// for a mandate, and applying it to the operator's *input* cut their brief at
/// 200 characters — with `clamp_description`'s own `…` on the end — before the
/// model ever read it. Nothing said so: the console's box has no counter, the
/// original text is never stored (the record carries the model's description,
/// not the operator's), and so a requirement written past character 200 simply
/// did not reach the role, the mandate or the persona, and left no trace that
/// it had been dropped.
///
/// What bounds this is prompt weight, so it is [`MAX_TURN_CHARS`] — the bound
/// every other piece of operator free text going into a copilot prompt already
/// obeys. The console holds the same number on the box itself, so the operator
/// meets the limit while typing rather than in a record that quietly lost the
/// end of their sentence.
pub const MAX_DESIGN_BRIEF: usize = MAX_TURN_CHARS;

/// The relationship the bug was, asserted where a later edit has to meet it.
///
/// A build failure rather than a test failure on purpose: the two constants are
/// easy to confuse, the wrong one was chosen once already, and choosing it
/// again would not break anything visible — it would quietly eat the end of
/// every brief longer than a roster card's line.
const _: () = assert!(
    MAX_DESIGN_BRIEF > crate::company::setup::MAX_DESCRIPTION,
    "a design brief cut to the card bound loses whatever the operator wrote past it"
);

/// Brings a design brief inside [`MAX_DESIGN_BRIEF`], collapsing whitespace.
///
/// A last resort, and one nothing should reach: the console holds the same
/// bound on the box itself, so an operator meets it while typing. This is here
/// so a caller that is not our console cannot push the grounding out of the
/// design prompt — the same belt every other free-text-into-a-prompt input
/// wears (`clamp_conversation`).
///
/// No ellipsis is appended, unlike `clamp_description`. That mark means "there
/// was more, and a reader can go and see it"; here there is nowhere to go, and
/// a brief ending in `…` reads to the model as an unfinished sentence — which,
/// given the role rule above, is the last thing to teach it to write.
pub fn clamp_design_brief(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(MAX_DESIGN_BRIEF).collect::<String>()
}

/// Brings a conversation inside the bounds the prompt obeys: the last
/// [`MAX_TURNS`], each clamped to [`MAX_TURN_CHARS`], blank turns dropped.
///
/// Host-side, so the console is not the only thing holding the bound — the same
/// argument the field clamps make.
pub fn clamp_conversation(turns: Vec<CopilotTurn>) -> Vec<CopilotTurn> {
    let kept: Vec<CopilotTurn> = turns
        .into_iter()
        .filter(|turn| !turn.text.trim().is_empty())
        .map(|turn| CopilotTurn {
            role: turn.role,
            text: turn.text.chars().take(MAX_TURN_CHARS).collect(),
        })
        .collect();
    let start = kept.len().saturating_sub(MAX_TURNS);
    kept[start..].to_vec()
}

/// One teammate a draft is about, plus everything the pass is allowed to see.
///
/// A closed set, assembled host-side from the company record. The console does
/// not compose it and cannot add to it: a draft is grounded in this teammate,
/// its siblings' roles, and what the operator typed into the hint — never in
/// the rest of the company.
#[derive(Debug, Clone, Default)]
pub struct ProfileSubject {
    /// The company's name, so a mandate reads as one of *this* company's.
    pub company_name: String,
    /// What the company produces (`[company].output`), when it declares it.
    pub company_output: Option<String>,
    /// The teammate's roster id.
    pub agent_id: String,
    /// Its name, when an operator has given it one.
    pub name: Option<String>,
    /// Its role — the one field a draft can always lean on.
    pub role: String,
    /// The mandate in force, so a redraft improves on it rather than ignoring
    /// it, and so a persona can be written to fit the job the card claims.
    pub description: Option<String>,
    /// The persona in force, for the same reason.
    pub instructions: Option<String>,
    /// The rest of the roster — **id and role only**.
    ///
    /// Named so a drafted mandate does not restate a sibling's. The delegation
    /// surface renders id and role and nothing else, so two teammates whose
    /// mandates overlap are two the company cannot tell apart when it comes to
    /// hand out work (issue #1162).
    pub siblings: Vec<Sibling>,
    /// The conversation so far, oldest first — already clamped.
    ///
    /// Empty on the opening turn, which means "draft something, I have not said
    /// anything yet". That path is kept deliberately: an operator staring at a
    /// blank persona box often wants a starting point to react to, and making
    /// them type first would ask for the very thing they opened the copilot
    /// because they could not write.
    pub conversation: Vec<CopilotTurn>,
}

/// One other teammate on the roster, as a draft is told about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sibling {
    pub id: String,
    pub role: String,
}

/// What one copilot turn produced.
///
/// Either an answer or a reason there is none — never both, and never neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileDraft {
    /// The copilot said something, and possibly drafted.
    Answered {
        /// What it says in the conversation — what it changed, or what it needs
        /// to know. Always present.
        reply: String,
        /// The whole field as it now stands, already clamped. `None` when the
        /// turn was a question rather than a draft.
        ///
        /// A copilot that must always produce text cannot ask what the operator
        /// means; it can only guess and hand back a paragraph. Letting a turn
        /// carry a question and no draft is what makes this a conversation
        /// rather than a slot machine with a text box.
        draft: Option<String>,
    },
    /// No answer at all, and why.
    Refused(DraftRefusal),
}

impl ProfileDraft {
    /// The drafted text, or `None` when this turn asked rather than drafted.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Answered { draft, .. } => draft.as_deref(),
            Self::Refused(_) => None,
        }
    }

    /// What the copilot said, or `None` when the pass refused.
    pub fn reply(&self) -> Option<&str> {
        match self {
            Self::Answered { reply, .. } => Some(reply),
            Self::Refused(_) => None,
        }
    }

    /// The refusal, or `None` when the copilot answered.
    pub fn refusal(&self) -> Option<DraftRefusal> {
        match self {
            Self::Answered { .. } => None,
            Self::Refused(reason) => Some(*reason),
        }
    }

    /// Builds the outcome for a model answer: the reply as-is, the draft
    /// clamped for its field, and a blank draft treated as "asked, did not
    /// draft" rather than as an empty suggestion.
    ///
    /// A turn with neither is unreadable. A model that answers with whitespace
    /// has technically replied, and putting that on screen reads as the copilot
    /// having nothing to say about a teammate rather than as the failure it is.
    pub fn from_answer(field: ProfileField, reply: &str, draft: Option<&str>) -> Self {
        let reply = reply.trim();
        let draft = draft
            .map(|text| field.clamp(text))
            .filter(|text| !text.trim().is_empty());
        match (reply.is_empty(), draft) {
            // Nothing said and nothing drafted: there is no turn here.
            (true, None) => Self::Refused(DraftRefusal::Unreadable),
            // A draft with no covering sentence is still a good turn — the
            // draft speaks for itself, and inventing prose for it would put
            // words in the copilot's mouth.
            (true, Some(draft)) => Self::Answered {
                reply: String::new(),
                draft: Some(draft),
            },
            (false, draft) => Self::Answered {
                reply: reply.to_string(),
                draft,
            },
        }
    }
}

/// The longest a designed role may be.
///
/// A role is an identity phrase, not a sentence: it is interpolated into
/// `persona_prompt`'s "You are {name}, the {role} at {company}." and rendered
/// beside an id in the orchestrator's Team block, both of which read a job
/// title and neither of which reads a paragraph. Every role shipped in
/// `companies/` and `companies/_globals/` today is under 25 characters, so this is loose
/// by a factor of two rather than tight.
///
/// Enforced host-side on the way out of a design pass, for the same reason
/// [`ProfileField::clamp`] is: the console must not be the only thing holding
/// a bound that ends up in a system prompt.
pub const MAX_ROLE: usize = 60;

/// The most words a designed role may be.
///
/// [`MAX_ROLE`] bounds the *characters*, and a sentence fits inside it:
/// `"Handles payroll and reconciles the books weekly"` is 46 characters, has
/// alphanumerics, no ellipsis, and matches no other field — so every other rule
/// here passes it, and it is then stored and read as the teammate's identity in
/// every prompt it runs.
///
/// `design_system_prompt` already asks for "a noun phrase of one to four
/// words". A brief is not a validator — that is the lesson of every other rule
/// in this file — so the contract is enforced rather than hoped for.
///
/// **Five, not four**, deliberately: one word of slack above what the prompt
/// asks, so a real title that runs slightly long ("VP of Brand and
/// Communications") is not thrown away, while a sentence — which is what the
/// failure actually looks like — still is. Refusing is loud and recoverable:
/// the operator gets the full form carrying what they typed, with Role as its
/// own field.
pub const MAX_ROLE_WORDS: usize = 5;

/// Brings a designed role inside [`MAX_ROLE`], collapsing whitespace.
///
/// Truncation here is a **last** resort and is not how a long answer is
/// normally handled — [`TeammateDesign::from_parts`] refuses a role that needs
/// cutting, because a job title with its end sliced off is worse than no job
/// title: it is stored, shown on every roster card, and read into every prompt,
/// and nobody was ever asked about it. This exists so the type cannot hold an
/// unbounded string even if a future caller forgets that rule.
pub fn clamp_role(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(MAX_ROLE).collect::<String>()
}

/// One field's normal form: whitespace collapsed, case folded, trailing
/// punctuation and the clamp's own `…` dropped.
///
/// Shared by the two sameness rules so they cannot come to disagree about what
/// "the same text" means — the failure they catch is one sentence wearing two
/// hats, and it does not stop being that when one copy gained a full stop.
fn normal(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_end_matches(['.', '!', '?', ';', ':', ',', '…'])
        .to_string()
}

/// Whether the role is the brief, or the front of it.
///
/// The rule both halves of the original defect reduce to: a job title is
/// something a model *wrote*, and a leading fragment of the operator's sentence
/// is something it *cut*. `roleFromDescription` — the split this route replaced
/// — produced exactly these, and an ellipsis is the only thing that made them
/// obvious. Without one they pass every other rule here.
///
/// Compared at a word boundary on the normal form, so "Runs wholesale outreach"
/// is caught against "Runs wholesale outreach to boutique retailers" while
/// "Wholesale Account Manager" is not caught against "Runs wholesale outreach…"
/// — it is not at the front of it.
///
/// ## Why not "reject verb-led roles", which is what the brief actually says
///
/// Because it cannot be done here without being wrong somewhere. The same
/// prompt says "Write in the same language the operator wrote in", so a list of
/// English verbs would refuse valid titles in every other language and pass
/// invalid ones — a validator that works for one language and silently degrades
/// for the rest is worse than the narrower rule. This catches the shape that
/// actually harms: the operator's own words stored back as an identity. A
/// verb-led title the model *invented* ("Payroll Manager" against a brief about
/// bookkeeping) is not a fragment of anything and is left alone.
fn leads_the_brief(role: &str, brief: &str) -> bool {
    let role = normal(role);
    let brief = normal(brief);
    if role.is_empty() || brief.is_empty() {
        return false;
    }
    // The whole brief handed back, and the front of it. The word boundary is
    // what keeps "Ops" from matching a brief that opens "Opsware migration".
    role == brief || brief.starts_with(&format!("{role} "))
}

/// Whether any two of a design's three fields are the same text.
///
/// Compared on a normal form — whitespace collapsed, case folded, trailing
/// punctuation and the clamp's own `…` dropped — because the failure this
/// catches is a model echoing one sentence into two slots, and it does not stop
/// being that when one copy gained a full stop or a capital letter. An exact
/// `==` would miss `"Owns stockists"` against `"Owns stockists."`, which is the
/// same record with a keystroke of difference.
///
/// All three pairs, not just mandate-against-persona. A role equal to the
/// mandate is the original defect exactly — the sentence stored as a job title
/// — and a role equal to the persona is the same answer arrived at from the
/// other end.
fn repeats_a_field(role: &str, description: &str, instructions: &str) -> bool {
    let role = normal(role);
    let description = normal(description);
    let instructions = normal(instructions);
    // A field that normalizes away to nothing is punctuation, and two of those
    // matching says nothing — the emptiness checks above already refused it.
    if role.is_empty() || description.is_empty() || instructions.is_empty() {
        return false;
    }
    role == description || role == instructions || description == instructions
}

/// A whole teammate as one design pass wrote it (issue #1989).
///
/// ## Why a role may be drafted here, when `ProfileField` deliberately excludes one
///
/// The exclusion above is real and it stays. Its reason is that **a role is
/// what delegation grounds on, so a drafted one would change who the company
/// routes work to** — and that is a statement about *editing an existing
/// teammate*: work is already routed to it, and a model re-pointing that
/// without the operator choosing to is the harm.
///
/// At **creation** there is nothing to re-route. The teammate does not exist,
/// no work is addressed to it, no orchestrator has ever seen it. So the
/// property the exclusion protects is not in play, and the alternative is not a
/// safe blank: `role` is required by every write path, `persona_prompt`
/// interpolates it unguarded, and the console's previous answer was to cut the
/// operator's sentence at sixty characters and store the front half as a job
/// title. A model that reads the sentence and answers "Wholesale Account
/// Manager" is strictly better than that, and it is shown to the operator on
/// the page the create lands on, in an editable field, before it can matter.
///
/// The separation is kept at the route, not here: `POST {scope}/team/design`
/// is creation-only and takes no agent id, while `POST {scope}/team/{id}/draft`
/// still refuses anything but `description` and `instructions`. There is no way
/// to reach this type with an existing teammate's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeammateDesign {
    /// The job title, bounded by [`MAX_ROLE`].
    pub role: String,
    /// The mandate — one line on what this teammate owns.
    pub description: String,
    /// The persona appended to this teammate's system prompt.
    pub instructions: String,
}

impl TeammateDesign {
    /// Builds a design from a model's three fields, or `None` when any of them
    /// is unusable.
    ///
    /// All-or-nothing on purpose. A partial design is the failure mode this
    /// whole change exists to remove: a teammate holding a real mandate and a
    /// role that is a fragment is exactly what shipped before, and it looks
    /// finished. If the pass cannot produce all three, the console hands the
    /// operator the full form and asks — which is honest, and which is the same
    /// answer a company with no model gets.
    ///
    /// A role needing truncation is refused rather than cut, for the reason
    /// given on [`clamp_role`].
    ///
    /// `brief` is the operator's own sentence, passed so the role can be
    /// checked against it — see the third bullet below.
    ///
    /// ## Why "three non-empty strings" was never the bar
    ///
    /// Two answers clear every length and emptiness check above and are still
    /// the exact record this route exists to stop shipping:
    ///
    /// - **A role the model truncated itself.** The brief tells it never to,
    ///   but a brief is not a validator, and `"Runs wholesale outreach to
    ///   boutique retailers and keeps the…"` is precisely the stored job title
    ///   that motivated the whole change — it does not stop being that because
    ///   a model wrote the `…` rather than a `String::truncate`. The console's
    ///   `designedTeammateFields` already refuses one; this is the half that
    ///   holds when the console is not the caller, and refusing here is what
    ///   turns a silent hand-over into a `DraftRefusal` the operator is shown.
    /// - **The same text in more than one field.** The operator's original
    ///   complaint was one sentence appearing as role, mandate and persona at
    ///   once. A model that echoes the sentence into two of the three has not
    ///   designed a teammate, it has restated the input in valid JSON, and
    ///   every length check passes. `design_system_prompt` says "this must NOT
    ///   restate the mandate" for the same reason; this enforces it.
    /// - **A role that is a sentence.** `MAX_ROLE` bounds characters, and a
    ///   sentence fits: `"Handles payroll and reconciles the books weekly"` is
    ///   46 of the 60 allowed. [`MAX_ROLE_WORDS`] enforces the shape the brief
    ///   asks for instead of hoping the model obeys it.
    /// - **A role that is the operator's brief, or the front of it.** The rule
    ///   above compares the three answers to each other and so misses the shape
    ///   that matters most: a brief of `"Runs wholesale outreach to boutique
    ///   retailers"` answered with role `"Runs wholesale outreach"`, a real
    ///   mandate and real instructions beside it, passes everything. That is
    ///   the clause split this route replaced, arriving without the ellipsis
    ///   that used to make it obvious, and only a comparison against the input
    ///   catches it. See [`leads_the_brief`].
    ///
    /// Both are refusals rather than repairs, for the reason the whole type is
    /// all-or-nothing: the operator gets the full form carrying what they
    /// typed, which is honest, where a salvaged two-thirds of a design looks
    /// finished on screen and is not.
    pub fn from_parts(
        role: &str,
        description: &str,
        instructions: &str,
        brief: &str,
    ) -> Option<Self> {
        let role = role.split_whitespace().collect::<Vec<_>>().join(" ");
        let description = description.trim();
        let instructions = instructions.trim();
        if role.is_empty() || description.is_empty() || instructions.is_empty() {
            return None;
        }
        if role.chars().count() > MAX_ROLE {
            return None;
        }
        // Not a job title anyone could read: punctuation, emoji, whitespace.
        if !role.chars().any(char::is_alphanumeric) {
            return None;
        }
        // A job title of one to four words has no ellipsis in it in either
        // spelling; one that does is an answer the model cut short. Checked on
        // the role alone — a *mandate* may legitimately end in `…`, because
        // that is the mark `clamp_description` itself leaves.
        if role.contains('…') || role.contains("...") {
            return None;
        }
        // A sentence fits inside MAX_ROLE. See MAX_ROLE_WORDS.
        if role.split_whitespace().count() > MAX_ROLE_WORDS {
            return None;
        }
        // Compared before the clamps, so a description cut to the card bound
        // cannot come out looking different from the persona it was copied
        // from and pass.
        if repeats_a_field(&role, description, instructions) {
            return None;
        }
        // The operator's own sentence, or the front of it, handed back as the
        // job title. Checking the three answers against each other does not
        // catch it, and neither does whole-brief equality alone: a brief of
        // "Runs wholesale outreach to boutique retailers" answered with
        // "Runs wholesale outreach" clears the length, word-count, ellipsis,
        // duplicate-field and equality rules, and is exactly the record this
        // route replaced — the clause split, arriving without its ellipsis.
        //
        // Blunt on purpose, and the false positive is the good kind. An
        // operator whose brief opens with the job title has answered a
        // different question from the one the box asks ("What should they
        // do?"), and the right response to that is the full form — where Role
        // is its own field — carrying what they typed, rather than a design
        // built on it.
        if leads_the_brief(&role, brief) {
            return None;
        }
        Some(Self {
            role,
            description: clamp_description(description),
            instructions: cap_persona_instructions(instructions),
        })
    }
}

/// What one design pass produced: a whole teammate, or a reason there is none.
///
/// The same two-armed shape as [`ProfileDraft`], and for the same reason — every
/// unhappy path here is something the operator is shown and can act on, so
/// there is no error for a caller to handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesignedTeammate {
    /// The model designed the teammate.
    Designed(TeammateDesign),
    /// It could not, and why.
    Refused(DraftRefusal),
}

impl DesignedTeammate {
    /// The design, or `None` when the pass refused.
    pub fn design(&self) -> Option<&TeammateDesign> {
        match self {
            Self::Designed(design) => Some(design),
            Self::Refused(_) => None,
        }
    }

    /// The refusal, or `None` when it designed one.
    pub fn refusal(&self) -> Option<DraftRefusal> {
        match self {
            Self::Designed(_) => None,
            Self::Refused(reason) => Some(*reason),
        }
    }
}

#[cfg(test)]
#[path = "profile_draft_tests.rs"]
mod tests;
