use super::*;

#[test]
fn a_label_becomes_a_safe_directory_name() {
    assert_eq!(slugify("Acme Corp"), "acme-corp");
    assert_eq!(slugify("  Acme/../etc  "), "acme-etc");
    assert_eq!(slugify("../../escape"), "escape");
    assert_eq!(slugify("日本語"), "instance");
    assert_eq!(slugify("A"), "a");
}

#[test]
fn a_root_that_escapes_the_data_dir_is_refused() {
    assert!(is_contained("instances/acme"));
    assert!(!is_contained("../elsewhere"));
    assert!(!is_contained("/etc"));
    assert!(!is_contained("instances/../../elsewhere"));
}

/// A fresh install has exactly the instance it has always had, rooted where
/// it has always been rooted.
#[tokio::test]
async fn a_fresh_data_dir_holds_one_instance_at_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let hosts = LocalHosts::load(dir.path().to_path_buf()).await;

    let listed = hosts.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, DEFAULT_INSTANCE_ID);
    assert!(listed[0].running, "{:?}", listed[0].error);
    assert_eq!(listed[0].data_dir, dir.path().display().to_string());
    assert!(hosts.default_instance().is_some());
}

/// The point of the whole module: two instances, two roots, two ports.
#[tokio::test]
async fn a_second_instance_gets_its_own_root_and_port() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;

    let created = hosts.create("Acme Corp").await.expect("it starts");
    assert_eq!(created.id, "acme-corp");
    assert!(created.running);
    assert_eq!(
        created.data_dir,
        dir.path().join("instances/acme-corp").display().to_string()
    );

    let listed = hosts.list();
    assert_eq!(listed.len(), 2);
    let ports: HashSet<_> = listed
        .iter()
        .map(|instance| instance.base_url.clone().expect("running"))
        .collect();
    assert_eq!(ports.len(), 2, "each instance binds its own port");
    // Two roots are two hosts, and the console tells them apart by this.
    assert_ne!(listed[0].instance_id, listed[1].instance_id);
}

/// Quiescing for a restart is not stopping.
///
/// The update path has to release every data root before the replacement
/// process reaches for it, but the operator did not ask for anything to be
/// switched off — so the relaunched application must come back running what
/// this one was running. `stop` would have written `autostart = false` and
/// brought the companies back down.
#[tokio::test]
async fn quiescing_releases_the_roots_without_editing_the_roster() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    hosts.create("Acme Corp").await.expect("it starts");

    let quiesced = hosts.quiesce();

    assert_eq!(quiesced.len(), 2, "both instances were listening");
    assert!(
        hosts.list().iter().all(|instance| !instance.running),
        "every root has to be free before the successor launches"
    );

    // What the next launch sees, over the roster this one left behind.
    drop(hosts);
    let relaunched = LocalHosts::load(dir.path().to_path_buf()).await;
    assert!(
        relaunched.list().iter().all(|instance| instance.running),
        "an instance that was up before the update must be up after it"
    );
}

/// The roster is what makes an instance a thing rather than a session.
#[tokio::test]
async fn instances_come_back_after_a_relaunch() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    let created = hosts.create("Acme").await.expect("it starts");
    let identity = created.instance_id.clone();
    // Quitting: every host is dropped, releasing its root and its port.
    drop(hosts);

    let relaunched = relaunch(dir.path()).await;
    let listed = relaunched.list();
    assert_eq!(listed.len(), 2);
    let acme = listed.iter().find(|i| i.id == "acme").expect("remembered");
    assert!(acme.running, "{:?}", acme.error);
    // The same data root is the same host, whatever port it landed on.
    assert_eq!(acme.instance_id, identity);
}

