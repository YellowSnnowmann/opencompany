//! Which ledgers a company has: the ones the runtime ships, and the ones the
//! workspace declares.
//!
//! # Why the board is registered but not re-implemented
//!
//! [`tasks`] is [`LedgerSource::Native`]: its rows keep living in the
//! [`TaskStore`](crate::ports::TaskStore) and its columns keep firing dispatch,
//! planning passes and run settles. None of that is expressible as a
//! declaration and none of it should be — a declared status cannot open an
//! attempt, and a company that could redefine `in_progress` into something that
//! does not dispatch has broken its own runtime from a JSON file.
//!
//! It is registered anyway, for one reason: so `list_ledgers` names **every**
//! ledger and `read_ledger` reads every ledger. A discovery surface that covers
//! the ledgers a company invented but not the one it already had is a surface
//! an agent stops trusting, and then stops using.
//!
//! # Why a company may add one
//!
//! Because the runtime cannot know in advance which axes a company records. A
//! recruiting firm tracks candidates; an agency tracks retainers; a research
//! desk tracks experiments. Without a way to declare one, an agent asked to
//! track any of them writes a workspace file nobody designed — and the
//! company's own record of what it decided ends up as prose scattered across
//! chat, notes and files.
//!
//! Four rules keep that from undoing the design, and all four are code:
//!
//! - **A declared slug may not shadow a built-in**, so `tasks` is always the
//!   board and every prompt and route naming it stays right.
//! - **A declared ledger may not claim another's derived path**, because two
//!   writers on one file is how each one's work disappears.
//! - **A declared ledger may not be [`LedgerSource::Native`]**, which is for
//!   ledgers this runtime renders in Rust.
//! - **The count is capped**, and what is past the cap is reported rather than
//!   silently refused.

use serde_json::{Value, json};

use super::spec::{DERIVED_DIR, LedgerSource, LedgerSpec, parse};
use crate::Result;
use crate::error::OpenCompanyError;

/// Ledgers a company may declare beyond the built-in set.
///
/// A runaway guard rather than a budget to spend. Twelve is far more axes than
/// a company has been observed to need, and a folder nobody counted is how a
/// surface grows forty near-duplicate entries that each say almost the same
/// thing.
pub const MAX_DECLARED: usize = 12;

/// The company's work board, kept where it is.
///
/// Registered as [`LedgerSource::Native`] so it is *reachable* through the
/// ledger surface — listed, read, indexed into a turn — while every write still
/// goes through the board's own routes and tools, which is where the dispatch
/// edge, the planning pass and the run settle all live.
///
/// Its statuses and sections are **built from**
/// [`super::board::PHASES`](super::board::PHASES) rather than written out again
/// beside it. That table is the one declaration of the board's reader-facing
/// vocabulary; a phase added there gains a status here and a section in the
/// rendered file, with no second list to remember.
///
/// # Three statuses, not six (issue #1512)
///
/// The board has six *stages* and the ledger declares three *phases*. The
/// runtime still moves a card through `planning`, `in_progress`, `paused` and
/// `in_review`, and each of those still means something distinct to the code
/// that fires it — but all four say the same thing to a reader: **somebody
/// started this and it is not finished**. Declaring them as four statuses asked
/// every agent to pick between them on every write, and the pick was
/// consistently wrong in ways nothing could catch: a card filed `in_review`
/// because a teammate wanted a second opinion, when `in_review` is defined by
/// the verdict that lands it in Done.
///
/// The stage is not lost. It rides on the row as the `stage` field below, so
/// *which* kind of working a card is doing is a property somebody reads off the
/// card, rather than a fourth pile to file it into.
fn tasks() -> Value {
    // One status per phase, in board order, each carrying the label the console
    // renders. `closed` is the table's, not a guess made here.
    let statuses: Vec<Value> = super::board::PHASES
        .iter()
        .map(|phase| {
            json!({
                "name": phase.id,
                "label": phase.label,
                "closed": phase.closed,
            })
        })
        .collect();
    // One section per phase, in the same order. There is no grouping left to
    // do: the phases *are* the grouping the six columns used to need one.
    let sections: Vec<Value> = super::board::PHASES
        .iter()
        .map(|phase| {
            json!({
                "heading": phase.label,
                "blurb": phase.blurb,
                "statuses": [phase.id],
                "order": "recent",
                // The archive is the one bounded section. "Recently done" is
                // what it says and what it was not: unbounded, it becomes the
                // largest thing in the file and answers a question — what did
                // we just finish — that five rows answer as well as ninety do.
                "cap": if phase.closed { 5 } else { super::budget::MAX_LISTED },
            })
        })
        .collect();
    json!({
        "slug": "tasks",
        "title": "Tasks",
        "purpose": "The company's work board: what has not started, what is being worked, and \
                    what is finished. Cards dispatch when they enter Working, so this ledger is \
                    written through the board — not with `record_entry`.",
        "source": "native",
        "derived": format!("{DERIVED_DIR}/tasks.md"),
        "writtenBy": "the board — `spawn_task` to open a card, `assign_task` to hand it over, or \
                      the console's board itself. `record_entry` does not write it, because \
                      entering a phase fires real work",
        "fields": [
            { "name": "id", "role": "id", "required": true },
            { "name": "title", "role": "title", "required": true,
              "description": "What the work is, in one line." },
            { "name": "column", "role": "status", "required": true,
              "description": "pending, working or done." },
            { "name": "stage", "role": "prose",
              "description": "Which kind of working: planning, in progress, paused, or waiting \
                              on your verdict. Read it; do not file by it." },
            { "name": "assignee", "role": "owner",
              "description": "The teammate or desk that owns it." },
            { "name": "priority", "role": "prose",
              "description": "low, medium or high." },
            { "name": "note", "role": "prose",
              "description": "The card's running history." }
        ],
        "statuses": statuses,
        "sections": sections,
        "checks": ["required-field", "known-status"]
    })
}

