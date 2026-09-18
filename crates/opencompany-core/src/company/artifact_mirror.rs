//! The seam between a task artifact and the shared workspace tree (issue #552,
//! folding in #327's missing push channel).
//!
//! # The problem this closes
//!
//! `publish_artifact` used to drain into the [`ArtifactStore`] and stop. An
//! artifact is reachable from exactly one place — the Artifacts tab of one
//! card — so a deliverable an agent explicitly published was invisible to the
//! operator browsing the workspace and to every *other* agent, whose only view
//! of shared company state is the note tree. "The CMO wrote the launch brief"
//! had no answer anyone could navigate to.
//!
//! # Two surfaces, one truth
//!
//! A published deliverable now lives twice, and the split is deliberate:
//!
//! * The **artifact chain is authoritative**. It holds the full version
//!   history, the authorship of each revision, and therefore
//!   [`ArtifactRecord::human_edit_diff`] — the one quality datum the artifact
//!   port exists to produce.
//! * The **workspace node is a projection** holding the *current* body only.
//!   It is what makes the deliverable browsable and readable by teammates.
//!
//! The rejected alternative was to make the node the storage and have the
//! artifact reference it. That would push versioning down into
//! [`WorkspaceStore`] across all three backends, turn every artifact read into
//! a two-store join, and re-open the `(task_id, source)` identity contract that
//! #244 settled. A projection costs one extra write; the inversion costs the
//! port.
//!
//! **The invariant**: `node.body == chain.latest().body` after any successful
//! write on either surface.
//!
//! # Ordering: chain first, wherever there is a choice
//!
//! Every path here writes the chain before the node when it can. The two
//! failure modes are not symmetric:
//!
//! * Chain ahead of node — a stale node. Visible, harmless, and self-healing:
//!   the next write on either surface reconciles it.
//! * Node ahead of chain — an edit to a published deliverable that the version
//!   history never recorded. That is silent, permanent, and corrupts
//!   `human_edit_diff`, which is the exact rot the artifact port was built to
//!   prevent.
//!
//! So a failed mirror is logged and tolerated in the first direction and
//! avoided in the second. One path cannot have it: the agent's
//! `workspace_write` tool must complete its compare-and-swap before it knows
//! the write landed at all, so there the node necessarily moves first. It is
//! the narrowest window available rather than a different policy.
//!
//! # The guarantee is owed to deliverables, not to every note
//!
//! "Avoided in the second direction" is a promise about *published* nodes, and
//! it costs something to keep: the reverse lookup runs on every save, so a
//! strict reading would make an unreachable artifact store refuse edits to
//! ordinary notes too — notes with no chain to corrupt, on a save that
//! otherwise never touches that store. That trades the whole tree's
//! availability for a guarantee none of it is owed.
//!
//! [`mirror_node_edit`] therefore separates *cannot record* from *cannot tell*
//! (see [`MirrorOutcome`]). Its callers keep failing closed once a node is
//! known to be a deliverable, and choose for themselves what an unanswerable
//! store means. The console `PUT` takes the availability side and says so in
//! its own doc, including what that costs when the store is down.
//!
//! # Why this module is in the default build
//!
//! [`mirror_node_edit`] has three callers across two layers — the console's
//! workspace `PUT` and artifact-append routes (`src/server/ops/`, always
//! compiled) and the agent's `workspace_write` tool (`src/harness/`, compiled
//! only under the `openhuman` feature). The shared half therefore cannot live
//! in the harness, or the default build could not reach it.

use sha2::{Digest, Sha256};

use crate::Result;
use crate::error::OpenCompanyError;
use crate::ports::artifacts::{ArtifactAuthor, ArtifactRecord, ArtifactStore};
use crate::ports::now_millis;
use crate::ports::types::CompanyId;
use crate::ports::workspace::{NodeKind, WorkspaceNode, WorkspaceOrigin, WorkspaceStore};

use super::workspace_names::{MAX_NAME_BYTES, kebab_name, kebab_name_or};
use super::workspace_scaffold::{ensure_artifact_folder_tracked, rollback_empty_minted_folders};

/// One publish, as [`materialize`] needs it.
///
/// A struct rather than seven positional parameters: five of the fields are
/// `&str`, so a call site that transposed `task_id` and `source` would compile
/// perfectly and file every deliverable in the wrong folder.
#[derive(Debug, Clone, Copy)]
pub struct PublishTarget<'a> {
    /// The agent that published this file — the owner of the `agents/<id>/`
    /// folder it lands under, and the authorship stamped on every node created
    /// or written along the way.
    pub agent_id: &'a str,
    /// The card the publish belongs to. Its id is the immutable half of the
    /// folder name beneath the agent's, so two tasks by one agent cannot
    /// collide on a common filename — and so the folder stays findable by the
    /// id an operator holds.
    pub task_id: &'a str,
    /// The card's human title, when the caller has one (issue #1687).
    ///
    /// The readable half of that folder's name. `None` — a caller with no
    /// board record to hand — names the folder by the id alone, which is what
    /// every folder was called before this.
    pub task_title: Option<&'a str>,
    /// The normalized workspace-relative path the agent published, e.g.
    /// `specs/launch.md`. Interior segments become folders.
    pub source: &'a str,
    /// What to store — the file's text, or its bytes (issue #553). Text lands
    /// as an ordinary note; bytes land as a binary node, which is what stopped
    /// a paid image generation from becoming a dangling digest.
    pub payload: MirrorPayload<'a>,
    /// The node the previous version of this artifact was mirrored into, when
    /// there was one. Reused if it still resolves; see [`materialize`].
    pub existing_node_id: Option<&'a str>,
}

