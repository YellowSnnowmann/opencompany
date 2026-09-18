use async_trait::async_trait;

use super::*;

#[test]
fn prompt_is_assembled_from_the_verdict_and_gap_tokens() {
    let prompt = system_prompt();
    for token in SufficiencyVerdict::tokens() {
        assert!(prompt.contains(token), "missing verdict token {token}");
    }
    for gap in [
        BlockerKind::Information,
        BlockerKind::Infrastructure,
        BlockerKind::Transient,
    ] {
        assert!(prompt.contains(gap.as_str()), "missing gap token");
    }
}

#[test]
fn text_outside_the_json_object_is_rejected() {
    let wrapped = "Sure, here you go: {\"verdict\":\"halt_benign\"} — hope that helps!";
    assert_eq!(parse_verdict(wrapped), None);
}

#[test]
fn parses_each_closed_verdict() {
    for expected in [
        SufficiencyVerdict::Continue,
        SufficiencyVerdict::Retry,
        SufficiencyVerdict::Recover,
        SufficiencyVerdict::Escalate {
            gap: BlockerKind::Infrastructure,
        },
        SufficiencyVerdict::HaltBenign,
    ] {
        let gap = match expected {
            SufficiencyVerdict::Escalate { gap } => {
                format!(",\"gap\":\"{}\"", gap.as_str())
            }
            _ => String::new(),
        };
        let answer = format!("{{\"verdict\":\"{}\"{gap}}}", expected.token());
        assert_eq!(parse_verdict(&answer), Some(expected));
    }
}

#[test]
fn insufficient_or_failed_output_is_never_halt_benign() {
    for input in [
        JudgeInput {
            instruction: "write it",
            output: "provider failed",
            criteria: None,
            execution_failed: true,
        },
        JudgeInput {
            instruction: "write it",
            output: "   ",
            criteria: None,
            execution_failed: false,
        },
    ] {
        assert_eq!(
            enforce_anti_suppression(SufficiencyVerdict::HaltBenign, input),
            SufficiencyVerdict::Retry
        );
    }
}

#[test]
fn the_run_request_survives_an_oversized_instruction() {
    let standing = "S".repeat(INSTRUCTION_CAP * 2);
    let instruction = format!("{standing}\n\nRequest for this run:\nship dark mode for iOS");
    let prompt = user_prompt(JudgeInput {
        instruction: &instruction,
        output: "done",
        criteria: None,
        execution_failed: false,
    });
    assert!(
        prompt.contains("ship dark mode for iOS"),
        "the run request must survive the instruction cap"
    );
    assert!(prompt.contains("Request for this run:"));
}

#[test]
fn recovered_evidence_stays_inside_the_judge_output_window() {
    let reply = "R".repeat(OUTPUT_CAP + 5_000);
    let evidence = "the renewal date is 2026-11-04";
    let verified = augment_with_recovery(&reply, evidence);
    let prompt = user_prompt(JudgeInput {
        instruction: "draft it",
        output: &verified,
        criteria: None,
        execution_failed: false,
    });
    assert!(
        prompt.contains(evidence),
        "recovered evidence must reach the judge, not fall past the output cap"
    );
    assert!(verified.chars().count() <= OUTPUT_CAP);
}

#[test]
fn recovery_augmentation_leaves_a_short_reply_whole() {
    let verified = augment_with_recovery("the draft", "a recovered fact");
    assert!(verified.starts_with("the draft"));
    assert!(verified.ends_with("a recovered fact"));
}

#[test]
fn recovery_material_is_bounded_on_unicode_boundaries() {
    let text = "🧠".repeat(MAX_RECOVERY_CHARS + 1);
    let capped = cap(&text, MAX_RECOVERY_CHARS);
    assert_eq!(capped.chars().count(), MAX_RECOVERY_CHARS + 1);
    assert!(capped.ends_with('…'));
}

/// Codex review on #1990 (#3903591432): a whole success-criteria sentence
/// pulls out its content words as focused single-term queries, dropping
/// short/filler words, so a fact titled on ONE of those words is
/// reachable even though it never contained the sentence verbatim.
#[test]
fn focus_terms_extracts_content_words_from_a_criteria_sentence() {
    let terms = focus_terms("The answer must include the renewal date");
    assert!(
        terms.contains(&"renewal".to_string()),
        "expected \"renewal\" among {terms:?}"
    );
    assert!(
        terms.contains(&"date".to_string()),
        "expected \"date\" among {terms:?}"
    );
    assert!(
        !terms.contains(&"the".to_string()) && !terms.contains(&"must".to_string()),
        "filler words must be dropped: {terms:?}"
    );
}

#[test]
fn focus_terms_is_capped_and_deduplicated() {
    let terms = focus_terms("alpha alpha bravo charlie delta echo foxtrot golf hotel india juliet");
    assert!(terms.len() <= MAX_FOCUS_TERMS, "{terms:?}");
    let unique: std::collections::HashSet<_> = terms.iter().collect();
    assert_eq!(unique.len(), terms.len(), "no duplicate terms: {terms:?}");
}

/// A [`crate::ports::FactStore`] whose `list` only matches an EXACT query
/// string — standing in for the real substring-match store closely enough
/// to prove whether a whole-sentence query alone can ever reach a fact
/// keyed on one of its content words.
pub(super) struct ExactMatchFactStore {
    pub(super) matches: &'static str,
    pub(super) fact: crate::ports::FactRecord,
}

