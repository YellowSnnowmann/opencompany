use super::*;

/// INPUT/STATE-axis (HT-074): `spawn_task` now grounds `assignee` on the
/// same terms `delegate_to_desk`/`delegate_to_teammate` already do (issue
/// #272) — a name that resolves to nobody on the roster is refused here,
/// in the model's own turn, rather than surviving as a queued card the
/// drain silently opens unowned with no signal anywhere that the assignee
/// was bogus.
///
/// The store is seeded (not empty) so grounding actually resolves the
/// roster rather than taking the fail-open path — an empty store would
/// pass this test for the wrong reason.
#[tokio::test]
async fn spawn_task_refuses_an_assignee_that_names_nobody_on_the_roster() {
    let company = CompanyId::new("acme");
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = SpawnTaskTool::new(
        queue.clone(),
        company.clone(),
        Arc::new(MemStore::seeded(seeded_record(&company))),
    );

    let outcome = tool
        .execute(json!({
            "title": "Investigate the outage",
            "assignee": "totally-nonexistent-agent-id",
        }))
        .await
        .unwrap();
    assert!(
        outcome.is_error,
        "an assignee naming nobody on the roster must be refused before queuing"
    );
    assert!(
        outcome.text().contains("totally-nonexistent-agent-id"),
        "the refusal names the target the model typed: {}",
        outcome.text()
    );
    assert_eq!(queue.queued(), 0, "nothing should have been staged");
}

/// The other half: a real teammate id grounds and queues under its
/// canonical form, and a blank/absent `assignee` opens the card unowned
/// without ever touching the store.
#[tokio::test]
async fn spawn_task_grounds_a_real_teammate_and_leaves_a_blank_assignee_alone() {
    let company = CompanyId::new("acme");
    let manifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "ceo"
role = "Chief Executive"
tier = "orchestrator"

[[agent]]
id = "eng"
role = "Engineer"
"#,
    )
    .expect("valid manifest");
    let record = CompanyRecord {
        manifest,
        ..seeded_record(&company)
    };
    let store = Arc::new(MemStore::seeded(record));

    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = SpawnTaskTool::new(queue.clone(), company.clone(), store.clone());
    let grounded = tool
        .execute(json!({ "title": "Fix the outage", "assignee": "ENG" }))
        .await
        .expect("execute");
    assert!(!grounded.is_error, "{}", grounded.text());

    let unassigned_tool = SpawnTaskTool::new(queue.clone(), company, store);
    let unassigned = unassigned_tool
        .execute(json!({ "title": "Untargeted work" }))
        .await
        .expect("execute");
    assert!(!unassigned.is_error, "{}", unassigned.text());

    let drained = queue.drain(MAX_DELEGATIONS_PER_TURN);
    assert_eq!(
        drained,
        vec![
            Delegation::SpawnTask {
                title: "Fix the outage".to_string(),
                note: None,
                assignee: Some("eng".to_string()),
            },
            Delegation::SpawnTask {
                title: "Untargeted work".to_string(),
                note: None,
                assignee: None,
            },
        ],
        "a display name grounds to the canonical roster id, and no assignee is queued as \
         None rather than being pushed through the resolver at all"
    );
}

/// AUTH-axis (HT-074): `spawn_task`'s grounding must scope its roster
/// lookup to the tool's OWN company (`self.company`), never to a
/// different one — a teammate id that is real, but only on ANOTHER
/// company's roster, must be refused exactly as an invented id would be,
/// not accidentally admitted through a leaked cross-tenant read.
#[tokio::test]
async fn spawn_task_grounds_only_against_its_own_companys_roster() {
    let acme = CompanyId::new("acme");
    let beta = CompanyId::new("beta");
    let beta_manifest = toml::from_str(
        r#"
[company]
name = "Beta"

[[agent]]
id = "ceo"
role = "Chief Executive"
tier = "orchestrator"

[[agent]]
id = "eng"
role = "Engineer"
"#,
    )
    .expect("valid manifest");
    let mut records = std::collections::HashMap::new();
    records.insert("acme".to_string(), seeded_record(&acme));
    records.insert(
        "beta".to_string(),
        CompanyRecord {
            manifest: beta_manifest,
            ..seeded_record(&beta)
        },
    );
    let store = Arc::new(TenantScopedCompanyStore { records });

    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = SpawnTaskTool::new(queue.clone(), acme, store);

    let outcome = tool
        .execute(json!({ "title": "Fix the outage", "assignee": "eng" }))
        .await
        .unwrap();
    assert!(
        outcome.is_error,
        "a teammate id real only on a DIFFERENT company's roster must be refused, not \
         leaked in: {}",
        outcome.text()
    );
    assert_eq!(queue.queued(), 0);
}

