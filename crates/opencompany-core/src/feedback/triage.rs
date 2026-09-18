//! Feedback triage: the full label taxonomy, dedupe/cluster, and consent
//! throttling (`docs/spec/feedback-loop/triage.md`).
//!
//! Where [`labels`](super::labels) mints the label set for a single filed item,
//! this module owns the *taxonomy* itself and the downstream triage behavior a
//! roster-job triage agent performs over the tracker:
//!
//! * [`classify_labels`] — the single source of truth for the four-axis label
//!   set (`type/`, `area/`, `sev/`, `source/`) plus the base `feedback` label.
//! * [`TriageAgent`] — searches existing issues, merges duplicates
//!   ([`TriageAgent::dedupe`]) by commenting on the canonical and closing the
//!   duplicate, and maintains cluster issues
//!   ([`TriageAgent::maintain_cluster`]).
//! * [`cluster_plans`] / [`ClusterPlan::promote`] — group similar issues and
//!   decide when a cluster crosses the `count × severity` promotion threshold.
//! * [`QualityLedger`] — throttles a company's auto-consent after repeated
//!   low-quality agent-filings.
//! * [`escalation_for`] and the release-notes helpers ([`map_fixed_issues`],
//!   [`process_bug_check`]) surface the normative escalation and
//!   "You said, we did" contracts as pure functions the caller acts on.
//!
//! Everything here is exercised offline against
//! [`MockGitHubClient`](super::github::MockGitHubClient); nothing touches the
//! network.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use crate::Result;
use crate::feedback::github::{ExistingIssue, GitHubClient, IssueDraft};
use crate::feedback::labels::area_for;
use crate::feedback::types::{ConsentMode, FeedbackItem};

/// The operator-impact axis of the label taxonomy (`sev/`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// A papercut: annoying but not blocking.
    Annoyance,
    /// The operator is blocked from getting work done.
    Blocked,
    /// The problem cost real money.
    MoneyLost,
}

impl Severity {
    /// The kebab-case wire token used in the `sev/` label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Annoyance => "annoyance",
            Self::Blocked => "blocked",
            Self::MoneyLost => "money-lost",
        }
    }

    /// The promotion weight of this severity (`count × weight` scores a
    /// cluster). Higher severity pulls a cluster over the threshold sooner.
    pub fn weight(self) -> u32 {
        match self {
            Self::Annoyance => 1,
            Self::Blocked => 3,
            Self::MoneyLost => 10,
        }
    }
}

/// The who-filed-it axis of the label taxonomy (`source/`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedbackSource {
    /// The operator filed it themselves.
    Operator,
    /// The company's brain filed it on the operator's behalf.
    AgentFiled,
    /// The platform filed it (fleet-wide signal).
    Platform,
}

impl FeedbackSource {
    /// The kebab-case wire token used in the `source/` label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::AgentFiled => "agent-filed",
            Self::Platform => "platform",
        }
    }
}

/// Builds the full four-axis label set for a feedback item.
///
/// Every issue carries `feedback` plus exactly one label from each axis:
/// `type/<category>`, `area/<surface>`, `sev/<severity>`, and
/// `source/<who>`. This is the single source of truth the filer and the triage
/// agent both consult.
pub fn classify_labels(
    item: &FeedbackItem,
    severity: Severity,
    source: FeedbackSource,
) -> Vec<String> {
    vec![
        "feedback".to_string(),
        format!("type/{}", item.category.as_str()),
        format!("area/{}", area_for(item)),
        format!("sev/{}", severity.as_str()),
        format!("source/{}", source.as_str()),
    ]
}

/// What a [`TriageAgent::dedupe`] pass decided for one candidate issue.
#[derive(Clone, Debug, PartialEq)]
pub enum DedupePlan {
    /// No earlier match; the candidate stands on its own.
    Distinct,
    /// The candidate duplicates an earlier `canonical` issue; a merge comment
    /// was posted on the canonical and the candidate (`closed`) was closed.
    Merged {
        /// The earlier issue kept as canonical.
        canonical: ExistingIssue,
        /// The candidate issue number that was closed as a duplicate.
        closed: u64,
    },
}

/// A group of similar issues that a triage agent can fold into one cluster.
#[derive(Clone, Debug, PartialEq)]
pub struct ClusterPlan {
    /// The normalized title key the members share.
    pub key: String,
    /// A human-readable summary (the first member's cleaned title).
    pub summary: String,
    /// The member issue numbers, ascending.
    pub members: Vec<u64>,
    /// The number of members.
    pub count: usize,
    /// The cluster issue title, e.g. `"12 reports: email drafts too formal"`.
    pub title: String,
}

impl ClusterPlan {
    /// The promotion score for this cluster at `severity`: `count × weight`.
    pub fn score(&self, severity: Severity) -> u32 {
        (self.count as u32).saturating_mul(severity.weight())
    }

