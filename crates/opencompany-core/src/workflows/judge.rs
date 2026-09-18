//! The opt-in semantic sufficiency gate for workflow agent nodes (issue #1866).

use std::time::Duration;

use serde::Deserialize;
use tinyinference::message::Message;
use tinyinference::model::{ModelRequest, ModelResponse};

use crate::harness::HarnessDeps;
use crate::harness::build::model_for_tier;
use crate::ports::blockers::BlockerKind;
use crate::ports::types::{CompanyId, CompanyRecord, TokenUsage};
use crate::runtime::delegation::RunTurn;

const JUDGE_TIMEOUT: Duration = Duration::from_secs(30);
/// The wall-clock bound on the single peer turn the last recovery rung spends.
/// Wider than [`JUDGE_TIMEOUT`] because a peer runs a full tool-using turn, not
/// a one-shot completion; a consultation that overruns it yields no evidence.
const PEER_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT_TOKENS: u32 = 256;
const MAX_QUERY_CHARS: usize = 800;
const MAX_RECOVERY_ITEMS: usize = 3;
const MAX_RECOVERY_CHARS: usize = 4_000;
const INSTRUCTION_CAP: usize = 8_000;
const INSTRUCTION_TAIL_RESERVE: usize = 2_000;
const OUTPUT_CAP: usize = 20_000;
const RECOVERED_CONTEXT_HEADING: &str = "\n\nRecovered company context:\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SufficiencyVerdict {
    Continue,
    Retry,
    Recover,
    Escalate { gap: BlockerKind },
    HaltBenign,
}

