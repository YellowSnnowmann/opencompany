use super::*;

use tinyinference::Result as TaResult;
use tinyinference::model::ChatModel;

/// A model that answers every request with one canned reply.
struct Canned(&'static str);

#[async_trait::async_trait]
impl ChatModel<()> for Canned {
    async fn invoke(&self, _state: &(), _request: ModelRequest) -> TaResult<ModelResponse> {
        Ok(ModelResponse::assistant(self.0))
    }
}

impl HarnessModel for Canned {
    fn telemetry_provider_id(&self) -> String {
        "test".to_string()
    }

    fn telemetry_model(&self) -> Option<crate::metering::ModelSlug> {
        None
    }
}

async fn titled(reply: &'static str) -> Option<TaskTitle> {
    TitleEvaluator::new(Arc::new(Canned(reply)), "chat-v1".to_string())
        .title("do the thing")
        .await
        .0
}

/// The shapes a model actually replies in — quoted, prefixed, emphasised,
/// full-stopped — all reduce to the bare name.
#[tokio::test]
async fn a_title_survives_the_shapes_a_model_replies_in() {
    for reply in [
        "Reword the middle pricing tier",
        "\"Reword the middle pricing tier\"",
        "Title: Reword the middle pricing tier",
        "**Reword the middle pricing tier**",
        "`Reword the middle pricing tier`",
        "Reword the middle pricing tier.",
        "Task: \"Reword the middle pricing tier\".",
        "  Reword the middle pricing tier  \n\nLet me know if you want another.",
    ] {
        assert_eq!(
            titled(reply).await.expect(reply).as_str(),
            "Reword the middle pricing tier",
            "{reply}"
        );
    }
}

/// A reply with no name in it leaves the caller on its fallback rather than
/// putting punctuation on the board.
#[tokio::test]
async fn an_unusable_reply_is_no_title_rather_than_a_bad_one() {
    for reply in ["", "   ", "\n\n", "\"\"", "**", "...", "Title:"] {
        assert!(titled(reply).await.is_none(), "{reply:?}");
    }
}

/// A model that ignores the word ceiling is cut to the cap the type
/// advertises — the prompt asks, the type enforces.
#[tokio::test]
async fn a_paragraph_is_bounded_rather_than_trusted() {
    let title = titled(
        "Take a really good look at the whole of the pricing page and then rewrite \
         every single one of the tiers from scratch including the middle one",
    )
    .await
    .expect("a long reply still names something");
    assert!(
        title.as_str().chars().count() <= crate::ports::tasks::TASK_TITLE_MAX_CHARS,
        "{title}"
    );
    assert!(!title.as_str().contains('\n'));
}

/// Nothing asked is nothing named, and the model is never consulted about
/// it — an empty request is the one input that makes a titler invent.
#[tokio::test]
async fn an_empty_request_is_not_sent_to_the_model() {
    let evaluator = TitleEvaluator::new(Arc::new(Canned("Invented work")), "m".to_string());
    for request in ["", "   ", "\n\t "] {
        assert!(evaluator.title(request).await.0.is_none(), "{request:?}");
    }
}

/// A request already written as a title comes back as one, not degraded.
#[tokio::test]
async fn an_already_good_title_is_left_alone() {
    assert_eq!(
        titled("Fix the login redirect")
            .await
            .expect("a title")
            .as_str(),
        "Fix the login redirect"
    );
}

/// Non-Latin scripts survive intact — the sanitiser is character-wise
/// throughout, and capitalisation is a no-op where the script has no case.
#[tokio::test]
async fn a_non_english_title_is_not_mangled() {
    assert_eq!(
        titled("価格ページの中段プランを書き直す")
            .await
            .expect("a title")
            .as_str(),
        "価格ページの中段プランを書き直す"
    );
}