/// One card-less workflow-run artifact.
///
/// Runs cannot use [`PublishTarget`] directly because its second path segment
/// is a task id and a workflow agent node has no card. This target preserves
/// the same author/source/payload contract while giving the mirror the two ids
/// that make the destination unique within a run.
#[derive(Debug, Clone, Copy)]
pub struct RunTarget<'a> {
    /// The roster agent whose sandbox produced the file.
    pub agent_id: &'a str,
    /// The workflow run that owns the capture.
    pub run_id: &'a str,
    /// The graph node whose turn wrote it.
    pub node_id: &'a str,
    /// The normalized path relative to that agent's workspace.
    pub source: &'a str,
    /// The captured file body.
    pub payload: MirrorPayload<'a>,
}

/// What [`materialize`] is being asked to put in the tree.
///
/// Borrowed rather than owned: the drain already holds the bytes it read, and a
/// copy of a 200 MiB video to cross one function boundary would be the single
/// largest allocation on the publish path.
#[derive(Debug, Clone, Copy)]
pub enum MirrorPayload<'a> {
    /// Prose — an editable, diffable, backlinkable note.
    Text(&'a str),
    /// Opaque bytes, with the media type the publisher inferred.
    Bytes {
        /// The file's contents, written verbatim.
        bytes: &'a [u8],
        /// The media type to store the node under.
        mime: &'a str,
    },
}

/// What a publish left in the tree: the node holding it, and — for bytes — the
/// digest **the store computed** while writing it (issue #668).
///
/// The digest is `None` for prose, whose version body is the content itself, so
/// there is nothing a hash would add. For bytes it is the only thing that tells
/// two versions of one deliverable apart, and it comes back from
/// [`WorkspaceStore::create_binary`] / [`write_binary`](WorkspaceStore::write_binary)
/// rather than being computed here — see those methods for why the provenance
/// matters more than the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mirrored {
    /// The workspace node now holding the deliverable.
    pub node_id: String,
    /// The store's `sha256` of the bytes it wrote, when the payload was bytes.
    pub sha256: Option<String>,
}

/// Put `target`'s body into the shared tree and return what it left there.
///
/// The layout is `artifacts/<agent-id>/<task-title>.<task-id>/<source…>`, the
/// task folder named by [`task_folder_name`] — readable half first, id last so
/// the folder is still findable by the id an operator holds (issue #1687). The
/// agent's folder
/// beneath that root is minted on demand by
/// [`ensure_artifact_folder`](super::workspace_scaffold::ensure_artifact_folder)
/// — member folders appear the first time somebody publishes something, so this
/// must **call** it rather than assume it exists.
///
/// It used to be `agents/<agent-id>/<task-id>/…`, which filed a deliverable in
/// the same folder as its author's scratch notes. Nothing migrates: a record
/// carrying an `existing_node_id` still revises the node it already has, so a
/// company that published before this change keeps its old nodes and its
/// console deep links, and only new paths land under `artifacts/`. A migration
/// would have to move nodes an operator may have organised by hand, to fix
/// something that is untidy rather than wrong.
///
/// # Interior path segments become folders
///
/// `specs/launch.md` lands as `…/<task-id>/specs/launch.md`, not as a file
/// literally named `specs/launch.md`. Flattening to the basename would make
/// `specs/a.md` and `docs/a.md` — two genuinely different deliverables of one
/// task — collide on one node and overwrite each other.
///
/// # Re-publish reuses the node, unless the operator removed it
///
/// `existing_node_id` is reused when it still resolves to a file, so a second
/// publish of the same path revises the note the operator has been reading
/// rather than opening a rival beside it. When it is absent (a pre-#552 record)
/// or no longer resolves (the operator deleted it, and deletions stick), a
/// fresh node is materialized and the *new* version carries the new id. Older
/// versions keep the id of the node that actually held them — honest history,
/// the same shape as `run_id`.
///
/// # Ambiguity is refused, never guessed
///
/// Identity here is by path and no backend enforces unique sibling names, so
/// every lookup is check-then-act. A name carried by a node of the wrong kind,
/// or by more than one node, is a [`Conflict`](OpenCompanyError::Conflict)
/// rather than a coin flip — the same fail-closed rule
/// [`workspace_scaffold`](super::workspace_scaffold) applies one level up.
pub async fn materialize(
    workspace: &dyn WorkspaceStore,
    company: &CompanyId,
    target: PublishTarget<'_>,
) -> Result<Mirrored> {
    // The cheap path, and the common one on a re-publish: the node from last
    // time still exists, so revise it in place and keep every reference to it
    // (the console's deep link, an operator's bookmark) working.
    //
    // A node whose *shape* changed — a markdown draft re-exported as a PDF, or
    // a PDF replaced by prose — cannot be revised in place, because neither
    // write path will convert one kind of node into the other (and the store
    // refuses if asked). Falling through to the path resolution below mints a
    // fresh node of the right kind, which is the same answer this function
    // already gives when the operator deleted the old one: the new version
    // carries the new id, older versions keep the id that actually held them.
    if let Some(existing) = target.existing_node_id
        && let Some((node, _)) = workspace.read(company, existing).await?
        && node.kind == NodeKind::File
        && node.is_binary() == matches!(target.payload, MirrorPayload::Bytes { .. })
    {
        let sha256 = write_payload(workspace, company, existing, target).await?;
        return Ok(Mirrored {
            node_id: existing.to_string(),
            sha256,
        });
    }

    // From here a publish may mint folders before the write that justifies
    // them exists. Track the ones this call freshly created so that a write
    // which then fails does not leave an empty `artifacts/<agent>/…` skeleton
    // standing (issue #1801) — the residual, non-race half of the empty folders
    // the Tidy(#700)/Repair(#759) buttons otherwise have to sweep. The cleanup
    // runs only on error, and removes only a folder that is still empty, so a
    // concurrent publisher that adopted one of these is never disturbed.
    let mut minted: Vec<String> = Vec::new();
    match materialize_fresh(workspace, company, target, &mut minted).await {
        Ok(mirrored) => Ok(mirrored),
        Err(err) => {
            rollback_empty_minted_folders(workspace, company, &minted).await;
            Err(err)
        }
    }
}

