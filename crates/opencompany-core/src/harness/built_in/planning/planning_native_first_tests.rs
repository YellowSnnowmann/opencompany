use std::sync::Arc;

use async_trait::async_trait;
use tinyinference::usage::Usage;

use super::planning_fixtures_tests::*;
use super::planning_whole_pass_tests::{card, read};
use super::*;
use crate::company::CompanyManifest;
use crate::ports::types::CompanyId;
use tempfile;

// ---------------------------------------------------------------------------
// Native-first routing: a built-in tool pre-empts a Composio prerequisite
// ---------------------------------------------------------------------------

fn teammate_with_grants(id: &str, grants: &[&str]) -> TeammateBrief {
    TeammateBrief {
        id: id.to_string(),
        role: "Role".to_string(),
        description: None,
        grants: grants.iter().map(|g| g.to_string()).collect(),
        global: false,
    }
}

/// A capability the company already serves with a built-in tool satisfies BOTH
/// the `connection` and `composio` prerequisite kinds — with a note that says a
/// built-in tool serves it and no Composio connection is needed — instead of
/// parking the card on a connection it never needed.
#[test]
fn a_native_capability_pre_empts_both_composio_prerequisite_kinds() {
    let mut e = evidence();
    e.native_capabilities = HashSet::from(["search".to_string()]);

    let (status, note) = verify_composio(&e, "search");
    assert_eq!(status, PrereqStatus::Satisfied);
    assert!(note.contains("built-in tool"), "{note}");
    assert!(!note.contains("Connections tab"), "{note}");

    let (status, note) = verify_connection(&e, "search");
    assert_eq!(status, PrereqStatus::Satisfied);
    assert!(note.contains("built-in tool"), "{note}");
    assert!(!note.contains("Connections tab"), "{note}");
}

/// A prerequisite that is NOT a native capability and has no connection still
/// parks — native-first widens nothing for a genuine third-party account.
#[test]
fn a_non_native_capability_without_a_connection_still_parks() {
    let mut e = evidence();
    e.native_capabilities = HashSet::from(["search".to_string()]);
    assert_eq!(verify_composio(&e, "gmail").0, PrereqStatus::Missing);
    assert_eq!(verify_connection(&e, "gmail").0, PrereqStatus::Missing);
}

/// The native branch is checked before the Composio-outage `unknown` guard, so
/// a built-in capability stays satisfied even when the probe is down — while a
/// genuine third-party name under the same outage still reads `unknown`.
#[test]
fn the_native_branch_wins_even_when_composio_is_unreachable() {
    let mut e = evidence();
    e.native_capabilities = HashSet::from(["search".to_string()]);
    e.composio_reachable = false;
    assert_eq!(verify_composio(&e, "search").0, PrereqStatus::Satisfied);
    assert_eq!(verify_connection(&e, "search").0, PrereqStatus::Satisfied);
    assert_eq!(verify_composio(&e, "gmail").0, PrereqStatus::Unknown);
}

/// The set is a union over the roster (no fixed assignee): an explicit
/// `search` grant on any one teammate marks `search` native — as long as this
/// deployment actually has a search backend wired — while `*` or `composio`
/// alone never confers the metered search family (and `composio` is not in
/// the native vocabulary).
#[test]
fn native_capabilities_are_a_union_over_the_roster_grants() {
    let maya = teammate_with_grants("maya", &["search"]);
    let set = native_capabilities_of(
        std::slice::from_ref(&maya),
        None,
        true,
        true,
        &crate::harness::toolbelt::CapabilityFilter::AllowAll,
    );
    assert!(set.contains("search"));

    let ann = teammate_with_grants("ann", &["*"]);
    let bob = teammate_with_grants("bob", &["composio"]);
    let set = native_capabilities_of(
        &[ann, bob],
        None,
        true,
        true,
        &crate::harness::toolbelt::CapabilityFilter::AllowAll,
    );
    assert!(!set.contains("search"));
    assert!(!set.contains("composio"));
}

