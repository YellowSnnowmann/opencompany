use super::tests_core::*;
use super::*;

/// Issue #1865 (Codex review): the boot reaper's card sweep bounces a
/// stranded card to To-do through the same guarded mover `abandon_run` and
/// the cycle's terminality backstop use, but historically never called the
/// notification helper those two do — so a crash-recovered dispatch
/// failure got the silent bounce chip and nothing else, while the exact
/// same failure discovered any other way was announced. This proves the
/// boot path now files the same `dispatch_failed` row.
#[tokio::test]
async fn boot_reaper_notifies_a_bounced_card_same_as_the_live_paths() {
    use crate::ports::runs::{NewRun, RunOutcome, RunStatus};
    use crate::ports::tasks::{COLUMN_IN_PROGRESS, COLUMN_PAUSED, TaskRecord};

    let home_dir = tmp_home("oc-run-reap-notify-");
    let home = home_dir.path().to_path_buf();
    let manifest = parse("[company]\nname=\"Acme\"\n[policy]\nmode=\"full\"\n");
    let id = CompanyId::new("acme");
    let card = |task: &str, column: &str| TaskRecord {
        id: task.to_string(),
        title: crate::ports::tasks::TaskTitle::authored("Draft the spec"),
        note: None,
        column: column.to_string(),
        priority: "medium".to_string(),
        assignee: "ceo".to_string(),
        updated_at_millis: 1,
        origin: None,
        parent_task_id: None,
        output: None,
        plan: None,
        planning_attempts: Vec::new(),
        deliverable: crate::ports::tasks::TaskDeliverable::Once,
        workflow_proposal: None,
        origin_run_id: None,
        origin_workflow_id: None,
        origin_message_seq: None,
        bounced: None,
    };

    let first_boot = RuntimeBuilder::new(home.clone(), manifest.clone())
        .with_id(id.clone())
        .build()
        .await
        .unwrap();
    let runs = first_boot.runs().clone();
    let tasks = first_boot.tasks().clone();

    // `card-a` will be stranded In Progress by the crash. `card-b` is
    // parked for a person and must raise nothing.
    tasks
        .upsert(&id, &card("card-a", COLUMN_IN_PROGRESS))
        .await
        .unwrap();
    tasks
        .upsert(&id, &card("card-b", COLUMN_PAUSED))
        .await
        .unwrap();
    runs.create_run(&id, NewRun::for_task("run-a", "card-a", "ceo"))
        .await
        .unwrap();
    runs.begin_run(&id, "run-a", crate::ports::types::EventSeq::new(1))
        .await
        .unwrap();
    runs.create_run(&id, NewRun::for_task("run-b", "card-b", "ceo"))
        .await
        .unwrap();
    runs.begin_run(&id, "run-b", crate::ports::types::EventSeq::new(2))
        .await
        .unwrap();
    runs.finish_run(&id, "run-b", RunOutcome::new(RunStatus::Paused))
        .await
        .unwrap();

    // The host dies here — no settle, no journal entry, nothing.
    drop(first_boot);

    let second_boot = RuntimeBuilder::new(home.clone(), manifest)
        .with_id(id.clone())
        .build()
        .await
        .unwrap();

    // Sanity: the reaper did land the card on To-do, same as the existing
    // `boot_returns_a_stranded_card_and_leaves_a_parked_one_alone` proves.
    let stranded = second_boot
        .tasks()
        .list(&id)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.id == "card-a")
        .expect("card survives the restart");
    assert_eq!(stranded.column, crate::ports::tasks::COLUMN_TODO);

    let notifications = second_boot.notifications().list(&id, "ceo").await.unwrap();
    let dispatch_failed: Vec<_> = notifications
        .iter()
        .filter(|n| n.notification.kind == "dispatch_failed")
        .collect();
    assert_eq!(
        dispatch_failed.len(),
        1,
        "the boot reaper must file exactly one dispatch_failed row for the \
         one card it bounced: {notifications:?}"
    );
    assert_eq!(dispatch_failed[0].notification.subject.id, "card-a");
    // The parked card never left In Progress from the reaper's point of
    // view (its run was already `Paused`), so it must not be named.
    assert!(
        !dispatch_failed[0].notification.title.contains("card-b"),
        "{:?}",
        dispatch_failed[0]
    );
}

