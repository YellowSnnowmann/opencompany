//! What the desk remembers between episodes, and the seam it remembers through.
//!
//! An episode is the only durable thing a room produces, and until now the only
//! place it went was the desk transcript — a thirty-message window that the next
//! similar question scrolls straight past. The live evidence: nine episodes of
//! `companies/hive_math_lab` on Project Euler produced **two** memory writes
//! between them, because the writes came from whichever teammate happened to
//! call `memory_store` inside its own turn, and a member spending its one line
//! on a marker rarely does.
//!
//! So the driver owns this rather than the members. It recalls once, before the
//! first turn, and renders what came back into every prompt of the episode; and
//! it writes exactly one note when the episode ends. Neither is a member's
//! choice, which is what makes it happen at all.
//!
//! # Best-effort, both directions
//!
//! A memory failure is logged and swallowed. An episode that reached a decision
//! has already made it durable in the transcript, and losing the note is worth
//! strictly less than failing the operator's message over a store that was
//! briefly unreachable. A recall that fails renders no block, which is exactly
//! what a cold store renders.
//!
//! # Why a trait and not the port
//!
//! [`crate::ports::ContextStore`] is where this actually lands, but the driver
//! is a pure fold over a journal and a turn seam — it holds no ports, and a
//! module that compiles in every build should not start holding one for this.
//! The trait is two functions wide, [`NullHiveMemory`] satisfies it, and the
//! real implementation lives in the brain beside the memory loop whose calls it
//! reuses.

use async_trait::async_trait;

use crate::Result;

/// Label prefix every desk note is stored under.
///
/// Namespaced by desk exactly as agent memories are namespaced by agent
/// (`agent-memory/<agent id>/<slug>`, see
/// `src/harness/built_in/memory_tools.rs`): the full label is
/// `hive/<desk id>/<slug>`, so one desk's deliberations are listable on their
/// own and never collide with another desk's or with a teammate's private
/// memories.
pub const HIVE_MEMORY_LABEL_PREFIX: &str = "hive";

/// How many recalled notes a prompt will carry.
pub const RECALL_LIMIT: usize = 3;

/// Max characters of any one recalled note rendered into a prompt.
pub const MAX_RECALL_CHARS: usize = 320;

/// The label prefix that scopes every note on one desk: `hive/<desk id>/`.
///
/// Trailing slash on purpose — it is used as a namespace rather than a string
/// prefix, so `hive/eng/` cannot match `hive/engineering/`.
#[must_use]
pub fn desk_prefix(desk_id: &str) -> String {
    format!("{HIVE_MEMORY_LABEL_PREFIX}/{desk_id}/")
}

/// The full label one note is stored under.
#[must_use]
pub fn note_label(desk_id: &str, title: &str) -> String {
    format!("{}{}", desk_prefix(desk_id), slug(title))
}

/// A short, stable, filesystem-safe name for a note title.
///
/// A local copy of the agent-memory slug rather than a shared one: this module
/// compiles in every build and `memory_tools` does not, so sharing the helper
/// would drag a harness-gated module into the default build for eight lines.
#[must_use]
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            out.push(character);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= 64 {
            break;
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "note".to_owned()
    } else {
        out
    }
}

/// One thing the desk remembers, as it will be rendered into a prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HiveMemoryHit {
    /// The remembered text. Rendered attributed as memory, never as a
    /// transcript line: it carries no sequence, so it can never be cited with
    /// `^N`, and a member that tried would be citing a message that does not
    /// exist in this conversation.
    pub snippet: String,
}

/// One note the desk writes when an episode ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HiveMemoryNote {
    /// The desk this happened on — the namespace half of the label.
    pub desk_id: String,
    /// A short name for the note; becomes the label's slug and its first line.
    pub title: String,
    /// The note itself.
    pub body: String,
}

/// The desk's own memory: what earlier episodes concluded, and where this one
/// goes.
#[async_trait]
pub trait HiveMemory: Send + Sync {
    /// Up to `limit` things this desk remembers that bear on `query`.
    ///
    /// # Errors
    ///
    /// Whatever the underlying store failed with. The driver logs it and
    /// renders no recall block, which is what a cold store renders anyway.
    async fn recall(&self, query: &str, limit: usize) -> Result<Vec<HiveMemoryHit>>;

    /// Write one note.
    ///
    /// # Errors
    ///
    /// Whatever the underlying store failed with. The driver logs it and
    /// finishes the episode: the decision is already durable in the transcript.
    async fn remember(&self, note: HiveMemoryNote) -> Result<()>;
}

/// A desk with no memory wired: recalls nothing, remembers nothing, succeeds.
///
/// The default the driver runs under, so a caller that has no context store —
/// every test that is not about memory, and any build that wires none — gets an
/// episode byte-identical to the one it got before this existed.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullHiveMemory;

#[async_trait]
impl HiveMemory for NullHiveMemory {
    async fn recall(&self, _query: &str, _limit: usize) -> Result<Vec<HiveMemoryHit>> {
        Ok(Vec::new())
    }

    async fn remember(&self, _note: HiveMemoryNote) -> Result<()> {
        Ok(())
    }
}
