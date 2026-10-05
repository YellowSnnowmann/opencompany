use super::*;

/// **Issue #348 review.** The recent-activity tail is ten slots wide, and a
/// discussion (#335) is an operator-driven writer into the same journal the
/// tail reads. A row per post would let one afternoon's thread on one card
/// push every dispatch, reply and approval out of the orchestrator's only
/// view of what the company has been doing — and replace them with rows it
/// cannot act on, since no agent participates in a discussion.
///
/// So: posts never hold a slot, the run events survive a thread that
/// outnumbers them, and the fact that people are talking is still reported —
/// as one folded count, with no message text (the same no-quoting rule
/// `summarize_event`'s arm carries).
#[tokio::test]
async fn discussion_posts_fold_to_one_line_instead_of_evicting_the_activity_tail() {
    use crate::ports::types::StoredEvent;
    use futures::stream::{self, BoxStream};

    /// A log that replays a fixed history.
    struct FixedLog(Vec<StoredEvent>);

    #[async_trait]
    impl EventLog for FixedLog {
        async fn append(&self, _id: &CompanyId, _event: CompanyEvent) -> crate::Result<EventSeq> {
            unreachable!("the insight surface only reads")
        }
        async fn read_from(
            &self,
            _id: &CompanyId,
            seq: EventSeq,
            limit: usize,
        ) -> crate::Result<Vec<StoredEvent>> {
            Ok(self
                .0
                .iter()
                .filter(|e| e.seq.value() >= seq.value())
                .take(limit)
                .cloned()
                .collect())
        }
        fn subscribe(
            &self,
            _id: &CompanyId,
        ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
            Box::pin(stream::empty())
        }
    }

    let company = CompanyId::new("acme");
    let mut history = vec![StoredEvent {
        seq: EventSeq::new(0),
        company: company.clone(),
        event: CompanyEvent::TaskDispatched {
            task_id: "t-1".to_string(),
            run_id: None,
        },
        at_millis: 1,
    }];
    // Twenty posts — twice the tail — on the one card, as an afternoon of
    // back-and-forth actually looks.
    for n in 0..20u64 {
        history.push(StoredEvent {
            seq: EventSeq::new(n + 1),
            company: company.clone(),
            event: CompanyEvent::TaskDiscussionPosted {
                task_id: "t-1".to_string(),
                text: format!("ping the vendor again ({n})"),
                by: None,
            },
            at_millis: 2 + n,
        });
    }
    history.push(StoredEvent {
        seq: EventSeq::new(21),
        company: company.clone(),
        event: CompanyEvent::DeskTaskCompleted {
            task_id: "t-1".to_string(),
            desk: "eng".to_string(),
            output: "shipped".to_string(),
            column: "done".to_string(),
            artifact_ids: Vec::new(),
            origin_chat_id: None,
            origin_parent: None,
        },
        at_millis: 30,
    });

    let log: Arc<dyn EventLog> = Arc::new(FixedLog(history));
    let tool = QueryCompanyTool::new(company, None, Some(log), None, None, None);
    let out = tool
        .execute(json!({}))
        .await
        .expect("execute")
        .output_for_llm(true);

    // Both run events survive the thread that buried them.
    assert!(out.contains("task dispatched"), "dispatch evicted: {out}");
    assert!(out.contains("task completed"), "completion evicted: {out}");
    // One folded line, not twenty rows — and no message text anywhere.
    assert!(out.contains("20 discussion posts"), "{out}");
    assert!(!out.contains("ping the vendor"), "post text quoted: {out}");
    assert_eq!(
        out.matches("discussion post").count(),
        1,
        "a post must not hold a slot of its own: {out}"
    );
}