#[tokio::test]
async fn a_stopped_instance_stays_stopped_across_a_relaunch() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    hosts.create("Acme").await.expect("it starts");

    let stopped = hosts.stop("acme").expect("a known instance");
    assert!(!stopped.running);
    assert!(stopped.base_url.is_none());
    drop(hosts);

    let relaunched = relaunch_until(dir.path(), default_is_running).await;
    let acme = relaunched
        .list()
        .into_iter()
        .find(|i| i.id == "acme")
        .expect("still rostered");
    assert!(!acme.running, "the stop button must survive a quit");
}

#[tokio::test]
async fn a_stopped_instance_can_be_started_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    let first = hosts.create("Acme").await.expect("it starts");
    hosts.stop("acme").expect("a known instance");

    let restarted = start_when_free(&mut hosts, "acme").await;
    assert!(restarted.running);
    assert_eq!(
        restarted.instance_id, first.instance_id,
        "the same root is the same host"
    );
}

/// The onboarding guarantee: a created instance opens the setup wizard.
///
/// Asserted over HTTP on `/spec`, because that is the only thing the
/// console actually consults — `ConnectionConsole` enters its `setup`
/// phase on `setup_complete === false` and nothing else. And the field is
/// computed as `stamp || !registry.is_empty()`, so "did it seed a company"
/// and "does the wizard open" are the same question asked twice. Asserting
/// an empty company list would prove only the first half.
#[tokio::test]
async fn a_created_instance_opens_the_setup_wizard() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    let created = hosts.create("Acme").await.expect("it starts");

    assert!(
        created.companies.is_empty(),
        "an instance an operator asked for must not be handed a company they did not choose"
    );
    let spec: serde_json::Value = reqwest::get(format!(
        "{}/spec",
        created.base_url.as_deref().expect("running")
    ))
    .await
    .expect("the reported address answers")
    .json()
    .await
    .expect("a spec document");
    assert_eq!(
        spec["setup_complete"],
        serde_json::json!(false),
        "a fresh root must report setup as outstanding, or the wizard never opens: {spec}"
    );
}

/// And the instance at the data root gets the same guarantee, which is the
/// half that used to be missing.
///
/// The pair matters more than either alone: these two hosts made exactly
/// one decision differently, and it was the decision that made onboarding
/// unreachable on the only install most operators will ever have. Asserted
/// over `/spec` for the reason the sibling test gives — `setup_complete` is
/// the whole of what the console consults, so an empty company list would
/// prove only half of it.
#[tokio::test]
async fn the_instance_at_the_data_root_opens_the_setup_wizard_too() {
    let dir = tempfile::tempdir().unwrap();
    let hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    let default = hosts.default_instance().expect("it starts");

    assert!(
        default.companies.is_empty(),
        "a fresh install must not be handed a company nobody chose"
    );
    let spec: serde_json::Value = reqwest::get(format!(
        "{}/spec",
        default.base_url.as_deref().expect("running")
    ))
    .await
    .expect("the reported address answers")
    .json()
    .await
    .expect("a spec document");
    assert_eq!(
        spec["setup_complete"],
        serde_json::json!(false),
        "the wizard opens on the default instance, or nothing does: {spec}"
    );
}

/// The migration guarantee, and the only thing standing between this change
/// and "my company is gone".
///
/// Not seeding a *fresh* root must not mean ignoring a *used* one. Every
/// install that has ever been opened keeps its company under the data dir
/// itself (see the module header), and that company has to come back
/// without a wizard in front of it — the operator set this machine up long
/// ago and has nothing left to decide.
#[tokio::test]
async fn a_data_root_that_already_holds_a_company_boots_straight_into_it() {
    let dir = tempfile::tempdir().unwrap();
    // What a used install looks like on disk: a bundle under the data dir,
    // put there by an older launch that seeded, or by a completed wizard.
    let existing = seed_a_company_into(dir.path()).await;
    assert_eq!(existing.len(), 1, "the fixture writes one company");

    let hosts = relaunch(dir.path()).await;
    let default = hosts.default_instance().expect("it starts");

    assert_eq!(
        default.companies, existing,
        "a root with a company in it must adopt it, not ignore it"
    );
    let spec: serde_json::Value = reqwest::get(format!(
        "{}/spec",
        default.base_url.as_deref().expect("running")
    ))
    .await
    .expect("the reported address answers")
    .json()
    .await
    .expect("a spec document");
    assert_eq!(
        spec["setup_complete"],
        serde_json::json!(true),
        "an install that is already set up must never be walked through setup: {spec}"
    );
}