/// The path-resolving, folder-minting half of [`materialize`], for a deliverable
/// with no reusable node.
///
/// Split out so [`materialize`] can undo the folders it minted when the write
/// that would have filled them fails (issue #1801). Every freshly created folder
/// id is pushed onto `minted`: the agent folder here, plus each task or interior
/// folder [`resolve_task_folder`] and [`resolve_folder`] mint. The caller cleans
/// them up only on error and only while still empty — see
/// [`rollback_empty_minted_folders`].
async fn materialize_fresh(
    workspace: &dyn WorkspaceStore,
    company: &CompanyId,
    target: PublishTarget<'_>,
    minted: &mut Vec<String>,
) -> Result<Mirrored> {
    let segments = split_source(target.source)?;
    let (dirs, filename) = segments
        .split_last()
        .map(|(last, rest)| (rest, last.as_str()))
        .expect("split_source rejects an empty path");

    let (agent_folder, created) =
        ensure_artifact_folder_tracked(workspace, company, target.agent_id).await?;
    if created {
        minted.push(agent_folder.clone());
    }

    // One tree read, then a walk that keeps its own view current: each folder
    // this creates is pushed onto `nodes`, so a `specs/deep/note.md` resolves
    // its second segment against the first segment it just minted rather than
    // against a snapshot that predates it.
    let mut nodes = workspace.tree(company).await?;
    let mut parent = agent_folder;
    parent = resolve_task_folder(
        workspace,
        company,
        &mut nodes,
        minted,
        &parent,
        target.task_id,
        target.task_title,
        target.agent_id,
    )
    .await?;
    for name in dirs.iter().map(String::as_str) {
        parent = resolve_folder(
            workspace,
            company,
            &mut nodes,
            minted,
            &parent,
            name,
            target.agent_id,
        )
        .await?;
    }

    match resolve_file(&nodes, &parent, filename)? {
        // A node is already there under this exact path — an earlier publish
        // whose id we lost, or a note the agent wrote by hand. Revising it is
        // the only non-destructive answer: minting a rival would leave the path
        // permanently ambiguous, which the tool layer's resolver then refuses
        // for every agent.
        Some(id) => {
            // The same shape guard as above: a node already at this path whose
            // kind disagrees with what is being published cannot be revised,
            // because no write path converts one kind into the other (and the
            // store refuses if asked). It has to be replaced.
            let replace = match workspace.read(company, &id).await? {
                Some((node, _)) => {
                    node.is_binary() != matches!(target.payload, MirrorPayload::Bytes { .. })
                }
                None => false,
            };
            if replace {
                return replace_payload(workspace, company, &parent, filename, &id, target).await;
            }
            let sha256 = write_payload(workspace, company, &id, target).await?;
            Ok(Mirrored {
                node_id: id,
                sha256,
            })
        }
        // Nothing at this path — *as of the read above*. Issue #697: that read
        // is not a claim, so two first publishes of one deliverable both land
        // here and, before this, both created. Two nodes, one name.
        //
        // The state does not decay. `resolve_file` answers a duplicated name
        // with `Conflict`, so a race that lasted microseconds refuses every
        // future publish to that deliverable, for every agent, until somebody
        // edits the tree by hand.
        None => create_first(workspace, company, &parent, filename, target).await,
    }
}

/// A workflow node id's path segment within a run's artifact folder.
///
/// `kebab_name_or` is not injective — `write_up` and `write-up` both normalize
/// to `write-up` — but workflow validation only requires raw node ids to be
/// unique, not their kebab form. Two nodes that collide there would otherwise
/// resolve to the same `materialize_run` destination, and the later capture
/// would silently overwrite the earlier node's output. Appending a short
/// stable hash of the RAW id (computed before normalization) makes the
/// segment collision-resistant while keeping the kebab prefix for
/// readability in the workspace tree.
fn run_node_segment(node_id: &str) -> String {
    let kebab = kebab_name_or(node_id, node_id);
    let digest = Sha256::digest(node_id.as_bytes());
    let mut suffix = String::with_capacity(8);
    for byte in digest.iter().take(4) {
        use std::fmt::Write as _;
        let _ = write!(suffix, "{byte:02x}");
    }
    format!("{kebab}-{suffix}")
}

/// Files a card-less workflow-node output into the shared workspace tree.
///
/// The layout is `artifacts/<agent>/runs/<run>/<node>-<hash>/<source…>`.
/// Reusing [`materialize`] keeps the same path validation, conflict handling,
/// binary storage, and atomic create semantics as task artifacts while the
/// `runs` segment prevents a run id from being mistaken for a task id.
pub async fn materialize_run(
    workspace: &dyn WorkspaceStore,
    company: &CompanyId,
    target: RunTarget<'_>,
) -> Result<Mirrored> {
    let run = kebab_name_or(target.run_id, target.run_id);
    let node = run_node_segment(target.node_id);
    let source = format!("{run}/{node}/{}", target.source);
    materialize(
        workspace,
        company,
        PublishTarget {
            agent_id: target.agent_id,
            task_id: "runs",
            task_title: None,
            source: &source,
            payload: target.payload,
            existing_node_id: None,
        },
    )
    .await
}

