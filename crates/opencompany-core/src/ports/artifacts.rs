//! The [`ArtifactStore`] port: versioned task outputs and the human-edit diff.
//!
//! A task produces things — a draft, a post, a generated file. Before this port
//! those outputs were folded into the card's `note` as plain text
//! (`append_result`), which loses two things worth keeping:
//!
//! * **Version history.** A task that runs, gets redirected, and re-runs just
//!   append-stacks more text into the note. There is no "the second draft
//!   replaced the first" — only a growing blob.
//! * **What a human changed before approving.** "The agent wrote X, the
//!   operator shipped Y" is the highest-signal quality datum the product can
//!   produce: heavy editing means the agent's instructions need work. Folded
//!   into note text, it is unrecoverable.
//!
//! So an artifact is a record with an ordered [`ArtifactVersion`] list, and an
//! operator edit is *a new version by a different author* rather than a
//! mutation of the old one. Nothing is ever overwritten, which is what makes
//! [`ArtifactRecord::human_edit_diff`] answerable at any later point.
//!
//! Independent of the per-task timeline (#185): a version may record which
//! timeline step produced it via [`ArtifactVersion::step_seq`], but that field
//! is optional and nothing here reads the event journal.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::ports::types::CompanyId;

/// What an artifact holds. Drives the console's renderer choice; the body is
/// always text on the wire (a binary artifact carries a reference, not bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    /// Plain text — the default for an agent reply.
    Text,
    /// Markdown source (a draft, a post).
    Markdown,
    /// A reference to a generated image (URL or workspace path, not bytes).
    Image,
    /// A reference to a generated file (workspace path, not bytes).
    File,
}

impl ArtifactKind {
    /// The stable wire word, for logs and route bodies.
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Text => "text",
            ArtifactKind::Markdown => "markdown",
            ArtifactKind::Image => "image",
            ArtifactKind::File => "file",
        }
    }
}

/// Who produced a version.
///
/// The whole point of the port: this is what makes an operator's pre-approval
/// edit distinguishable from the agent's own re-run, and therefore what makes
/// the quality signal computable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactAuthor {
    /// An agent turn produced it.
    Agent,
    /// A human edited it (typically just before approving).
    Operator,
}

/// One immutable revision of an artifact.
///
/// Versions are append-only and 1-based. A version is never edited in place —
/// an operator's change appends a new one — so the full provenance chain
/// survives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactVersion {
    /// 1-based revision number, contiguous within the artifact.
    pub version: u32,
    /// The revision's content.
    pub body: String,
    /// Whether an agent or a human wrote this revision.
    pub author: ArtifactAuthor,
    /// The agent id, or the operator's label. Free-form and display-only.
    pub author_id: String,
    /// Epoch-millis the revision was recorded.
    pub created_at_millis: u64,
    /// The journal sequence of the timeline step that produced it, when known
    /// (#185's `task_id`-correlated events). Optional and purely a
    /// cross-reference — this port never reads the event log, so an artifact
    /// stands on its own without the timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_seq: Option<u64>,
    /// Why this revision exists (e.g. `"operator edit before approval"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The task **attempt** ([`RunRecord`](crate::ports::runs::RunRecord)) that
    /// produced this revision, when an agent turn under one did (issue #242).
    ///
    /// Per *version*, not per record, because that is the granularity the
    /// question has: a card dispatched twice produces v1 under the first attempt
    /// and v2 under the second, and a record-level field would remember only the
    /// later one — losing the link the first attempt's row needs to point at
    /// what it actually wrote. The record-level answer is still available as
    /// `latest().run_id`.
    ///
    /// `None` for an operator edit (no attempt produced it), for a revision
    /// written by an untracked dispatch, and for every artifact stored before
    /// this field existed. Artifacts are persisted as a JSON blob on every
    /// backend (`artifact_json` on sqlite, a document on MongoDB, a file on the
    /// filesystem), so this needs **no schema migration** and a pre-#242 record
    /// loads with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The workspace node this revision's body was mirrored into (issue #552).
    ///
    /// # What the two surfaces are, and which one is authoritative
    ///
    /// A published deliverable now lives twice: here, as the **authoritative
    /// version history**, and as one node in the company's shared workspace
    /// tree, which holds only the *current* body. The tree is the surface every
    /// agent and the operator can browse, so a deliverable that never reached
    /// it was invisible to everybody but the Artifacts tab of one card.
    ///
    /// The chain stays the truth. The node is a projection of its latest
    /// version, and this field is the link between them. The invariant both
    /// write paths keep is `node.body == chain.latest().body` after any
    /// successful write on either surface.
    ///
    /// # Per version, not per record — and deliberately so
    ///
    /// Same reasoning as [`run_id`](Self::run_id) directly above. An operator
    /// may delete a published node (deletions stick), and the next publish of
    /// that path materializes a **fresh** node with a new id. Recording the id
    /// per record would rewrite history and claim the old versions had always
    /// lived in the new node; recording it per version says the honest thing —
    /// v1 and v2 were mirrored into a node that no longer exists, v3 into this
    /// one.
    ///
    /// `None` for a version nobody mirrored: an artifact recorded while no
    /// workspace store was wired, a version whose node write failed (the
    /// deliverable is still recorded — see
    /// `harness::publish_node`), and every artifact stored before this field
    /// existed. Artifacts persist as a JSON blob on every backend, so this
    /// needs **no schema migration** and a pre-#552 record loads with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_node_id: Option<String>,
}