/// PR #1946 follow-up: a grant alone is not proof of wiring. `search`/`media`
/// additionally need a backend on this deployment's harness deps
/// (`build_agent`'s second gate) — a granted-but-uncredentialed namespace
/// wires no tool at all, so the evidence must not call it native either.
/// Every other namespace (`shell` here) wires off the grant alone and is
/// unaffected by either backend flag.
#[test]
fn native_capabilities_require_the_backend_not_just_the_grant() {
    let fully_granted = teammate_with_grants("maya", &["search", "media", "shell"]);
    let teammates = [fully_granted];

    // Neither backend configured: search/media are withheld even though the
    // grant is there; shell (no backend gate) still comes through.
    let set = native_capabilities_of(
        &teammates,
        None,
        false,
        false,
        &crate::harness::toolbelt::CapabilityFilter::AllowAll,
    );
    assert!(!set.contains("search"), "{set:?}");
    assert!(!set.contains("media"), "{set:?}");
    assert!(set.contains("shell"), "{set:?}");

    // Only search wired: search native, media still withheld.
    let set = native_capabilities_of(
        &teammates,
        None,
        true,
        false,
        &crate::harness::toolbelt::CapabilityFilter::AllowAll,
    );
    assert!(set.contains("search"), "{set:?}");
    assert!(!set.contains("media"), "{set:?}");

    // Both wired: both native.
    let set = native_capabilities_of(
        &teammates,
        None,
        true,
        true,
        &crate::harness::toolbelt::CapabilityFilter::AllowAll,
    );
    assert!(set.contains("search"), "{set:?}");
    assert!(set.contains("media"), "{set:?}");
}

/// PR #1946 follow-up: once a card has a fixed assignee, the roster union is
/// the wrong question — dispatch only ever invokes that one teammate, so their
/// grants (not some other roster member's) decide whether the built-in tool is
/// on the belt this card will actually run against. `settled_assignee`
/// preserves an operator-chosen assignee rather than overriding it, so this is
/// the same fixed case.
#[test]
fn native_capabilities_narrow_to_the_fixed_assignee() {
    let teammates = [
        teammate_with_grants("maya", &["search"]),
        teammate_with_grants("bob", &["shell"]),
    ];

    // No fixed assignee: roster union — `bob` doesn't hold `search`, but
    // `maya` does, so the company can serve it.
    let set = native_capabilities_of(
        &teammates,
        None,
        true,
        true,
        &crate::harness::toolbelt::CapabilityFilter::AllowAll,
    );
    assert!(set.contains("search"), "{set:?}");

    // Fixed to `bob`, who holds no search grant: the union answer would say
    // `search` is native, but `bob` is who this card actually dispatches to,
    // so it must not be.
    let bob = teammate_with_grants("bob", &["shell"]);
    let set = native_capabilities_of(
        &teammates,
        Some(&bob),
        true,
        true,
        &crate::harness::toolbelt::CapabilityFilter::AllowAll,
    );
    assert!(!set.contains("search"), "{set:?}");

    // Fixed to `maya`, who does hold it: still native.
    let maya = teammate_with_grants("maya", &["search"]);
    let set = native_capabilities_of(
        &teammates,
        Some(&maya),
        true,
        true,
        &crate::harness::toolbelt::CapabilityFilter::AllowAll,
    );
    assert!(set.contains("search"), "{set:?}");
}

/// The active [`CapabilityFilter`](crate::harness::toolbelt::CapabilityFilter)
/// gates native evidence the same way [`toolbelt::namespace_denied`] gates the
/// live belt (mirrored on the Composio-brief side by
/// `native_caps_for_composio_brief`). A tenant tier that denies `search` still
/// leaves the grant and the backend wired — `search_backend_configured` and
/// `grants_confer` alone would both say yes — so only the filter itself can
/// catch it; a namespace the filter does not mention stays unaffected.
#[test]
fn native_capabilities_respect_the_active_capability_filter() {
    let maya = teammate_with_grants("maya", &["search", "shell"]);
    let teammates = [maya];

    let deny_search = crate::harness::toolbelt::CapabilityFilter::DenyNamespaces(
        std::collections::HashSet::from(["search"]),
    );
    let set = native_capabilities_of(&teammates, None, true, true, &deny_search);
    assert!(
        !set.contains("search"),
        "a tenant-tier denial must not be reported as native evidence: {set:?}"
    );
    assert!(set.contains("shell"), "{set:?}");

    let set = native_capabilities_of(
        &teammates,
        None,
        true,
        true,
        &crate::harness::toolbelt::CapabilityFilter::AllowAll,
    );
    assert!(set.contains("search"), "{set:?}");
}

