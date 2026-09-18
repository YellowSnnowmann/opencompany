//! The workflow copilot's thread convention, and the boundary it now carries
//! (issues #303, #416).
//!
//! The console's per-workflow copilot addresses `POST {scope}/chat` on a thread
//! id derived from the workflow: `workflow-copilot:<workflow-id>`. A manifest
//! desk id is letters, digits and underscores, so the `:` means the thread can
//! never collide with a real desk, and it never appears in `GET {scope}/desks`.
//!
//! ## Why this lives here and not in the console
//!
//! Until #416 the prefix was a console-side convention that the host merely
//! failed to recognise: the thread selected the responder (nothing matched, so
//! the orchestrator answered) and the journal thread, and **nothing else**. The
//! answer was grounded in one workflow because the console inlined that
//! workflow's graph into the message — but the teammate answering was the
//! company orchestrator, holding whole-company context and its full tool
//! surface. Asking it about the rest of the company was prevented only by a
//! sentence in the prompt, which is advice, not a boundary.
//!
//! So the host now reads the thread itself. [`workflow_of_thread`] is the one
//! place that decides "this is a copilot turn", and every confinement keys off
//! it:
//!
//! * the harness runs the turn on a **confined agent** — no tools, no company
//!   memory, no delegation (see `harness::confine`);
//! * the chat handler does not open a board card from a copilot message, so a
//!   question phrased as a request ("add a node that emails the report") cannot
//!   leave work on the company's board.
//!
//! Nothing here is an authorization check and it must not be read as one: an
//! operator who can open the copilot can already address the orchestrator from
//! the Chat tab with the same authority. This narrows what one *turn* reaches,
//! which is what makes an answer about a workflow an answer about that workflow
//! and nothing else.

/// The prefix that marks a chat thread as one workflow's copilot.
pub const COPILOT_THREAD_PREFIX: &str = "workflow-copilot:";

/// The workflow a chat thread is the copilot for, or `None` for every ordinary
/// thread (a desk, a teammate DM, an unaddressed message).
///
/// The id is returned trimmed, and an empty id is **not** a copilot thread: a
/// bare `workflow-copilot:` names no workflow, so there is nothing to confine a
/// turn *to*, and treating it as a copilot turn would give the confinement a
/// blank subject to describe.
pub fn workflow_of_thread(chat: Option<&str>) -> Option<&str> {
    let id = chat?.strip_prefix(COPILOT_THREAD_PREFIX)?.trim();
    (!id.is_empty()).then_some(id)
}

/// Whether a chat thread is a workflow copilot's.
pub fn is_copilot_thread(chat: Option<&str>) -> bool {
    workflow_of_thread(chat).is_some()
}

#[cfg(test)]
#[path = "copilot_tests.rs"]
mod tests;