/// Publishes a deliverable to a path that nothing occupies yet, and loses
/// rather than duplicates if that stops being true (issue #697).
///
/// # Why this is not just `create_payload`
///
/// It was, and that is the defect. `resolve_file` returning `None` is a
/// statement about the instant it read the tree; a plain create acts on it
/// later, and two publishers that both read "free" both created. The window is
/// small and the damage is permanent, which is the worst combination — nothing
/// cleans up after it and every later publish is refused.
///
/// # The shape is `replace_payload`'s, deliberately
///
/// Stage under a name no publish can produce and [`resolve_file`] will never
/// match, then ask the store to install it conditionally. The only difference
/// is what the caller expects to find: a republish names the node it supersedes,
/// a first publish asserts the name is still free. One primitive answers both
/// (see [`WorkspaceStore::swap_files`]), which is what keeps the loser-cleanup
/// rule — consume the staged node, payload included — in one place rather than
/// two that drift.
///
/// Staging costs the same quota it costs a republish: the payload is charged
/// while it is staged, so a company at its ceiling can be refused here. That is
/// the trade #662 already argued, and a refusal leaves nothing behind.
async fn create_first(
    workspace: &dyn WorkspaceStore,
    company: &CompanyId,
    parent: &str,
    filename: &str,
    target: PublishTarget<'_>,
) -> Result<Mirrored> {
    let staged_name = format!("{filename}.publishing-{}", crate::ports::generate_id());
    let staged = create_payload(
        workspace,
        company,
        Some(parent.to_string()),
        &staged_name,
        target,
    )
    .await?;

    match workspace
        .swap_files(company, None, &staged.node_id, filename)
        .await
    {
        Ok(Some(node)) => Ok(Mirrored {
            node_id: node.id,
            sha256: node.sha256,
        }),
        Ok(None) => Err(OpenCompanyError::Conflict(format!(
            "the deliverable at `{filename}` was created by another publish while this one was \
             being prepared; nothing was overwritten — publish again to revise it"
        ))),
        Err(err) => {
            // The same reasoning as the republish path: an indeterminate store
            // error may or may not have committed, so the staging id is logged
            // for recovery rather than deleted.
            tracing::error!(
                company = %company,
                staged = %staged.node_id,
                name = %staged_name,
                error = %err,
                "[publish] the store could not decide the staged first publish; its id is logged \
                 for recovery rather than deleted after an indeterminate write"
            );
            Err(err)
        }
    }
}

/// Overwrites `node_id` with whatever `target` carries, on the matching path.
async fn write_payload(
    workspace: &dyn WorkspaceStore,
    company: &CompanyId,
    node_id: &str,
    target: PublishTarget<'_>,
) -> Result<Option<String>> {
    match target.payload {
        MirrorPayload::Text(body) => {
            workspace
                .write(company, node_id, body, origin(target.agent_id))
                .await?;
            Ok(None)
        }
        MirrorPayload::Bytes { bytes, mime } => {
            let node = workspace
                .write_binary(company, node_id, bytes, Some(mime), origin(target.agent_id))
                .await?;
            Ok(node.sha256)
        }
    }
}

/// Replaces the node at a path with one of the **other** kind — prose becoming
/// bytes, or the reverse — without a window in which the deliverable does not
/// exist (issue #662).
///
/// # Why not delete-then-create
///
/// That is what this used to do, and it turned a *refused* publish into a
/// destructive one. `create_payload` fails for designed reasons — over
/// `max_blob_mb`, over `tree_quota_gb`, a store error — and quota refusal is an
/// intended outcome of the same work that introduced this path. When it failed,
/// the old deliverable had already been deleted and nothing restored it: the
/// operator was left with an artifact record pointing at a node id that no
/// longer resolved, and the previous deliverable — which was fine — destroyed by
/// a publish that did not succeed.
///
/// # The staging window costs quota, and that changes who succeeds
///
/// The replacement is minted while the superseded node still exists, so both
/// payloads are charged against `tree_quota_gb` until the swap. Delete-first
/// freed the old bytes before asking for the new ones. A company close to its
/// quota republishing a large deliverable is therefore **refused where it
/// previously succeeded** — the end state of a successful publish is unchanged,
/// but which publishes succeed is not.
///
/// That is the right trade, and it is the same one the rest of this doc argues:
/// a refusal leaves the previous deliverable intact and is recoverable by
/// raising the quota or deleting something, where the old behaviour destroyed
/// it. Recorded here because the operator-visible symptom — a quota error on a
/// republish of something that already fits — is otherwise unexplainable.
///
/// The old code argued the deletion was safe because "its history lives on the
/// artifact chain". That holds when the replacement lands and not otherwise, and
/// nothing distinguished the two. It is also **false for a binary**, whose
/// artifact version records neither content nor digest (issue #668) — so the
/// chain recovers nothing. Minting first removes the need for that argument
/// rather than working around it.
///
/// # Why the replacement is staged under another name
///
/// The obvious create-then-delete — mint the replacement at the final path,
/// then remove the old node — briefly puts **two** nodes at one path, and if the
/// delete then fails they stay there. [`resolve_file`] answers a duplicated name
/// with `Conflict`, so that state does not decay: it refuses every future
/// publish to that path, for every agent. Staging under a name nothing resolves
/// keeps the path unambiguous at every instant.
///
/// # The store owns the compare-and-swap
///
/// * **The create fails** — the common case, and the one this issue is about.
///   Nothing has changed: the old deliverable is intact, the path still resolves
///   to it, and the error propagates. Publishing is refused rather than
///   destructive.
/// * **Another publisher wins** — [`WorkspaceStore::swap_files`] consumes this
///   publisher's staging node and returns `None`; the caller receives a conflict
///   and the final path still names exactly the winner.
/// * **The store fails** — the old node remains the compare-and-swap authority.
///   The staging id is logged rather than blindly deleted: a distributed store
///   error can be an indeterminate response to a committed write, and deleting
///   that id could destroy the successful replacement.
async fn replace_payload(
    workspace: &dyn WorkspaceStore,
    company: &CompanyId,
    parent: &str,
    filename: &str,
    superseded: &str,
    target: PublishTarget<'_>,
) -> Result<Mirrored> {
    // A name no publish can produce and `resolve_file` will never match, so the
    // path keeps resolving to exactly one node while the swap is in flight.
    let staged_name = format!("{filename}.publishing-{}", crate::ports::generate_id());
    let staged = create_payload(
        workspace,
        company,
        Some(parent.to_string()),
        &staged_name,
        target,
    )
    .await?;

    match workspace
        // `Some`, emphatically: this is a republish, and it must lose if the
        // node it expected to supersede is no longer the one at the path.
        // `None` here would mean "install only if the name is free", which for
        // a path that is by definition occupied would refuse every republish.
        .swap_files(company, Some(superseded), &staged.node_id, filename)
        .await
    {
        Ok(Some(node)) => Ok(Mirrored {
            node_id: node.id,
            sha256: node.sha256,
        }),
        Ok(None) => Err(OpenCompanyError::Conflict(format!(
            "the deliverable at `{filename}` was replaced by another publish while this one \
             was being prepared; nothing was overwritten — publish again"
        ))),
        Err(err) => {
            tracing::error!(
                company = %company,
                staged = %staged.node_id,
                name = %staged_name,
                error = %err,
                "[publish] the store could not decide the staged replacement; its id is logged \
                 for recovery rather than deleted after an indeterminate write"
            );
            Err(err)
        }
    }
}