/// Skipping the *seed* must not skip the *adopt*.
///
/// A company the wizard writes into a created instance's root is a bundle
/// on disk and nothing else. If `RunSetupWizard` meant "register nothing",
/// the instance would come back from every relaunch serving an empty
/// registry — and, worse, reporting setup outstanding again, so the
/// operator would be walked through the wizard once per launch.
#[tokio::test]
async fn a_created_instance_keeps_the_company_it_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    hosts.create("Acme").await.expect("it starts");
    drop(hosts);

    // What completing the wizard leaves behind: a bundle in that root, put
    // there by the same seeding path the default instance uses.
    let root = dir.path().join("instances/acme");
    let seeded = seed_a_company_into(&root).await;

    let relaunched = relaunch(dir.path()).await;
    let acme = relaunched
        .list()
        .into_iter()
        .find(|instance| instance.id == "acme")
        .expect("still rostered");
    assert_eq!(
        acme.companies, seeded,
        "a created instance must adopt what its root already holds"
    );
}

/// Writes a company into `root` the way completing setup does, and returns
/// its id. Uses a seeding host so the bundle is a real one.
async fn seed_a_company_into(root: &Path) -> Vec<String> {
    take_root(root).await.companies().to_vec()
}

/// Starts a seeding host over `root`, retrying while a released `flock`
/// clears.
///
/// Every take in this module needs this, not just the ones that look like
/// a relaunch. `flock` belongs to the open file description, so a
/// subprocess spawned anywhere in this test binary between `fork` and
/// `exec` holds a copy of the lock — and the suite spawns `git` constantly.
/// A bare `expect` on a root released microseconds earlier therefore fails
/// a few runs in five, and the failure reads as "the roster is broken"
/// rather than "the kernel had not caught up".
async fn take_root(root: &Path) -> EmbeddedHost {
    let mut last = None;
    for _ in 0..200 {
        match embedded::start(root.to_path_buf()).await {
            Ok(host) => return host,
            Err(error) => last = Some(error),
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("a released root must become takeable: {last:?}");
}

#[tokio::test]
async fn two_instances_may_share_a_label() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    let first = hosts.create("Acme").await.expect("it starts");
    let second = hosts.create("Acme").await.expect("it starts");

    assert_eq!(first.id, "acme");
    assert_eq!(second.id, "acme-2");
    assert_ne!(first.data_dir, second.data_dir);
}

#[tokio::test]
async fn forgetting_keeps_the_data_and_refuses_the_default() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    hosts.create("Acme").await.expect("it starts");
    let root = dir.path().join("instances/acme");
    assert!(root.exists());

    assert!(
        hosts.forget(DEFAULT_INSTANCE_ID).is_err(),
        "the root instance is not removable"
    );
    hosts.forget("acme").expect("a known instance");

    assert_eq!(hosts.list().len(), 1);
    assert!(root.exists(), "forgetting is not deleting");
}

#[tokio::test]
async fn deleting_removes_a_created_instance_and_its_data() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    hosts.create("Acme").await.expect("it starts");
    let root = dir.path().join("instances/acme");
    std::fs::write(root.join("company-data"), "valuable").unwrap();

    hosts.delete("acme").await.expect("a created instance");

    assert_eq!(hosts.list().len(), 1);
    assert!(!root.exists(), "delete removes the instance data root");
    assert!(
        hosts.delete(DEFAULT_INSTANCE_ID).await.is_err(),
        "the application data root is never recursively deleted"
    );
}

