//! The workspace tree read: `Company.workspaceTree` / `workspaceFile` over the
//! [`WorkspaceStore`] port, with `[[wikilink]]` backlinks computed at read.

use std::num::NonZeroUsize;
use std::sync::Arc;

use async_graphql::{ID, SimpleObject};

use crate::company::runtime::CompanyRuntime;
use crate::company::workspace_links::file_with_backlinks;
use crate::company::workspace_search::{
    DEFAULT_SEARCH_LIMIT, MAX_SEARCH_RESULTS, search_workspace,
};
use crate::ports::iso8601;
use crate::ports::workspace::{NodeKind, WorkspaceNode, WorkspaceOrigin};

/// Who authored a workspace node. Mirrors [`WorkspaceOrigin`] (issue #326).
///
/// Flattened into `kind` + optional `agentId` rather than exposed as a GraphQL
/// union: `agentId` is non-null exactly when `kind` is `agent`, and a union of
/// two empty types plus one single-field type buys a client nothing but three
/// inline fragments. The Rust type keeps the invariant; this is its projection.
#[derive(SimpleObject, Clone)]
#[graphql(name = "WorkspaceOrigin")]
pub struct WorkspaceOriginGql {
    /// `seed`, `operator` or `agent`.
    pub kind: String,
    /// The agent's roster id — set only when `kind` is `agent`.
    pub agent_id: Option<String>,
}

impl From<WorkspaceOrigin> for WorkspaceOriginGql {
    fn from(origin: WorkspaceOrigin) -> Self {
        match origin {
            WorkspaceOrigin::Seed => Self {
                kind: "seed".to_string(),
                agent_id: None,
            },
            WorkspaceOrigin::Operator => Self {
                kind: "operator".to_string(),
                agent_id: None,
            },
            WorkspaceOrigin::Agent { id } => Self {
                kind: "agent".to_string(),
                agent_id: Some(id),
            },
        }
    }
}

/// One node (folder or file) in the workspace tree. Mirrors [`WorkspaceNode`].
#[derive(SimpleObject, Clone)]
#[graphql(name = "FsNode")]
pub struct FsNodeGql {
    /// The node id (stable ULID).
    pub id: ID,
    /// The node name.
    pub name: String,
    /// `folder` or `file`.
    pub kind: String,
    /// The parent node id, or null at the root.
    pub parent_id: Option<ID>,
    /// When it was last updated, ISO-8601 UTC.
    pub updated_at: String,
    /// Who created the node.
    pub created_by: WorkspaceOriginGql,
    /// Who last wrote the node's content (a rename or move does not change it).
    pub updated_by: WorkspaceOriginGql,
    /// The payload's media type — **set only on a binary node** (issue #553),
    /// and null on a folder or a prose note.
    ///
    /// Its presence is the whole test for "this node holds bytes, not text", and
    /// without it no GraphQL consumer could discover that binaries exist at all:
    /// a payload's text `read` is an empty body by contract, so a 4 MB PNG and a
    /// genuinely empty note were the same answer on this surface (issue #669).
    /// The REST `FsNode` has carried these three since #553; this is the field
    /// set catching up, not a new concept.
    pub mime: Option<String>,
    /// The payload's length in bytes; null unless `mime` is set.
    ///
    /// `Float`, not `Int`, because the store's `size` is a `u64` and GraphQL's
    /// `Int` is 32-bit — the same choice `usage` and `Approval.atMillis` made,
    /// for the same reason. Today's 64 MiB per-write cap fits in an `Int`; the
    /// cap is configurable and the field should not be the thing that stops it
    /// being raised.
    pub size: Option<f64>,
    /// The payload's sha256, the same digest the blob route serves as its
    /// `ETag`; null unless `mime` is set.
    ///
    /// This is what makes "the bytes changed" observable to a consumer that
    /// cannot see the bytes — a re-publish keeps the node id and changes this.
    pub sha256: Option<String>,
}

impl From<WorkspaceNode> for FsNodeGql {
    fn from(node: WorkspaceNode) -> Self {
        let kind = match node.kind {
            NodeKind::Folder => "folder",
            NodeKind::File => "file",
        };
        Self {
            id: ID(node.id),
            name: node.name,
            kind: kind.to_string(),
            parent_id: node.parent_id.map(ID),
            updated_at: iso8601(node.updated_at_millis),
            created_by: node.created_by.into(),
            updated_by: node.updated_by.into(),
            mime: node.mime,
            size: node.size.map(|bytes| bytes as f64),
            sha256: node.sha256,
        }
    }
}

/// A single workspace file with its content and inbound `[[wikilink]]` backlinks.
#[derive(SimpleObject)]
#[graphql(name = "WorkspaceFile")]
pub struct WorkspaceFileGql {
    /// The file id.
    pub id: ID,
    /// The file name.
    pub name: String,
    /// The file content.
    pub content: String,
    /// When it was last updated, ISO-8601 UTC.
    pub updated_at: String,
    /// Who created this file.
    pub created_by: WorkspaceOriginGql,
    /// Who last wrote the content above.
    pub updated_by: WorkspaceOriginGql,
    /// Other files whose content links to this one via `[[name]]`.
    pub backlinks: Vec<FsNodeGql>,
}

