use super::tests_core::*;
use super::*;

/// Issue #242, the property this whole PR exists to create, proven across a
/// real restart: a host killed mid-run leaves the attempt's **partial trace
/// intact**, and the next boot settles the row it stranded.
///
/// The kill is simulated by simply not settling — which is exactly what a
/// `SIGKILL` looks like from the store's side, and the reason the boot
/// reaper's claim is a proof rather than a timeout heuristic: a cycle is a
/// process-local spawn, so an active row at boot cannot belong to anything
/// still alive.
#[tokio::test]
async fn a_killed_run_keeps_its_partial_trace_and_is_settled_on_the_next_boot() {
    use crate::ports::runs::{NewRun, RunStatus, RunStepRecord};
    use crate::ports::types::{EventSeq, TurnStep, TurnStepKind, TurnStepStatus};

    let home = tmp_home("opencompany-run-restart-");
    let manifest: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n[policy]\nmode = \"full\"\n",
    )
    .expect("manifest");
    let id = CompanyId::new("acme");

    // --- boot 1: a card is dispatched, starts, writes two steps… and dies.
    {
        let rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest.clone())
            .with_id(id.clone())
            .build()
            .await
            .expect("first boot");
        let runs = rt.runs();
        runs.create_run(&id, NewRun::for_task("run-1", "t-1", "ceo"))
            .await
            .expect("mint");
        runs.begin_run(&id, "run-1", EventSeq::new(3))
            .await
            .expect("begin");
        for (step_seq, label, status) in [
            (0u32, "Reading the brief", TurnStepStatus::Ok),
            (1, "Searching the web", TurnStepStatus::Running),
        ] {
            runs.append_run_step(
                &id,
                &RunStepRecord {
                    run_id: "run-1".to_string(),
                    step_seq,
                    at_millis: 100 + step_seq as u64,
                    step: TurnStep {
                        kind: TurnStepKind::ToolCall,
                        status,
                        label: label.to_string(),
                        detail: None,
                        elapsed_ms: None,
                        ..TurnStep::default()
                    },
                },
            )
            .await
            .expect("append step");
        }
        // …and the process is gone. Nothing settles the row.
    }

    // --- boot 2: the builder's reaper runs before anything is dispatched.
    let rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(id.clone())
        .build()
        .await
        .expect("second boot");

    let reaped = rt
        .runs()
        .get_run(&id, "run-1")
        .await
        .expect("read")
        .expect("the row survives the restart");
    assert_eq!(
        reaped.status,
        RunStatus::Failed,
        "an attempt whose process died must not still claim to be running"
    );
    assert_eq!(
        reaped.error.as_deref(),
        Some(crate::ports::runs::ORPHAN_ERROR)
    );

    // The whole point: the steps written before the kill are still there,
    // including the tool call that never got to finish.
    let steps = rt
        .runs()
        .list_run_steps(&id, "run-1")
        .await
        .expect("list steps");
    assert_eq!(steps.len(), 2, "the partial trace must survive the restart");
    assert_eq!(steps[0].step.label, "Reading the brief");
    assert_eq!(steps[0].step.status, TurnStepStatus::Ok);
    assert_eq!(
        steps[1].step.status,
        TurnStepStatus::Running,
        "the call that was in flight when the host died reads as in flight"
    );
}