#[tokio::test]
async fn deleting_keeps_data_when_the_roster_cannot_be_updated() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    hosts.create("Acme").await.expect("it starts");
    let root = dir.path().join("instances/acme");
    std::fs::write(root.join("company-data"), "valuable").unwrap();

    let roster = dir.path().join(ROSTER_FILE);
    let saved_roster = dir.path().join("instances.saved.json");
    std::fs::rename(&roster, &saved_roster).unwrap();
    std::fs::create_dir(&roster).unwrap();

    let error = hosts
        .delete("acme")
        .await
        .expect_err("an unwritable roster must fail deletion");

    assert!(error.contains("could not write the instance roster"));
    assert!(root.join("company-data").exists(), "data stays retryable");
    assert!(hosts.list().iter().any(|instance| instance.id == "acme"));

    std::fs::remove_dir(&roster).unwrap();
    std::fs::rename(&saved_roster, &roster).unwrap();
}

#[tokio::test]
async fn a_failed_atomic_roster_replace_keeps_the_previous_roster() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    hosts.create("Acme").await.expect("it starts");
    let roster = dir.path().join(ROSTER_FILE);
    let before = std::fs::read(&roster).unwrap();
    hosts.instances[1].entry.label = "Changed only in memory".to_string();

    let error = hosts
        .try_persist_with(|| Err(std::io::Error::other("injected before replace")))
        .expect_err("the injected replacement failure must be reported");

    assert!(error.contains("injected before replace"));
    assert_eq!(
        std::fs::read(&roster).unwrap(),
        before,
        "a failed replacement must not truncate or change the live roster"
    );
}

#[tokio::test]
async fn deleting_refuses_a_hand_written_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(ROSTER_FILE),
        r#"{"instances":[{"id":"kept","label":"Kept","root":"instances/kept"}]}"#,
    )
    .unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    let root = dir.path().join("instances/kept");
    assert!(root.exists());

    assert!(hosts.delete("kept").await.is_err());
    assert!(
        root.exists(),
        "delete only owns roots it minted under instances"
    );
}

#[tokio::test]
async fn renaming_keeps_the_root_and_therefore_the_data() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    let created = hosts.create("Acme").await.expect("it starts");

    let renamed = hosts.rename("acme", "Acme Holdings").expect("known");
    assert_eq!(renamed.label, "Acme Holdings");
    assert_eq!(renamed.id, created.id);
    assert_eq!(renamed.data_dir, created.data_dir);
    assert_eq!(renamed.instance_id, created.instance_id);
}

/// A busy root is a row with a reason on it, not a launch that fails.
#[tokio::test]
async fn an_instance_whose_root_is_held_is_reported_rather_than_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    hosts.create("Acme").await.expect("it starts");
    drop(hosts);

    // Something else takes one of the two roots — a second window, or an
    // `opencompany serve` in a terminal. Retried, because a root released a
    // moment ago is not takeable a moment later: see `take_root`.
    let squatter = take_root(&dir.path().join("instances/acme")).await;

    let relaunched = relaunch_until(dir.path(), default_is_running).await;
    let listed = relaunched.list();
    assert_eq!(listed.len(), 2, "every instance still has a row");
    let acme = listed.iter().find(|i| i.id == "acme").unwrap();
    assert!(!acme.running);
    assert!(acme.error.is_some(), "the row says why");
    let default = listed.iter().find(|i| i.id == DEFAULT_INSTANCE_ID).unwrap();
    assert!(default.running, "one busy root must not stop the others");
    drop(squatter);
}

/// A stopped instance still says who it is.
///
/// The console prunes its remembered connections against this list, and
/// `removeConnection` forgets the persisted profile — so an instance that
/// went quiet about its identity while stopped would have its connection id
/// dropped, and with it the tour state, last-read channel and mail draft
/// scoped to that id. Stopping is not forgetting, at either end.
#[tokio::test]
async fn a_stopped_instance_still_reports_its_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    let running = hosts.create("Acme").await.expect("it starts");
    let identity = running.instance_id.clone();
    assert!(identity.is_some());

    let stopped = hosts.stop("acme").expect("a known instance");
    assert!(!stopped.running);
    assert!(stopped.base_url.is_none(), "nothing is listening");
    assert_eq!(
        stopped.instance_id, identity,
        "a stopped instance is the same host, and must still be recognisable as it"
    );
}