/// FAIL-axis (HT-074): when the company record cannot be read at all —
/// the same store failure `DelegateToDeskTool`/`DelegateToTeammateTool`
/// fail OPEN on for the orchestrator's own unrestricted copy (see
/// `Grounding::ungrounded`) — `spawn_task` must fail open too, not refuse
/// to open a card just because the roster could not be checked this
/// instant. The assignee is queued exactly as typed, unresolved, the same
/// as it has always been for a request with no assignee to ground.
#[tokio::test]
async fn spawn_task_fails_open_when_the_company_record_cannot_be_read() {
    struct BrokenStore;
    #[async_trait::async_trait]
    impl CompanyStore for BrokenStore {
        async fn load(&self, _id: &CompanyId) -> crate::Result<Option<CompanyRecord>> {
            Err(crate::OpenCompanyError::Store("store is down".to_string()))
        }
        async fn save(&self, _record: &CompanyRecord) -> crate::Result<()> {
            Ok(())
        }
        async fn list(&self) -> crate::Result<Vec<CompanySummary>> {
            Ok(Vec::new())
        }
        async fn append_ledger(&self, _id: &CompanyId, _entry: LedgerEntry) -> crate::Result<()> {
            Ok(())
        }
    }

    let company = CompanyId::new("acme");
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = SpawnTaskTool::new(queue.clone(), company, Arc::new(BrokenStore));

    let outcome = tool
        .execute(json!({ "title": "Investigate the outage", "assignee": "eng" }))
        .await
        .unwrap();
    assert!(
        !outcome.is_error,
        "a store failure must not block opening the card: {}",
        outcome.text()
    );
    let drained = queue.drain(MAX_DELEGATIONS_PER_TURN);
    assert_eq!(
        drained,
        vec![Delegation::SpawnTask {
            title: "Investigate the outage".to_string(),
            note: None,
            assignee: Some("eng".to_string()),
        }],
        "the assignee is queued as typed, unresolved, when grounding could not run at all"
    );
}

/// FAIL-axis (HT-076): every fixture in this module hand-writes its
/// `CompanyRecord` manifest with short, convenient agent ids ("ceo",
/// "writer"). The real setup pipeline
/// (`company::setup::manifest_from_setup`) derives ids from the agent's
/// ROLE text via `unique_agent_id`/`snake_id` — multi-word,
/// underscore-separated ids no hand fixture happens to produce. This
/// proves `delegate_to_teammate`'s grounding
/// (`CompanyRecord::resolve_teammate_key`) agrees with that real shape,
/// not just the fixtures' convenient one.
#[tokio::test]
async fn delegate_to_teammate_grounds_against_a_realistically_derived_roster_id() {
    let agents = vec![
        crate::company::setup::ProposedAgent {
            name: "Head".to_string(),
            role: "Head of Product Strategy".to_string(),
            description: "Owns the roadmap.".to_string(),
            focus: None,
        },
        crate::company::setup::ProposedAgent {
            name: "Ops".to_string(),
            role: "Chief Operating Officer".to_string(),
            description: "Runs the business.".to_string(),
            focus: None,
        },
    ];
    let manifest = crate::company::setup::manifest_from_setup(
        &crate::company::setup::SetupAnswers::default(),
        &agents,
        None,
    );
    let real_id = manifest.agents[0].id.clone();
    assert!(
        real_id.contains('_'),
        "the real roster builder derives multi-word ids, unlike this module's short hand \
         fixtures: got {real_id:?}"
    );

    let company = CompanyId::new("acme");
    let record = CompanyRecord {
        manifest,
        ..seeded_record(&company)
    };
    let store: Arc<dyn CompanyStore> = Arc::new(MemStore::seeded(record));
    let queue = DelegationQueue::default();
    let _claim = queue.claim();
    let tool = DelegateToTeammateTool::new(queue.clone(), company, store);

    let out = tool
        .execute(json!({ "teammate": real_id.clone(), "instruction": "review the roadmap" }))
        .await
        .unwrap();
    assert!(
        !out.is_error,
        "grounding must resolve a real setup-derived id, not just the hand fixtures' short \
         ones: {}",
        out.text()
    );
    let drained = queue.drain(MAX_DELEGATIONS_PER_TURN);
    assert_eq!(
        drained,
        vec![Delegation::DelegateToTeammate {
            teammate: real_id,
            instruction: "review the roadmap".to_string(),
        }]
    );
}