/// **Issue #726, the headline**: a company whose data directory is destroyed
/// keeps its at-most-once set and its parked approvals, because the journal
/// lives in the storage backend rather than on the filesystem.
///
/// This is the hosted failure, reproduced: on a mongodb tenant `/data` is
/// documented ephemeral scratch, so container replacement — a deploy, a
/// reschedule, a node drain, an OOM kill — takes `journal.jsonl` with it.
/// Before this change every effect that had already executed became eligible
/// to fire a second time and every parked approval, grant and standing grant
/// silently vanished. The `remove_dir_all` below IS that container
/// replacement.
#[tokio::test]
async fn a_backend_journal_survives_the_loss_of_the_whole_data_directory() {
    use crate::ports::journal::MemoryJournalStore;
    use crate::ports::types::EffectGroup;
    use crate::runtime::journal::{ApprovalConversation, TaskLink};

    let home = tmp_home("opencompany-journal-durability-");
    let manifest: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n[policy]\nmode = \"full\"\n",
    )
    .expect("manifest");
    let id = CompanyId::new("acme");
    // One sink, shared across both boots — the database that outlives the
    // container, standing in for sqlite/mongodb so the proof holds in the
    // default build rather than only behind a cargo feature.
    let sink = Arc::new(MemoryJournalStore::default());
    let approval = crate::ports::types::ApprovalId::new("ap-1");
    let effect = crate::ports::types::Effect {
        kind: "filing.submit".into(),
        group: EffectGroup::Sign,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::Value::Null,
        agent: None,
        run_id: None,
    };

    // --- boot 1: an effect executes at most once, and an approval parks.
    {
        let rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest.clone())
            .with_id(id.clone())
            .with_journal_store(sink.clone())
            .build()
            .await
            .expect("first boot");
        crate::runtime::cycle::execute_effect_once(&rt, "k", &effect, Some("t-1"))
            .await
            .expect("execute the effect once");
        rt.journal
            .record_parked(
                &approval,
                &effect,
                1_000,
                TaskLink::Task { id: "t-1".into() },
                ApprovalConversation::default(),
                None,
            )
            .await
            .expect("park an approval");
        assert!(rt.journal.is_executed("k"));
    }

    // --- the container is replaced: `/data` is gone, every byte of it.
    std::fs::remove_dir_all(home.path()).expect("destroy the data directory");
    assert!(
        !Bundle::new(home.path().to_path_buf(), &id)
            .journal_jsonl()
            .exists(),
        "the filesystem journal must really be gone for this to prove anything"
    );

    // --- boot 2: same backend, brand new (empty) data directory.
    let rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(id.clone())
        .with_journal_store(sink)
        .build()
        .await
        .expect("second boot");

    assert!(
        rt.journal.is_executed("k"),
        "the committed key must survive the container: without it the effect \
         is eligible to fire a second time"
    );
    let pending = rt.journal.pending();
    assert_eq!(pending.len(), 1, "the parked approval must survive too");
    assert_eq!(
        pending[0].id, approval,
        "and with its original id, so the operator's console link still resolves"
    );
    assert_eq!(
        pending[0].task,
        Some(TaskLink::Task { id: "t-1".into() }),
        "and still linked to the card it was parked for"
    );
}

/// **Issue #726**: an existing filesystem journal is imported into the
/// backend exactly **once**, and the receipt is what makes the second boot a
/// no-op.
///
/// The re-import is not a cosmetic inefficiency. `complete_import` clears
/// before it copies, so a second import would delete every key the backend
/// accumulated after the first one — un-committing effects that have already
/// run. The gate is the only thing standing between the migration and that.
#[tokio::test]
async fn a_filesystem_journal_is_imported_once_and_the_receipt_blocks_the_rest() {
    use crate::ports::journal::MemoryJournalStore;

    let home = tmp_home("opencompany-journal-import-");
    let manifest: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n[policy]\nmode = \"full\"\n",
    )
    .expect("manifest");
    let id = CompanyId::new("acme");
    seed_filesystem_journal(home.path(), &id, &["k-legacy"]).await;

    let sink = Arc::new(MemoryJournalStore::default());
    let legacy_path = Bundle::new(home.path().to_path_buf(), &id).journal_jsonl();

    // --- boot 1: the file is imported, and left where it was.
    {
        let rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest.clone())
            .with_id(id.clone())
            .with_journal_store(sink.clone())
            .build()
            .await
            .expect("first boot");
        assert!(
            rt.journal.is_executed("k-legacy"),
            "the pre-existing at-most-once key must reach the backend"
        );
        assert!(
            legacy_path.exists(),
            "the source file stays in place: a rollback to an older binary \
             must still find the history it knows how to read"
        );
        // A key committed after the migration — this is what a second import
        // would destroy.
        rt.journal
            .record_executed(
                "k-after",
                ExecutedEffect {
                    kind: "payment.send".into(),
                    amount_usd: Some(12.0),
                    task_id: Some("t-2".into()),
                    at_millis: 2_000,
                    irreversible: true,
                },
            )
            .await
            .expect("commit a key against the backend");
    }

    // --- boot 2: the receipt is closed, so nothing is re-imported.
    let rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(id.clone())
        .with_journal_store(sink)
        .build()
        .await
        .expect("second boot");
    assert!(
        rt.journal.is_executed("k-after"),
        "a re-import would have cleared this key and re-armed an effect that \
         has already run"
    );
    assert!(rt.journal.is_executed("k-legacy"));
}