#[tokio::test]
async fn workspace_seeds_once_and_operator_deletions_stick() {
    let home_dir = tmp_home("oc-seed-");
    let home = home_dir.path().to_path_buf();
    // A company definition dir with a workspace subtree.
    let seed_dir = home.join("def");
    std::fs::create_dir_all(seed_dir.join("workspace/brand")).unwrap();
    std::fs::write(seed_dir.join("workspace/readme.md"), "# Root").unwrap();
    std::fs::write(seed_dir.join("workspace/brand/voice.md"), "# Voice").unwrap();

    let manifest = parse("[company]\nname=\"Acme\"\n[policy]\nmode=\"full\"\n");
    let id = CompanyId::new("acme");
    let runtime = RuntimeBuilder::new(home.clone(), manifest.clone())
        .with_id(id.clone())
        .with_seed_dir(seed_dir.clone())
        .build()
        .await
        .unwrap();
    // Seeded: readme.md, brand/, brand/voice.md — plus runtime scaffold
    // (the system roots and the explanatory note under each root that
    // carries one), which is not what the re-seed gate is about. The
    // explanatory notes are excluded by their *parent*, not by name: they
    // are all called `readme.md`, and so is the seeded one this asserts on.
    let seeded = |tree: &[crate::ports::WorkspaceNode]| {
        let scaffold_roots: Vec<&str> = tree
            .iter()
            .filter(|node| {
                node.parent_id.is_none()
                    && crate::company::workspace_scaffold::SYSTEM_ROOTS
                        .contains(&node.name.as_str())
            })
            .map(|node| node.id.as_str())
            .collect();
        let mut names: Vec<String> = tree
            .iter()
            .filter(|node| {
                !crate::company::workspace_scaffold::SYSTEM_ROOTS.contains(&node.name.as_str())
                    && !node
                        .parent_id
                        .as_deref()
                        .is_some_and(|parent| scaffold_roots.contains(&parent))
            })
            .map(|node| node.name.clone())
            .collect();
        names.sort();
        names
    };
    let tree = runtime.workspace().tree(&id).await.unwrap();
    assert_eq!(seeded(&tree), vec!["brand", "readme.md", "voice.md"]);

    // Operator deletes a node.
    let voice = tree.iter().find(|n| n.name == "voice.md").unwrap();
    runtime.workspace().delete(&id, &voice.id).await.unwrap();

    // Rebuild: the deletion sticks (no re-seed).
    drop(runtime);
    let runtime = RuntimeBuilder::new(home.clone(), manifest)
        .with_id(id.clone())
        .with_seed_dir(seed_dir)
        .build()
        .await
        .unwrap();
    let tree = runtime.workspace().tree(&id).await.unwrap();
    assert_eq!(
        seeded(&tree),
        vec!["brand", "readme.md"],
        "workspace re-seeded despite operator deletion"
    );
    // Sanity: the record store still loads.
    assert!(runtime.store().load(&id).await.unwrap().is_some());
}

/// The baseline's ledgers reach a company with no bundle at all — the
/// platform-provisioned tenant shape, which is most of them.
#[tokio::test]
async fn the_baseline_ledgers_are_seeded_without_a_bundle() {
    let home_dir = tmp_home("oc-ledger-seed-");
    let manifest = parse("[company]\nname=\"Acme\"\n");
    let id = CompanyId::new("acme");
    let runtime = RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest)
        .with_id(id.clone())
        .build()
        .await
        .unwrap();

    let stored: Vec<String> = runtime
        .ledgers()
        .list_specs(&id)
        .await
        .unwrap()
        .into_iter()
        .map(|spec| spec.slug)
        .collect();
    for global in crate::globals::ledgers() {
        assert!(
            stored.contains(&global.slug),
            "`{}` was not seeded: {stored:?}",
            global.slug
        );
    }
    // The built-ins stay in the runtime. Persisting a copy is what lets a
    // company's stored version drift from the code every prompt is written
    // against.
    assert!(!stored.contains(&"tasks".to_string()));
}

