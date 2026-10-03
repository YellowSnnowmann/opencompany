//! The first-run setup pass: one tool-less model call that designs a company's
//! starting team from three answers (`docs/spec/runtime/company-setup.md`).
//!
//! A sibling of [`planning`](super::planning) and
//! [`workflow_build`](super::workflow_build), built the same way and bounded the
//! same way.
//!
//! ## The model designs the team; the host bounds its shape
//!
//! The whole promise of asking someone about their business is that the answer
//! changes what they get. So the model authors the roster: the roles, the
//! line-up, and each agent's mandate all come from what the operator actually
//! said. Someone running a shop and a YouTube channel gets both staffed.
//!
//! What the host keeps is the *shape*, enforced after the fact by
//! [`validate_roster`](crate::company::setup::validate_roster) rather than
//! trusted to a prompt: four to six agents, no duplicate roles, mandates that
//! fit on a card. A prompt is advice; validation is a boundary.
//!
//! [`match_template`](crate::company::setup::match_template) still runs first,
//! and its curated roster does two jobs here — neither of them constraining the
//! answer:
//!
//! * **A quality bar.** It goes into the prompt as a reference team, so the
//!   model can see the register the mandates are written in rather than
//!   inferring it. It is explicitly not a menu to pick from.
//! * **The floor.** Every way this pass can fail — no credential, a timeout, an
//!   unreadable answer, an empty roster — lands on that curated team. So the
//!   fallback is a real industry roster rather than an apology, which is what
//!   makes the never-strand rule (decision D3) cheap to keep. See
//!   [`RosterBuilder::propose`].
//!
//! ## The operator's answers are data, never instructions
//!
//! All three answers are free text a person typed. They are the *subject* of the
//! call, and the system prompt says so: text asking the model to change its
//! output format or invent unrelated agents is described, not obeyed. The blast
//! radius is small by construction — the worst a hostile answer can do is
//! produce a silly roster the operator immediately edits, because this pass has
//! no tools, writes nothing, and hands its result back for the console to create
//! through the ordinary `POST {scope}/team` route.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tinyinference::message::Message;
use tinyinference::model::{ModelRequest, ModelResponse};

use crate::company::setup::{
    AgentFocus, FallbackReason, MIN_AGENTS, ProposedAgent, RosterProposal, RosterSource,
    SetupAnswers, is_entirely_reference_team, job_items, match_template, template_proposal,
    uncovered_indices, validate_roster,
};
use crate::harness::HarnessDeps;
use crate::harness::build::model_for_tier;
use crate::harness::provider::HarnessModel;
use crate::ports::types::TokenUsage;

/// How long the pass may spend inside the model call before it is abandoned.
///
/// Much tighter than planning's 120s, because the two are waited on differently:
/// a planning pass runs against a card in a column, while this one runs with a
/// person watching a build-out screen on their first minute in the product. A
/// slow provider should cost them the curated template a few seconds in, not a
/// blank screen for two minutes.
const SETUP_TIMEOUT: Duration = Duration::from_secs(45);

/// Output-token ceiling. A roster is six short rows; this stops a model that has
/// decided to write prose from spending a new company's budget on its first act.
///
/// Sized for reasoning models, which spend part of the budget on deliberation
/// before the first visible token. At 1,500 a reasoning model could exhaust it
/// on reasoning alone and return nothing usable (#2533).
const MAX_OUTPUT_TOKENS: u32 = 4_000;

/// Rewrites a curated roster in the operator's terms. One model call, no tools,
/// no retry.
pub struct RosterBuilder {
    model: Arc<dyn HarnessModel>,
    model_name: String,
}

impl RosterBuilder {
    /// Builds a builder over an explicit model.
    pub fn new(model: Arc<dyn HarnessModel>, model_name: impl Into<String>) -> Self {
        Self {
            model,
            model_name: model_name.into(),
        }
    }

    /// Builds the company's setup builder from the harness deps — the **same**
    /// `Arc<dyn HarnessModel>` the roster runs on, exactly as
    /// [`WorkflowBuilder::from_deps`](super::workflow_build::WorkflowBuilder::from_deps),
    /// so a console BYOK switch re-points setup with no second credential path.
    pub fn from_deps(deps: &HarnessDeps) -> Self {
        let model_name = deps
            .model_override
            .clone()
            .unwrap_or_else(|| model_for_tier(None));
        Self::new(deps.provider.clone(), model_name)
    }