/// **Issue #726**: an import interrupted between the copy and the receipt is
/// re-run whole, not resumed.
///
/// A partial copy behind a closed gate is the bug the receipt exists to
/// prevent — it is a set of at-most-once keys that quietly went missing. So
/// the retry must **replace** what the interrupted attempt wrote rather than
/// append to it: no duplicates, and nothing from the source left behind.
#[tokio::test]
async fn an_import_interrupted_before_its_receipt_is_re_run_whole() {
    use crate::ports::journal::{JournalStore, MemoryJournalStore};

    let home = tmp_home("opencompany-journal-partial-");
    let manifest: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n[policy]\nmode = \"full\"\n",
    )
    .expect("manifest");
    let id = CompanyId::new("acme");
    seed_filesystem_journal(home.path(), &id, &["k-0", "k-1", "k-2"]).await;

    // A crash mid-import: the first line copied, the receipt never written.
    let sink = Arc::new(MemoryJournalStore::default());
    let source = crate::store::fs::read_lines_lossy(
        &Bundle::new(home.path().to_path_buf(), &id).journal_jsonl(),
    )
    .await
    .expect("read the source journal");
    sink.complete_import(&id, vec![source[0].clone()])
        .await
        .expect("a partial copy");
    sink.forget_receipt(&id);

    let rt = RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(id.clone())
        .with_journal_store(sink.clone())
        .build()
        .await
        .expect("boot after the interrupted import");

    for key in ["k-0", "k-1", "k-2"] {
        assert!(
            rt.journal.is_executed(key),
            "{key} must be present after the retry: a resumed-rather-than-restarted \
             import is how at-most-once keys go missing"
        );
    }
    assert_eq!(
        sink.read_journal(&id).await.expect("read back").len(),
        3,
        "the retry replaces the partial copy; it must not append a second one"
    );
    assert!(
        sink.journal_imported(&id).await.expect("gate"),
        "and the retry closes the gate"
    );
}

#[test]
fn slugifies_display_names() {
    assert_eq!(company_id_from_name("Acme Co!").as_ref(), "acme-co");
    assert_eq!(company_id_from_name("  Widgets  ").as_ref(), "widgets");
    assert_eq!(company_id_from_name("***").as_ref(), "company");
}

/// The shipped companies actually hand their agents the workspace tools
/// (issue #177, gap 2).
///
/// Before this, `[tools].allow` listed no `workspace` grant while every
/// agent enumerated its tools explicitly — and per-agent grants are narrowed
/// by the company allow-list, so *no* agent received even `workspace_list`.
/// The tools existed (#237) and no shipped company could reach them, which
/// made the "an agent writes a note, the operator sees it" round trip
/// impossible out of the box.
///
/// Reads are namespace-covered and writes need an explicit grant, so this
/// also pins the asymmetry: readers must NOT come out write-capable.
#[cfg(feature = "openhuman")]
#[test]
fn shipped_companies_grant_the_workspace_tools() {
    use crate::company::grants_workspace_write_explicit;
    use crate::harness::build::grants_cover;

    for company in ["e2e_harness", "openhuman_demo"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../companies")
            .join(company);
        let manifest = CompanyManifest::from_path(&path)
            .unwrap_or_else(|e| panic!("{company} manifest must parse: {e}"));

        // The company's own roster only: the global baseline is appended to
        // every manifest, and what it is granted is the company-wide belt
        // like any teammate that requests nothing — not something this
        // bundle's author decided, so not something this test pins.
        for agent in manifest.agents.iter().filter(|agent| !agent.global) {
            let grants = agent_effective_grants(&manifest.tools.allow, agent.tools.as_deref());
            assert!(
                grants_cover(&grants, "workspace"),
                "{company}/{} must reach the workspace tools; effective grants: {grants:?}",
                agent.id
            );
            // Only the writer edits notes; everyone else is read-only, so a
            // reader can never overwrite operator-owned guidance.
            assert_eq!(
                grants_workspace_write_explicit(&grants),
                agent.id == "writer",
                "{company}/{} write access is wrong; effective grants: {grants:?}",
                agent.id
            );
            // Every shipped agent asks for `mcp:*`, and `agent_effective_grants`
            // intersects that request with the company allow-list — so an
            // allow-list that omits it silently hands the agent no MCP at all.
            // Both manifests were in exactly that state before this test
            // existed (`openhuman_demo` had no allow-list, which covers
            // nothing, so its agents resolved to an empty toolbelt). Asserted
            // here because the symptom is a missing capability, not an error:
            // nothing logs, nothing fails, the tools are simply absent.
            //
            // Probed with `grant_matches` against a concrete `mcp:<server>`
            // name rather than `grants_cover`: MCP grants are colon-namespaced
            // (`mcp:*`, `mcp:notion`) while `grants_cover` only understands the
            // dot form, so it answers `false` for a grant list that plainly
            // contains `mcp:*`.
            if agent.tools.iter().flatten().any(|tool| tool == "mcp:*") {
                assert!(
                    grants
                        .iter()
                        .any(|grant| grant_matches(grant, "mcp:any-server")),
                    "{company}/{} asks for mcp:* but the allow-list does not \
                     cover it; effective grants: {grants:?}",
                    agent.id
                );
            }
        }
    }
}

