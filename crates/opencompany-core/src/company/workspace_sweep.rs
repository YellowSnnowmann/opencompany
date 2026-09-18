//! Issue #700: remove the empty `agents/<id>/` folders a pre-#570 company is
//! still carrying, and nothing else.
//!
//! Every company provisioned before issue #570 minted one folder per roster
//! member at boot, whether or not that member ever produced anything. #570
//! stopped doing it and deliberately chose no backfill — the right call at the
//! time, and the reason there is no migration to lean on here. What changed
//! since is what those folders now *mean*:
//!
//! * #570 made a member folder mean "this teammate produced something", so on a
//!   pre-#570 tenant the two populations are indistinguishable by eye.
//! * #552 publishes deliverables into `<agent>/<task-id>/` under a member
//!   folder — `artifacts/` today, `agents/` when this was written — so an empty
//!   folder here reads as "this agent has produced nothing" — a claim, and on
//!   these tenants an unfounded one.
//! * #607 made the tree searchable with a bounded result set, so a name-matching
//!   empty folder competes for slots against notes that actually hold something.
//!
//! # The whole risk is the emptiness predicate
//!
//! The failure mode is deleting somebody's work, and the port's
//! [`delete`](WorkspaceStore::delete) is **recursive** — so a folder handed to it
//! takes its subtree with it whether or not the caller knew the subtree was
//! there. Issue #671 found exactly this bug one layer over, in the agent delete
//! tool: the tool layer's `PathIndex` omits every node it cannot address by path
//! (a name carrying a separator, a dangling or cyclic ancestor chain), so a
//! folder holding only such children looks empty by every path-shaped measure
//! while `delete` would still take them.
//!
//! [`empty_agent_folder_candidates`] therefore counts **structurally**, over
//! every node `tree()` returned, before and without any path rendering — the
//! same `parent id → child count` map #671 replaced the prefix scan with. It is
//! also exact per node id rather than per rendered path: two folders may share a
//! path, they never share an id.
//!
//! The shape is not hypothetical on the tenants this targets. The `fs` backend
//! rejects separator-carrying names at creation (`reject_unsafe_name`); the
//! sqlite and mongodb backends do not, and hosted tenants run those.
//!
//! # Why there is no authorship filter
//!
//! Stamping would be the obvious narrower predicate — sweep only what the #551
//! boot burst created — and it cannot work here. A node written before issue
//! #326 has no origin field at all and deserializes to
//! [`Operator`](crate::ports::workspace::WorkspaceOrigin::Operator), which is
//! precisely the population this exists for. Structural emptiness is the only
//! predicate that is safe on the affected tenants, and naming every folder
//! before it goes (see `dry_run`) is what keeps that honest.
//!
//! # Operator-triggered, not automatic
//!
//! Nothing here runs at boot. A tenant finding its tree changed by an upgrade it
//! did not ask for is the outcome #570 and #645 both avoided by leaving existing
//! nodes alone; the console's action is the opt-in that replaces it. The
//! functions below are a stateless function of the current tree, so running the
//! sweep twice removes nothing the second time — a property the tests assert
//! rather than assume.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::Result;
use crate::company::workspace_scaffold::{AGENTS_ROOT, Found, find};
use crate::error::OpenCompanyError;
use crate::ports::types::CompanyId;
use crate::ports::workspace::{NodeKind, WorkspaceNode, WorkspaceStore};

/// One folder the sweep removed, or would remove.
///
/// The name is the point. "Removed 17 empty folders" is a number an operator
/// cannot check; an operator who disagrees with the sweep needs to know *what*
/// went, which is why this carries the name on the preview as well as on the
/// result. The id comes along because a console that wants to drop the row it is
/// showing needs to match it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SweptFolder {
    /// The node id.
    pub id: String,
    /// The folder's name, as the tree holds it.
    pub name: String,
}

impl From<&WorkspaceNode> for SweptFolder {
    fn from(node: &WorkspaceNode) -> Self {
        Self {
            id: node.id.clone(),
            name: node.name.clone(),
        }
    }
}

