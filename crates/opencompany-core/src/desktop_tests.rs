use super::*;

use crate::ports::CompanyStore;

#[test]
fn ships_the_full_company_template_catalog() {
    assert_eq!(PRESETS.len(), 19);
    assert_eq!(
        preset(DEFAULT_PRESET_ID).unwrap().name,
        "Agentic Marketing Agency"
    );
    assert!(PRESETS.iter().all(|preset| !preset.manifest.is_empty()));
}

/// The embedded roster must equal the one the same bundle yields on disk.
///
/// Two code paths now produce a roster — `agents/` read off the filesystem
/// by `CompanyManifest::from_located`, and the table `build.rs` embeds — and
/// a company must not depend on which one loaded it. They share a parser, so
/// field-level drift is unlikely; **order** is the real risk, and it is
/// load-bearing: `orchestrator_id` falls back to "the first agent declared"
/// when nobody is tagged `tier = "orchestrator"`, so a different sort would
/// silently hand the company to a different teammate.
///
/// Compares ids in order, plus each agent's resolved prompt documents, which
/// is where the embedded path does the most work that disk gets for free.
#[test]
fn the_embedded_roster_matches_the_bundle_on_disk() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../companies");

    for preset in PRESETS {
        let bundle = root.join(preset.id);
        if !bundle.join(crate::company::agent_file::AGENTS_DIR).is_dir() {
            continue;
        }

        let from_disk = crate::company::agent_file::load_agents(&bundle)
            .unwrap_or_else(|e| panic!("bundle `{}` must load from disk: {e}", preset.id));
        let embedded = embedded_roster(preset.id)
            .unwrap_or_else(|e| panic!("bundle `{}` must load embedded: {e}", preset.id));

        assert_eq!(
            embedded.iter().map(|a| &a.id).collect::<Vec<_>>(),
            from_disk.iter().map(|a| &a.id).collect::<Vec<_>>(),
            "bundle `{}` yields a different roster order embedded than on \
             disk, which changes which teammate orchestrates",
            preset.id
        );
        for (embedded, disk) in embedded.iter().zip(from_disk.iter()) {
            assert_eq!(
                embedded.prompt_files_resolved, disk.prompt_files_resolved,
                "agent `{}` in bundle `{}` resolves different prompt \
                 documents embedded than on disk",
                disk.id, preset.id
            );
            assert_eq!(embedded.role, disk.role);
            assert_eq!(embedded.tier, disk.tier);
            assert_eq!(embedded.tools, disk.tools);
        }
    }
}

/// Every bundled template must seed a company that has teammates.
///
/// The roster moved out of `company.toml` and into `agents/*.toml`, and the
/// disk path followed it — `CompanyManifest::from_located` reads the bundle
/// when `agents/` is present. The embedded path did not: a preset carries
/// only the `include_str!`'d `company.toml`, so `first_run_manifest` parsed
/// a manifest whose `[[agent]]` entries no longer exist and seeded an empty
/// roster. That reaches both desktop first-run and the setup flow, since
/// `seed_company` builds from this manifest.
///
/// Asserted for every preset rather than just the default, because the
/// failure is per bundle and silent: a company with no teammates still
/// boots and still serves, it simply never answers.
#[test]
fn every_preset_seeds_a_non_empty_roster() {
    for preset in PRESETS {
        let manifest = first_run_manifest(preset.id)
            .unwrap_or_else(|e| panic!("preset `{}` must parse: {e}", preset.id));

        assert!(
            !manifest.agents.is_empty(),
            "preset `{}` seeds a company with no teammates — its roster lives \
             in `agents/*.toml`, which the embedded manifest does not carry",
            preset.id
        );
    }
}