/// `native_capabilities_of` takes `media_backend_configured` as a plain bool,
/// so it cannot see the gap this test closes: `gather_evidence` used to derive
/// that bool from `deps.media.is_some()` alone, while `media_tools` — the
/// function that actually builds the belt — additionally refuses any backend
/// whose URL isn't exactly HTTPS. A `deps.media` present but pointed at a
/// non-HTTPS host wired zero media tools yet still read as natively satisfied.
/// `gather_evidence` now reuses `MediaBackend::is_https`, the same predicate
/// `media_tools` gates on, so the two cannot diverge again.
#[tokio::test]
async fn native_media_evidence_requires_an_https_backend() {
    let manifest: CompanyManifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "maya"
role = "Writer"
tools = ["media"]

[policy]
mode = "full"

[tools]
allow = ["media"]
"#,
    )
    .expect("fixture manifest parses");

    let home = tempfile::Builder::new()
        .prefix("opencompany-planning-media-")
        .tempdir()
        .expect("tempdir");
    let mut runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(CompanyId::new("acme-media"))
        .build()
        .await
        .expect("runtime");

    let mut deps = crate::harness::workflow_wiring_deps(
        &runtime,
        None,
        crate::harness::toolbelt::CapabilityFilter::AllowAll,
        None,
    );
    deps.media = Some(crate::harness::toolbelt::MediaBackend {
        backend_url: "http://media.example".to_string(),
        auth_token: "tok".to_string(),
    });
    runtime.set_workflow_harness_deps(deps);

    let runtime = Arc::new(runtime);
    let evidence = gather_evidence(&runtime, &card("c1", "maya"))
        .await
        .expect("evidence");

    assert!(
        !evidence.native_capabilities.contains("media"),
        "a non-HTTPS media backend must not be reported as natively wired: {:?}",
        evidence.native_capabilities
    );
}

/// `media_backend_from_env`/`with_media_backend` are gated only on the broader
/// `openhuman` feature, not `media` — so `deps.media` can carry a resolved,
/// HTTPS-valid backend in a build that never compiles `media_tools`, which is
/// `#[cfg(feature = "media")]` in full. Without also checking the feature here,
/// this evidence would report `media` native in exactly the build where the
/// belt can never carry a media tool at all. This test only runs in that build
/// (the crate's `--features openhuman` gate, `media` off) — an HTTPS backend
/// present, and still no native `media` credited.
#[cfg(not(feature = "media"))]
#[tokio::test]
async fn native_media_evidence_requires_the_media_feature() {
    let manifest: CompanyManifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "maya"
role = "Writer"
tools = ["media"]

[policy]
mode = "full"

[tools]
allow = ["media"]
"#,
    )
    .expect("fixture manifest parses");

    let home = tempfile::Builder::new()
        .prefix("opencompany-planning-media-feature-")
        .tempdir()
        .expect("tempdir");
    let mut runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(CompanyId::new("acme-media-feature"))
        .build()
        .await
        .expect("runtime");

    let mut deps = crate::harness::workflow_wiring_deps(
        &runtime,
        None,
        crate::harness::toolbelt::CapabilityFilter::AllowAll,
        None,
    );
    deps.media = Some(crate::harness::toolbelt::MediaBackend {
        backend_url: "https://media.example".to_string(),
        auth_token: "tok".to_string(),
    });
    runtime.set_workflow_harness_deps(deps);

    let runtime = Arc::new(runtime);
    let evidence = gather_evidence(&runtime, &card("c1", "maya"))
        .await
        .expect("evidence");

    assert!(
        !evidence.native_capabilities.contains("media"),
        "a build without the `media` feature must never credit native media, \
         even with a valid HTTPS backend: {:?}",
        evidence.native_capabilities
    );
}