/// Creates a fresh node of the right kind holding `target`'s payload.
async fn create_payload(
    workspace: &dyn WorkspaceStore,
    company: &CompanyId,
    parent: Option<String>,
    filename: &str,
    target: PublishTarget<'_>,
) -> Result<Mirrored> {
    let mut node = WorkspaceNode {
        id: crate::ports::generate_id(),
        name: filename.to_string(),
        kind: NodeKind::File,
        parent_id: parent,
        updated_at_millis: now_millis(),
        created_by: origin(target.agent_id),
        updated_by: origin(target.agent_id),
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    };
    match target.payload {
        MirrorPayload::Text(body) => {
            workspace.create(company, &node, Some(body)).await?;
            Ok(Mirrored {
                node_id: node.id,
                sha256: None,
            })
        }
        MirrorPayload::Bytes { bytes, mime } => {
            node.mime = Some(mime.to_string());
            let stamped = workspace.create_binary(company, &node, bytes).await?;
            Ok(Mirrored {
                node_id: stamped.id,
                sha256: stamped.sha256,
            })
        }
    }
}

/// Record an edit to `node_id` on the artifact chain that owns it, when one
/// does.
///
/// The reverse lookup that keeps the two surfaces from diverging: a workspace
/// node the operator (or an agent) rewrites may be a *published deliverable*,
/// and an edit to one that never reached the version history is exactly the
/// silent corruption the artifact port exists to prevent.
///
/// Answers [`MirrorOutcome::Ordinary`] — and touches nothing — when `node_id`
/// names an ordinary note. Most of the tree is ordinary notes, so this is the
/// common answer and deliberately not an error.
///
/// # Two failures, told apart on purpose
///
/// The lookup and the append fail for different reasons and are not returned
/// alike. A failed **append** is an `Err`: the store answered, so this node is
/// known to be a published deliverable, and the caller must not write the node
/// behind a version that was never recorded. A failed **lookup** is
/// [`MirrorOutcome::Undetermined`] inside `Ok`, because it establishes nothing
/// — the node may be a deliverable or may be one of the ordinary notes that
/// are nearly the whole tree, and only the caller knows whether its own work
/// can proceed without that answer.
///
/// Collapsing the second into [`MirrorOutcome::Ordinary`] would read as "no
/// chain here, carry on" on every store fault, which is precisely how the
/// fail-closed guarantee for deliverables would stop applying without anything
/// appearing to change.
///
/// # The scan, named rather than hidden
///
/// This lists the company's artifacts and looks for one whose *latest* version
/// carries `node_id`. That is a linear scan per save. It is bounded by what
/// artifacts are — a task's drafts and posts, not a repository — and buying an
/// index before there is a workload to size it against would be guessing. The
/// latest version rather than any version is the point: an operator's deletion
/// of a node sticks, so an old version's id names a node that is gone, and
/// matching on it would mirror today's edit into yesterday's history.
pub async fn mirror_node_edit(
    artifacts: &dyn ArtifactStore,
    company: &CompanyId,
    node_id: &str,
    body: &str,
    author: ArtifactAuthor,
    author_id: &str,
    note: Option<String>,
) -> Result<MirrorOutcome> {
    let mut record = match published_record_for_node(artifacts, company, node_id).await {
        Ok(Some(record)) => record,
        Ok(None) => return Ok(MirrorOutcome::Ordinary),
        Err(err) => return Ok(MirrorOutcome::Undetermined(err)),
    };
    let version = record.push_version(body, author, author_id, now_millis(), note);
    // The appended version lives in the same node as the one before it. Without
    // this the *next* edit's reverse lookup — which reads the latest version —
    // would find nothing and silently stop mirroring.
    record.stamp_workspace_node(node_id);
    // Fail-closed, and the one place in this function that is: the lookup
    // succeeded, so this node *is* a deliverable, and a caller that wrote it
    // anyway would leave the history claiming the agent's draft shipped
    // unchanged.
    artifacts.upsert(company, &record).await?;
    Ok(MirrorOutcome::Recorded(MirroredEdit {
        artifact_id: record.id,
        version,
    }))
}