/// A versioned output of one task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    /// Stable id within the company.
    pub id: String,
    /// The task that produced it.
    pub task_id: String,
    /// A short display title.
    pub title: String,
    /// What it holds.
    pub kind: ArtifactKind,
    /// Every revision, oldest first. Never empty in practice — an artifact is
    /// created with its first version — but the accessors tolerate empty so a
    /// hand-written or truncated record cannot panic a read route.
    pub versions: Vec<ArtifactVersion>,
    /// Epoch-millis the artifact was first created.
    pub created_at_millis: u64,
    /// Epoch-millis of the newest revision.
    pub updated_at_millis: u64,
    /// **What this artifact is an artifact _of_** (issue #244): the normalized
    /// workspace-relative path the agent published, e.g. `specs/launch.md`.
    ///
    /// # The identity contract
    ///
    /// `(task_id, source)` is the artifact's identity. Republishing the same
    /// path on a later attempt appends a **version to this record**; publishing
    /// a different path opens a new one. Nothing else may decide which record a
    /// revision extends.
    ///
    /// That is a correction, not a refinement. The extend target used to be
    /// chosen by *recency* — the most recently `updated_at_millis` artifact on
    /// the card — which meant an operator editing artifact B made B the target
    /// for the next agent write to A. The agent's v3 of the spec would land as
    /// v4 of the invoice, and `human_edit_diff` would report a rewrite that
    /// never happened. Since that diff is the entire reason this port exists,
    /// recency selection did not merely mis-file a record; it corrupted the one
    /// number the product is trying to produce.
    ///
    /// # `None` means "not published from a file"
    ///
    /// Every record minted after #244 carries a `source`, because the only way
    /// to mint one is `publish_artifact` on a workspace file. So `None` marks a
    /// **legacy** record from the era when a completed dispatch's chat reply was
    /// captured automatically — including the refusals and blocker messages that
    /// made the Artifacts tab claim deliverables it did not have. Those records
    /// are kept (nothing deletes the past) and the console labels them as what
    /// they are.
    ///
    /// # Limits, named rather than hidden
    ///
    /// Identity is the path, so **renaming a file starts a new lineage**. The
    /// alternative — tracking moves — needs either content hashing or a rename
    /// hook the file tools do not have, and would guess wrong exactly when two
    /// drafts are similar. A new record for a renamed file is honest; a wrong
    /// merge is not.
    ///
    /// Additive on the wire: artifacts persist as an opaque JSON blob on all
    /// three backends (`artifact_json` on sqlite, a document on MongoDB, a file
    /// on the filesystem), so this needs **no migration** and a pre-#244 record
    /// loads with `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl ArtifactRecord {
    /// Creates an artifact with its first (agent-authored) version.
    pub fn new(
        id: impl Into<String>,
        task_id: impl Into<String>,
        title: impl Into<String>,
        kind: ArtifactKind,
        body: impl Into<String>,
        author_id: impl Into<String>,
        at_millis: u64,
    ) -> Self {
        let id = id.into();
        Self {
            id,
            task_id: task_id.into(),
            title: title.into(),
            kind,
            versions: vec![ArtifactVersion {
                version: 1,
                body: body.into(),
                author: ArtifactAuthor::Agent,
                author_id: author_id.into(),
                created_at_millis: at_millis,
                step_seq: None,
                note: None,
                run_id: None,
                workspace_node_id: None,
            }],
            created_at_millis: at_millis,
            updated_at_millis: at_millis,
            source: None,
        }
    }

    /// Stamps the workspace-relative path this artifact was published from
    /// (issue #244), giving it the second half of its `(task_id, source)`
    /// identity.
    ///
    /// A builder rather than an eighth parameter on [`new`](Self::new): every
    /// existing call site — the REST create route, the conformance suite, the
    /// tests — has no path to name, and widening the signature would make them
    /// all pass a `None` that means nothing to them.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// The newest revision, or `None` on an empty record.
    pub fn latest(&self) -> Option<&ArtifactVersion> {
        self.versions.last()
    }

    /// A revision by its 1-based number.
    pub fn version(&self, version: u32) -> Option<&ArtifactVersion> {
        self.versions.iter().find(|v| v.version == version)
    }

    /// Appends a revision, numbering it after the current newest and advancing
    /// `updated_at_millis`. Returns the new version number.
    ///
    /// Numbering is derived from the existing max rather than `len()`, so a
    /// record that ever lost a row mid-history still numbers monotonically
    /// instead of silently reusing a number that already means something else.
    pub fn push_version(
        &mut self,
        body: impl Into<String>,
        author: ArtifactAuthor,
        author_id: impl Into<String>,
        at_millis: u64,
        note: Option<String>,
    ) -> u32 {
        let next = self.versions.iter().map(|v| v.version).max().unwrap_or(0) + 1;
        self.versions.push(ArtifactVersion {
            version: next,
            body: body.into(),
            author,
            author_id: author_id.into(),
            created_at_millis: at_millis,
            step_seq: None,
            note,
            run_id: None,
            workspace_node_id: None,
        });
        self.updated_at_millis = at_millis;
        next
    }

    /// Stamps the newest revision with the attempt that produced it (#242).
    ///
    /// A separate call rather than a sixth parameter on
    /// [`push_version`](Self::push_version) / [`new`](Self::new): only the
    /// dispatch path has a run to name, and widening those signatures would make
    /// every operator-edit and test call site pass a `None` it has no meaning
    /// for. A no-op on an empty record.
    pub fn stamp_run(&mut self, run_id: impl Into<String>) {
        if let Some(latest) = self.versions.last_mut() {
            latest.run_id = Some(run_id.into());
        }
    }

    /// Stamps the newest revision with the workspace node its body was mirrored
    /// into (#552).
    ///
    /// [`stamp_run`](Self::stamp_run)'s sibling, and a separate call for
    /// exactly the same reason: only the two mirroring paths (the publish drain
    /// and the console's node write) have a node to name, and widening
    /// [`push_version`](Self::push_version) / [`new`](Self::new) would make the
    /// REST create route, the conformance suite and every test pass a `None`
    /// that means nothing to them. A no-op on an empty record.
    pub fn stamp_workspace_node(&mut self, node_id: impl Into<String>) {
        if let Some(latest) = self.versions.last_mut() {
            latest.workspace_node_id = Some(node_id.into());
        }
    }

    /// Rewrites the newest revision's body — the reconciliation a publish owes
    /// when the storage outcome it described turns out differently (#663).
    ///
    /// # Why amending is right here, and only here
    ///
    /// The chain is the authoritative version history, so rewriting a recorded
    /// body is not something to do casually. This is confined to the drain that
    /// *created* the version: the body was composed before the workspace was
    /// asked (the ordering is deliberate — the record is written first so a node
    /// is never created for an unrecorded deliverable), so the version spends a
    /// moment describing an outcome that has not happened yet. Amending it is
    /// how that moment ends with the truth rather than with a claim.
    ///
    /// It is **not** a general edit path: an operator revision is a new version,
    /// with its own authorship, which is what `human_edit_diff` reads. A no-op
    /// on an empty record.
    pub fn amend_latest_body(&mut self, body: impl Into<String>) {
        if let Some(latest) = self.versions.last_mut() {
            latest.body = body.into();
        }
    }

    /// The workspace node the newest revision was mirrored into, if any.
    ///
    /// The re-publish and console-edit paths both ask this question — "is there
    /// already a node for this deliverable?" — and both must ask it of the
    /// *latest* version rather than of any version, because an operator's
    /// deletion of a node is honoured: older versions keep pointing at the node
    /// that held them, and only the newest says where the body lives now.
    pub fn workspace_node_id(&self) -> Option<&str> {
        self.latest()?.workspace_node_id.as_deref()
    }

    /// The diff of what a human changed before approving: the last
    /// agent-authored revision → the last operator-authored revision after it.
    ///
    /// `None` when no human has edited (the common, healthy case) or when no
    /// agent version precedes the edit. Anchoring on the last agent version
    /// *before* the operator's — rather than simply v1 → latest — is what keeps
    /// the diff meaningful across a redirect: if the agent re-ran and then a
    /// human tweaked the second draft, the interesting change is against that
    /// second draft, not the abandoned first.
    pub fn human_edit_diff(&self) -> Option<ArtifactDiff> {
        let last_operator = self
            .versions
            .iter()
            .rev()
            .find(|v| v.author == ArtifactAuthor::Operator)?;
        let last_agent_before = self
            .versions
            .iter()
            .rfind(|v| v.author == ArtifactAuthor::Agent && v.version < last_operator.version)?;
        Some(ArtifactDiff::between(last_agent_before, last_operator))
    }
}