/// `gather_evidence` end-to-end: a fully-granted, fully-backed `shell` is
/// still withheld from `Evidence::native_capabilities` when the deployment's
/// active [`CapabilityFilter`](crate::harness::toolbelt::CapabilityFilter)
/// denies it. Before this fix `gather_evidence` never read
/// `deps.capabilities` at all, so a tenant-tier denial applied to the live
/// belt (`filter_by_capabilities`) went unseen here — planning could mark a
/// namespace-denied prerequisite `Satisfied` off evidence the dispatched
/// agent's actual belt disagreed with.
#[tokio::test]
async fn gather_evidence_respects_the_active_capability_filter() {
    let manifest: CompanyManifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "maya"
role = "Ops"
tools = ["shell"]

[policy]
mode = "full"

[tools]
allow = ["shell"]
"#,
    )
    .expect("fixture manifest parses");

    let home = tempfile::Builder::new()
        .prefix("opencompany-planning-capfilter-")
        .tempdir()
        .expect("tempdir");
    let mut runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(CompanyId::new("acme-capfilter"))
        .build()
        .await
        .expect("runtime");

    let deny_shell = crate::harness::toolbelt::CapabilityFilter::DenyNamespaces(
        std::collections::HashSet::from(["shell"]),
    );
    let deps = crate::harness::workflow_wiring_deps(&runtime, None, deny_shell, None);
    runtime.set_workflow_harness_deps(deps);

    let runtime = Arc::new(runtime);
    let evidence = gather_evidence(&runtime, &card("c1", "maya"))
        .await
        .expect("evidence");

    assert!(
        !evidence.native_capabilities.contains("shell"),
        "a tenant-tier denial on the active CapabilityFilter must not be \
         reported as native evidence: {:?}",
        evidence.native_capabilities
    );
}

/// A meter reporting spend already past a plan's per-namespace budget — the
/// same shape `HarnessPool::ensure_impl`'s
/// `ensure_gates_shell_tools_once_the_token_budget_is_crossed` test uses to
/// prove the live belt gets gated.
struct ExhaustedMeter(Vec<crate::ports::usage::UsageSample>);

#[async_trait]
impl crate::ports::UsageMeter for ExhaustedMeter {
    async fn record(
        &self,
        _company: &CompanyId,
        _sample: &crate::ports::usage::UsageSample,
    ) -> crate::Result<()> {
        Ok(())
    }
    async fn query(
        &self,
        _company: &CompanyId,
        _since_millis: u64,
    ) -> crate::Result<Vec<crate::ports::usage::UsageSample>> {
        Ok(self.0.clone())
    }
}

/// `runtime.workflow_harness_deps.capabilities` is the boot-time identity
/// snapshot [`RuntimeBuilder`](crate::runtime::RuntimeBuilder) always seeds as
/// [`AllowAll`](crate::harness::toolbelt::CapabilityFilter::AllowAll) — with a
/// `[plan]` set, only `HarnessPool::ensure_impl` re-resolves the tenant's live
/// spend against the meter every turn, and installs the result solely onto its
/// own local `fresh_deps` used to build the roster, never writing it back onto
/// `runtime.workflow_harness_deps`. Exactly the same seam `tenant_search` had
/// before the fix two commits up, one field over: after a namespace budget is
/// exhausted post-boot, `gather_evidence` reading the stale `AllowAll`
/// snapshot would still mark that namespace's evidence native while the next
/// dispatched agent has the tool actually stripped from its belt. This test
/// wires a `[plan]` budgeting `shell` at 100 tokens and a meter already
/// reporting 150 spent — an exhausted budget the very first `gather_evidence`
/// call ever sees, with no prior `HarnessPool::ensure` in between — and proves
/// the evidence agrees with what the belt would be gated to, not with the
/// stale identity snapshot.
#[tokio::test]
async fn native_shell_evidence_reflects_a_budget_exhausted_after_boot() {
    let manifest: CompanyManifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "maya"
role = "Ops"
tools = ["shell"]

[policy]
mode = "full"

[tools]
allow = ["shell"]
"#,
    )
    .expect("fixture manifest parses");

    let home = tempfile::Builder::new()
        .prefix("opencompany-planning-budget-exhausted-")
        .tempdir()
        .expect("tempdir");
    let mut runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(CompanyId::new("acme-budget-exhausted"))
        .build()
        .await
        .expect("runtime");

    let plan = crate::harness::capability_budget::CapabilityPlan {
        period: crate::harness::capability_budget::BudgetPeriod::Daily,
        budgets: std::collections::BTreeMap::from([("shell".to_string(), 100u64)]),
        total_budget: None,
    };
    let meter: Arc<dyn crate::ports::UsageMeter> =
        Arc::new(ExhaustedMeter(vec![crate::ports::usage::UsageSample {
            at_millis: crate::ports::now_millis(),
            agent: "maya".into(),
            provider: "managed".into(),
            input_tokens: 100,
            output_tokens: 50,
            cached_input_tokens: 0,
            cost_usd: 0.0,
            kind: crate::ports::usage::SampleKind::Inference,
            run_id: None,
            model: None,
        }]));

    // `deps.capabilities` is set to the same `AllowAll` identity
    // `RuntimeBuilder` always seeds — the stale boot-time snapshot this fix
    // must stop trusting once a `[plan]`/meter pair is wired.
    let deps = crate::harness::workflow_wiring_deps(
        &runtime,
        Some(meter),
        crate::harness::toolbelt::CapabilityFilter::AllowAll,
        Some(plan),
    );
    runtime.set_workflow_harness_deps(deps);

    let runtime = Arc::new(runtime);
    let evidence = gather_evidence(&runtime, &card("c1", "maya"))
        .await
        .expect("evidence");

    assert!(
        !evidence.native_capabilities.contains("shell"),
        "a namespace budget already exhausted when this evidence pass first \
         runs must not be reported as native off the stale AllowAll boot \
         snapshot: {:?}",
        evidence.native_capabilities
    );
}