/// What the company is trying to achieve, and how each goal stands.
///
/// The axis a board cannot carry. A card is a piece of work with an assignee
/// and a finish; a goal outlives any one card, is owned rather than assigned,
/// and is *measured* rather than completed. Filing goals as cards makes the
/// board unreadable — a quarter's objective sits in To-do for three months
/// under a blurb telling everyone to work the first row they can — and filing
/// them in a note makes them invisible to everything.
fn goals() -> Value {
    json!({
        "slug": "goals",
        "title": "Goals",
        "purpose": "What this company is trying to achieve, how each goal is measured, and where \
                    each one stands. Read it before opening work nobody asked for; record against \
                    a goal when work moves it.",
        "source": "events",
        "derived": format!("{DERIVED_DIR}/goals.md"),
        "fields": [
            { "name": "id", "role": "id", "required": true },
            { "name": "goal", "role": "title", "required": true,
              "description": "The outcome, stated so somebody could tell whether it happened." },
            { "name": "status", "role": "status", "required": true,
              "description": "active, met or dropped." },
            { "name": "measure", "role": "prose",
              "description": "How anybody would know this is met. A goal with no measure is a \
                              mood." },
            { "name": "owner", "role": "owner",
              "description": "The teammate or desk accountable for it." },
            { "name": "due", "role": "date",
              "description": "When it is meant to be met, as a date." },
            { "name": "progress", "role": "prose",
              "description": "Where it stands now, in a line or two." },
            { "name": "reason", "role": "prose",
              "description": "Why it closed. Required when it closes." },
            { "name": "refs", "role": "refs",
              "description": "Task ids, artifacts or links that carry the work." }
        ],
        // Three, like every ledger here (issue #1512). What went: `proposed`
        // (a goal nobody has committed to is a chat message, and filing it here
        // made the Active section the minority of the file), `at_risk` (a
        // status that says how a goal is going, which is what `progress` is
        // for — and a goal moved there stayed there, because nothing moves it
        // back), and the `met`/`missed` split (both are "this is over"; which
        // one it was is the first clause of the reason, which is required).
        "statuses": [
            { "name": "active", "aliases": ["proposed", "at_risk"] },
            { "name": "met", "closed": true, "needs_reason": true },
            { "name": "dropped", "closed": true, "needs_reason": true,
              "aliases": ["missed"] }
        ],
        "sections": [
            {
                "heading": "Active",
                "blurb": "Being worked toward now. Most recently updated first — record against a \
                          goal to raise it. If one is slipping, say so in `progress` rather than \
                          looking for a status that means it.",
                "statuses": ["active"],
                "order": "recent"
            },
            {
                "heading": "Settled",
                "blurb": "Over, each with the reason — which is where met, missed and abandoned \
                          are told apart. Kept: a goal dropped without a recorded reason is one \
                          somebody proposes again next quarter.",
                "statuses": ["met", "dropped"],
                "cap": 20
            }
        ],
        "checks": ["required-field", "known-status", "closed-needs-reason"]
    })
}