/// What [`mirror_node_edit`] was able to do — and, when it could not act,
/// whether the caller may carry on without it.
///
/// [`Ordinary`](MirrorOutcome::Ordinary) and
/// [`Undetermined`](MirrorOutcome::Undetermined) both mean "nothing was
/// recorded", and that is the whole reason they are separate variants rather
/// than one absent value: the first is a complete answer from a healthy store
/// and the second is no answer at all.
#[derive(Debug)]
pub enum MirrorOutcome {
    /// `node_id` is a published deliverable, and this edit is now a version on
    /// its chain.
    Recorded(MirroredEdit),
    /// The store answered, and `node_id` names no artifact — an ordinary note.
    /// There is no chain here for a node write to get ahead of.
    Ordinary,
    /// The store could not be read, so whether `node_id` is published is
    /// **unknown** rather than "no". Carries the fault so a caller that
    /// tolerates it can still say why in a log.
    Undetermined(OpenCompanyError),
}

/// What [`mirror_node_edit`] appended, for a caller that wants to log or return
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirroredEdit {
    /// The artifact the edit was recorded on.
    pub artifact_id: String,
    /// The version number the edit became.
    pub version: u32,
}

/// The artifact whose current body lives in `node_id`, if any.
///
/// Shared by [`mirror_node_edit`] and the console's artifact-append route,
/// which needs the same "is this a published deliverable?" answer from the
/// other direction.
pub async fn published_record_for_node(
    artifacts: &dyn ArtifactStore,
    company: &CompanyId,
    node_id: &str,
) -> Result<Option<ArtifactRecord>> {
    Ok(artifacts
        .list(company, None)
        .await?
        .into_iter()
        .find(|record| record.workspace_node_id() == Some(node_id)))
}

/// This agent's authorship stamp. A published deliverable is the agent's work,
/// so every node created or written along its path is attributed to it.
fn origin(agent_id: &str) -> WorkspaceOrigin {
    WorkspaceOrigin::Agent {
        id: agent_id.to_string(),
    }
}

/// Split a normalized publish path into its segments, rejecting anything that
/// cannot name a chain of workspace nodes.
///
/// The publish tool normalizes before it gets here, so this is a guard against
/// a hand-built `PendingPublish` rather than the ordinary path — but a `..`
/// reaching [`WorkspaceStore::create`] as a node *name* would render a
/// traversal-shaped path in the console, and the sqlite and mongodb backends do
/// not reject one.
fn split_source(source: &str) -> Result<Vec<String>> {
    let segments: Vec<&str> = source
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return Err(OpenCompanyError::InvalidRequest(format!(
            "`{source}` names no workspace path segments, so it cannot be published into the tree"
        )));
    }
    for segment in &segments {
        if *segment == "." || *segment == ".." || segment.contains('\\') || segment.contains('\0') {
            return Err(OpenCompanyError::InvalidRequest(format!(
                "`{source}` contains a segment that cannot name a workspace node"
            )));
        }
    }
    // Every segment becomes a node name, so it is minted under the workspace's
    // one naming rule: lowercase and dashed. The sandbox is the agent's own
    // scratch and names files however it likes; the tree is what the operator
    // reads, and `specs/Launch Plan.md` arriving there as `specs/launch-plan.md`
    // is what keeps one document to one spelling.
    //
    // The artifact record's `source` is deliberately *not* rewritten to match:
    // it names the file in the sandbox the agent actually published, and it is
    // the key a republish extends the same record by. Normalizing it would make
    // the record claim a path the agent cannot read back.
    Ok(segments.into_iter().map(kebab_name).collect())
}

/// Adopt-or-create the folder `name` under `parent`, keeping `nodes` current.
///
/// # The snapshot answers, the store decides (issue #759)
///
/// The `nodes` snapshot is a fast path and nothing more: a hit means the folder
/// was already there when the tree was read, and a folder does not stop
/// existing. A *miss* is only a statement about that instant, and the create
/// used to act on it later — so two publishes needing `Agents/<agent>/<task>/`
/// both saw it free and both created, leaving two folders under one name.
///
/// That state does not decay. The `many` arm below answers a duplicated name
/// with `Conflict`, so a race lasting microseconds refuses every later publish
/// beneath that path, for every agent, permanently. The write therefore always
/// goes through [`WorkspaceStore::adopt_or_create_folder`], which decides the
/// contention where it can actually be decided — under the store's own lock,
/// transaction or unique index.
///
/// The node pushed back into the snapshot is the one the **store** returned, so
/// a publisher that adopted somebody else's folder walks on with the winner's
/// id rather than one it invented.
async fn resolve_folder(
    workspace: &dyn WorkspaceStore,
    company: &CompanyId,
    nodes: &mut Vec<WorkspaceNode>,
    minted: &mut Vec<String>,
    parent: &str,
    name: &str,
    agent_id: &str,
) -> Result<String> {
    let matches: Vec<&WorkspaceNode> = children_named(nodes, parent, name);
    match matches.as_slice() {
        [one] if one.kind == NodeKind::Folder => Ok(one.id.clone()),
        [_] => Err(OpenCompanyError::Conflict(format!(
            "`{name}` already exists as a note, not a folder, so a deliverable cannot be published \
             beneath it"
        ))),
        [] => {
            // Only a genuine mint is a rollback candidate (issue #1801): a
            // publisher that adopted somebody else's folder must not have it
            // swept if this publish then fails.
            let claim = workspace
                .adopt_or_create_folder(company, Some(parent), name, origin(agent_id))
                .await?;
            let created = claim.was_created();
            let node = claim.into_node();
            let id = node.id.clone();
            if created {
                minted.push(id.clone());
            }
            nodes.push(node);
            Ok(id)
        }
        many => Err(OpenCompanyError::Conflict(format!(
            "{count} nodes under this folder are named `{name}`, so the path is ambiguous",
            count = many.len()
        ))),
    }
}