/// A bundle's own ledgers are seeded beside the baseline's, and a bundle
/// declaration of the same slug replaces the global rather than colliding
/// with it.
#[tokio::test]
async fn a_bundle_ledger_is_seeded_and_supersedes_a_global_of_the_same_slug() {
    let home_dir = tmp_home("oc-ledger-bundle-");
    let seed_dir = home_dir.path().join("def");
    std::fs::create_dir_all(seed_dir.join("ledgers")).unwrap();
    std::fs::write(seed_dir.join("ledgers/pipeline.toml"), PIPELINE_LEDGER).unwrap();
    let shadowing_slug = crate::globals::ledgers()[0].slug.clone();
    std::fs::write(
        seed_dir.join(format!("ledgers/{shadowing_slug}.toml")),
        PIPELINE_LEDGER.replace("Deal pipeline", "Ours, not the baseline's"),
    )
    .unwrap();

    let manifest = parse("[company]\nname=\"Acme\"\n");
    let id = CompanyId::new("acme");
    let runtime = RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest)
        .with_id(id.clone())
        .with_seed_dir(seed_dir)
        .build()
        .await
        .unwrap();

    let specs = runtime.ledgers().list_specs(&id).await.unwrap();
    let slugs: Vec<&str> = specs.iter().map(|spec| spec.slug.as_str()).collect();
    assert!(slugs.contains(&"pipeline"), "{slugs:?}");
    assert_eq!(
        specs
            .iter()
            .filter(|spec| spec.slug == shadowing_slug)
            .count(),
        1,
        "the company's own declaration replaces the global, it does not sit beside it"
    );
    assert_eq!(
        specs
            .iter()
            .find(|spec| spec.slug == shadowing_slug)
            .unwrap()
            .title,
        "Ours, not the baseline's"
    );
}

/// Seeded once. Retiring a ledger has to stick across a restart, or
/// "only a person retires one" is a rule the next boot takes back.
#[tokio::test]
async fn ledgers_seed_once_and_a_retirement_sticks() {
    let home_dir = tmp_home("oc-ledger-once-");
    let home = home_dir.path().to_path_buf();
    let manifest = parse("[company]\nname=\"Acme\"\n");
    let id = CompanyId::new("acme");

    let runtime = RuntimeBuilder::new(home.clone(), manifest.clone())
        .with_id(id.clone())
        .build()
        .await
        .unwrap();
    let retired = crate::globals::ledgers()[0].slug.clone();
    runtime.ledgers().delete_spec(&id, &retired).await.unwrap();
    drop(runtime);

    let runtime = RuntimeBuilder::new(home, manifest)
        .with_id(id.clone())
        .build()
        .await
        .unwrap();
    let slugs: Vec<String> = runtime
        .ledgers()
        .list_specs(&id)
        .await
        .unwrap()
        .into_iter()
        .map(|spec| spec.slug)
        .collect();
    assert!(
        !slugs.contains(&retired),
        "`{retired}` came back after a person retired it: {slugs:?}"
    );
}