/// What the company decided, and what it decided *against*.
///
/// The cheapest mistake a company makes is re-opening a question it already
/// settled, and nothing in a chat log prevents it: the reasoning is in a thread
/// nobody can find, and the person who remembers has left. A decision row costs
/// one write and is read for exactly one thing — *have we already ruled this
/// out* — which is why the rejected alternatives are a field rather than a
/// footnote.
fn decisions() -> Value {
    json!({
        "slug": "decisions",
        "title": "Decisions",
        "purpose": "What this company has decided, why, and what it decided against. Read it \
                    before re-opening a question — re-litigating something already settled is the \
                    cheapest mistake available, and the reason beside it is what prevents it.",
        "source": "events",
        "derived": format!("{DERIVED_DIR}/decisions.md"),
        "fields": [
            { "name": "id", "role": "id", "required": true },
            { "name": "decision", "role": "title", "required": true,
              "description": "What was decided, in one line." },
            { "name": "status", "role": "status", "required": true,
              "description": "proposed, accepted or retired." },
            { "name": "context", "role": "prose",
              "description": "The question this answers, and what forced it." },
            { "name": "rationale", "role": "prose",
              "description": "Why this and not the alternatives." },
            { "name": "alternatives", "role": "prose",
              "description": "What was considered and rejected. The half a chat log loses." },
            { "name": "decided_by", "role": "owner",
              "description": "Who made the call." },
            { "name": "decided_on", "role": "date" },
            { "name": "reason", "role": "prose",
              "description": "Why it was retired — replaced by what, or reversed on what \
                              evidence. Required when it closes." },
            { "name": "refs", "role": "refs" }
        ],
        // Three (issue #1512). `superseded` and `reversed` were one status
        // wearing two words: both mean *this is no longer the answer*, both
        // require a reason, and the reason is the only place the difference was
        // ever legible — "replaced by D-14" versus "we were wrong". Asking a
        // writer to also encode it in the status bought a coin-flip.
        "statuses": [
            { "name": "proposed" },
            { "name": "accepted" },
            { "name": "retired", "closed": true, "needs_reason": true,
              "aliases": ["superseded", "reversed"] }
        ],
        "sections": [
            {
                "heading": "Standing",
                "blurb": "In force now. These are the answers; do not re-derive them.",
                "statuses": ["accepted"],
                "order": "recent"
            },
            {
                "heading": "Proposed",
                "blurb": "Put forward, not yet settled.",
                "statuses": ["proposed"],
                "order": "recent"
            },
            {
                "heading": "No longer in force",
                "blurb": "Retired, each with the reason — say whether it was replaced or reversed, \
                          and by what. Kept rather than deleted: a decision that was reversed is \
                          exactly the one somebody will otherwise make again.",
                "statuses": ["retired"],
                "cap": 20
            }
        ],
        "checks": ["required-field", "known-status", "closed-needs-reason"]
    })
}

/// Every built-in declaration, in the order a listing shows them.
pub fn builtin_documents() -> Vec<Value> {
    vec![tasks(), goals(), decisions()]
}

/// Every built-in declaration, parsed.
///
/// # Panics
///
/// Never in practice: `builtins_are_valid` parses all of them, so a malformed
/// built-in is a compile-time-adjacent failure rather than a runtime one. A
/// built-in that will not parse is this crate's bug and is dropped with the
/// fault rather than taking the whole registry down — a company must still
/// reach its board when a *different* built-in is broken.
pub fn builtins() -> (Vec<LedgerSpec>, Vec<String>) {
    let mut specs = Vec::new();
    let mut faults = Vec::new();
    for document in builtin_documents() {
        match parse(&document, true) {
            Ok(spec) => specs.push(spec),
            Err(error) => faults.push(format!("a built-in ledger is malformed: {error}")),
        }
    }
    (specs, faults)
}

/// Every ledger one company has, built-in and declared.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    specs: Vec<LedgerSpec>,
    faults: Vec<String>,
}