    /// Builds a pass with **no company behind it**, for first-run setup.
    ///
    /// The merged wizard runs before any company exists, so there is no
    /// `CompanyRuntime` to hang a builder off and no `HarnessDeps` to build one
    /// from. The credential is resolved in the order the operator would expect:
    ///
    /// 1. `credential` — what they just typed into the wizard. It is used
    ///    without being persisted, so the apply that writes `config.toml` stays
    ///    a single atomic step rather than a write-then-generate sequence that
    ///    can half-land.
    /// 2. otherwise whatever the host already has
    ///    ([`harness_inference_from_env`]), which covers a laptop that was
    ///    already configured and a hosted tenant whose control plane injected
    ///    one.
    ///
    /// `None` when neither yields a credential — the caller then ships the
    /// curated team, which is a supported answer rather than a failure.
    ///
    /// ## Deliberately unmetered
    ///
    /// [`crate::metering::roster_build`] charges the company bucket, and here
    /// there is no company to charge: the call happens before the thing that
    /// would be billed exists. Inventing an attribution — a placeholder id, the
    /// company that is *about* to be created — would put a row in a Usage view
    /// for a period the company did not exist. One unbilled call per install is
    /// the honest trade.
    ///
    /// `api_url` is the host's resolved platform URL, so a typed managed key
    /// is probed against the platform that minted it — `config.toml` can name
    /// staging where the environment says nothing.
    pub fn for_setup(
        env: &dyn crate::app::config::EnvSource,
        api_url: Option<&str>,
        provider: Option<&str>,
        base_url: Option<&str>,
        credential: Option<&str>,
        model: Option<&str>,
    ) -> Option<Self> {
        use crate::harness::provider::{
            DEFAULT_HOSTED_MODEL, HostedProvider, HostedProviderConfig,
            harness_inference_from_env_at, platform_inference_url_at,
        };

        let selected_provider = provider.map(str::trim).filter(|value| !value.is_empty());
        if let Some(provider) = selected_provider.filter(|provider| *provider != "managed") {
            let base_url = crate::company::inference::normalize_setup_base_url(provider, base_url)
                .unwrap_or_else(|| crate::company::inference::effective_base_url(provider, None));
            let credential = credential
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map_or(crate::company::Credential::None, |key| {
                    crate::company::Credential::from_value(key.to_string())
                });
            // The attribution headers come from the catalogue, not from a
            // literal here. This block used to spell the referer
            // `https://opencompany.ai` while the turn path
            // (`harness::built_in::provider`) sent
            // `https://opencompany.tinyhumans.ai`, so one company's traffic
            // arrived in OpenRouter's dashboard as two apps. Nothing compared
            // the two copies, which is exactly why there were two.
            let extra_headers =
                if crate::company::inference::normalize_provider(provider) == "openrouter" {
                    vec![
                        (
                            "HTTP-Referer".to_string(),
                            crate::company::inference::catalogue::OPENROUTER_REFERER.to_string(),
                        ),
                        (
                            "X-Title".to_string(),
                            crate::company::inference::catalogue::OPENROUTER_TITLE.to_string(),
                        ),
                    ]
                } else {
                    Vec::new()
                };
            let model_name = model
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .unwrap_or(DEFAULT_HOSTED_MODEL);
            return Some(Self::new(
                Arc::new(HostedProvider::new_direct(
                    HostedProviderConfig {
                        base_url,
                        credential,
                        extra_headers,
                    },
                    provider,
                )),
                model_name,
            ));
        }

        let typed = credential
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(|key| {
                let base_url = platform_inference_url_at(env, api_url);
                (
                    HostedProviderConfig {
                        base_url,
                        credential: crate::company::Credential::from_value(key.to_string()),
                        extra_headers: Vec::new(),
                    },
                    env.get("OPENCOMPANY_INFERENCE_MODEL"),
                )
            });

        let (config, model_override) =
            typed.or_else(|| harness_inference_from_env_at(env, api_url))?;
        let model_name = model
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string)
            .or(model_override)
            .unwrap_or_else(|| DEFAULT_HOSTED_MODEL.to_string());
        Some(Self::new(Arc::new(HostedProvider::new(config)), model_name))
    }

    /// The provider slug this pass's usage is metered under, read live so a BYOK
    /// switch re-attributes the next pass.
    pub fn provider_slug(&self) -> String {
        self.model.telemetry_provider_id()
    }

    /// The model this pass's usage is metered against, read live off the
    /// provider and already folded onto the closed vocabulary (issue #1749).
    /// `None` before the provider has issued a turn, or when it cannot name a
    /// model.
    pub fn model_slug(&self) -> Option<crate::metering::ModelSlug> {
        self.model.telemetry_model()
    }

    /// Proposes a roster for these answers.
    ///
    /// **Infallible by design.** There is no `Result`, because there is no
    /// failure a caller could usefully handle: every unhappy path returns the
    /// curated template that was already chosen, and the returned
    /// [`RosterProposal::generated`] says which happened. The usage is returned
    /// alongside so the caller can meter what was genuinely spent — including
    /// on a call that came back unreadable, because those tokens were still
    /// billed.
    pub async fn propose(&self, answers: &SetupAnswers) -> (RosterProposal, TokenUsage) {
        let template = match_template(answers);
        let jobs = job_items(&answers.automate);
        // The reason travels with the fallback, because the operator's next
        // move depends on it: "add a key" and "tell us more" are different
        // sentences, and one covering both is too vague to act on.
        let fallback = |reason: FallbackReason| template_proposal(answers, reason);
        // One deadline for the whole pass, not one per call. The re-ask below is
        // a second call, and the thing being bounded is how long a person stares
        // at a build-out screen — which does not double because the host decided
        // to check its own work.
        let deadline = Instant::now() + SETUP_TIMEOUT;

        let first = self
            .attempt(
                Message::user(user_prompt(template, answers, &jobs)),
                deadline,
            )
            .await;
        let mut usage = first.usage;
        let Some(drafted) = first.roster else {
            // `attempt` reports no roster for two different reasons, and only it
            // knows which: a call that never landed, or an answer that could not
            // be read. Carried through rather than guessed at here.
            return (fallback(first.reason), usage);
        };

        let mut best = drafted;
        // Coverage is judged against the roster that SURVIVED validation, not
        // the draft: an agent dropped as a duplicate cannot own a job, and
        // counting its claim would report a gap as covered.
        let mut gaps = uncovered_indices(&jobs, &best.claimed);

        // One re-ask, naming the gaps. Bounded at one because a second is a
        // conversation, and this pass runs while someone waits: if naming the
        // missing jobs outright did not produce an owner for them, a third
        // phrasing of the same request is unlikely to, and the honest move is to
        // hand the operator the gap rather than spend their first minute hiding
        // it.
        if !gaps.is_empty() && Instant::now() < deadline {
            tracing::info!(
                template = template.key,
                uncovered = gaps.len(),
                "[setup] the roster left jobs unowned; asking once more"
            );
            let retry = self
                .attempt(
                    Message::user(retry_prompt(&best.agents, &jobs, &gaps)),
                    deadline,
                )
                .await;
            usage.fold(&retry.usage);
            if let Some(second) = retry.roster {
                let still = uncovered_indices(&jobs, &second.claimed);
                // Kept only if it actually covers more. A re-ask that trades one
                // gap for another has not improved the roster, and swapping to it
                // would churn a team the first pass had already got right.
                if still.len() < gaps.len() {
                    gaps = still;
                    best = second;
                }
            }
        }

        // Too thin to be a company: take the curated team WHOLE rather than
        // padding the model's answer with strangers.
        //
        // This is the decision that used to live inside `validate_roster` as a
        // silent top-up, and moving it here is the point. Padding produced a
        // roster that was part-authored and part-canned with no way to tell
        // which — a yoga studio was handed a Content Strategist it had never
        // asked for, presented exactly like the three agents it had. An operator
        // now always sees one authored team or the other.
        if best.agents.len() < MIN_AGENTS {
            tracing::info!(
                template = template.key,
                returned = best.agents.len(),
                minimum = MIN_AGENTS,
                "[setup] the model's roster was too thin to be a company; shipping the curated one"
            );
            return (fallback(FallbackReason::NotDesignable), usage);
        }

        let uncovered: Vec<String> = gaps.iter().filter_map(|i| jobs.get(*i).cloned()).collect();
        // A roster that is entirely the reference team is the reference team,
        // whatever produced it. Reporting it as designed would put "built from
        // what you told us" over a roster nobody designed — and the review
        // screen's provenance sentence is the one thing there an operator cannot
        // check for themselves.
        if is_entirely_reference_team(&best.agents, template) {
            tracing::info!(
                template = template.key,
                "[setup] the model returned the reference team unchanged; reporting it as curated"
            );
            let mut proposal = fallback(FallbackReason::NotDesignable);
            proposal.jobs = jobs;
            return (proposal, usage);
        }

        if !uncovered.is_empty() {
            tracing::info!(
                template = template.key,
                uncovered = uncovered.len(),
                "[setup] shipping a roster with unowned jobs, reported to the operator"
            );
        }

        (
            RosterProposal {
                agents: best.agents,
                template_key: template.key,
                source: RosterSource::Model,
                jobs,
                uncovered,
                // A designed roster has no fallback reason to report.
                reason: None,
            },
            usage,
        )
    }

    /// One model call, parsed and validated. Never fails upward: an unreachable
    /// model, a timeout and an unreadable answer all yield "no roster from this
    /// attempt", but only the first two are `ModelUnreachable` — an unreadable
    /// answer is `NotDesignable`, and the caller's next step differs for the two.
    async fn attempt(&self, message: Message, deadline: Instant) -> Attempt {
        let now = Instant::now();
        if now >= deadline {
            return Attempt::unreachable();
        }
        let budget = deadline - now;

        let request = ModelRequest {
            messages: vec![Message::system(system_prompt()), message],
            model: Some(self.model_name.clone()),
            temperature: Some(crate::company::inference::dialect::DETERMINISTIC),
            max_tokens: Some(MAX_OUTPUT_TOKENS),
            ..ModelRequest::default()
        };

        let response = match tokio::time::timeout(budget, self.model.invoke(&(), request)).await {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => {
                // A response that stopped on `finish_reason: length` with no
                // visible content was a real, billed round trip: the model
                // answered, it just ran out of output budget. That is not a
                // connectivity failure, and the operator's next move is not to
                // check the network or the key (#2533).
                if is_output_budget_exhausted(&err) {
                    tracing::warn!(
                        error = %err,
                        max_output_tokens = MAX_OUTPUT_TOKENS,
                        "[setup] the model ran out of output tokens before it answered; \
                         setup caps its reply at max_output_tokens, so retry or pick a \
                         model with more output headroom in Settings"
                    );
                    return Attempt {
                        roster: None,
                        usage: TokenUsage::default(),
                        reason: FallbackReason::OutputBudgetExhausted,
                    };
                }
                tracing::info!(error = %err, "[setup] the model could not be reached");
                return Attempt::unreachable();
            }
            Err(_elapsed) => {
                tracing::info!(
                    seconds = SETUP_TIMEOUT.as_secs(),
                    "[setup] the model did not answer in time"
                );
                return Attempt::unreachable();
            }
        };

        let usage = usage_from(&response);
        let Some(draft) = parse_draft(&response.text()) else {
            tracing::info!("[setup] the model's answer could not be read as a roster");
            // Reached, answered, unreadable. Not a connectivity problem, so the
            // operator's next move is "say more", not "add a key".
            return Attempt {
                roster: None,
                usage,
                reason: FallbackReason::NotDesignable,
            };
        };

        Attempt {
            roster: Some(Drafted::from_draft(draft)),
            usage,
            reason: FallbackReason::NotDesignable,
        }
    }
}