/// The cap counts the company's own roster, and every load appends more.
///
/// `apply_globals` puts the host's baseline teammates into `agents` on
/// every production load. A cap that counted the whole list would spend
/// most of its budget on teammates the company neither added nor can
/// remove, and a company with a designed roster would be refused its first
/// mint. The manifest here is built the way production builds one, so the
/// baseline is present and the count has to see past it.
#[tokio::test]
async fn add_agent_counts_manifest_teammates_toward_the_roster_cap() {
    let company = CompanyId::new("acme");
    let mut record = seeded_record(&company);
    let mut manifest: crate::company::CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"designer\"\nrole = \"Designer\"\n",
    )
    .expect("valid manifest");
    manifest.apply_globals();
    assert!(
        manifest.agents.len() > manifest.own_agents().count(),
        "this test is only meaningful while the baseline is appended to a roster"
    );
    record.manifest = manifest;
    let store = Arc::new(MemStore::seeded(record));
    let tool = unscoped_add_agent(company.clone(), store.clone());

    for i in 1..crate::company::setup::MAX_AGENTS {
        let result = tool
            .execute(json!({ "name": format!("Teammate {i}"), "role": "Generalist" }))
            .await
            .expect("execute");
        assert!(
            !result.is_error,
            "mint {i} unexpectedly refused: {}",
            result.text()
        );
    }

    let result = tool
        .execute(json!({ "name": "One too many", "role": "Generalist" }))
        .await
        .expect("execute");
    assert!(result.is_error, "{}", result.text());
    let record = store.load(&company).await.unwrap().expect("persisted");
    assert_eq!(
        record.overlay_agents.len(),
        crate::company::setup::MAX_AGENTS - 1,
        "refusal must not persist another teammate"
    );
}

/// A roster at the setup cap refuses further minting.
#[tokio::test]
async fn add_agent_refuses_once_the_roster_reaches_the_setup_cap() {
    let company = CompanyId::new("acme");
    let store = Arc::new(MemStore::seeded(seeded_record(&company)));
    let tool = unscoped_add_agent(company.clone(), store.clone());

    for i in 0..crate::company::setup::MAX_AGENTS {
        let result = tool
            .execute(json!({ "name": format!("Teammate {i}"), "role": "Generalist" }))
            .await
            .unwrap();
        assert!(!result.is_error);
    }
    let result = tool
        .execute(json!({ "name": "One too many", "role": "Generalist" }))
        .await
        .unwrap();
    assert!(
        result.is_error,
        "a roster already at the setup cap must refuse further minting"
    );
}

#[tokio::test]
async fn add_agent_does_not_count_retired_teammates_toward_the_roster_cap() {
    let company = CompanyId::new("acme");
    let mut record = seeded_record(&company);
    record.manifest = toml::from_str(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"designer\"\nrole = \"Designer\"\n",
    )
    .expect("valid manifest");
    record.overlay_retired_agents.push("designer".to_string());
    let store = Arc::new(MemStore::seeded(record));
    let tool = unscoped_add_agent(company.clone(), store.clone());

    for i in 0..crate::company::setup::MAX_AGENTS {
        let result = tool
            .execute(json!({ "name": format!("Teammate {i}"), "role": "Generalist" }))
            .await
            .expect("execute");
        assert!(!result.is_error, "{}", result.text());
    }
    let record = store.load(&company).await.unwrap().expect("persisted");
    assert_eq!(
        record.overlay_agents.len(),
        crate::company::setup::MAX_AGENTS
    );
}