impl SufficiencyVerdict {
    pub const fn token(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Retry => "retry",
            Self::Recover => "recover",
            Self::Escalate { .. } => "escalate",
            Self::HaltBenign => "halt_benign",
        }
    }

    pub const fn tokens() -> [&'static str; 5] {
        [
            Self::Continue.token(),
            Self::Retry.token(),
            Self::Recover.token(),
            Self::Escalate {
                gap: BlockerKind::Information,
            }
            .token(),
            Self::HaltBenign.token(),
        ]
    }

    fn from_parts(token: &str, gap: Option<&str>) -> Option<Self> {
        let tokens = Self::tokens();
        Some(if token == tokens[0] {
            Self::Continue
        } else if token == tokens[1] {
            Self::Retry
        } else if token == tokens[2] {
            Self::Recover
        } else if token == tokens[3] {
            Self::Escalate {
                gap: BlockerKind::from_wire(gap?)?,
            }
        } else if token == tokens[4] {
            Self::HaltBenign
        } else {
            return None;
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct JudgeInput<'a> {
    pub instruction: &'a str,
    pub output: &'a str,
    pub criteria: Option<&'a str>,
    pub execution_failed: bool,
}

#[derive(Debug, Deserialize)]
struct JudgeAnswer {
    verdict: String,
    #[serde(default)]
    gap: Option<String>,
}

fn system_prompt() -> String {
    let [
        continue_token,
        retry_token,
        recover_token,
        escalate_token,
        halt_token,
    ] = SufficiencyVerdict::tokens();
    let gaps = [
        BlockerKind::Information.as_str(),
        BlockerKind::Infrastructure.as_str(),
        BlockerKind::Transient.as_str(),
    ]
    .join("|");
    format!(
        "Judge whether one workflow node's output is semantically sufficient. Return one JSON \
         object only: {{\"verdict\":\"TOKEN\",\"gap\":\"GAP\"}}. TOKEN is exactly one of \
         {continue_token}|{retry_token}|{recover_token}|{escalate_token}|{halt_token}. GAP is \
         required only for {escalate_token} and is exactly one of {gaps}. Choose {continue_token} \
         when the output fulfills the instruction and criteria. Choose {retry_token} for an \
         inadequate attempt likely fixed by rerunning. Choose {recover_token} only when existing \
         company knowledge may fill a missing fact. Choose {escalate_token} when a person or \
         infrastructure change is required. Choose {halt_token} only when the requested work is \
         intentionally unnecessary or already satisfied. Never choose {halt_token} for blank, \
         failed, errored, refused, truncated, or otherwise insufficient output. Do not use tools."
    )
}

fn user_prompt(input: JudgeInput<'_>) -> String {
    format!(
        "INSTRUCTION:\n{}\n\nSUCCESS CRITERIA:\n{}\n\nEXECUTION FAILED: {}\n\nOUTPUT:\n{}",
        cap_keeping_tail(input.instruction, INSTRUCTION_CAP, INSTRUCTION_TAIL_RESERVE),
        input.criteria.unwrap_or("No additional criteria supplied."),
        input.execution_failed,
        cap(input.output, OUTPUT_CAP),
    )
}

/// Composes a reply and the evidence recovery found into the text the
/// re-verification pass judges, keeping the evidence inside [`OUTPUT_CAP`].
pub(crate) fn augment_with_recovery(reply: &str, evidence: &str) -> String {
    let block = format!("{RECOVERED_CONTEXT_HEADING}{evidence}");
    let room = OUTPUT_CAP.saturating_sub(block.chars().count() + 1);
    format!("{}{block}", cap(reply, room))
}

fn parse_verdict(text: &str) -> Option<SufficiencyVerdict> {
    let answer: JudgeAnswer = serde_json::from_str(text.trim()).ok()?;
    SufficiencyVerdict::from_parts(answer.verdict.trim(), answer.gap.as_deref().map(str::trim))
}

fn enforce_anti_suppression(
    verdict: SufficiencyVerdict,
    input: JudgeInput<'_>,
) -> SufficiencyVerdict {
    if verdict == SufficiencyVerdict::HaltBenign
        && (input.execution_failed || input.output.trim().is_empty())
    {
        SufficiencyVerdict::Retry
    } else {
        verdict
    }
}

pub async fn judge_sufficiency(
    deps: &HarnessDeps,
    company: &CompanyId,
    input: JudgeInput<'_>,
) -> SufficiencyVerdict {
    if crate::harness::HarnessPool::total_ceiling_spent(company, deps).await {
        tracing::info!(
            company = %company,
            "[capability-budget] total token ceiling reached; skipping the sufficiency judge \
             (no model call)"
        );
        return SufficiencyVerdict::Retry;
    }

    let model = deps
        .model_override
        .clone()
        .unwrap_or_else(|| model_for_tier(None));
    let request = ModelRequest {
        messages: vec![
            Message::system(system_prompt()),
            Message::user(user_prompt(input)),
        ],
        model: Some(model),
        temperature: Some(crate::company::inference::dialect::DETERMINISTIC),
        max_tokens: Some(MAX_OUTPUT_TOKENS),
        ..ModelRequest::default()
    };
    let response = match tokio::time::timeout(JUDGE_TIMEOUT, deps.provider.invoke(&(), request))
        .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => {
            tracing::warn!(company = %company, error = %err, "workflow sufficiency judge failed");
            return SufficiencyVerdict::Retry;
        }
        Err(_) => {
            tracing::warn!(company = %company, "workflow sufficiency judge timed out");
            return SufficiencyVerdict::Retry;
        }
    };

    let usage = usage_from(&response);
    crate::metering::record_judge_usage(
        &usage,
        &deps.provider.telemetry_provider_id(),
        deps.provider.telemetry_model(),
        company,
        deps.store.as_ref(),
        deps.meter.as_deref(),
    )
    .await;

    let verdict = parse_verdict(&response.text()).unwrap_or(SufficiencyVerdict::Retry);
    enforce_anti_suppression(verdict, input)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryResult {
    pub evidence: Option<String>,
    pub log: String,
}

const MAX_FOCUS_TERMS: usize = 6;
const MIN_FOCUS_TERM_LEN: usize = 4;

/// Candidate single-word `FactStore` queries pulled out of a natural-language
/// question (issue #1990 review, #3903591432): `FactStore::list`'s query is a
/// case-insensitive substring match over a fact's title + body, so the whole
/// question almost never appears verbatim even when a fact IS the answer — a
/// fact titled "Renewal date" never matches the sentence "The answer must
/// include the renewal date". Lowercased, deduplicated, common short/filler
/// words dropped, capped so the bounded recovery ladder stays bounded.
fn focus_terms(question: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the",
        "and",
        "that",
        "this",
        "with",
        "from",
        "must",
        "have",
        "will",
        "shall",
        "should",
        "answer",
        "include",
        "includes",
        "criteria",
        "success",
        "about",
        "into",
        "onto",
        "than",
        "then",
        "when",
        "what",
        "which",
        "specifically",
        "please",
        "need",
        "needs",
    ];
    let mut terms = Vec::new();
    for word in question.split(|c: char| !c.is_alphanumeric()) {
        let lower = word.to_lowercase();
        if lower.chars().count() < MIN_FOCUS_TERM_LEN {
            continue;
        }
        if STOPWORDS.contains(&lower.as_str()) || terms.contains(&lower) {
            continue;
        }
        terms.push(lower);
        if terms.len() >= MAX_FOCUS_TERMS {
            break;
        }
    }
    terms
}

/// What a workflow node lends the last recovery rung so it can put the question
/// to a teammate: the turn seam a peer runs on, the roster it is chosen from,
/// and the node's own agent, which is never its own peer.
pub struct PeerConsult<'a> {
    pub turn: &'a dyn RunTurn,
    pub record: &'a CompanyRecord,
    pub exclude_agent: &'a str,
    /// The workflow this recovery rung belongs to (issue #2150), so the
    /// peer's turn can be scoped with the same `RunOrigin::Dispatched`
    /// trust a workflow agent node's own turn carries.
    pub workflow_id: &'a str,
}

