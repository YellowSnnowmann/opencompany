//! Read outcomes and wording for `query_company`'s sections, so a section
//! whose store failed says so instead of rendering as empty.

use std::sync::Arc;

use crate::ports::CompanyStore;
use crate::ports::types::{CompanyId, CompanyRecord};

pub(super) const FACTS_UNREADABLE: &str = "_Facts could not be read. This is not the same as \
     having none — do not record a fact again because it is missing here._\n";
pub(super) const ACTIVITY_UNREADABLE: &str = "_Recent activity could not be read. This is not \
     the same as nothing having happened._\n";
pub(super) const WORKFLOWS_PARTIAL: &str = "_The company record could not be read, so workflows \
     created at runtime are missing from this list. Do not create a workflow because it is \
     absent here — read again first._\n";
pub(super) const ROSTER_UNREADABLE: &str = "_The roster could not be read (company record \
     unreadable). Do not add or re-create teammates because of this._\n";
pub(super) const ROSTER_EMPTY: &str = "_No teammates on the roster._\n";
pub(super) const DESKS_UNREADABLE: &str = "_Desks could not be read (company record \
     unreadable). Do not create a desk or reroute work because none is listed._\n";
pub(super) const BOARD_UNREADABLE: &str = "_The board could not be read. Open cards may exist — \
     use list_tasks, and do not open a card again because it is missing here._\n";
pub(super) const BOARD_UNWIRED: &str = "_No task board is wired on this host._\n";

/// What loading the company record produced.
pub(super) enum RecordRead {
    Loaded(Box<CompanyRecord>),
    Missing,
    Failed,
    NotWired,
}

impl RecordRead {
    /// Loads `company`'s record, logging a store failure.
    pub(super) async fn load(store: Option<&Arc<dyn CompanyStore>>, company: &CompanyId) -> Self {
        let Some(store) = store else {
            return Self::NotWired;
        };
        match store.load(company).await {
            Ok(Some(record)) => Self::Loaded(Box::new(record)),
            Ok(None) => Self::Missing,
            Err(error) => {
                tracing::warn!(%company, %error, "query_company: company record read failed");
                Self::Failed
            }
        }
    }

    /// The record, when it loaded.
    pub(super) fn record(&self) -> Option<&CompanyRecord> {
        match self {
            Self::Loaded(record) => Some(record),
            _ => None,
        }
    }

    /// Whether the store answered with an error.
    pub(super) fn failed(&self) -> bool {
        matches!(self, Self::Failed)
    }
}

/// Unwraps one section's read, logging a failure and recording `section` as
/// unreadable.
pub(super) fn section<T: Default>(
    read: crate::Result<T>,
    section: &'static str,
    company: &CompanyId,
    unreadable: &mut Vec<&'static str>,
) -> T {
    match read {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%company, %error, section, "query_company: section read failed");
            unreadable.push(section);
            T::default()
        }
    }
}