#[tokio::test]
async fn concurrent_add_agent_calls_cannot_exceed_the_roster_cap() {
    let company = CompanyId::new("acme");
    let store = Arc::new(YieldingStore {
        record: StdMutex::new(Some(seeded_record(&company))),
    });
    let first = unscoped_add_agent(company.clone(), store.clone());
    let second = unscoped_add_agent(company.clone(), store.clone());
    for i in 1..crate::company::setup::MAX_AGENTS {
        let result = first
            .execute(json!({ "name": format!("Teammate {i}"), "role": "Generalist" }))
            .await
            .expect("execute");
        assert!(!result.is_error, "{}", result.text());
    }

    let (a, b) = tokio::join!(
        first.execute(json!({ "name": "Jamie", "role": "Growth Lead" })),
        second.execute(json!({ "name": "Alex", "role": "Support Lead" })),
    );
    let (a, b) = (a.expect("execute"), b.expect("execute"));
    assert_eq!([a, b].iter().filter(|result| result.is_error).count(), 1);
    let record = store.load(&company).await.unwrap().expect("persisted");
    assert_eq!(
        record.overlay_agents.len(),
        crate::company::setup::MAX_AGENTS
    );
}

/// FAIL-axis (HT-079, the concurrency half): `add_agent` is a read-modify-
/// write over the whole record — load, push onto `overlay_agents`, save —
/// with awaits on both ends. `company_write_lock` is what stops two of them
/// interleaving; without it the second save writes a record built from a
/// snapshot taken before the first landed, and one minted teammate simply
/// disappears while its caller is told it was added.
#[tokio::test]
async fn concurrent_add_agent_calls_cannot_lose_a_mint_to_the_load_push_save_race() {
    let company = CompanyId::new("acme");
    let store = Arc::new(YieldingStore {
        record: StdMutex::new(Some(seeded_record(&company))),
    });
    let first = unscoped_add_agent(company.clone(), store.clone());
    let second = unscoped_add_agent(company.clone(), store.clone());

    let (a, b) = tokio::join!(
        first.execute(json!({ "name": "Jamie", "role": "Growth Lead" })),
        second.execute(json!({ "name": "Alex", "role": "Support Lead" })),
    );
    assert!(!a.expect("execute").is_error);
    assert!(!b.expect("execute").is_error);

    let record = store.load(&company).await.unwrap().expect("persisted");
    let names: Vec<&str> = record
        .overlay_agents
        .iter()
        .map(|agent| agent.name.as_str())
        .collect();
    assert_eq!(
        names.len(),
        2,
        "both mints were acknowledged, so both must survive the race: {names:?}"
    );
    assert!(
        names.contains(&"Jamie") && names.contains(&"Alex"),
        "{names:?}"
    );
}

/// The other half of the same window: the duplicate-name guard reads
/// `overlay_agents` from a snapshot and pushes onto it, so two concurrent
/// mints of the SAME name are exactly the check-then-act the write lock has
/// to serialise. One must be refused, and the roster must hold one entry.
#[tokio::test]
async fn concurrent_add_agent_calls_for_one_name_mint_it_once() {
    let company = CompanyId::new("acme");
    let store = Arc::new(YieldingStore {
        record: StdMutex::new(Some(seeded_record(&company))),
    });
    let first = unscoped_add_agent(company.clone(), store.clone());
    let second = unscoped_add_agent(company.clone(), store.clone());

    let (a, b) = tokio::join!(
        first.execute(json!({ "name": "Jamie", "role": "Growth Lead" })),
        second.execute(json!({ "name": "Jamie", "role": "Growth Lead" })),
    );
    let (a, b) = (a.expect("execute"), b.expect("execute"));
    assert_eq!(
        [&a, &b].iter().filter(|r| r.is_error).count(),
        1,
        "exactly one of two identical mints must be refused.\nfirst: {}\nsecond: {}",
        a.text(),
        b.text()
    );

    let record = store.load(&company).await.unwrap().expect("persisted");
    assert_eq!(
        record.overlay_agents.len(),
        1,
        "the duplicate guard must leave exactly one teammate: {:?}",
        record
            .overlay_agents
            .iter()
            .map(|agent| agent.name.as_str())
            .collect::<Vec<_>>()
    );
}