/// How the one peer consultation ended, so `ask_around` can log each outcome
/// distinctly rather than collapsing "nobody knew" into "nobody was asked".
enum PeerOutcome {
    Answered {
        id: String,
        role: String,
        answer: String,
    },
    Empty {
        id: String,
    },
    Failed {
        id: String,
        error: String,
    },
    TimedOut {
        id: String,
    },
    NoRoleMatched,
    CeilingReached,
}

/// The roster entry whose role, description or name shares the most
/// [`focus_terms`] with the question, or `None` when none shares any.
///
/// Pure and model-free: selecting a peer must not itself cost a model call.
/// Ties keep roster order, so one roster and question always pick one peer.
fn pick_peer(
    record: &CompanyRecord,
    question: &str,
    exclude_agent: &str,
) -> Option<crate::company::Agent> {
    let terms = focus_terms(question);
    if terms.is_empty() {
        return None;
    }
    let mut best: Option<(usize, crate::company::Agent)> = None;
    for agent in record.effective_agents() {
        if agent.id == exclude_agent {
            continue;
        }
        let haystack = format!(
            "{} {} {}",
            agent.role,
            agent.description.as_deref().unwrap_or_default(),
            agent.name.as_deref().unwrap_or_default()
        )
        .to_lowercase();
        let score = terms
            .iter()
            .filter(|term| haystack.contains(term.as_str()))
            .count();
        if score > 0 && best.as_ref().is_none_or(|(top, _)| score > *top) {
            best = Some((score, agent));
        }
    }
    best.map(|(_, agent)| agent)
}

fn peer_prompt(question: &str) -> String {
    format!(
        "A workflow step cannot finish because it is missing company information. Answer from \
         what you already know, in a few sentences. If you do not know, reply with nothing at \
         all.\n\n{question}"
    )
}

/// Puts the question to exactly one peer, under [`PEER_TIMEOUT`], through
/// `run_background` — a consultation is not a workflow node and must not be
/// tagged as one on the console's run trace.
async fn consult_peer(
    deps: &HarnessDeps,
    company: &CompanyId,
    question: &str,
    consult: &PeerConsult<'_>,
) -> PeerOutcome {
    if crate::harness::HarnessPool::total_ceiling_spent(company, deps).await {
        tracing::info!(
            company = %company,
            "[capability-budget] total token ceiling reached; skipping the peer recovery rung \
             (no model call)"
        );
        return PeerOutcome::CeilingReached;
    }
    let Some(peer) = pick_peer(consult.record, question, consult.exclude_agent) else {
        return PeerOutcome::NoRoleMatched;
    };
    let ask = peer_prompt(question);
    // Issue #2150: the peer's turn is dispatched by this recovery rung, not
    // asked by an operator — the same trust a workflow agent node's own turn
    // carries, named for the peer actually chosen rather than the node that
    // raised the question.
    let origin = crate::harness::built_in::run_origin::claim(
        crate::harness::built_in::run_origin::RunOrigin::Dispatched {
            agent: peer.id.clone(),
            source: crate::harness::built_in::run_origin::DispatchSource::Workflow {
                workflow_id: consult.workflow_id.to_string(),
            },
            scope: None,
        },
    );
    match tokio::time::timeout(
        PEER_TIMEOUT,
        origin.scoped(Box::pin(
            consult.turn.run_background(company, &peer.id, &ask, None),
        )),
    )
    .await
    {
        Ok(Ok(outcome)) => {
            let answer = cap(&outcome.reply, MAX_RECOVERY_CHARS);
            if answer.is_empty() {
                PeerOutcome::Empty { id: peer.id }
            } else {
                PeerOutcome::Answered {
                    id: peer.id,
                    role: peer.role,
                    answer,
                }
            }
        }
        Ok(Err(err)) => PeerOutcome::Failed {
            id: peer.id,
            error: err.to_string(),
        },
        Err(_) => PeerOutcome::TimedOut { id: peer.id },
    }
}