/// No shipped template narrows Composio to a hand-written toolkit list.
///
/// Absent means open mode: the host answers with the backend's live catalog,
/// which is every toolkit it permits. A non-empty list is authoritative and
/// offered verbatim — the catalog is not consulted and nothing may widen it —
/// so declaring one here caps both the agent belt and the Connections tab at
/// whatever was typed, for every operator who starts from that template.
///
/// `software_company` briefly carried such a list, added to work
/// around a console that rendered zero provider rows for an empty one
/// (#397). That root cause is fixed, so a list added here now buys nothing
/// and silently restores the cap. Narrowing a template is a legitimate
/// choice — it is just one that has to be made deliberately, which is what
/// this test forces (#550).
#[test]
fn no_shipped_template_caps_the_composio_toolkit_list() {
    for preset in PRESETS {
        let manifest: toml::Value = toml::from_str(preset.manifest)
            .unwrap_or_else(|e| panic!("{} manifest does not parse: {e}", preset.id));
        let declared = manifest
            .get("tools")
            .and_then(|tools| tools.get("composio"))
            .and_then(|composio| composio.get("toolkits"));
        assert!(
            declared.is_none(),
            "{} declares [tools.composio].toolkits = {:?}, which pins its \
             Connections tab to that list instead of the backend's catalog. \
             Remove it, or narrow this template on purpose and say why here.",
            preset.id,
            declared.unwrap(),
        );
    }
}

/// `manifest_parsed` is what `server::setup::templates()` calls to describe
/// each shipped preset without touching disk — so a preset that fails to
/// parse must report a specific, attributable error rather than panic or
/// silently produce garbage.
#[test]
fn manifest_parsed_reads_a_real_preset() {
    let preset = preset(DEFAULT_PRESET_ID).expect("default preset is shipped");
    let manifest = preset.manifest_parsed().unwrap();
    assert_eq!(manifest.company.name, "Agentic Marketing Agency");
    assert!(
        !manifest.agents.is_empty(),
        "the default preset should ship at least one agent"
    );
}

#[test]
fn manifest_parsed_reports_a_configuration_error_for_a_malformed_manifest() {
    let bad = DesktopPreset {
        id: "not-a-real-preset",
        name: "Not A Real Preset",
        manifest: "this is not valid toml >>>",
    };
    let err = bad.manifest_parsed().unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("not-a-real-preset"),
        "the error should name the offending preset id: {message}"
    );
}

#[tokio::test]
async fn starts_a_loopback_runtime_from_the_default_preset() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = start_local(directory.path(), DEFAULT_PRESET_ID)
        .await
        .unwrap();
    assert!(runtime.config().api_url.starts_with("http://127.0.0.1:"));
    assert_eq!(runtime.config().company, "agentic-marketing-agency");
}

/// A desktop company must come up with an agent harness attached.
///
/// The gap this pins was two-part and each half was silent on its own. The
/// desktop shell shipped without the `openhuman` feature, so `src/harness/`
/// was not compiled at all and the console's inference test answered "This
/// build cannot reach a model — the agent harness is not compiled in"; and
/// `register` — the only path a desktop company is built through — never
/// called `with_harness`, so even a build that *did* compile one in would
/// have handed every company the echo brain instead.
///
/// Gated on the feature because there is nothing to attach without it, and
/// asserted here rather than on `attach` itself because the seam that broke
/// is this registration path, not the helper.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_seeded_company_has_a_harness() {
    let directory = tempfile::tempdir().unwrap();
    let state = state_over(directory.path());

    bootstrap_companies(&state, DEFAULT_PRESET_ID)
        .await
        .expect("a fresh root bootstraps");

    let runtime = state.registry().sole().expect("the company is registered");
    assert!(
        runtime.harness().is_some(),
        "a desktop company was built without a harness pool, so every turn \
         falls back to the echo brain",
    );
}

/// A state over `home`, as an embedded host builds one.
fn state_over(home: &std::path::Path) -> AppState {
    AppState::new(AppConfig::default()).with_home(home.to_path_buf())
}

fn store_over(home: &std::path::Path) -> crate::store::FsCompanyStore {
    crate::store::FsCompanyStore::new(home.to_path_buf())
}

/// A state shaped like the packaged desktop: loopback, and `none` host-wide.
///
/// The override is what the shipped shell sets (`src-tauri/src/embedded.rs`),
/// and it is set *there* rather than in the preset manifests deliberately —
/// `[users].mode = "none"` beside a `[users].admins` entry is a manifest
/// `validate_users` flags, and both seeding paths treat a flagged manifest as
/// a hard error. The override never rewrites the manifest, so nothing is
/// flagged.
fn desktop_state_over(home: &std::path::Path) -> AppState {
    AppState::new(AppConfig {
        auth_mode_override: Some(crate::app::config::AuthMode::None),
        ..AppConfig::default()
    })
    .with_home(home.to_path_buf())
}