/// One line of a diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffOp {
    /// Unchanged, present in both sides.
    Equal,
    /// Only in the newer side.
    Insert,
    /// Only in the older side.
    Delete,
}

/// A single diff line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    /// What happened to this line.
    pub op: DiffOp,
    /// The line's text (no trailing newline).
    pub text: String,
}

/// A line-level diff between two revisions, plus the churn summary that makes
/// it a quality signal rather than just a rendering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDiff {
    /// The older revision's number.
    pub from_version: u32,
    /// The newer revision's number.
    pub to_version: u32,
    /// The line-by-line diff, in newer-side order with deletions interleaved.
    pub lines: Vec<DiffLine>,
    /// Lines only in the newer side.
    pub added: usize,
    /// Lines only in the older side.
    pub removed: usize,
    /// Fraction of the older side's lines that changed, 0.0–1.0.
    ///
    /// This is the number worth alerting on: sustained high churn on an agent's
    /// artifacts means its instructions need work. Defined against the older
    /// (agent) side so "the operator rewrote most of it" reads as ~1.0. An
    /// empty original with added lines is 1.0 rather than a divide-by-zero.
    pub churn: f64,
}

impl ArtifactDiff {
    /// Diffs two revisions by line.
    pub fn between(from: &ArtifactVersion, to: &ArtifactVersion) -> Self {
        let lines = diff_lines(&from.body, &to.body);
        let added = lines.iter().filter(|l| l.op == DiffOp::Insert).count();
        let removed = lines.iter().filter(|l| l.op == DiffOp::Delete).count();
        let original = from.body.lines().count();
        let churn = if original == 0 {
            if added == 0 { 0.0 } else { 1.0 }
        } else {
            (removed as f64 / original as f64).min(1.0)
        };
        Self {
            from_version: from.version,
            to_version: to.version,
            lines,
            added,
            removed,
            churn,
        }
    }
}