/// Bounded fact → workspace → one-peer recovery, in that order, each rung run
/// only when every rung above it found nothing.
///
/// The first two rungs are local reads. The third spends one turn on exactly
/// one roster peer chosen by [`pick_peer`] and bounded by [`PEER_TIMEOUT`] —
/// never a broadcast; `consult = None` skips it. Each rung's outcome is
/// recorded in [`RecoveryResult::log`], which reaches the operator's blocker
/// card when the ladder ends empty.
pub async fn ask_around(
    deps: &HarnessDeps,
    company: &CompanyId,
    question: &str,
    consult: Option<PeerConsult<'_>>,
) -> RecoveryResult {
    let query = cap(question, MAX_QUERY_CHARS);
    let mut evidence = Vec::new();
    let mut log = Vec::new();

    if let Some(facts) = deps.facts.as_ref() {
        let mut queries = vec![query.clone()];
        queries.extend(focus_terms(&query));
        let mut seen_ids = std::collections::HashSet::new();
        let mut query_errors = Vec::new();
        'queries: for q in &queries {
            match facts.list(company, Some(q), None).await {
                Ok(rows) => {
                    for row in rows {
                        if evidence.len() >= MAX_RECOVERY_ITEMS {
                            break 'queries;
                        }
                        if seen_ids.insert(row.id.clone()) {
                            evidence.push(format!("fact: {} — {}", row.title, row.body));
                        }
                    }
                }
                Err(err) => query_errors.push(err.to_string()),
            }
        }
        if evidence.is_empty() && !query_errors.is_empty() {
            log.push(format!("facts: unavailable ({})", query_errors.join("; ")));
        } else {
            log.push(format!("facts: {} match(es)", evidence.len()));
        }
    } else {
        log.push("facts: unavailable".to_string());
    }

    if evidence.is_empty() {
        match deps
            .context
            .search(company, &query, MAX_RECOVERY_ITEMS)
            .await
        {
            Ok(hits) => {
                for hit in hits {
                    evidence.push(format!("workspace: {}", hit.snippet));
                }
                log.push(format!("workspace: {} match(es)", evidence.len()));
            }
            Err(err) => log.push(format!("workspace: unavailable ({err})")),
        }
    } else {
        log.push("workspace: skipped after fact match".to_string());
    }

    if evidence.is_empty() {
        match consult {
            None => log.push("peer: skipped; no consultation handle".to_string()),
            Some(consult) => match consult_peer(deps, company, &query, &consult).await {
                PeerOutcome::Answered { id, role, answer } => {
                    evidence.push(format!("peer {id} ({role}): {answer}"));
                    log.push(format!("peer: {id} answered"));
                }
                PeerOutcome::Empty { id } => log.push(format!("peer: {id} had no answer")),
                PeerOutcome::Failed { id, error } => {
                    log.push(format!("peer: {id} could not answer ({error})"));
                }
                PeerOutcome::TimedOut { id } => log.push(format!("peer: {id} timed out")),
                PeerOutcome::NoRoleMatched => {
                    log.push("peer: skipped; no role matched the question".to_string());
                }
                PeerOutcome::CeilingReached => {
                    log.push("peer: skipped; token ceiling reached".to_string());
                }
            },
        }
    } else {
        log.push("peer: skipped after local match".to_string());
    }

    let joined = cap(&evidence.join("\n"), MAX_RECOVERY_CHARS);
    RecoveryResult {
        evidence: (!joined.is_empty()).then_some(joined),
        log: log.join("; "),
    }
}

/// Caps `text` to `max` characters while keeping its last `tail` characters,
/// which is where [`compose_turn_message`](crate::workflows::caps) leaves the
/// run's own request.
fn cap_keeping_tail(text: &str, max: usize, tail: usize) -> String {
    let trimmed = text.trim();
    let total = trimmed.chars().count();
    if total <= max {
        return trimmed.to_string();
    }
    let tail = tail.min(max);
    let head = max - tail;
    let head_text: String = trimmed.chars().take(head).collect();
    let tail_text: String = trimmed.chars().skip(total - tail).collect();
    format!(
        "{head_text}\n[… {} characters elided …]\n{tail_text}",
        total - max
    )
}

fn cap(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    trimmed.chars().take(max).collect::<String>() + "…"
}

fn usage_from(response: &ModelResponse) -> TokenUsage {
    let tokens = response.usage.unwrap_or_default();
    let cost_usd = response
        .raw
        .as_ref()
        .and_then(|raw| raw.pointer("/openhuman_usage_meta/charged_amount_usd"))
        .and_then(serde_json::Value::as_f64)
        .filter(|cost| cost.is_finite() && *cost > 0.0)
        .unwrap_or(0.0);
    TokenUsage {
        input: tokens.input_tokens,
        output: tokens.output_tokens,
        cached_input: tokens.cache_read_tokens,
        cost_usd,
    }
}

#[cfg(test)]
#[path = "judge_parsing_tests.rs"]
mod tests_parsing;
#[cfg(test)]
#[path = "judge_peer_tests.rs"]
mod tests_peer;