/// A **shipped** bundle seeds its own ledgers, and their derived files
/// appear in the workspace.
///
/// The other seeding tests build their bundle in a tempdir, so they prove
/// the mechanism and not the content. This one boots
/// `companies/law_firm` exactly as an operator would and asserts
/// that the axes that vertical is *about* — its matter list, its deadlines —
/// are actually there, which is the whole point of the feature and the one
/// thing a tempdir fixture cannot check.
#[tokio::test]
async fn a_shipped_bundle_seeds_its_own_ledgers_and_renders_them() {
    let home_dir = tmp_home("oc-ledger-shipped-");
    let bundle = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../companies")
        .join("law_firm");
    let manifest = CompanyManifest::from_path(&bundle).expect("the shipped bundle parses");
    let id = CompanyId::new("firm");
    let runtime = RuntimeBuilder::new(home_dir.path().to_path_buf(), manifest)
        .with_id(id.clone())
        .with_seed_dir(bundle)
        .build()
        .await
        .unwrap();

    let slugs: Vec<String> = runtime
        .ledgers()
        .list_specs(&id)
        .await
        .unwrap()
        .into_iter()
        .map(|spec| spec.slug)
        .collect();
    for expected in ["matters", "deadlines", "positions"] {
        assert!(
            slugs.contains(&expected.to_string()),
            "`{expected}` was not seeded from the bundle: {slugs:?}"
        );
    }

    // And the derived files are published, so the axes are legible to
    // everything that already reads the workspace rather than only through
    // the ledger tools.
    let ctx = crate::company::ledgers::Ledgers::new(id.clone(), runtime.ledgers().clone())
        .with_workspace_opt(Some(runtime.workspace().clone()));
    crate::company::ledgers::republish_all(&ctx)
        .await
        .expect("republished");
    let tree = runtime.workspace().tree(&id).await.unwrap();
    for name in ["matters.md", "deadlines.md", "positions.md"] {
        assert!(
            tree.iter().any(|node| node.name == name),
            "`{name}` was not rendered"
        );
    }
}

/// Boot lays down `agents/` and operator-only `secrets/readme.md`. `desks/`
/// has no producer, so it is minted on first use instead of standing empty.
///
/// The per-agent folder is deliberately absent: it is minted the first time
/// that agent produces something, so a roster of teammates who have done
/// nothing yet leaves no trace in the tree.
///
/// Also pins the two gates the seeding block above does NOT share: this
/// runs with **no** `seed_dir` (the provisioned-tenant and desktop shape),
/// and it runs again on a workspace that is no longer empty — which is how
/// an existing company picks the root up.
#[tokio::test]
async fn boot_provisions_the_system_roots_and_nothing_inside_them() {
    use crate::company::workspace_scaffold::{AGENTS_ROOT, ARTIFACTS_ROOT, SECRETS_ROOT};
    use crate::ports::workspace::{NodeKind, WorkspaceOrigin};

    let home_dir = tmp_home("oc-agents-");
    let home = home_dir.path().to_path_buf();
    let id = CompanyId::new("acme");
    let roster = |agents: &str| {
        parse(&format!(
            "[company]\nname=\"Acme\"\n[policy]\nmode=\"full\"\n{agents}"
        ))
    };

    let runtime = RuntimeBuilder::new(
        home.clone(),
        roster("[[agent]]\nid=\"ceo\"\nrole=\"Chief Executive\"\n"),
    )
    .with_id(id.clone())
    .build()
    .await
    .unwrap();
    let tree = runtime.workspace().tree(&id).await.unwrap();
    let mut names: Vec<&str> = tree.iter().map(|n| n.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            AGENTS_ROOT,
            ARTIFACTS_ROOT,
            "readme.md",
            "readme.md",
            SECRETS_ROOT
        ],
        "boot provisions the managed roots with no seed dir — no `desks/`, and no \
         folder for a teammate that has produced nothing"
    );
    for node in &tree {
        assert_eq!(node.created_by, WorkspaceOrigin::Seed);
    }
    assert_eq!(
        tree.iter()
            .filter(|node| node.parent_id.is_none() && node.kind == NodeKind::Folder)
            .count(),
        crate::company::workspace_scaffold::SYSTEM_ROOTS.len(),
    );

    // An existing, non-empty workspace: an `is_empty` gate would have
    // skipped this boot entirely, and a company that predates the feature
    // would never get its roots.
    //
    // With one managed root, deleting it would leave the tree empty and
    // stop pinning that. A lazily-minted desk folder stands in for the
    // content a real company would have — and doubles as the #645 check
    // that boot neither re-manages, duplicates nor disturbs a `desks/` that
    // already exists.
    crate::company::workspace_scaffold::ensure_desk_folder(
        runtime.workspace().as_ref(),
        &id,
        "creative_studio",
    )
    .await
    .unwrap();
    let agents_root = tree
        .iter()
        .find(|n| n.name == AGENTS_ROOT)
        .unwrap()
        .id
        .clone();
    runtime.workspace().delete(&id, &agents_root).await.unwrap();
    drop(runtime);
    let runtime = RuntimeBuilder::new(
        home,
        roster(
            "[[agent]]\nid=\"ceo\"\nrole=\"Chief Executive\"\n\
             [[agent]]\nid=\"cmo\"\nrole=\"Chief Marketing\"\n",
        ),
    )
    .with_id(id.clone())
    .build()
    .await
    .unwrap();
    let tree = runtime.workspace().tree(&id).await.unwrap();
    let mut names: Vec<&str> = tree.iter().map(|n| n.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            AGENTS_ROOT,
            ARTIFACTS_ROOT,
            "creative-studio",
            "desks",
            "readme.md",
            "readme.md",
            SECRETS_ROOT,
        ],
        "the deleted root was re-provisioned, and the unmanaged `desks/` left as it stood"
    );
}