/// Issue #632: a fresh install must end up somewhere its owner can work.
///
/// The answer changed shape rather than going away. The seeded company used
/// to name a synthetic mailbox in `[users].admins`, because `eligibility`
/// admits nobody a manifest has not named and a company nobody is eligible
/// for is the same dead end as no company at all. The desktop now runs
/// [`AuthMode::None`](crate::app::config::AuthMode::None), where there is no
/// eligibility question to answer: the person at the machine *is* the
/// principal. A bootstrap roster would grant nothing to nobody.
///
/// So the assertion moved rather than relaxed — an empty `[users].admins` is
/// now the correct state, and the thing that must hold is the mode the
/// company is actually built in.
#[tokio::test]
async fn a_first_run_seeds_a_company_its_owner_needs_no_account_for() {
    let directory = tempfile::tempdir().unwrap();
    let state = desktop_state_over(directory.path());

    let ids = bootstrap_companies(&state, DEFAULT_PRESET_ID)
        .await
        .expect("a fresh root bootstraps");

    assert_eq!(ids.len(), 1, "one starter company, not a fleet");
    let runtime = state
        .registry()
        .sole()
        .expect("the seeded company is registered, not merely written");
    assert_eq!(
        runtime.auth_mode(),
        crate::app::config::AuthMode::None,
        "the host-wide mode is what a desktop company is built in"
    );
    let record = runtime
        .store()
        .load(runtime.id())
        .await
        .unwrap()
        .expect("the seeded company persists");
    assert!(
        record.manifest.users.admins.is_empty(),
        "a `none`-mode company has no bootstrap roster to name, and naming \
         one here would put `[users].admins` under a mode that grants it \
         nothing: {:?}",
        record.manifest.users.admins
    );
    assert_eq!(
        record.template_provenance.map(|p| p.source_id),
        Some(DEFAULT_PRESET_ID.to_string()),
        "the install records which template it started from"
    );
}

/// The second launch is the one that would go wrong quietly: seeding again
/// hands the operator a duplicate starter company, and every launch after
/// that another.
#[tokio::test]
async fn a_later_launch_adopts_what_the_root_already_holds() {
    let directory = tempfile::tempdir().unwrap();
    let first = bootstrap_companies(&state_over(directory.path()), DEFAULT_PRESET_ID)
        .await
        .unwrap();

    // A wholly fresh state, as a relaunched application has.
    let relaunched = state_over(directory.path());
    let second = bootstrap_companies(&relaunched, DEFAULT_PRESET_ID)
        .await
        .unwrap();

    assert_eq!(first, second, "the same company comes back");
    assert_eq!(
        std::fs::read_dir(directory.path().join("companies"))
            .unwrap()
            .count(),
        1,
        "and no second bundle was written"
    );
    assert!(relaunched.registry().sole().is_some());
}

/// A damaged bundle costs its own company and nothing else.
///
/// The failure this rules out is the whole-desktop one: a boot that gave up
/// on the first unreadable bundle would leave an operator with no local host
/// at all — not even the companies that are perfectly intact — and the
/// console renders that as "no embedded host", which reads like the app is
/// broken rather than like one company is.
#[tokio::test]
async fn a_damaged_bundle_does_not_take_the_healthy_one_with_it() {
    let directory = tempfile::tempdir().unwrap();
    let state = state_over(directory.path());
    let healthy = first_run_manifest(DEFAULT_PRESET_ID).unwrap();
    let healthy_id = company_id_from_name(&healthy.company.name);
    register(&state, healthy_id.clone(), healthy, None)
        .await
        .unwrap();

    let damaged = directory.path().join("companies").join("broken");
    std::fs::create_dir_all(&damaged).unwrap();
    std::fs::write(
        damaged.join("company.toml"),
        "this is not manifest = toml {{",
    )
    .unwrap();

    let relaunched = state_over(directory.path());
    let ids = bootstrap_companies(&relaunched, DEFAULT_PRESET_ID)
        .await
        .expect("a damaged bundle must not fail the boot");

    assert_eq!(ids, vec![healthy_id], "the intact company still comes up");
    assert!(
        relaunched.registry().sole().is_some(),
        "and it is the only one, rather than joined by a second starter"
    );
}