    /// Whether this cluster crosses the promotion `threshold` at `severity`.
    pub fn promote(&self, severity: Severity, threshold: u32) -> bool {
        self.score(severity) >= threshold
    }
}

/// The escalation flags a filed issue's labels imply (`triage.md`).
///
/// Pure: the caller executes the side effects (paging, mirroring). We only
/// surface the decision so it stays inspectable and testable offline.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EscalationPlan {
    /// `sev/money-lost` issues page maintainers.
    pub pages_maintainers: bool,
    /// `area/brain` issues that reproduce upstream mirror to the owning repo.
    pub mirror_to: Option<String>,
}

/// Computes the [`EscalationPlan`] implied by an issue's `labels`.
pub fn escalation_for(labels: &[String]) -> EscalationPlan {
    let pages_maintainers = labels.iter().any(|l| l == "sev/money-lost");
    let mirror_to = labels
        .iter()
        .any(|l| l == "area/brain")
        .then(|| "tinyhumansai/medulla".to_string());
    EscalationPlan {
        pages_maintainers,
        mirror_to,
    }
}

/// Groups similar existing issues into candidate clusters.
///
/// Issues are keyed by a normalized title (lowercased, any leading `[type]`
/// prefix stripped, whitespace collapsed); a key with two or more members
/// becomes a [`ClusterPlan`]. Single issues are not clusters and are omitted.
/// Returned deterministically ordered by key.
pub fn cluster_plans(issues: &[ExistingIssue]) -> Vec<ClusterPlan> {
    let mut groups: HashMap<String, (String, Vec<u64>)> = HashMap::new();
    for issue in issues {
        let key = normalize_title(&issue.title);
        let entry = groups
            .entry(key)
            .or_insert_with(|| (strip_type_prefix(&issue.title), Vec::new()));
        entry.1.push(issue.number);
    }

    let mut plans: Vec<ClusterPlan> = groups
        .into_iter()
        .filter(|(_, (_, members))| members.len() >= 2)
        .map(|(key, (summary, mut members))| {
            members.sort_unstable();
            let count = members.len();
            let title = format!("{count} reports: {summary}");
            ClusterPlan {
                key,
                summary,
                members,
                count,
                title,
            }
        })
        .collect();
    plans.sort_by(|a, b| a.key.cmp(&b.key));
    plans
}

/// Normalizes a title into a clustering key.
fn normalize_title(title: &str) -> String {
    strip_type_prefix(title)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Strips a leading `[type] ` prefix (as minted by the filer) from a title.
fn strip_type_prefix(title: &str) -> String {
    let trimmed = title.trim();
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some(idx) = rest.find(']')
    {
        return rest[idx + 1..].trim().to_string();
    }
    trimmed.to_string()
}

/// A roster-job triage agent operating over one repo's issue tracker.
///
/// Not kernel plumbing: it is a helper a triage company (or a maintenance
/// sweep) drives against a [`GitHubClient`]. Every method is offline-testable
/// against the mock client.
pub struct TriageAgent {
    client: Arc<dyn GitHubClient>,
    repo: String,
}

impl TriageAgent {
    /// Builds a triage agent filing against `repo` through `client`.
    pub fn new(client: Arc<dyn GitHubClient>, repo: impl Into<String>) -> Self {
        Self {
            client,
            repo: repo.into(),
        }
    }

    /// Merges `candidate` into an earlier canonical issue if one exists.
    ///
    /// Searches the tracker for the candidate's title; if an *earlier* issue
    /// matches, comments on that canonical noting the merge and closes the
    /// candidate, returning [`DedupePlan::Merged`]. Otherwise the candidate is
    /// [`DedupePlan::Distinct`].
    pub async fn dedupe(&self, candidate: &ExistingIssue) -> Result<DedupePlan> {
        let hits = self
            .client
            .search_issues(&self.repo, &candidate.title)
            .await?;
        // The canonical is the earliest matching issue other than the candidate.
        let canonical = hits
            .into_iter()
            .filter(|issue| issue.number != candidate.number)
            .min_by_key(|issue| issue.number);

        match canonical {
            Some(canonical) => {
                self.client
                    .comment_issue(
                        &self.repo,
                        canonical.number,
                        &format!(
                            "Folding in duplicate #{} — same report as this issue.",
                            candidate.number
                        ),
                    )
                    .await?;
                self.client
                    .close_issue(&self.repo, candidate.number)
                    .await?;
                Ok(DedupePlan::Merged {
                    canonical,
                    closed: candidate.number,
                })
            }
            None => Ok(DedupePlan::Distinct),
        }
    }

    /// Materializes a [`ClusterPlan`]: opens a cluster issue, then comments on
    /// and closes every member, pointing each at the cluster.
    ///
    /// `area_label` is the owning `area/<…>` value (e.g.
    /// `template:marketing_agency`); it is appended to the cluster title and
    /// applied as an `area/` label. Returns the created cluster issue URL.
    pub async fn maintain_cluster(&self, plan: &ClusterPlan, area_label: &str) -> Result<String> {
        let title = format!("{} — {}", plan.title, area_label);
        let body = format!(
            "Cluster of {} similar reports: {}.\n\nMembers: {}",
            plan.count,
            plan.summary,
            plan.members
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let cluster_url = self
            .client
            .create_issue(
                &self.repo,
                &IssueDraft {
                    title,
                    body,
                    labels: vec!["feedback".to_string(), format!("area/{area_label}")],
                },
            )
            .await?;

        for member in &plan.members {
            self.client
                .comment_issue(
                    &self.repo,
                    *member,
                    &format!("Folded into cluster: {cluster_url}"),
                )
                .await?;
            self.client.close_issue(&self.repo, *member).await?;
        }
        Ok(cluster_url)
    }
}

impl std::fmt::Debug for TriageAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TriageAgent")
            .field("repo", &self.repo)
            .finish_non_exhaustive()
    }
}

