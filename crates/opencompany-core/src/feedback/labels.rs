//! GitHub label taxonomy (`docs/spec/feedback-loop/triage.md`).
//!
//! Every filed issue carries `feedback` plus one label from each axis:
//! `type/`, `area/`, `sev/`, and `source/`. Agent-filed issues always carry
//! `source/agent-filed` so triage can weight them.

use crate::feedback::triage::{FeedbackSource, Severity, classify_labels};
use crate::feedback::types::{FeedbackCategory, FeedbackItem};

/// Builds the full label set for an agent-filed feedback issue.
///
/// A thin wrapper over [`classify_labels`](crate::feedback::triage::classify_labels)
/// — the single source of truth — with the agent-filer defaults: `sev/annoyance`
/// and `source/agent-filed`. The `type/` label follows the category; `area/`
/// defaults per category (or `template:<name>` for a template gap with a known
/// template).
pub fn labels_for(item: &FeedbackItem) -> Vec<String> {
    classify_labels(item, Severity::Annoyance, FeedbackSource::AgentFiled)
}

/// The owning surface for a feedback item.
pub(crate) fn area_for(item: &FeedbackItem) -> String {
    if item.category == FeedbackCategory::TemplateGap
        && let Some(name) = &item.template_name
        && !name.trim().is_empty()
    {
        return format!("template:{name}");
    }
    match item.category {
        FeedbackCategory::WrongOutput => "brain",
        FeedbackCategory::Bug | FeedbackCategory::ApprovalFriction => "runtime",
        FeedbackCategory::MissingCapability | FeedbackCategory::TemplateGap => "product",
        FeedbackCategory::Docs => "product",
    }
    .to_string()
}

#[cfg(test)]
#[path = "labels_tests.rs"]
mod tests;
