//! The wire types a ledger is written and read through.
//!
//! They live in the core rather than on the port because the fold defines them:
//! an event *is* a set of field assignments against an id, and the port's only
//! job is to keep them in order.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Who recorded something.
///
/// Not decoration, and not only provenance. It is the authority the delete
/// guard reads: **an agent may create a ledger and record into it; only a
/// person may delete.** A row whose author is unrecorded is a row nothing can
/// hold to that rule afterwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorKind {
    /// A person, through the console or the API with a user session.
    Human,
    /// A teammate's turn, through a ledger tool.
    #[default]
    Agent,
    /// The runtime itself — a settle, a sweep, a migration.
    System,
}

impl AuthorKind {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
            Self::System => "system",
        }
    }

    /// Whether this author may delete — a ledger, or a row on one.
    ///
    /// **Only a person.** An agent's whole relationship with a ledger is
    /// additive: it opens rows, amends them, and closes them with a reason, and
    /// every one of those is recoverable by reading the log. Deletion is not,
    /// and a runtime where a turn can erase the record of what it did is one
    /// whose record means nothing. Closing a row is the agent-shaped version of
    /// being finished with it, and it keeps the reason.
    ///
    /// [`System`](Self::System) is deliberately not exempt: a sweep that could
    /// delete rows is the same loss with no author to ask about it.
    pub fn may_delete(self) -> bool {
        matches!(self, Self::Human)
    }
}

/// Who recorded an event, in enough detail to render a byline.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerAuthor {
    /// Whether a person, a teammate or the runtime wrote it.
    pub kind: AuthorKind,
    /// The user id, teammate id, or subsystem name. Empty when unattributed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// What a reader is shown — a display name, a desk label. Falls back to
    /// [`id`](Self::id) when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

impl LedgerAuthor {
    /// A teammate author.
    pub fn agent(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            kind: AuthorKind::Agent,
            label: id.clone(),
            id,
        }
    }

    /// A person author.
    pub fn human(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            kind: AuthorKind::Human,
            id: id.into(),
            label: label.into(),
        }
    }

    /// The runtime itself.
    pub fn system(what: impl Into<String>) -> Self {
        let what = what.into();
        Self {
            kind: AuthorKind::System,
            label: what.clone(),
            id: what,
        }
    }

    /// The byline a rendered row carries.
    pub fn byline(&self) -> String {
        let name = if self.label.trim().is_empty() {
            self.id.trim()
        } else {
            self.label.trim()
        };
        if name.is_empty() {
            self.kind.as_str().to_string()
        } else {
            format!("{name} ({})", self.kind.as_str())
        }
    }
}

/// One append-only write against one entry.
///
/// # Everything is a merge
///
/// There is one write operation, and opening, amending, blocking, closing and
/// re-prioritising a row are all it. An event names an entry and carries some
/// fields; the fold applies them in order; absent keys are left alone. Closing
/// a task is `{status: "done", reason: "shipped"}` and nothing else.
///
/// That is not simplification for its own sake. A vocabulary of operations
/// means a vocabulary of *inverses* to get wrong — what `unblock` does to a row
/// that was never blocked, whether `reopen` restores the old status or the
/// default — and every one of those is a decision the fold would have to make
/// silently. A merge has no inverse to get wrong, and the log keeps the whole
/// history either way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEvent {
    /// The ledger's slug.
    pub ledger: String,
    /// The entry this event names. Reusing an id amends that entry.
    pub id: String,
    /// Who wrote it.
    pub author: LedgerAuthor,
    /// When, in epoch millis.
    pub at_millis: u64,
    /// The fields to set. `None` **clears** a field, which is the one thing a
    /// merge cannot otherwise express; everything else is a set.
    ///
    /// A `BTreeMap` so a stored event round-trips in a stable key order and two
    /// backends cannot disagree about the bytes.
    #[serde(default)]
    pub fields: BTreeMap<String, Option<String>>,
}

impl LedgerEvent {
    /// A field's assigned value on this event, if it assigns one.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.fields.get(name).and_then(Option::as_deref)
    }
}