#[async_trait::async_trait]
impl crate::ports::FactStore for ExactMatchFactStore {
    async fn list(
        &self,
        _company: &CompanyId,
        query: Option<&str>,
        _kind: Option<crate::ports::FactKind>,
    ) -> crate::Result<Vec<crate::ports::FactRecord>> {
        Ok(match query {
            Some(q) if q == self.matches => vec![self.fact.clone()],
            _ => Vec::new(),
        })
    }

    async fn upsert(
        &self,
        _company: &CompanyId,
        _fact: &crate::ports::FactRecord,
    ) -> crate::Result<()> {
        unreachable!("not exercised by this test")
    }

    async fn delete(&self, _company: &CompanyId, _id: &str) -> crate::Result<bool> {
        unreachable!("not exercised by this test")
    }
}

/// Codex review on #1990 (#3903591432): the whole-sentence query used to
/// be the ONLY query `ask_around` ever sent to the fact store. A fact
/// titled "Renewal date" — reachable by the focused term "renewal" — was
/// unreachable by the full sentence "The answer must include the renewal
/// date" against a case-insensitive substring match.
#[tokio::test]
async fn ask_around_finds_a_fact_reachable_only_by_a_focused_term() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-focused-query-")
        .tempdir()
        .expect("tempdir");
    let (mut deps, _journal) =
        crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    deps.facts = Some(std::sync::Arc::new(ExactMatchFactStore {
        matches: "renewal",
        fact: crate::ports::FactRecord {
            id: "f1".to_string(),
            kind: crate::ports::FactKind::Fact,
            title: "Renewal date".to_string(),
            body: "The contract renews on March 1st.".to_string(),
            source: "test".to_string(),
            updated_at_millis: 0,
        },
    }));

    let result = ask_around(
        &deps,
        &CompanyId::new("acme"),
        "The answer must include the renewal date",
        None,
    )
    .await;

    let evidence = result
        .evidence
        .expect("a fact reachable only by a focused term must still be found");
    assert!(
        evidence.contains("Renewal date"),
        "expected the matched fact in the evidence: {evidence}"
    );
}

/// A meter that always reports enough spend to exhaust any total ceiling,
/// regardless of `since_millis` — standing in for a company already past
/// its plan-level token cap.
pub(super) struct ExhaustedMeter;

#[async_trait]
impl crate::ports::UsageMeter for ExhaustedMeter {
    async fn record(
        &self,
        _company: &CompanyId,
        _sample: &crate::ports::UsageSample,
    ) -> crate::Result<()> {
        Ok(())
    }

    async fn query(
        &self,
        _company: &CompanyId,
        _since_millis: u64,
    ) -> crate::Result<Vec<crate::ports::UsageSample>> {
        Ok(vec![crate::ports::UsageSample {
            at_millis: 0,
            agent: "ceo".to_string(),
            provider: "managed".to_string(),
            input_tokens: 10_000,
            output_tokens: 0,
            cached_input_tokens: 0,
            cost_usd: 0.0,
            kind: crate::ports::SampleKind::Inference,
            run_id: None,
            model: None,
        }])
    }
}

/// A provider that counts every `invoke` rather than ever answering one —
/// the judge must never reach it once the total ceiling is spent.
#[derive(Default)]
struct PanicIfInvokedProvider {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl tinyinference::model::ChatModel<()> for PanicIfInvokedProvider {
    async fn invoke(
        &self,
        _state: &(),
        _request: ModelRequest,
    ) -> tinyinference::Result<ModelResponse> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ModelResponse::assistant(
            "{\"verdict\":\"continue\"}".to_string(),
        ))
    }
}

impl crate::harness::provider::HarnessModel for PanicIfInvokedProvider {
    fn telemetry_provider_id(&self) -> String {
        "panic-if-invoked".to_string()
    }
}

/// Codex review on #1990 (#3904894255): `HarnessPool::run_inner` refuses
/// dispatch at the plan-level total token ceiling before ever invoking the
/// agent model, but `judge_sufficiency` called `deps.provider.invoke`
/// unconditionally — a `verify`-declared node whose company had already
/// exhausted its ceiling still paid for a judge turn, and because that
/// refusal is not `execution_failed`, retries could repeat the spend.
/// Exhausting the ceiling and asking the judge to verdict must now cost
/// zero inference calls.
#[tokio::test]
async fn a_spent_total_ceiling_skips_the_judge_call_entirely() {
    let dir = tempfile::Builder::new()
        .prefix("oc-1990-judge-ceiling-")
        .tempdir()
        .expect("tempdir");
    let (mut deps, _journal) =
        crate::workflows::gated_tool_turn_tests::deps(String::new(), dir.path());
    let provider = std::sync::Arc::new(PanicIfInvokedProvider::default());
    deps.provider = provider.clone();
    deps.plan = Some(crate::harness::capability_budget::CapabilityPlan {
        period: crate::harness::capability_budget::BudgetPeriod::Daily,
        budgets: Default::default(),
        total_budget: Some(1_000),
    });
    deps.meter = Some(std::sync::Arc::new(ExhaustedMeter));

    let verdict = judge_sufficiency(
        &deps,
        &CompanyId::new("acme"),
        JudgeInput {
            instruction: "send the report",
            output: "the report has been sent",
            criteria: None,
            execution_failed: false,
        },
    )
    .await;

    assert_eq!(
        provider.calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the judge must not spend inference once the total ceiling is spent"
    );
    assert_eq!(
        verdict,
        SufficiencyVerdict::Retry,
        "a budget-refused verify must not be accepted as sufficient"
    );
}