#[tokio::test]
async fn user_auth_stores_default_to_fs_and_are_reachable() {
    use crate::ports::{
        InviteRecord, LoginCodeRecord, SessionRecord, UserRecord, UserRole, UserStatus,
    };

    let home_dir = tmp_home("oc-users-");
    let home = home_dir.path().to_path_buf();
    let manifest = parse("[company]\nname=\"Acme\"\n[policy]\nmode=\"full\"\n");
    let id = CompanyId::new("acme");
    // No with_users/with_sessions/with_login_codes override: the builder must
    // fall back to the shared fs backend rather than leaving a hole.
    let runtime = RuntimeBuilder::new(home.clone(), manifest)
        .with_id(id.clone())
        .build()
        .await
        .unwrap();

    runtime
        .users()
        .upsert_user(
            &id,
            &UserRecord {
                id: "u1".into(),
                email: "ada@example.com".into(),
                display_name: None,
                avatar: None,
                role: UserRole::Admin,
                status: UserStatus::Active,
                password_hash: None,
                must_change_password: false,
                created_at_millis: 1,
                last_seen_at_millis: None,
                updated_at_millis: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        runtime
            .users()
            .find_user_by_email(&id, "ada@example.com")
            .await
            .unwrap()
            .unwrap()
            .id,
        "u1"
    );

    runtime
        .users()
        .upsert_invite(
            &id,
            &InviteRecord {
                id: "i1".into(),
                email: "bob@example.com".into(),
                role: UserRole::Member,
                invited_by: "manifest".into(),
                created_at_millis: 1,
                expires_at_millis: 10,
                accepted_at_millis: None,
                notified_at_millis: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(runtime.users().list_invites(&id).await.unwrap().len(), 1);

    runtime
        .sessions()
        .create(
            &id,
            &SessionRecord {
                id: "s1".into(),
                token_hash: "hash".into(),
                user_id: "u1".into(),
                created_at_millis: 1,
                expires_at_millis: 10,
                user_agent: None,
                kind: crate::ports::SessionKind::Browser,
                label: None,
            },
        )
        .await
        .unwrap();
    assert!(
        runtime
            .sessions()
            .find_by_token_hash(&id, "hash")
            .await
            .unwrap()
            .is_some()
    );

    runtime
        .login_codes()
        .create(
            &id,
            &LoginCodeRecord {
                id: "c1".into(),
                code_hash: "codehash".into(),
                email: "ada@example.com".into(),
                created_at_millis: 1,
                expires_at_millis: 10,
                consumed_at_millis: None,
            },
        )
        .await
        .unwrap();
    assert!(
        runtime
            .login_codes()
            .consume(&id, "codehash", 2)
            .await
            .unwrap()
            .is_some()
    );
}