/// The root is part of what a workspace *is*, not a projection of the
/// roster: a company with no agents at all still gets it.
#[tokio::test]
async fn boot_provisions_the_roots_for_a_company_with_no_agents() {
    use crate::company::workspace_scaffold::{AGENTS_ROOT, ARTIFACTS_ROOT, SECRETS_ROOT};

    let home_dir = tmp_home("oc-noagents-");
    let id = CompanyId::new("acme");
    let runtime = RuntimeBuilder::new(
        home_dir.path().to_path_buf(),
        parse("[company]\nname=\"Acme\"\n[policy]\nmode=\"full\"\n"),
    )
    .with_id(id.clone())
    .build()
    .await
    .unwrap();

    let tree = runtime.workspace().tree(&id).await.unwrap();
    let mut names: Vec<&str> = tree.iter().map(|n| n.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            AGENTS_ROOT,
            ARTIFACTS_ROOT,
            "readme.md",
            "readme.md",
            SECRETS_ROOT
        ]
    );
}

/// Issue #85: the launch path's template provenance is stamped onto the
/// record at first build, survives a rebuild that supplies no provenance
/// (carried forward), and a company built with no provenance records `None`.
#[tokio::test]
async fn template_provenance_stamped_at_launch_and_carried_forward() {
    let home_dir = tmp_home("oc-prov-");
    let home = home_dir.path().to_path_buf();
    let manifest = parse("[company]\nname=\"Acme\"\n[policy]\nmode=\"full\"\n");
    let id = CompanyId::new("acme");
    let provenance = TemplateProvenance {
        source_id: "law_firm".to_string(),
        version: None,
        path: Some("companies/law_firm".to_string()),
    };

    // First launch from a template: provenance is stamped onto the record.
    let runtime = RuntimeBuilder::new(home.clone(), manifest.clone())
        .with_id(id.clone())
        .with_template_provenance(provenance.clone())
        .build()
        .await
        .unwrap();
    let stamped = runtime.store().load(&id).await.unwrap().unwrap();
    assert_eq!(stamped.template_provenance.as_ref(), Some(&provenance));
    drop(runtime);

    // Rebuild without re-supplying provenance: the record carries it forward.
    let runtime = RuntimeBuilder::new(home.clone(), manifest.clone())
        .with_id(id.clone())
        .build()
        .await
        .unwrap();
    let carried = runtime.store().load(&id).await.unwrap().unwrap();
    assert_eq!(
        carried.template_provenance,
        Some(provenance),
        "provenance was dropped on rebuild"
    );
    drop(runtime);

    // A company built with no provenance (raw-manifest provision) records None.
    let other = CompanyId::new("raw");
    let runtime = RuntimeBuilder::new(home.clone(), manifest)
        .with_id(other.clone())
        .build()
        .await
        .unwrap();
    let raw = runtime.store().load(&other).await.unwrap().unwrap();
    assert!(raw.template_provenance.is_none());
}

