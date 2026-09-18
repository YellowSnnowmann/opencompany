//! The wire shape every account-key fan-out route answers with, one line per
//! [`company_key::SlotReport`](crate::company::company_key::SlotReport)
//! (`docs/key-reworks/phase-4a-account-key-fanout.md` §3.1).
//!
//! Pulled out of [`ops::company_key`](super::company_key), the first route to
//! need it (keys rework #2306, slice 4a), so the Composio route (slice 4c)
//! can serialise the same shape rather than a second copy of it. `outcome` is
//! `filled | rotated | cleared | rolledBack | kept | skipped | failed | ok`;
//! `detail` is the camelCase
//! [`company_key::SkipReason`](crate::company::company_key::SkipReason), the
//! fixed string `"store"` for a plain
//! [`company_key::SlotOutcome::Failed`](crate::company::company_key::SlotOutcome::Failed),
//! or the probe class (`auth`, `endpoint`, …) for a health-slot failure.

use serde::Serialize;

use crate::company::company_key;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlotReportDto {
    pub(crate) slot: company_key::Slot,
    pub(crate) outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<&'static str>,
}

impl From<&company_key::SlotReport> for SlotReportDto {
    fn from(report: &company_key::SlotReport) -> Self {
        use company_key::SlotOutcome;
        let (outcome, detail) = match report.outcome {
            SlotOutcome::Filled => ("filled", None),
            SlotOutcome::Rotated => ("rotated", None),
            SlotOutcome::Cleared => ("cleared", None),
            SlotOutcome::RolledBack => ("rolledBack", None),
            SlotOutcome::Kept(reason) => ("kept", Some(skip_reason_str(reason))),
            SlotOutcome::Skipped(reason) => ("skipped", Some(skip_reason_str(reason))),
            SlotOutcome::Failed => ("failed", Some("store")),
            SlotOutcome::HealthOk => ("ok", None),
            SlotOutcome::HealthFailed(class) => ("failed", Some(class.as_str())),
        };
        Self {
            slot: report.slot,
            outcome,
            detail,
        }
    }
}

/// The camelCase wire spelling of a [`company_key::SkipReason`].
pub(crate) fn skip_reason_str(reason: company_key::SkipReason) -> &'static str {
    use company_key::SkipReason;
    match reason {
        SkipReason::AlreadyCurrent => "alreadyCurrent",
        SkipReason::CustomKey => "customKey",
        SkipReason::AlreadyEmpty => "alreadyEmpty",
        SkipReason::RowExists => "rowExists",
        SkipReason::LegacyManagedConfig => "legacyManagedConfig",
        SkipReason::NeedsModel => "needsModel",
        SkipReason::DefaultAlreadySet => "defaultAlreadySet",
        SkipReason::InferenceNotWritten => "inferenceNotWritten",
        SkipReason::InferenceRejected => "inferenceRejected",
        SkipReason::KeyCleared => "keyCleared",
        SkipReason::ProviderDisabled => "providerDisabled",
    }
}