/// Whether a model error is an empty turn that stopped on the output-token
/// limit, as opposed to a transport or provider failure.
///
/// The provider layer folds the stop reason into the error text
/// (`... (finish_reason: length; ...)`) and does not expose it structurally, so
/// this matches on both halves: the empty-turn message and the `length` stop.
fn is_output_budget_exhausted(err: &tinyinference::Error) -> bool {
    let text = err.to_string();
    text.contains("carried neither") && text.contains("finish_reason: length")
}

/// What one call produced. `usage` is reported whether or not a roster came
/// back — an unreadable answer was still billed.
struct Attempt {
    roster: Option<Drafted>,
    usage: TokenUsage,
    /// Why `roster` is `None`. Meaningless when it is `Some`.
    reason: FallbackReason,
}

impl Attempt {
    /// No roster, because the call never landed — a timeout, or a provider that
    /// could not be reached.
    ///
    /// This is [`FallbackReason::ModelUnreachable`], not [`NoModel`](FallbackReason::NoModel):
    /// a builder exists (that is why the call was made), so the operator's
    /// credential is not the thing to fix.
    fn unreachable() -> Self {
        Self {
            roster: None,
            usage: TokenUsage::default(),
            reason: FallbackReason::ModelUnreachable,
        }
    }
}

/// A validated roster and the job indices its surviving agents claimed.
struct Drafted {
    agents: Vec<ProposedAgent>,
    claimed: Vec<usize>,
}