/// `runtime.workflow_harness_deps.tenant_search` is a boot-time snapshot —
/// only `HarnessPool::ensure` re-resolves the live BYO search connection, and
/// only into the pool's own local `fresh_deps` used to build the roster, never
/// written back onto `runtime.workflow_harness_deps`. A company that pastes a
/// BYO key in the console after startup therefore gets a roster with a live
/// `web_search` tool while this evidence, read straight off the stale
/// snapshot, still says `search` is not natively wired — parking the card on a
/// missing `connection: search` prerequisite the belt does not actually have.
/// `gather_evidence` now re-resolves the same way `HarnessPool::ensure` does
/// when the boot snapshot has nothing, so the two cannot diverge.
#[tokio::test]
async fn native_search_evidence_reflects_a_byo_key_added_after_boot() {
    let manifest: CompanyManifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "maya"
role = "Researcher"
tools = ["search"]

[policy]
mode = "full"

[tools]
allow = ["search"]
"#,
    )
    .expect("fixture manifest parses");

    let home = tempfile::Builder::new()
        .prefix("opencompany-planning-search-")
        .tempdir()
        .expect("tempdir");
    let mut runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(CompanyId::new("acme-search"))
        .build()
        .await
        .expect("runtime");

    // The boot-time snapshot: no BYO search configured yet.
    let mut deps = crate::harness::workflow_wiring_deps(
        &runtime,
        None,
        crate::harness::toolbelt::CapabilityFilter::AllowAll,
        None,
    );
    deps.tenant_search = None;
    runtime.set_workflow_harness_deps(deps);

    let runtime = Arc::new(runtime);

    // The company pastes a BYO key in the console after startup — landing
    // straight in the secret store, the way the console route does, with no
    // `HarnessPool::ensure` in between to refresh `workflow_harness_deps`.
    runtime
        .secrets()
        .set(
            runtime.id(),
            crate::company::search::PROVIDER_SECRET,
            crate::ports::types::SecretValue("brave".to_string()),
        )
        .await
        .expect("write provider");
    runtime
        .secrets()
        .set(
            runtime.id(),
            crate::company::search::API_KEY_SECRET,
            crate::ports::types::SecretValue("test-key".to_string()),
        )
        .await
        .expect("write key");

    let evidence = gather_evidence(&runtime, &card("c1", "maya"))
        .await
        .expect("evidence");

    assert!(
        evidence.native_capabilities.contains("search"),
        "a BYO search key added after boot must reach the evidence the same live \
         way it reaches the roster, not stay hidden behind the stale boot snapshot: {:?}",
        evidence.native_capabilities
    );
}