/// Line-level diff via a longest-common-subsequence table.
///
/// Hand-rolled on purpose: the crate takes no diff dependency, and a line LCS
/// is a dozen lines of DP. Quadratic in line count, which is fine for artifact
/// bodies (drafts and posts, not repositories) — and bounded below by
/// [`MAX_DIFF_LINES`], past which the diff degrades to a whole-body
/// replace rather than allocating an enormous table.
fn diff_lines(from: &str, to: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = from.lines().collect();
    let b: Vec<&str> = to.lines().collect();

    if a.len() > MAX_DIFF_LINES || b.len() > MAX_DIFF_LINES {
        // Degrade rather than build an n×m table over huge inputs. Still a
        // truthful diff — every old line removed, every new line added.
        let mut out = Vec::with_capacity(a.len() + b.len());
        out.extend(a.iter().map(|l| DiffLine {
            op: DiffOp::Delete,
            text: (*l).to_string(),
        }));
        out.extend(b.iter().map(|l| DiffLine {
            op: DiffOp::Insert,
            text: (*l).to_string(),
        }));
        return out;
    }

    // lcs[i][j] = length of the LCS of a[i..] and b[j..].
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push(DiffLine {
                op: DiffOp::Equal,
                text: a[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(DiffLine {
                op: DiffOp::Delete,
                text: a[i].to_string(),
            });
            i += 1;
        } else {
            out.push(DiffLine {
                op: DiffOp::Insert,
                text: b[j].to_string(),
            });
            j += 1;
        }
    }
    for line in a.iter().skip(i) {
        out.push(DiffLine {
            op: DiffOp::Delete,
            text: (*line).to_string(),
        });
    }
    for line in b.iter().skip(j) {
        out.push(DiffLine {
            op: DiffOp::Insert,
            text: (*line).to_string(),
        });
    }
    out
}

/// Line ceiling past which [`diff_lines`] degrades to a whole-body replace
/// instead of building a quadratic LCS table.
pub const MAX_DIFF_LINES: usize = 2_000;

/// Durable per-company task artifacts. Company A's artifacts MUST be invisible
/// to company B.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Lists artifacts, most-recently-updated first, optionally narrowed to one
    /// task.
    async fn list(&self, company: &CompanyId, task_id: Option<&str>)
    -> Result<Vec<ArtifactRecord>>;
    /// Fetches one artifact by id, with its full version history.
    async fn get(&self, company: &CompanyId, id: &str) -> Result<Option<ArtifactRecord>>;
    /// Inserts or replaces an artifact by id.
    async fn upsert(&self, company: &CompanyId, artifact: &ArtifactRecord) -> Result<()>;
    /// Deletes an artifact by id; returns whether one was removed.
    async fn delete(&self, company: &CompanyId, id: &str) -> Result<bool>;
}

#[cfg(test)]
#[path = "artifacts_tests.rs"]
mod tests;