/// One workspace search hit (issue #607).
///
/// The node is nested rather than flattened: `FsNode` is already the projection
/// every other workspace read hands back, and re-declaring its fields here would
/// be a second copy to keep in step. What search adds is the two things only a
/// search knows — where the node sits, and why it came back.
#[derive(SimpleObject)]
#[graphql(name = "WorkspaceSearchHit")]
pub struct WorkspaceSearchHitGql {
    /// The matching node.
    pub node: FsNodeGql,
    /// Its logical path, e.g. `standards/Engineering.md`.
    pub path: String,
    /// `name` or `content`.
    pub matched: String,
    /// Text around the first body match; null for a name match, a folder and a
    /// binary node.
    pub excerpt: Option<String>,
}

/// A page of workspace search hits, plus how many matched in total.
#[derive(SimpleObject)]
#[graphql(name = "WorkspaceSearchResults")]
pub struct WorkspaceSearchResultsGql {
    /// The hits: name matches first, then content matches, freshest first
    /// within each group.
    pub hits: Vec<WorkspaceSearchHitGql>,
    /// Matches before the limit was applied.
    pub total: i32,
}

/// Resolves `Company.workspaceTree`.
pub(crate) async fn resolve_tree(
    runtime: &Arc<CompanyRuntime>,
) -> async_graphql::Result<Vec<FsNodeGql>> {
    let nodes = runtime.workspace().tree(runtime.id()).await?;
    Ok(nodes.into_iter().map(FsNodeGql::from).collect())
}

/// Resolves `Company.workspaceFile(id)`, returning null when absent.
///
/// The node + content + backlink scan is
/// [`file_with_backlinks`](crate::company::workspace_links::file_with_backlinks),
/// shared with the REST `GET …/workspace/file/{id}` route the console reads, so
/// the two surfaces can never report different backlinks for the same note.
///
/// # A binary node is refused here, exactly as it is over REST (issue #669)
///
/// The port's honest answer to a prose-shaped read of a payload is `""`, and
/// this resolver used to pass that straight through — so a 4 MB PNG and an
/// empty note were the same response, on a surface whose whole job is to be
/// unambiguous about what a node contains. The REST twin has always refused,
/// naming the route that does serve the bytes; `WorkspaceFileBody` and
/// `WorkspaceFileGql` are documented as differing only in timestamp shape, and
/// answering differently about a node's *kind* is a much larger divergence than
/// the one that documentation permits.
///
/// Adding `mime` to this type instead was the alternative, and it is the wrong
/// one: it would make the twins differ in field set rather than in timestamps,
/// and it would leave `content: ""` sitting in the response for a client to
/// misread. Discovery belongs on the tree, where [`FsNodeGql`] now carries
/// `mime`/`size`/`sha256` — so a consumer learns a node is binary *before*
/// asking for its text, which is the order that avoids the error entirely.
pub(crate) async fn resolve_file(
    runtime: &Arc<CompanyRuntime>,
    id: &str,
) -> async_graphql::Result<Option<WorkspaceFileGql>> {
    let Some((node, content, backlinks)) =
        file_with_backlinks(runtime.workspace().as_ref(), runtime.id(), id).await?
    else {
        return Ok(None);
    };

    if let Some(mime) = &node.mime {
        return Err(async_graphql::Error::new(format!(
            "`{}` holds {mime} data, not text; fetch it from \
             `…/workspace/blob/{}` instead",
            node.name, node.id
        )));
    }

    Ok(Some(WorkspaceFileGql {
        id: ID(node.id),
        name: node.name,
        content,
        updated_at: iso8601(node.updated_at_millis),
        created_by: node.created_by.into(),
        updated_by: node.updated_by.into(),
        backlinks: backlinks.into_iter().map(FsNodeGql::from).collect(),
    }))
}

/// Resolves `Company.workspaceSearch(query, prefix, limit)`.
///
/// The scan is
/// [`search_workspace`](crate::company::workspace_search::search_workspace),
/// shared with the REST `GET …/workspace/search` route the console calls and
/// with the agent `workspace_search` tool — the same reason
/// [`resolve_file`] shares its backlink scan. Three surfaces that each answered
/// "which notes mention X" their own way would drift, and a caller looking at
/// one of them would have no way to notice.
pub(crate) async fn resolve_search(
    runtime: &Arc<CompanyRuntime>,
    query: &str,
    prefix: Option<&str>,
    limit: Option<i32>,
) -> async_graphql::Result<WorkspaceSearchResultsGql> {
    // A negative or zero `limit` is refused rather than coerced. Coercing it to
    // the default ignores an argument the caller meant, and coercing it to
    // "everything" is the unbounded read this surface exists to avoid.
    let limit = match limit {
        None => NonZeroUsize::new(DEFAULT_SEARCH_LIMIT).expect("the default limit is non-zero"),
        Some(n) => usize::try_from(n)
            .ok()
            .and_then(NonZeroUsize::new)
            .ok_or_else(|| {
                async_graphql::Error::new(format!(
                    "`limit` must be between 1 and {MAX_SEARCH_RESULTS}; omit it for the default \
                     of {DEFAULT_SEARCH_LIMIT}"
                ))
            })?,
    };

    let outcome = search_workspace(
        runtime.workspace().as_ref(),
        runtime.id(),
        query,
        prefix,
        limit,
    )
    .await?;
    Ok(WorkspaceSearchResultsGql {
        // Saturating rather than `as`: a wrap would report a negative count for
        // a workspace nobody will ever have, and saturating is the answer that
        // stays monotonic.
        total: i32::try_from(outcome.total).unwrap_or(i32::MAX),
        hits: outcome
            .hits
            .into_iter()
            .map(|hit| WorkspaceSearchHitGql {
                node: FsNodeGql::from(hit.node),
                path: hit.path,
                matched: hit.matched.as_str().to_string(),
                excerpt: hit.excerpt,
            })
            .collect(),
    })
}