impl Drafted {
    /// Validates the draft and collects the claims of the agents that survived.
    ///
    /// The pairing matters: `validate_roster` drops duplicates and anything past
    /// [`MAX_AGENTS`](crate::company::setup::MAX_AGENTS), and a dropped agent's
    /// claim must go with it. Roles are matched by their trimmed spelling
    /// because that is exactly what validation preserves.
    fn from_draft(draft: RosterDraft) -> Self {
        let claims: Vec<(String, Vec<usize>)> = draft
            .agents
            .iter()
            .map(|a| (a.role.trim().to_string(), a.covers.clone()))
            .collect();

        let agents = validate_roster(draft.agents.into_iter().map(ProposedAgent::from).collect());

        let mut claimed: Vec<usize> = Vec::new();
        for agent in &agents {
            let Some((_, covers)) = claims.iter().find(|(role, _)| role == &agent.role) else {
                continue;
            };
            for index in covers {
                if !claimed.contains(index) {
                    claimed.push(*index);
                }
            }
        }
        Self { agents, claimed }
    }
}

impl std::fmt::Debug for RosterBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RosterBuilder")
            .field("model_name", &self.model_name)
            .finish_non_exhaustive()
    }
}

/// One agent as the model returns it. Every field defaulted, so a row missing
/// one is a row `validate_roster` can judge rather than a parse failure that
/// discards the whole answer.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DraftAgent {
    name: String,
    role: String,
    description: String,
    /// Which numbered jobs this agent claims to own. The claim the host checks
    /// — see [`Drafted::from_draft`] and
    /// [`uncovered_jobs`](crate::company::setup::uncovered_jobs).
    covers: Vec<usize>,
    /// The job shape, which decides the teammate's tool belt. A free `String`
    /// here and resolved through [`AgentFocus::from_wire`], so an invented value
    /// costs that teammate its narrowing rather than the operator their roster.
    focus: String,
}