/// The name a task's deliverable folder is minted under: the card's title,
/// then its id (issue #1687).
///
/// # Why the title, and why the id is still in it
///
/// The folder used to be named by the card ULID alone. That is a perfectly
/// good *key* and a useless *label*: an operator opening `artifacts/<agent>/`
/// saw a column of `01hq8zm4x…` and could not tell what any of them held
/// without opening each one. The card's title is the one string that already
/// says what the work was.
///
/// The id stays because it is the only thing in the name that is unique and
/// immutable. Dropping it would mean two cards a teammate titled "Weekly
/// update" share one folder and overwrite each other's deliverables, and it
/// would leave an operator holding a card id with nothing in the tree to match
/// it against. Title-then-id also puts the readable half first, which is what
/// survives the explorer's `truncate`.
///
/// # The title half is budgeted, the id half is not
///
/// [`kebab_name`] bounds a whole name at [`MAX_NAME_BYTES`]; here two names are
/// being joined, so the title is trimmed to whatever the id leaves and any
/// separator the cut exposed is trimmed with it. The id is never truncated — a
/// partial ULID is not the id, and matching one is the whole point of
/// [`task_folder_task_id`]. A card whose title normalizes to nothing (an emoji,
/// punctuation) is named by the id alone rather than by `untitled`, which is
/// what [`kebab_name`] would otherwise hand back for every one of them at once.
///
/// # The two halves are joined by a dot, not by a dash
///
/// [`TASK_ID_BOUNDARY`] is the only separator that makes the id half
/// *findable*, which is the whole job of [`resolve_task_folder`]. A dash cannot:
/// a seed card's id is `[a-z0-9-]` (`task_file::normalize_task_id`), so cards
/// `login` and `fix-login` are both legal, and `password-reset-fix-login` ends
/// with `-login` as surely as `password-reset-login` does. Matching on that
/// suffix files one card's deliverables in the other's folder, and once both
/// have published it makes the shorter id ambiguous forever.
///
/// A dot has no such twin. Neither id grammar can produce one — a board card's
/// id is a ULID, a seed card's is `[a-z0-9-]` — so the name's *last* dot is
/// always the join, and the text after it is the whole id and nothing else. It
/// is also still one lawful workspace name: [`kebab_name`] keeps a dot that
/// something precedes, and collapses a dash run, so `--` would fail
/// [`is_kebab_name`](super::workspace_names::is_kebab_name) where this passes.
fn task_folder_name(task_id: &str, task_title: Option<&str>) -> String {
    let id = kebab_name(task_id);
    // An id carrying the boundary itself would put the join in the wrong place,
    // so such a card is named by its id alone — which is what every folder was
    // called before this, and what the `name == id` arm of the lookup still
    // matches. Guarding an input neither id grammar can produce keeps this a
    // total function rather than one with an unstated precondition.
    let Some(title) = task_title.filter(|_| !id.contains(TASK_ID_BOUNDARY)) else {
        return id;
    };
    // `kebab_name_or` falls back **only** when the title normalized to nothing,
    // which is the distinction `kebab_name` flattens: it answers `untitled` both
    // for a card actually titled "Untitled" — which has a perfectly good name —
    // and for one titled "🎉", which has none. Falling back to the id keeps the
    // second off `untitled.<id>` without taking the first's title away.
    let mut slug = kebab_name_or(title, task_id);
    if slug == id {
        return id;
    }
    // `+ 1` for the boundary joining the two halves. Both halves are ASCII by
    // construction (`kebab_name` emits only `[a-z0-9.-]`), so a byte cut is
    // always a character cut.
    let room = MAX_NAME_BYTES.saturating_sub(id.len() + 1);
    if slug.len() > room {
        slug.truncate(room);
    }
    while slug.ends_with('-') || slug.ends_with('.') {
        slug.pop();
    }
    if slug.is_empty() {
        return id;
    }
    format!("{slug}{TASK_ID_BOUNDARY}{id}")
}

/// The character [`task_folder_name`] joins the readable half to the id half
/// with, and therefore the boundary [`task_folder_task_id`] reads back.
const TASK_ID_BOUNDARY: char = '.';

/// The task id `name` was composed around, when it was composed by
/// [`task_folder_name`] at all.
///
/// The **last** boundary rather than the first, because the readable half can
/// hold dots of its own (`v1.2 plan` normalizes to `v1.2-plan`) while the id
/// half can hold none. So the tail is the whole id, exactly, and a lookup on it
/// is an equality test rather than the unbounded suffix match a dash join would
/// force — see [`task_folder_name`] for the card pair that breaks.
fn task_folder_task_id(name: &str) -> Option<&str> {
    name.rsplit_once(TASK_ID_BOUNDARY).map(|(_, id)| id)
}