/// Which folders directly under `agents/` hold nothing at all.
///
/// Pure: it decides from one tree snapshot and touches no store, which is what
/// lets the shapes that matter — an unaddressable child, a dangling chain — be
/// pinned on hand-built input that no backend would let a test create.
///
/// Three conditions, all necessary:
///
/// 1. **A direct child of the `Agents` root, by node id.** Never by rendered
///    path: the root is resolved with the scaffold's own
///    [`find`](crate::company::workspace_scaffold::find), so the sweep and the
///    scaffold cannot disagree about which node that root is.
/// 2. **A folder.** A *file* sitting directly under `agents/` is somebody's
///    note, not a stray container.
/// 3. **No children, counted structurally.** See the module docs: the count runs
///    over every node the store returned, so a child with no renderable path
///    still protects its parent.
///
/// # Errors
///
/// An ambiguous root — several nodes named `Agents`, or a *file* carrying the
/// name — is refused rather than guessed at. "Under `agents/`" has no single
/// answer there, and picking one would delete beneath a root the scaffold itself
/// refuses to touch. No root at all is not an error: there is simply nothing to
/// sweep.
pub(crate) fn empty_agent_folder_candidates(
    nodes: &[WorkspaceNode],
) -> Result<Vec<&WorkspaceNode>> {
    let root_id = match find(nodes, None, AGENTS_ROOT) {
        Found::Folder(id) => id,
        Found::Free => return Ok(Vec::new()),
        Found::Collision(why) => {
            return Err(OpenCompanyError::Conflict(format!(
                "{why}, so which folders are `{AGENTS_ROOT}/` members is undecidable; nothing \
                 was removed"
            )));
        }
    };

    // Counted before anything is filtered, and over parent ids rather than
    // rendered paths — the issue #671 measure. A child whose name carries a
    // separator has no path, is absent from every path-shaped index, and would
    // still be taken by the port's recursive delete; here it is a child like
    // any other.
    let mut child_count: HashMap<&str, usize> = HashMap::new();
    for node in nodes {
        if let Some(parent) = node.parent_id.as_deref() {
            *child_count.entry(parent).or_insert(0) += 1;
        }
    }

    Ok(nodes
        .iter()
        .filter(|node| {
            node.parent_id.as_deref() == Some(root_id.as_str())
                && node.kind == NodeKind::Folder
                && child_count
                    .get(node.id.as_str())
                    .copied()
                    .unwrap_or_default()
                    == 0
        })
        .collect())
}

/// Remove every provably empty `agents/<id>/` folder in `company`, naming what
/// went.
///
/// `dry_run` returns the candidates without touching anything, so the console
/// can name every folder on a confirm dialog before the operator agrees to lose
/// them. A real run answers with what it actually removed, which is not
/// necessarily the same list — see below.
///
/// # Two snapshots, on purpose
///
/// A real run re-reads the tree and intersects the two candidate sets by id
/// before deleting anything. Issue #552's publish path can mint a deliverable
/// into a folder at any moment, and a candidate list computed once is a claim
/// about a tree that has since moved on. Two reads do not make this atomic —
/// the port offers no transaction across `tree` and `delete`, and pretending
/// otherwise would be worse than saying so — but they narrow the window to the
/// gap between the second read and each delete, instead of leaving it open for
/// the whole preview-then-confirm round trip.
///
/// A folder that raced away entirely (`delete` answering `false`) is logged and
/// left out of the result. It is not an error: the outcome the caller asked for
/// — that folder gone — is the outcome they got.
pub async fn sweep_empty_agent_folders(
    store: &dyn WorkspaceStore,
    company: &CompanyId,
    dry_run: bool,
) -> Result<Vec<SweptFolder>> {
    let nodes = store.tree(company).await?;
    let candidates: Vec<SweptFolder> = empty_agent_folder_candidates(&nodes)?
        .into_iter()
        .map(SweptFolder::from)
        .collect();

    if dry_run {
        return Ok(candidates);
    }

    let fresh = store.tree(company).await?;
    let still_empty: HashSet<&str> = empty_agent_folder_candidates(&fresh)?
        .into_iter()
        .map(|node| node.id.as_str())
        .collect();

    let mut removed = Vec::new();
    for folder in candidates {
        if !still_empty.contains(folder.id.as_str()) {
            tracing::info!(
                company = %company,
                node = %folder.id,
                name = %folder.name,
                "[workspace] `{AGENTS_ROOT}/{}` gained a child between the preview and the \
                 sweep; leaving it alone",
                folder.name
            );
            continue;
        }
        if store.delete(company, &folder.id).await? {
            removed.push(folder);
        } else {
            tracing::info!(
                company = %company,
                node = %folder.id,
                name = %folder.name,
                "[workspace] `{AGENTS_ROOT}/{}` was already gone",
                folder.name
            );
        }
    }

    tracing::info!(
        company = %company,
        count = removed.len(),
        folders = %removed
            .iter()
            .map(|folder| folder.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        "[workspace] swept empty `{AGENTS_ROOT}/` member folders"
    );

    Ok(removed)
}

#[cfg(test)]
#[path = "workspace_sweep_tests.rs"]
mod tests;