/// Duplicate ids are dropped wherever they sit, not only side by side.
///
/// `dedup_by` compares neighbours, so the entry between the two `acme` rows
/// is enough to defeat it. Two rows sharing an id share a root: the second
/// never starts because the first holds the lock, and `index_of` resolves
/// only the first, so `stop`, `rename` and `forget` cannot reach the other.
#[tokio::test]
async fn a_roster_repeating_an_id_out_of_order_keeps_one_row() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(ROSTER_FILE),
        r#"{"instances":[
            {"id":"acme","label":"Acme","root":"instances/acme"},
            {"id":"other","label":"Other","root":"instances/other"},
            {"id":"acme","label":"Acme again","root":"instances/acme"}
        ]}"#,
    )
    .unwrap();

    let hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    let listed = hosts.list();
    let acme: Vec<_> = listed.iter().filter(|i| i.id == "acme").collect();
    assert_eq!(acme.len(), 1, "one row per id: {listed:?}");
    // The first occurrence, so listing order is what the file said.
    assert_eq!(acme[0].label, "Acme");
    assert!(listed.iter().any(|i| i.id == "other"), "{listed:?}");
    assert!(
        listed.iter().all(|i| i.running),
        "no row may be left holding a root another row already took: {listed:?}"
    );
}

/// A hand-edited roster cannot point a host outside the data dir.
#[tokio::test]
async fn a_roster_naming_an_escaping_root_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(ROSTER_FILE),
        r#"{"instances":[{"id":"evil","label":"Evil","root":"../../elsewhere"}]}"#,
    )
    .unwrap();

    let hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    let listed = hosts.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, DEFAULT_INSTANCE_ID);
}

/// An unreadable roster degrades to the install everyone already had.
#[tokio::test]
async fn a_corrupt_roster_falls_back_to_the_default_instance() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(ROSTER_FILE), "{ not json").unwrap();

    let hosts = LocalHosts::load(dir.path().to_path_buf()).await;
    assert_eq!(hosts.list().len(), 1);
    assert!(hosts.default_instance().is_some());
}

/// Loads over `root`, retrying while a just-released `flock` clears.
///
/// See `embedded::test::stopping_a_host_frees_its_root_and_its_port` for
/// why the release is not instantaneous. The condition is passed in
/// because "settled" differs per test: one of these deliberately relaunches
/// into a root something else is holding, where waiting for everything to
/// run would wait forever.
async fn relaunch_until(root: &Path, settled: impl Fn(&[LocalInstanceInfo]) -> bool) -> LocalHosts {
    for _ in 0..200 {
        let hosts = LocalHosts::load(root.to_path_buf()).await;
        if settled(&hosts.list()) {
            return hosts;
        }
        drop(hosts);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("released roots must become takeable");
}

/// The condition for a relaunch where some instance is expected *not* to
/// come back — only the root instance has to have taken its root.
fn default_is_running(listed: &[LocalInstanceInfo]) -> bool {
    listed
        .iter()
        .any(|instance| instance.id == DEFAULT_INSTANCE_ID && instance.running)
}

/// The ordinary relaunch: every rostered instance that wants to run, runs.
async fn relaunch(root: &Path) -> LocalHosts {
    relaunch_until(root, |listed| {
        listed.iter().all(|instance| instance.running)
    })
    .await
}

/// Starts `id`, retrying for the same reason `relaunch` does.
async fn start_when_free(hosts: &mut LocalHosts, id: &str) -> LocalInstanceInfo {
    let mut last = None;
    for _ in 0..200 {
        match hosts.start(id).await {
            Ok(info) => return info,
            Err(error) => last = Some(error),
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("a released root must become takeable: {last:?}");
}