/// `gather_evidence` resolves the capability filter live, but it does so
/// *before* [`call_model`] runs — and `run_planning_pass` charges that call's
/// own tokens through
/// [`record_usage`] only afterward. A tenant sitting just under a namespace's
/// budget, whose planning call is itself what tips the spend over, must not
/// have that namespace's evidence stamped native off the pre-charge filter —
/// `HarnessPool::ensure` re-resolves fresh before the dispatched agent's turn
/// and would strip the tool the card's evidence just told the operator it had.
///
/// The plan budgets `shell` at 300 tokens (Daily) with 250 already spent —
/// under budget, so the filter `gather_evidence` resolves before the model
/// call allows `shell`. The scripted model's own reply reports 100 more
/// tokens, which `record_usage` charges to the SAME meter this plan reads
/// from — pushing the period's spend to 350, over the 300 budget. The
/// prerequisite the model asked about names `shell` as a `connection`, the
/// same claim shape `verify_connection` short-circuits `Satisfied` for a
/// native capability.
#[tokio::test]
async fn native_shell_evidence_reflects_the_planning_calls_own_charge() {
    let manifest: CompanyManifest = toml::from_str(
        r#"
[company]
name = "Acme"

[[agent]]
id = "maya"
role = "Ops"
tools = ["shell"]

[policy]
mode = "full"

[tools]
allow = ["shell"]
"#,
    )
    .expect("fixture manifest parses");

    let home = tempfile::Builder::new()
        .prefix("opencompany-planning-charge-crosses-budget-")
        .tempdir()
        .expect("tempdir");
    let mut runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .with_id(CompanyId::new("acme-charge-crosses-budget"))
        .build()
        .await
        .expect("runtime");

    // Pre-existing spend: 250 of a 300-token daily `shell` budget — under
    // budget, so the filter `gather_evidence` resolves before the model call
    // still allows `shell`.
    runtime
        .usage()
        .record(
            runtime.id(),
            &crate::ports::usage::UsageSample {
                at_millis: crate::ports::now_millis(),
                agent: "maya".into(),
                provider: "managed".into(),
                input_tokens: 200,
                output_tokens: 50,
                cached_input_tokens: 0,
                cost_usd: 0.0,
                kind: crate::ports::usage::SampleKind::Inference,
                run_id: None,
                model: None,
            },
        )
        .await
        .expect("seed the prior spend");

    let plan = crate::harness::capability_budget::CapabilityPlan {
        period: crate::harness::capability_budget::BudgetPeriod::Daily,
        budgets: std::collections::BTreeMap::from([("shell".to_string(), 300u64)]),
        total_budget: None,
    };
    // The SAME meter `record_usage` charges the planning call's tokens to —
    // proving the re-resolve reads the spend this very pass just wrote, not a
    // second, disconnected meter.
    let deps = crate::harness::workflow_wiring_deps(
        &runtime,
        Some(runtime.usage().clone()),
        crate::harness::toolbelt::CapabilityFilter::AllowAll,
        Some(plan),
    );
    runtime.set_workflow_harness_deps(deps);

    let reply = r#"{"description":"Run the release script","steps":[{"title":"Run it","detail":"execute the release script locally"}],
        "prerequisites":[{"kind":"connection","name":"shell","why":"the release script runs on this host"}],
        "risks":[],"verification":"the script exits 0","scope":"the release script only"}"#;
    let model = ScriptedModel::replying_with_usage(
        reply,
        Usage {
            input_tokens: 80,
            output_tokens: 20,
            ..Default::default()
        },
    );
    runtime.set_planner(Arc::new(TaskPlanner::new(model, "chat-v1")));

    let runtime = Arc::new(runtime);
    runtime
        .tasks()
        .upsert(runtime.id(), &card("t-charge", "maya"))
        .await
        .unwrap();

    run_planning_pass(Arc::clone(&runtime), "t-charge".to_string()).await;

    let after = read(&runtime, "t-charge").await;
    let plan = after.plan.expect("the brief is on the card");
    let shell = plan
        .prerequisites
        .iter()
        .find(|p| p.name == "shell")
        .expect("the shell prerequisite was verified");
    assert_ne!(
        shell.status,
        PrereqStatus::Satisfied,
        "the planning call's own 100 tokens push the 250-already-spent tenant past the \
         300-token shell budget — evidence resolved before that charge must not still report \
         shell native: {shell:?}"
    );
    assert_eq!(
        after.column,
        crate::ports::tasks::COLUMN_PAUSED,
        "a shell prerequisite that is no longer satisfied is a blocker, so the card must not \
         dispatch on a belt that will have the tool stripped"
    );
}