/// Issue #420: the recent-activity tail keeps only [`RECENT_EVENTS`] rows,
/// and it used to drop everything older in silence — a full log read as
/// complete, the same silent-cut class the facts section one block down
/// already announces. The tail now names how many rows fell off the far end
/// (in the markdown) and reports the count (in the JSON summary). Discussion
/// posts pushed past the tail are dropped rows too, so they count toward it
/// rather than folding into their own line.
#[tokio::test]
async fn query_company_announces_the_dropped_event_tail() {
    use crate::ports::types::StoredEvent;
    use futures::stream::{self, BoxStream};

    /// A log that replays a fixed history.
    struct FixedLog(Vec<StoredEvent>);

    #[async_trait]
    impl EventLog for FixedLog {
        async fn append(&self, _id: &CompanyId, _event: CompanyEvent) -> crate::Result<EventSeq> {
            unreachable!("the insight surface only reads")
        }
        async fn read_from(
            &self,
            _id: &CompanyId,
            seq: EventSeq,
            limit: usize,
        ) -> crate::Result<Vec<StoredEvent>> {
            Ok(self
                .0
                .iter()
                .filter(|e| e.seq.value() >= seq.value())
                .take(limit)
                .cloned()
                .collect())
        }
        fn subscribe(
            &self,
            _id: &CompanyId,
        ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
            Box::pin(stream::empty())
        }
    }

    let company = CompanyId::new("acme");

    // A distinct, non-discussion event so every row occupies a tail slot.
    let dispatch = |seq: u64| StoredEvent {
        seq: EventSeq::new(seq),
        company: company.clone(),
        event: CompanyEvent::TaskDispatched {
            task_id: format!("t-{seq}"),
            run_id: None,
        },
        at_millis: seq + 1,
    };

    // (a) Five more row-events than the tail is wide: the five oldest fall
    // off, the notice sits at the top, and the JSON summary counts them.
    let over: Vec<StoredEvent> = (0..(RECENT_EVENTS as u64 + 5)).map(dispatch).collect();
    let log: Arc<dyn EventLog> = Arc::new(FixedLog(over));
    let tool = QueryCompanyTool::new(company.clone(), None, Some(log), None, None, None);
    let result = tool.execute(json!({})).await.expect("execute");
    let md = result.output_for_llm(true);
    let activity = md
        .split("## Recent activity\n")
        .nth(1)
        .expect("recent activity section");
    assert!(
        activity.starts_with("- […5 earlier event(s) not shown]"),
        "the dropped tail must be announced at the top: {md}"
    );
    assert!(
        result
            .output_for_llm(false)
            .contains("\"events_not_shown\": 5"),
        "the JSON summary must count the drop: {}",
        result.output_for_llm(false)
    );

    // (b) Exactly the tail width: nothing was dropped, so nothing is said.
    let exact: Vec<StoredEvent> = (0..RECENT_EVENTS as u64).map(dispatch).collect();
    let log: Arc<dyn EventLog> = Arc::new(FixedLog(exact));
    let result = QueryCompanyTool::new(company.clone(), None, Some(log), None, None, None)
        .execute(json!({}))
        .await
        .expect("execute");
    assert!(
        !result
            .output_for_llm(true)
            .contains("earlier event(s) not shown"),
        "a complete tail must stay silent: {}",
        result.output_for_llm(true)
    );
    assert!(
        result
            .output_for_llm(false)
            .contains("\"events_not_shown\": 0"),
        "a complete tail reports zero dropped: {}",
        result.output_for_llm(false)
    );

    // (c) Discussion posts older than the tail are dropped rows: they count
    // toward the drop, not toward the fold line. Three posts (oldest) then
    // enough dispatches to fill the tail — the posts never get visited.
    let mut mixed: Vec<StoredEvent> = Vec::new();
    for seq in 0..3u64 {
        mixed.push(StoredEvent {
            seq: EventSeq::new(seq),
            company: company.clone(),
            event: CompanyEvent::TaskDiscussionPosted {
                task_id: "t-1".to_string(),
                text: format!("older chatter {seq}"),
                by: None,
            },
            at_millis: seq + 1,
        });
    }
    for seq in 3..(RECENT_EVENTS as u64 + 5) {
        mixed.push(dispatch(seq));
    }
    // total = 3 posts + (RECENT_EVENTS + 2) dispatches; the tail holds
    // RECENT_EVENTS dispatches, so 2 dispatches + 3 posts = 5 fall off.
    let log: Arc<dyn EventLog> = Arc::new(FixedLog(mixed));
    let result = QueryCompanyTool::new(company.clone(), None, Some(log), None, None, None)
        .execute(json!({}))
        .await
        .expect("execute");
    let md = result.output_for_llm(true);
    assert!(
        md.contains("- […5 earlier event(s) not shown]"),
        "dropped discussion posts must count toward the tail drop: {md}"
    );
    assert!(
        !md.contains("discussion post"),
        "an unvisited post must not also fold into its own line: {md}"
    );
    assert!(
        result
            .output_for_llm(false)
            .contains("\"events_not_shown\": 5"),
        "{}",
        result.output_for_llm(false)
    );
}

/// Issue #410, point 4 (audit the same silent-cut class elsewhere): the
/// fact list is capped at [`FACT_LIMIT`], and it used to be capped in
/// silence. A company past twenty facts handed the orchestrator a partial
/// memory that read as complete, so "we have no record of that" was a
/// conclusion it could reach from a truncated list. The cut now says it
/// happened and names the argument that narrows it.
#[tokio::test]
async fn query_company_says_when_the_fact_list_was_cut() {
    // Exactly at the cap: complete, so no notice.
    let (company, memory) =
        seeded_memory("cut-exact", (0..FACT_LIMIT).map(|i| format!("Fact {i}"))).await;
    let out = QueryCompanyTool::new(company, Some(memory), None, None, None, None)
        .execute(json!({}))
        .await
        .expect("execute")
        .output_for_llm(true);
    assert!(!out.contains("TRUNCATED"), "nothing was cut: {out}");

    // Past the cap: the cut is announced, counted, and points at `query`.
    let (company, memory) =
        seeded_memory("cut-many", (0..FACT_LIMIT + 7).map(|i| format!("Fact {i}"))).await;
    let out = QueryCompanyTool::new(company, Some(memory), None, None, None, None)
        .execute(json!({}))
        .await
        .expect("execute")
        .output_for_llm(true);
    assert!(
        out.contains("TRUNCATED"),
        "the cut must be announced: {out}"
    );
    assert!(out.contains("7 more fact(s) not shown"), "{out}");
    assert!(out.contains("query_company"), "{out}");
}