impl Registry {
    /// Builds a registry from the built-ins plus a company's own declarations.
    ///
    /// A declaration that will not load is skipped **with a fault** rather than
    /// failing the whole build: a company that wrote one bad ledger must still
    /// be able to reach its board and its goals.
    pub fn build(declared: impl IntoIterator<Item = LedgerSpec>) -> Self {
        let (mut specs, mut faults) = builtins();
        let mut count = 0_usize;
        for mut spec in declared {
            spec.builtin = false;
            if let Err(error) = spec.normalize() {
                faults.push(format!("`{}` was not loaded: {error}", spec.slug));
                continue;
            }
            if let Some(reason) = rejected(&spec, &specs) {
                faults.push(format!("`{}` was not loaded: {reason}", spec.slug));
                continue;
            }
            count += 1;
            if count > MAX_DECLARED {
                faults.push(format!(
                    "`{}` was not loaded: this company already defines {MAX_DECLARED} ledgers, \
                     which is the cap. Retire one nothing reads before adding another.",
                    spec.slug
                ));
                continue;
            }
            specs.push(spec);
        }
        Self { specs, faults }
    }

    /// Every ledger, built-ins first.
    pub fn specs(&self) -> &[LedgerSpec] {
        &self.specs
    }

    /// What could not be loaded, and why.
    pub fn faults(&self) -> &[String] {
        &self.faults
    }

    /// The ledger named `slug`, if there is one.
    pub fn find(&self, slug: &str) -> Option<&LedgerSpec> {
        let slug = slug.trim().to_ascii_lowercase();
        self.specs.iter().find(|spec| spec.slug == slug)
    }

    /// The ledger named `slug`, or an error naming the ones that exist.
    ///
    /// The discovery path a model actually follows: a caller that guessed
    /// learns the real names from the failure, in one turn, without having
    /// thought to list them first.
    ///
    /// # Errors
    ///
    /// Returns [`OpenCompanyError::NotFound`] when nothing carries that slug.
    pub fn require(&self, slug: &str) -> Result<&LedgerSpec> {
        self.find(slug).ok_or_else(|| {
            OpenCompanyError::NotFound(format!(
                "there is no ledger called `{}`. This company has: {}",
                slug.trim(),
                self.slugs().join(", ")
            ))
        })
    }

    /// Every slug, for a message that has to name them.
    pub fn slugs(&self) -> Vec<String> {
        self.specs.iter().map(|spec| spec.slug.clone()).collect()
    }

    /// The ledger that owns `path`, if a ledger renders there.
    ///
    /// The workspace write guard: an edit to one of these would be erased by
    /// the next derivation, having looked like it landed. Refusing the write
    /// and naming the tool that actually writes the row is the only honest
    /// answer, and the tool differs per ledger — which is why
    /// [`LedgerSpec::written_by`] travels with the declaration rather than
    /// being guessed at the point of refusal.
    pub fn owner_of_derived(&self, path: &str) -> Option<&LedgerSpec> {
        let path = path.trim().trim_start_matches('/');
        self.specs.iter().find(|spec| spec.derived == path)
    }

    /// Whether a new declaration may join, and why not if it may not.
    ///
    /// # Errors
    ///
    /// Returns [`OpenCompanyError::InvalidRequest`] when the slug is taken, the
    /// derived path is taken, the source is native, or the cap is reached.
    pub fn admits(&self, spec: &LedgerSpec) -> Result<()> {
        if let Some(reason) = rejected(spec, &self.specs) {
            return Err(OpenCompanyError::InvalidRequest(reason));
        }
        let declared = self.specs.iter().filter(|spec| !spec.builtin).count();
        if declared >= MAX_DECLARED {
            return Err(OpenCompanyError::InvalidRequest(format!(
                "this company already defines {MAX_DECLARED} ledgers, which is the cap. Retire one \
                 nothing reads before adding another."
            )));
        }
        Ok(())
    }
}

/// Why a declaration may not join the registry, if it may not.
fn rejected(spec: &LedgerSpec, existing: &[LedgerSpec]) -> Option<String> {
    if let Some(clash) = existing.iter().find(|other| other.slug == spec.slug) {
        return Some(if clash.builtin {
            format!(
                "`{}` is a built-in ledger and cannot be redefined — every prompt and route naming \
                 it would then be wrong about what it holds.",
                spec.slug
            )
        } else {
            format!("`{}` is already a ledger on this company.", spec.slug)
        });
    }
    if let Some(clash) = existing.iter().find(|other| other.derived == spec.derived) {
        return Some(format!(
            "`{}` is already written by the `{}` ledger. Two writers on one derived file is how \
             each one's work disappears.",
            spec.derived, clash.slug
        ));
    }
    if spec.source == LedgerSource::Native {
        return Some(
            "`native` is for ledgers this runtime renders in Rust, like the task board. A ledger \
             you define keeps the `events` source, so the engine can actually read it."
                .to_string(),
        );
    }
    None
}

#[cfg(test)]
#[path = "registry_test.rs"]
mod test;