/// Per-handle filing quality, used to throttle auto-consent.
///
/// A company that repeatedly files low-quality issues (e.g. filings that are
/// immediately closed as duplicates, or maintainer-flagged) has its `Auto`
/// consent downgraded to `Assisted` once its low-quality ratio crosses a
/// threshold, so the operator confirms each filing again. In-memory, mirroring
/// [`RateLimiter`](super::github::RateLimiter).
#[derive(Debug)]
pub struct QualityLedger {
    threshold: f64,
    min_samples: usize,
    filings: StdMutex<HashMap<String, Counts>>,
}

/// The per-handle filing tally.
#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    filed: usize,
    low_quality: usize,
}

impl QualityLedger {
    /// Builds a ledger that downgrades `Auto` consent once a handle has at
    /// least `min_samples` filings and a low-quality ratio at or above
    /// `threshold` (0.0–1.0).
    pub fn new(threshold: f64, min_samples: usize) -> Self {
        Self {
            threshold,
            min_samples,
            filings: StdMutex::new(HashMap::new()),
        }
    }

    /// Records that `handle` filed an issue (of any quality).
    pub fn record_filed(&self, handle: &str) {
        let mut filings = self.filings.lock().expect("quality ledger poisoned");
        filings.entry(handle.to_string()).or_default().filed += 1;
    }

    /// Records that `handle`'s most recent filing was low-quality (e.g. an
    /// immediate duplicate). Callers still call [`record_filed`](Self::record_filed)
    /// for the same filing; this only bumps the low-quality tally.
    pub fn record_low_quality(&self, handle: &str) {
        let mut filings = self.filings.lock().expect("quality ledger poisoned");
        filings.entry(handle.to_string()).or_default().low_quality += 1;
    }

    /// The consent mode `handle` effectively gets given its filing history.
    ///
    /// Only `Auto` is subject to throttling; `Manual`/`Assisted` pass through
    /// unchanged. An `Auto` handle over the low-quality threshold is downgraded
    /// to `Assisted`.
    pub fn effective_consent(&self, handle: &str, configured: ConsentMode) -> ConsentMode {
        if configured != ConsentMode::Auto {
            return configured;
        }
        let filings = self.filings.lock().expect("quality ledger poisoned");
        if let Some(counts) = filings.get(handle)
            && counts.filed >= self.min_samples
            && (counts.low_quality as f64 / counts.filed as f64) >= self.threshold
        {
            return ConsentMode::Assisted;
        }
        configured
    }
}

impl Default for QualityLedger {
    /// Downgrades after 3+ filings with at least half low-quality.
    fn default() -> Self {
        Self::new(0.5, 3)
    }
}

/// Joins fixed issue URLs to the feedback items that caused them.
///
/// The "You said, we did" contract: given the issue URLs closed by a release
/// and the company's feedback items, returns each `(issue_url, item)` pair
/// whose item links that issue, so the caller can surface *"things you flagged
/// were fixed"* to the right operators.
pub fn map_fixed_issues<'a>(
    fixed: &[String],
    items: &'a [FeedbackItem],
) -> Vec<(String, &'a FeedbackItem)> {
    let mut out = Vec::new();
    for url in fixed {
        for item in items {
            if item.filed_issue_url.as_deref() == Some(url.as_str()) {
                out.push((url.clone(), item));
            }
        }
    }
    out
}

/// Flags release-notes process bugs.
///
/// A `feedback`-labeled issue closed by a release but absent from the release
/// notes is a process bug (`triage.md`). Returns every closed feedback issue
/// URL missing from `notes_issue_urls`.
pub fn process_bug_check(
    closed_feedback_issues: &[String],
    notes_issue_urls: &[String],
) -> Vec<String> {
    closed_feedback_issues
        .iter()
        .filter(|url| !notes_issue_urls.iter().any(|n| n == *url))
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "triage_tests.rs"]
mod tests;