/// A fresh company whose memory holds one learning per text.
async fn seeded_memory(
    tag: &str,
    texts: impl Iterator<Item = String>,
) -> (CompanyId, crate::memory::CompanyMemory) {
    let company = CompanyId::new(format!("{tag}-{}", uuid::Uuid::new_v4().simple()));
    let memory = crate::memory::CompanyMemory::new(&company);
    for text in texts {
        memory
            .learn(&text, crate::memory::LearningKind::Fact, Vec::new())
            .await
            .expect("learn");
    }
    (company, memory)
}

/// Issue #420, the residual: the whole insight document is handed to the
/// model through the harness tool-result path, which hard-cuts anything past
/// its byte budget — blindly. A facts list long enough would carry that cut
/// into the sections below it, dropping the facts `[TRUNCATED]` marker and
/// the Desks list `delegate_to_desk` reads. So each fact body is capped and
/// the facts section is bounded in bytes; the marker and every later section
/// stay inside the outer budget. Cutting a body counts characters, never
/// bytes, so a multibyte body cannot panic mid-codepoint.
#[tokio::test]
async fn query_company_bounds_the_insight_document_size() {
    let render = |tag: &'static str, texts: Vec<String>| async move {
        let (company, memory) = seeded_memory(tag, texts.into_iter()).await;
        QueryCompanyTool::new(company, Some(memory), None, None, None, None)
            .execute(json!({}))
            .await
            .expect("execute")
            .output_for_llm(true)
    };
    let fact_body = |line: &str| -> Option<String> {
        line.strip_prefix("- **")
            .and_then(|rest| rest.split_once("**: "))
            .map(|(_, body)| body.to_string())
    };

    // (e) A single multi-KB multibyte body: cut on a char boundary, marked
    // with an ellipsis, exactly the cap wide, and no panic.
    let out = render("bounds-one", vec!["é".repeat(5_000)]).await;
    let body = out.lines().find_map(fact_body).expect("fact line");
    assert!(body.ends_with('…'), "a cut body is marked: {body:?}");
    assert_eq!(
        body.chars().count(),
        MAX_FACT_BODY_CHARS,
        "the body is cut to exactly the cap"
    );
    assert!(
        body.chars().take(MAX_FACT_BODY_CHARS - 1).all(|c| c == 'é'),
        "the cut landed on a codepoint boundary, not inside one"
    );

    // (f) Enough capped bodies to blow the section byte budget. The count
    // reflects the budget cut, not merely FACT_LIMIT, and the marker plus
    // every section below Facts survives the outer tool-result cut.
    let heavy: Vec<String> = (0..FACT_LIMIT)
        .map(|i| format!("{i}{}", "é".repeat(MAX_FACT_BODY_CHARS)))
        .collect();
    let out = render("bounds-heavy", heavy).await;
    let shown = out
        .lines()
        .filter_map(fact_body)
        .filter(|body| body.contains('é'))
        .count();
    assert!(
        (1..FACT_LIMIT).contains(&shown),
        "the byte budget must cut before FACT_LIMIT yet keep at least one: shown={shown}"
    );
    assert!(
        out.contains(&format!("{} more fact(s) not shown", FACT_LIMIT - shown)),
        "the marker counts the budget cut: {out}"
    );
    for header in [
        "[TRUNCATED",
        "## Recent activity",
        "## Saved workflows",
        "## Team",
        "## Desks",
    ] {
        assert!(
            out.contains(header),
            "the facts cut must not carry the outer budget into `{header}`: {out}"
        );
    }

    // (g) A small document renders its learnings verbatim and announces
    // nothing.
    let out = render(
        "bounds-small",
        vec!["Body 0".to_string(), "Body 1".to_string()],
    )
    .await;
    assert!(out.contains("- **Body 0**: Body 0\n"), "{out}");
    assert!(out.contains("- **Body 1**: Body 1\n"), "{out}");
    assert!(!out.contains("TRUNCATED"), "nothing was cut: {out}");
    assert!(!out.contains('…'), "nothing was truncated: {out}");
}
