//! The company's memory, as the console's Brain page reads and manages it,
//! under both scope forms:
//!
//! | Route | What |
//! | --- | --- |
//! | `GET /memory` | a page of items (`?kind=&agent=&cursor=&limit=`), or ranked matches (`?query=`) |
//! | `POST /memory` | store a learning at the company root (an operator fact) |
//! | `DELETE /memory/{id}` | forget one item |
//! | `GET /memory/status` | whether memory is on, and with which engine |
//! | `GET /memory/agents` | the teammates with logged turns |
//! | `DELETE /memory/agents/{agent_id}` | forget one teammate's turns |
//! | `POST /memory/recall` | a synthesised answer from memory |
//! | `GET /memory/brain` | the brain's document sources |
//! | `GET /memory/traces` | the retained cycle-trace window (not memory) |
//!
//! Memory is OpenHuman's, scoped to the company ([`crate::memory`]); every
//! route goes through [`CompanyRuntime::memory`](crate::CompanyRuntime::memory),
//! which never reaches another company's subtree. Memory being off answers
//! `409 not_configured` (status excepted). A forget journals a
//! [`CompanyEvent::MemoryFactDeleted`] per the Operator-rights section of
//! `docs/spec/company-brain/memory.md`.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::OpenCompanyError;
use crate::memory::{
    BrainSources, LearningKind, MemoryAgents, MemoryItem, MemoryItemKind, MemoryPage, MemoryQuery,
    MemoryStatus, RecallAnswer,
};
use crate::ports::types::{CompanyEvent, CompressedTrace};
use crate::runtime::maintenance::TRACE_RETENTION_LIMIT;
use crate::server::error::ApiError;
use crate::server::ops::{ScopedCompany, scoped};

/// Most ranked matches a `?query=` read returns.
const SEARCH_LIMIT: usize = 50;

/// Largest page a listing may ask for.
const MAX_PAGE: usize = 200;

/// Tag on a learning an operator wrote from the console.
pub const OPERATOR_TAG: &str = "operator";

/// Builds the memory route fragment.
pub fn router() -> Router<AppState> {
    scoped("/memory", post(create_learning).get(list_items))
        .merge(scoped("/memory/status", get(memory_status)))
        .merge(scoped("/memory/agents", get(memory_agents)))
        .merge(scoped("/memory/agents/{agent_id}", delete(forget_agent)))
        .merge(scoped("/memory/recall", post(recall)))
        .merge(scoped("/memory/brain", get(brain_sources)))
        .merge(scoped("/memory/traces", get(list_traces)))
        .merge(scoped("/memory/{item_id}", delete(forget_item)))
}

/// `GET /memory` query parameters.
#[derive(Debug, Default, Deserialize)]
struct ListParams {
    /// Ranked matches for this text instead of a newest-first page.
    query: Option<String>,
    /// One item kind: `learning`, `conversation` or `document`.
    kind: Option<String>,
    /// One teammate's node.
    agent: Option<String>,
    /// The engine cursor of the next page.
    cursor: Option<String>,
    /// Page size.
    limit: Option<usize>,
}

/// `GET /memory` — a page of the company's memory, or ranked matches.
async fn list_items(
    company: ScopedCompany,
    Query(params): Query<ListParams>,
) -> Result<Json<MemoryPage>, ApiError> {
    let kind = match params.kind.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => Some(MemoryItemKind::parse(raw).ok_or_else(|| {
            OpenCompanyError::InvalidRequest(format!(
                "unknown memory kind `{raw}`: expected learning, conversation or document"
            ))
        })?),
    };
    let memory = company.runtime.memory();
    if let Some(query) = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        let items = memory.search(query, kind, SEARCH_LIMIT).await?;
        return Ok(Json(MemoryPage {
            items,
            next_cursor: None,
        }));
    }
    let page = memory
        .list(MemoryQuery {
            agent_id: params.agent.filter(|agent| !agent.trim().is_empty()),
            kind,
            tags_any: Vec::new(),
            limit: params.limit.map(|limit| limit.clamp(1, MAX_PAGE)),
            cursor: params.cursor.filter(|cursor| !cursor.is_empty()),
        })
        .await?;
    Ok(Json(page))
}

/// `POST /memory` body.
#[derive(Debug, Deserialize)]
struct CreateLearning {
    /// What the company should remember.
    text: String,
    /// What kind of statement it is; defaults to `fact`.
    #[serde(default)]
    kind: Option<LearningKind>,
}

/// `POST /memory` — an operator fact, stored as a learning at the company
/// root, where every teammate's recall reaches it.
async fn create_learning(
    company: ScopedCompany,
    Json(body): Json<CreateLearning>,
) -> Result<Json<MemoryItem>, ApiError> {
    let text = body.text.trim();
    if text.is_empty() {
        return Err(OpenCompanyError::InvalidRequest("a memory needs some text".into()).into());
    }
    let item = company
        .runtime
        .memory()
        .learn(
            text,
            body.kind.unwrap_or_default(),
            vec![OPERATOR_TAG.to_string()],
        )
        .await?;
    Ok(Json(item))
}