impl From<DraftAgent> for ProposedAgent {
    fn from(draft: DraftAgent) -> Self {
        Self {
            name: draft.name,
            role: draft.role,
            description: draft.description,
            focus: AgentFocus::from_wire(&draft.focus),
        }
    }
}

/// The model's whole answer.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RosterDraft {
    agents: Vec<DraftAgent>,
}

/// Recovers the token/cost totals from a completed call — the same shape
/// [`planning`](super::planning) reads, from the same billing envelope.
fn usage_from(response: &ModelResponse) -> TokenUsage {
    let tokens = response.usage.unwrap_or_default();
    let cost_usd = response
        .raw
        .as_ref()
        .and_then(|raw| raw.pointer("/openhuman_usage_meta/charged_amount_usd"))
        .and_then(serde_json::Value::as_f64)
        .filter(|c| c.is_finite() && *c > 0.0)
        .unwrap_or(0.0);
    TokenUsage {
        input: tokens.input_tokens,
        output: tokens.output_tokens,
        cached_input: tokens.cache_read_tokens,
        cost_usd,
    }
}

/// Pulls the JSON object out of a model answer, tolerating a ```` ```json ````
/// fence and a sentence either side — the two things every model does anyway.
///
/// Shares [`planning`](super::planning)'s shape rather than its code because the
/// two parse different schemas; what is shared is the tolerance, and the refusal
/// to guess. An answer with no object in it returns `None` and the caller ships
/// the template, which is a better outcome than a roster assembled from prose.
fn parse_draft(text: &str) -> Option<RosterDraft> {
    let body = text.trim();
    let body = match body.find("```") {
        Some(start) => {
            let after = &body[start + 3..];
            let after = after.strip_prefix("json").unwrap_or(after);
            after.split("```").next().unwrap_or(after)
        }
        None => body,
    };
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    if end <= start {
        return None;
    }
    let draft: RosterDraft = serde_json::from_str(&body[start..=end]).ok()?;
    (!draft.agents.is_empty()).then_some(draft)
}

mod prompts;

use prompts::{retry_prompt, system_prompt, user_prompt};

#[cfg(test)]
#[path = "roster_build_tests.rs"]
mod tests;