/// Adopt-or-create the folder holding `task_id`'s deliverables, **matched by
/// id rather than by name** (issue #1687).
///
/// # Why this is not [`resolve_folder`] with a different name
///
/// [`resolve_folder`] matches a name exactly, and a task folder's name is no
/// longer a function of the task alone: it carries the card's title, and a
/// title is editable. An exact-name lookup would therefore stop finding the
/// folder the moment somebody renamed the card, and the next publish would
/// mint a rival beside it — one task, two folders, deliverables split across
/// both. Matching on the id suffix makes the lookup depend only on the half
/// that cannot change.
///
/// The same match is what **adopts** a folder minted before this change, whose
/// name is the bare id: a company that has published already keeps its existing
/// folders and its console deep links, and only a task publishing for the first
/// time gets a titled name. Nothing is renamed, for the reason
/// [`workspace_names`](super::workspace_names) gives at length — an operator
/// must not find their tree rearranged by an upgrade they did not ask for, and
/// a rename breaks every reference anyone kept to the old name.
///
/// # A note wearing the id is refused, even when a folder also matches
///
/// A deliverable cannot be published beneath a note, so a match of the wrong
/// kind is a [`Conflict`](OpenCompanyError::Conflict) rather than a guess —
/// the same fail-closed rule [`resolve_folder`] applies to a name. It is
/// checked **before** any folder is chosen, not only when no folder matched:
/// no backend enforces unique sibling names, so a legacy or imported tree can
/// carry a note and a folder under one name, and publishing into the folder
/// would leave the deliverable at a path `PathIndex` reads as ambiguous — a
/// note the agent that just wrote it could not then open.
///
/// # Two folders for one task: the oldest wins, deterministically
///
/// [`resolve_folder`]'s create is atomic, but it is keyed by the folder's
/// **name**, and two *first* publishes of one task can now compute two
/// different names — one caller holding the card's title and one holding
/// `None` ([`PublishTarget::task_title`]), or a retitle landing between them.
/// Both then see no match, both create, and the tree carries two folders for
/// one task.
///
/// Answering that with `Conflict` would be the worst of the options available:
/// the state does not decay, so a race lasting microseconds would refuse every
/// later publish for that task permanently — the exact failure
/// [`resolve_folder`] documents at length and the store's atomic
/// adopt-or-create exists to prevent. And there is no identity here to guess
/// at, which is what makes this unlike [`resolve_folder`]'s duplicate *name*:
/// both folders were matched on this task's own immutable id, so both are
/// provably its own. The lowest node id therefore wins — node ids are ULIDs, so
/// that is the older of the two, and it is the same answer on every later
/// publish, in every process, on every backend. The deliverables that landed in
/// the loser stay where they are and stay readable; nothing is moved or
/// renamed.
///
/// Closing the window instead of converging after it would need an
/// adopt-or-create keyed by something other than the name — a new
/// [`WorkspaceStore`] method across all three backends, which is a change to
/// the port rather than to this naming rule.
// The `minted` accumulator (issue #1801) pushes this over clippy's threshold;
// the args are the cohesive publish context plus the two walk accumulators, so
// bundling them would trade one honest signature for a struct that exists only
// to satisfy the lint — the same call the repo's other sites make.
#[allow(clippy::too_many_arguments)]
async fn resolve_task_folder(
    workspace: &dyn WorkspaceStore,
    company: &CompanyId,
    nodes: &mut Vec<WorkspaceNode>,
    minted: &mut Vec<String>,
    parent: &str,
    task_id: &str,
    task_title: Option<&str>,
    agent_id: &str,
) -> Result<String> {
    let id = kebab_name(task_id);
    let mut folders: Vec<&str> = Vec::new();
    let mut other_kind = false;
    for node in nodes.iter().filter(|node| {
        node.parent_id.as_deref() == Some(parent)
            && (node.name == id || task_folder_task_id(&node.name) == Some(id.as_str()))
    }) {
        match node.kind {
            NodeKind::Folder => folders.push(&node.id),
            _ => other_kind = true,
        }
    }
    if other_kind {
        return Err(OpenCompanyError::Conflict(format!(
            "`{id}` already exists as a note, not a folder, so a deliverable cannot be \
             published beneath it"
        )));
    }
    if let Some(oldest) = folders.iter().min() {
        return Ok((*oldest).to_string());
    }
    let name = task_folder_name(task_id, task_title);
    resolve_folder(workspace, company, nodes, minted, parent, &name, agent_id).await
}

/// The existing file `name` under `parent`, or `None` when the name is free.
fn resolve_file(nodes: &[WorkspaceNode], parent: &str, name: &str) -> Result<Option<String>> {
    let matches = children_named(nodes, parent, name);
    match matches.as_slice() {
        [one] if one.kind == NodeKind::File => Ok(Some(one.id.clone())),
        [_] => Err(OpenCompanyError::Conflict(format!(
            "`{name}` already exists as a folder, not a note, so a deliverable cannot be published \
             over it"
        ))),
        [] => Ok(None),
        many => Err(OpenCompanyError::Conflict(format!(
            "{count} nodes under this folder are named `{name}`, so the path is ambiguous",
            count = many.len()
        ))),
    }
}

/// Every node directly under `parent` carrying `name`.
fn children_named<'a>(
    nodes: &'a [WorkspaceNode],
    parent: &str,
    name: &str,
) -> Vec<&'a WorkspaceNode> {
    nodes
        .iter()
        .filter(|node| node.parent_id.as_deref() == Some(parent) && node.name == name)
        .collect()
}

#[cfg(test)]
#[path = "artifact_mirror_tests_support.rs"]
mod artifact_mirror_tests_support;
#[cfg(test)]
#[path = "artifact_mirror_tests_faults.rs"]
mod tests_faults;
#[cfg(test)]
#[path = "artifact_mirror_tests_folders.rs"]
mod tests_folders;
#[cfg(test)]
#[path = "artifact_mirror_tests_publish.rs"]
mod tests_publish;