/// `DELETE /memory/{item_id}` path.
#[derive(Debug, Deserialize)]
struct ItemPath {
    item_id: String,
}

/// `DELETE /memory/{item_id}` — forgets one item the company holds.
async fn forget_item(
    company: ScopedCompany,
    Path(ItemPath { item_id }): Path<ItemPath>,
) -> Result<StatusCode, ApiError> {
    let forgotten = company
        .runtime
        .memory()
        .forget(vec![item_id.clone()])
        .await?;
    if forgotten == 0 {
        return Err(OpenCompanyError::NotFound(format!("memory {item_id}")).into());
    }
    journal_forget(&company, item_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /memory/status` — whether memory is on. Never an error: memory being
/// off is the answer, with the reason.
async fn memory_status(company: ScopedCompany) -> Json<MemoryStatus> {
    Json(company.runtime.memory().status().await)
}

/// `GET /memory/agents` — the teammates with logged turns.
async fn memory_agents(company: ScopedCompany) -> Result<Json<MemoryAgents>, ApiError> {
    Ok(Json(company.runtime.memory().agents().await?))
}

/// `DELETE /memory/agents/{agent_id}` path.
#[derive(Debug, Deserialize)]
struct AgentPath {
    agent_id: String,
}

/// How many items a bulk forget removed.
#[derive(Debug, Serialize)]
struct Forgotten {
    forgotten: usize,
}

/// `DELETE /memory/agents/{agent_id}` — forgets one teammate's logged turns
/// and the beliefs built from them; the shared learnings and brain stay.
async fn forget_agent(
    company: ScopedCompany,
    Path(AgentPath { agent_id }): Path<AgentPath>,
) -> Result<Json<Forgotten>, ApiError> {
    let forgotten = company.runtime.memory().forget_agent(&agent_id).await?;
    if forgotten > 0 {
        journal_forget(&company, format!("agent:{agent_id}")).await?;
    }
    Ok(Json(Forgotten { forgotten }))
}

/// `POST /memory/recall` body.
#[derive(Debug, Deserialize)]
struct RecallBody {
    /// The question.
    question: String,
    /// One teammate's turns (plus the shared learnings) instead of the whole
    /// company.
    #[serde(default)]
    agent: Option<String>,
}

/// `POST /memory/recall` — answers a question from the company's memory.
async fn recall(
    company: ScopedCompany,
    Json(body): Json<RecallBody>,
) -> Result<Json<RecallAnswer>, ApiError> {
    let question = body.question.trim();
    if question.is_empty() {
        return Err(OpenCompanyError::InvalidRequest("ask a question".into()).into());
    }
    let agent = body
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty());
    Ok(Json(
        company.runtime.memory().recall(question, agent).await?,
    ))
}

/// `GET /memory/brain` — the brain's document sources and their sizes.
async fn brain_sources(company: ScopedCompany) -> Result<Json<BrainSources>, ApiError> {
    Ok(Json(company.runtime.memory().brain_sources().await?))
}

/// Journals an operator forget to the event log (audit trail).
pub(crate) async fn journal_forget(company: &ScopedCompany, what: String) -> Result<(), ApiError> {
    company
        .runtime
        .events()
        .append(
            company.id(),
            CompanyEvent::MemoryFactDeleted { fact_id: what },
        )
        .await?;
    Ok(())
}

/// A persisted cycle trace as exposed to an operator.
///
/// The route returns the full live retention window: maintenance retains at
/// most [`TRACE_RETENTION_LIMIT`] traces, so this materialises no unbounded
/// store read.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceEntry {
    cycle_id: String,
    summary: String,
    at_millis: u64,
}

impl From<CompressedTrace> for TraceEntry {
    fn from(trace: CompressedTrace) -> Self {
        Self {
            cycle_id: trace.cycle_id,
            summary: trace.summary,
            at_millis: trace.at_millis,
        }
    }
}

/// `GET /memory/traces` — the retained, newest-last cycle trace window.
///
/// Trace summaries are intentionally not injected into a cycle: current
/// producers emit placeholders, and real compression/consumption needs its
/// own design. This inspection surface makes the durable record visible while
/// retention keeps the read bounded.
async fn list_traces(company: ScopedCompany) -> Result<Json<Vec<TraceEntry>>, ApiError> {
    let traces = company
        .runtime
        .traces()
        .recent_traces(company.id(), TRACE_RETENTION_LIMIT)
        .await?;
    Ok(Json(traces.into_iter().map(TraceEntry::from).collect()))
}

#[cfg(test)]
#[path = "memory_route_tests.rs"]
mod route_tests;