/// A rebuild that does not touch `[policy]` leaves the console override
/// alone (issue #562).
///
/// The half that makes the feature durable. Clearing on every rebuild would
/// mean a routine redeploy silently reverting the operator's console action,
/// with nothing in the console showing the tier had moved back — the exact
/// mirror of the failure the other half prevents.
#[test]
fn an_unchanged_seed_policy_leaves_the_override_alone() {
    let seed = seed_policy("supervised", &["payment.send"], None);
    let carried = carry_policy_override(&seed, &seed.clone(), Some(&held_override("full")));
    assert_eq!(carried.and_then(|o| o.mode).as_deref(), Some("full"));
}

/// A seed `[policy]` change clears the override — version control wins when
/// it speaks.
///
/// **The security half.** Without it, an operator tightening `[policy]` in
/// `company.toml` and redeploying would find a looser console override
/// silently still in force: a runtime write outliving a seed rollback, which
/// is the named harm that makes `[tools]` / `[policy]` seed-authoritative in
/// the first place. An approval gate is precisely what that rule was written
/// about.
#[test]
fn a_changed_seed_policy_clears_the_override() {
    let before = seed_policy("full", &["payment.send"], None);
    let tightened = seed_policy("supervised", &["payment.send"], None);
    assert!(
        carry_policy_override(&before, &tightened, Some(&held_override("full"))).is_none(),
        "a tightened seed must clear a looser console override"
    );

    // Loosening the seed clears it too. The rule is "the seed spoke", not
    // "the seed got stricter" — an operator who edits `[policy]` at all has
    // turned their attention to the gate, and guessing which of their edits
    // was meant to lose to the console is a guess that can pick wrong
    // silently.
    let loosened = seed_policy("full", &["payment.send"], None);
    assert!(
        carry_policy_override(&tightened, &loosened, Some(&held_override("readonly"))).is_none()
    );
}

/// Any field of `[policy]` counts as the seed speaking, not just `mode`.
///
/// `always_approve` is the operator's real lever — it wins over every tier
/// including `full` — so an edit to it that left a console override standing
/// would be the same hole through a different field.
#[test]
fn every_policy_field_counts_as_the_seed_speaking() {
    let base = seed_policy("supervised", &["payment.send"], None);

    let list_changed = seed_policy("supervised", &["payment.send", "filing.submit"], None);
    assert!(carry_policy_override(&base, &list_changed, Some(&held_override("full"))).is_none());

    let threshold_changed = seed_policy("supervised", &["payment.send"], Some(1.0));
    assert!(
        carry_policy_override(&base, &threshold_changed, Some(&held_override("full"))).is_none()
    );
}

/// With no override held there is nothing to carry, whatever the seed did.
#[test]
fn no_override_carries_nothing() {
    let before = seed_policy("supervised", &[], None);
    let after = seed_policy("full", &[], None);
    assert!(carry_policy_override(&before, &before.clone(), None).is_none());
    assert!(carry_policy_override(&before, &after, None).is_none());
}

/// A rebuild that does not touch `[tools]` leaves the console grants alone
/// (issue #1796).
///
/// The half that makes the one-click grant durable. Clearing on every
/// rebuild would put the operator straight back in the dead end: the
/// integration would read "Connected" and reach nobody again after the next
/// restart, with nothing in the console saying the grant had been dropped.
#[test]
fn an_unchanged_seed_tools_block_leaves_the_grants_alone() {
    let seed = seed_tools(&["*"]);
    let carried =
        carry_tool_grants_override(&seed, &seed.clone(), Some(&held_grants(&["chargebee"])));
    assert_eq!(
        carried.map(|o| o.added),
        Some(vec!["chargebee".to_string()])
    );
}