/// Archiving removes a company from the registry deliberately. A boot that
/// re-registered every bundle on disk would undo that at the next launch,
/// silently, and the operator would find it back in the picker.
#[tokio::test]
async fn an_archived_company_is_not_brought_back() {
    let directory = tempfile::tempdir().unwrap();
    let state = state_over(directory.path());
    bootstrap_companies(&state, DEFAULT_PRESET_ID)
        .await
        .unwrap();
    // A second company, so the archived one is skipped rather than merely
    // replaced by the first-run seed.
    let kept = first_run_manifest("law_firm").unwrap();
    let kept_id = company_id_from_name(&kept.company.name);
    register(&state, kept_id.clone(), kept, None).await.unwrap();

    let store = store_over(directory.path());
    let archived_id =
        company_id_from_name(&first_run_manifest(DEFAULT_PRESET_ID).unwrap().company.name);
    let mut record = store.load(&archived_id).await.unwrap().unwrap();
    record.lifecycle = "archived".to_string();
    store.save(&record).await.unwrap();

    let relaunched = state_over(directory.path());
    let ids = bootstrap_companies(&relaunched, DEFAULT_PRESET_ID)
        .await
        .unwrap();

    assert_eq!(ids, vec![kept_id]);
    assert!(
        relaunched.registry().get(&archived_id).is_none(),
        "an archived company must stay unaddressable"
    );
}

/// `adopt_companies` registers what is on disk and seeds nothing.
///
/// This is the half `serve` calls when no `--company` was named. A company
/// can now reach a data root without ever being named on a command line —
/// the first-run setup flow puts one there — and before this existed, an
/// operator who finished setup, was told to restart for their settings to
/// apply, and did, came back to "serving with no companies" with their
/// company sitting unread on disk.
#[tokio::test]
async fn adopting_registers_a_stored_company_without_seeding_one() {
    let directory = tempfile::tempdir().unwrap();

    // An empty root adopts nothing and — unlike `bootstrap_companies` —
    // must not invent a starter company. `serve` is not the desktop app.
    let empty = state_over(directory.path());
    assert!(adopt_companies(&empty).await.unwrap().is_empty());
    assert_eq!(empty.registry().len(), 0, "adopting must never seed");

    // Now put one there, the way setup does.
    let seeded = state_over(directory.path());
    let id = seed_company(&seeded, "law_firm").await.unwrap();

    // A later boot finds it.
    let relaunched = state_over(directory.path());
    let adopted = adopt_companies(&relaunched).await.unwrap();
    assert_eq!(
        adopted.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
        vec![id.clone()],
    );
    assert!(relaunched.registry().get(&id).is_some());
    assert_eq!(
        adopted[0].1.company.name, "Agentic Law Firm",
        "the manifest comes back with the id, because serve needs its schedules"
    );
}

/// The host-wide sign-in mode reaches a company registered through this
/// path, not just one named by `serve --company`.
///
/// Without it the first-run setup flow is a lie: it writes `auth_mode` to
/// `config.toml`, tells the operator the setting needs a restart, and the
/// restart then adopts the company under its manifest's own mode instead —
/// configuration silently ignored, which is the exact failure the setup
/// surface exists to prevent.
#[tokio::test]
async fn the_host_wide_auth_mode_reaches_an_adopted_company() {
    let directory = tempfile::tempdir().unwrap();

    let seeding = state_over(directory.path());
    let id = seed_company(&seeding, "law_firm").await.unwrap();
    assert_eq!(
        seeding.registry().get(&id).unwrap().auth_mode(),
        crate::app::config::AuthMode::Email,
        "the shipped template's own mode, with no override in play"
    );

    // Loopback, so `none` is permitted — a routable host refuses it.
    let overridden = AppState::new(AppConfig {
        bind: "127.0.0.1:8080".to_string(),
        auth_mode_override: Some(crate::app::config::AuthMode::None),
        ..AppConfig::default()
    })
    .with_home(directory.path().to_path_buf());

    adopt_companies(&overridden).await.unwrap();

    assert_eq!(
        overridden.registry().get(&id).unwrap().auth_mode(),
        crate::app::config::AuthMode::None,
        "the host-wide mode must outrank the manifest's `[users].mode`"
    );
}
