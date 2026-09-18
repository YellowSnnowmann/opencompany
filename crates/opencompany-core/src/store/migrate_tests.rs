use super::*;

/// A scratch home that cleans itself up, named per test so parallel runs
/// never share a tree.
struct TempHome(PathBuf);

impl TempHome {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oc-migrate-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch home");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Creates a bundle directory holding one marker file.
    fn bundle(&self, relative: &str) -> PathBuf {
        let dir = self.0.join(relative);
        std::fs::create_dir_all(&dir).expect("bundle dir");
        std::fs::write(dir.join("company.toml"), "[company]\n").expect("manifest");
        dir
    }

    /// A bundle the way `Bundle::ensure_dirs` creates one: subdirectories
    /// only, no `company.toml` and no `meta.json`. ~20 call sites produce
    /// exactly this, and a sqlite or mongodb install never materializes a
    /// manifest on the filesystem at all.
    fn marker_less_bundle(&self, relative: &str) -> PathBuf {
        let dir = self.0.join(relative);
        for sub in ["memory", "context/blobs", "secrets", "keys"] {
            std::fs::create_dir_all(dir.join(sub)).expect("bundle subdir");
        }
        std::fs::write(dir.join("keys/agent.ed25519"), "seed").expect("identity seed");
        dir
    }

    /// A bare directory with nothing in it.
    fn dir(&self, relative: &str) -> PathBuf {
        let dir = self.0.join(relative);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn write(&self, relative: &str, body: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
        std::fs::write(&path, body).expect("file");
        path
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_missing_nest_is_a_no_op() {
    // The hosted boot, and every local boot after the first.
    let home = TempHome::new("absent");
    home.bundle("companies/acme");

    let migration = migrate_legacy_nest(home.path()).expect("no nest to migrate");

    assert!(migration.is_empty());
    assert!(migration.report().is_empty(), "a no-op says nothing");
    assert!(home.path().join("companies/acme/company.toml").exists());
}

#[test]
fn nested_bundles_move_up_one_level() {
    let home = TempHome::new("move");
    home.bundle("companies/companies/acme");
    home.bundle("companies/companies/globex");

    let migration = migrate_legacy_nest(home.path()).expect("migrates");

    assert!(home.path().join("companies/acme/company.toml").exists());
    assert!(home.path().join("companies/globex/company.toml").exists());
    // The emptied nest is removed, so the layout is genuinely canonical.
    assert!(!home.path().join("companies/companies").exists());
    assert_eq!(migration.moved.len(), 2);
    assert!(migration.collisions.is_empty());
    assert_eq!(migration.report().len(), 2);
}

#[test]
fn re_running_is_silent() {
    let home = TempHome::new("idempotent");
    home.bundle("companies/companies/acme");

    migrate_legacy_nest(home.path()).expect("first run migrates");
    let second = migrate_legacy_nest(home.path()).expect("second run is a no-op");

    assert!(second.is_empty(), "{second:?}");
    assert!(home.path().join("companies/acme/company.toml").exists());
}

#[test]
fn a_collision_keeps_both_copies_and_names_both_paths() {
    // The user who ran the app both ways. Two event logs and two signing
    // keys cannot be merged, so neither copy may be touched.
    let home = TempHome::new("collision");
    home.bundle("companies/acme");
    home.write("companies/acme/events.jsonl", "canonical\n");
    home.bundle("companies/companies/acme");
    home.write("companies/companies/acme/events.jsonl", "legacy\n");

    let migration = migrate_legacy_nest(home.path()).expect("migrates");

    assert!(migration.moved.is_empty());
    assert_eq!(migration.collisions.len(), 1);
    assert_eq!(migration.collisions[0].what, Relocated::Company);
    // Both copies survive with their own event logs.
    assert_eq!(
        std::fs::read_to_string(home.path().join("companies/acme/events.jsonl")).unwrap(),
        "canonical\n"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("companies/companies/acme/events.jsonl")).unwrap(),
        "legacy\n"
    );
    // The nest survives too, since it is not empty.
    assert!(home.path().join("companies/companies").is_dir());
    // Both paths are named, so the operator can act without guessing.
    let report = migration.report().join("\n");
    assert!(
        report.contains(&home.path().join("companies/acme").display().to_string()),
        "{report}"
    );
    assert!(
        report.contains(
            &home
                .path()
                .join("companies/companies/acme")
                .display()
                .to_string()
        ),
        "{report}"
    );
}

#[test]
fn a_collision_does_not_block_the_other_bundles() {
    let home = TempHome::new("partial");
    home.bundle("companies/acme");
    home.bundle("companies/companies/acme");
    home.bundle("companies/companies/globex");

    let migration = migrate_legacy_nest(home.path()).expect("migrates");

    assert_eq!(migration.moved.len(), 1);
    assert_eq!(migration.collisions.len(), 1);
    assert!(home.path().join("companies/globex/company.toml").exists());
    assert!(home.path().join("companies/companies/acme").is_dir());
}

#[test]
fn a_company_genuinely_slugged_companies_is_untouched() {
    // `<home>/companies/companies` holding a manifest is a bundle, not a
    // nest. This guard is also what proves a hosted tenant is never
    // dissolved.
    let home = TempHome::new("real-slug");
    home.bundle("companies/companies");
    home.write("companies/companies/events.jsonl", "real bundle\n");

    let migration = migrate_legacy_nest(home.path()).expect("leaves the bundle alone");

    assert!(migration.is_empty());
    assert!(
        home.path()
            .join("companies/companies/company.toml")
            .exists()
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("companies/companies/events.jsonl")).unwrap(),
        "real bundle\n"
    );
}

#[test]
fn a_meta_json_alone_also_marks_a_real_bundle() {
    // A bundle whose manifest is not materialized yet still has `meta.json`;
    // that is not a nest either.
    let home = TempHome::new("meta-marker");
    home.write("companies/companies/meta.json", "{}\n");

    let migration = migrate_legacy_nest(home.path()).expect("leaves the bundle alone");

    assert!(migration.is_empty());
    assert!(home.path().join("companies/companies/meta.json").exists());
}

#[test]
fn a_marker_less_bundle_slugged_companies_is_not_dissolved() {
    // The shape every `Bundle::ensure_dirs` call site produces, and the only
    // shape a sqlite or mongodb install ever has on the filesystem: no
    // manifest, no `meta.json`, and a signing key that cannot be re-derived.
    // Reading it as a nest would rename `keys/`, `secrets/` and
    // `tasks.json` up into `<home>/companies/` and orphan the identity —
    // which is precisely the hosted case, since hosted is the non-fs store.
    let home = TempHome::new("marker-less-slug");
    home.marker_less_bundle("companies/companies");
    home.write("companies/companies/tasks.json", "[]");

    let migration = migrate_legacy_nest(home.path()).expect("leaves the bundle alone");

    assert!(migration.is_empty(), "{migration:?}");
    assert!(
        home.path()
            .join("companies/companies/keys/agent.ed25519")
            .exists()
    );
    assert!(home.path().join("companies/companies/tasks.json").exists());
    // Nothing was scattered up into the companies directory.
    assert!(!home.path().join("companies/keys").exists());
    assert!(!home.path().join("companies/secrets").exists());
    assert!(!home.path().join("companies/tasks.json").exists());
}

#[test]
fn a_marker_less_nested_bundle_still_moves() {
    // The widened guard must not cost the migration its purpose: a bundle
    // with no manifest is exactly what a sqlite install has, and it is
    // still nested one level too deep.
    let home = TempHome::new("marker-less-nest");
    home.marker_less_bundle("companies/companies/acme");

    let migration = migrate_legacy_nest(home.path()).expect("migrates");

    assert_eq!(migration.moved.len(), 1, "{migration:?}");
    assert!(
        home.path()
            .join("companies/acme/keys/agent.ed25519")
            .exists()
    );
    assert!(!home.path().join("companies/companies").exists());
}

#[test]
fn an_unrecognised_nest_entry_is_left_where_it_is() {
    // Only bundle-shaped directories move. Anything else is either a stray
    // or evidence the nest is a bundle after all, and either way this
    // migration has nowhere to put it.
    let home = TempHome::new("stray");
    home.bundle("companies/companies/acme");
    home.write("companies/companies/notes.txt", "stray\n");
    home.dir("companies/companies/empty");

    let migration = migrate_legacy_nest(home.path()).expect("migrates");

    assert_eq!(migration.moved.len(), 1, "{migration:?}");
    assert!(home.path().join("companies/acme/company.toml").exists());
    assert!(home.path().join("companies/companies/notes.txt").exists());
    assert!(!home.path().join("companies/notes.txt").exists());
    // The nest survives because it is not empty — and the next boot, having
    // nothing left it recognises, says nothing at all.
    assert!(home.path().join("companies/companies").is_dir());
    assert!(migrate_legacy_nest(home.path()).expect("rerun").is_empty());
}

#[test]
fn the_harness_and_mcp_trees_move_up_with_the_bundles() {
    // Both hang off the resolved home rather than off a bundle, so the same
    // one-level shift that orphaned the database orphans every agent's
    // working files and the operator's installed MCP servers — including
    // the environment values stored with them.
    let home = TempHome::new("home-dirs");
    home.bundle("companies/companies/acme");
    home.write("companies/harness/acme/ceo/workspace/notes.md", "draft\n");
    home.write("companies/mcp/registry.db", "installs");

    let migration = migrate_legacy_nest(home.path()).expect("migrates");

    assert_eq!(
        std::fs::read_to_string(home.path().join("harness/acme/ceo/workspace/notes.md")).unwrap(),
        "draft\n"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("mcp/registry.db")).unwrap(),
        "installs"
    );
    assert!(!home.path().join("companies/harness").exists());
    assert!(!home.path().join("companies/mcp").exists());
    let moved: Vec<Relocated> = migration.moved.iter().map(|(what, _)| *what).collect();
    assert!(
        moved.contains(&Relocated::HarnessWorkspace),
        "{migration:?}"
    );
    assert!(moved.contains(&Relocated::McpRegistry), "{migration:?}");
    // The report names them for what they are, not as company bundles.
    let report = migration.report().join("\n");
    assert!(report.contains("harness agent workspace"), "{report}");
    assert!(report.contains("MCP server registry"), "{report}");
}

#[test]
fn a_company_slugged_like_the_harness_tree_is_never_moved_as_one() {
    // `<home>/companies/harness` is both where the legacy harness tree sat
    // and where a company slugged `harness` has its canonical bundle. Only
    // one of the two is bundle-shaped.
    let home = TempHome::new("harness-company");
    home.marker_less_bundle("companies/harness");

    let migration = migrate_legacy_nest(home.path()).expect("leaves the bundle alone");

    assert!(migration.is_empty(), "{migration:?}");
    assert!(
        home.path()
            .join("companies/harness/keys/agent.ed25519")
            .exists()
    );
    assert!(
        !home.path().join("harness").exists(),
        "the bundle must not be relocated as a runtime tree"
    );
}

#[test]
fn an_mcp_registry_at_the_destination_is_left_for_the_operator() {
    // Two registries hold two sets of installed servers. Merging them is
    // not a decision this migration can make.
    let home = TempHome::new("mcp-collision");
    home.write("companies/mcp/registry.db", "legacy");
    home.write("mcp/registry.db", "canonical");

    let migration = migrate_legacy_nest(home.path()).expect("migrates");

    assert!(migration.moved.is_empty(), "{migration:?}");
    assert_eq!(migration.collisions.len(), 1);
    assert_eq!(migration.collisions[0].what, Relocated::McpRegistry);
    assert_eq!(
        std::fs::read_to_string(home.path().join("mcp/registry.db")).unwrap(),
        "canonical"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("companies/mcp/registry.db")).unwrap(),
        "legacy"
    );
}

#[test]
fn a_file_move_never_replaces_the_destination() {
    // The property the whole database set rests on. `rename` would have
    // replaced the destination silently, and no check taken beforehand can
    // close that window — only a move that cannot replace anything can.
    let home = TempHome::new("no-replace");
    let from = home.write("companies/opencompany.db", "legacy");
    let to = home.write("opencompany.db", "live");

    assert_eq!(
        move_file_no_replace(&from, &to).expect("reports rather than replaces"),
        Moved::Occupied
    );

    assert_eq!(std::fs::read_to_string(&to).unwrap(), "live");
    assert_eq!(std::fs::read_to_string(&from).unwrap(), "legacy");
}

#[test]
fn a_half_linked_database_is_finished_rather_than_reported() {
    // The crash window inside a no-replace move: the link is in place and
    // the source is not yet unlinked, so one file is reachable under both
    // names. Reading that as two databases would send the operator to
    // resolve a collision between a file and itself.
    let home = TempHome::new("half-linked");
    let legacy = home.write("companies/opencompany.db", "one database");
    std::fs::hard_link(&legacy, home.path().join("opencompany.db")).expect("half a move");

    let migration = migrate_legacy_nest(home.path()).expect("finishes the move");

    assert!(migration.collisions.is_empty(), "{migration:?}");
    assert!(!legacy.exists(), "the source link is unlinked");
    assert_eq!(
        std::fs::read_to_string(home.path().join("opencompany.db")).unwrap(),
        "one database"
    );
}

#[test]
fn an_orphaned_sqlite_database_moves_with_its_siblings() {
    // `serve` passed the resolved home to `open_storage`, so the legacy
    // default's database landed inside the bundle home.
    let home = TempHome::new("sqlite");
    home.bundle("companies/companies/acme");
    home.write("companies/opencompany.db", "db");
    home.write("companies/opencompany.db-wal", "wal");
    home.write("companies/opencompany.db-shm", "shm");

    let migration = migrate_legacy_nest(home.path()).expect("migrates");

    for name in LEGACY_SQLITE_FILES {
        assert!(home.path().join(name).exists(), "{name} moved up");
        assert!(
            !home.path().join("companies").join(name).exists(),
            "{name} left behind"
        );
    }
    assert_eq!(
        migration
            .moved
            .iter()
            .filter(|(what, _)| *what == Relocated::Database)
            .count(),
        3
    );
}

#[test]
fn a_database_without_a_write_ahead_log_still_moves() {
    let home = TempHome::new("sqlite-clean");
    home.write("companies/opencompany.db", "db");

    migrate_legacy_nest(home.path()).expect("migrates");

    assert!(home.path().join("opencompany.db").exists());
    assert!(!home.path().join("companies/opencompany.db").exists());
}

#[test]
fn a_database_at_the_destination_is_never_overwritten() {
    // Two databases means two histories. Overwriting one destroys an
    // install, so both are kept and the operator is told.
    let home = TempHome::new("sqlite-collision");
    home.write("companies/opencompany.db", "legacy");
    home.write("opencompany.db", "canonical");

    let migration = migrate_legacy_nest(home.path()).expect("migrates");

    assert!(migration.moved.is_empty());
    assert_eq!(migration.collisions.len(), 1);
    assert_eq!(migration.collisions[0].what, Relocated::Database);
    assert_eq!(
        std::fs::read_to_string(home.path().join("opencompany.db")).unwrap(),
        "canonical"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("companies/opencompany.db")).unwrap(),
        "legacy"
    );
}

#[test]
fn a_wal_at_the_destination_blocks_the_whole_database_move() {
    // Half a move pairs a moved database with a stranded log, so a single
    // occupied sibling stops the set.
    let home = TempHome::new("wal-collision");
    home.write("companies/opencompany.db", "legacy");
    home.write("companies/opencompany.db-wal", "legacy wal");
    home.write("opencompany.db-wal", "canonical wal");

    let migration = migrate_legacy_nest(home.path()).expect("migrates");

    assert!(migration.moved.is_empty());
    assert_eq!(migration.collisions.len(), 1);
    assert!(!home.path().join("opencompany.db").exists());
    assert!(home.path().join("companies/opencompany.db").exists());
}

#[test]
fn a_company_slugged_like_the_database_is_never_moved_as_one() {
    // `slug` permits `.`, so a company really can be called
    // `opencompany.db` — and its canonical bundle is at exactly the path the
    // legacy database would occupy. Moving that directory up would delete the
    // company from every bundle lookup and hand sqlite a directory to open.
    let home = TempHome::new("db-named-company");
    home.bundle("companies/opencompany.db");
    home.write("companies/opencompany.db/events.jsonl", "a real company\n");

    let migration = migrate_legacy_nest(home.path()).expect("leaves the bundle alone");

    assert!(migration.is_empty(), "{migration:?}");
    assert!(
        home.path()
            .join("companies/opencompany.db/company.toml")
            .exists()
    );
    assert!(
        !home.path().join("opencompany.db").exists(),
        "the bundle must not be relocated as a database"
    );
}

#[test]
fn a_half_moved_database_set_is_finished_on_the_next_run() {
    // A run that moved `opencompany.db` and then died leaves the sidecars
    // behind. Keying detection off the database alone would call that
    // finished, pairing a relocated database with a stranded write-ahead log
    // — losing whatever the log still held.
    let home = TempHome::new("half-moved-db");
    home.write("opencompany.db", "already moved");
    home.write("companies/opencompany.db-wal", "stranded wal");
    home.write("companies/opencompany.db-shm", "stranded shm");

    let migration = migrate_legacy_nest(home.path()).expect("resumes");

    assert_eq!(migration.moved.len(), 2, "{migration:?}");
    assert!(migration.collisions.is_empty(), "{migration:?}");
    assert_eq!(
        std::fs::read_to_string(home.path().join("opencompany.db-wal")).unwrap(),
        "stranded wal"
    );
    assert!(home.path().join("opencompany.db-shm").exists());
    assert!(!home.path().join("companies/opencompany.db-wal").exists());
    // The already-moved database is untouched.
    assert_eq!(
        std::fs::read_to_string(home.path().join("opencompany.db")).unwrap(),
        "already moved"
    );
}

#[test]
fn a_source_another_process_already_moved_is_not_a_failure() {
    // `serve` booting and a hand-run `export` against one home both migrate.
    // Losing that race must not abort the loser with a NotFound that means
    // "already done".
    let home = TempHome::new("raced");
    let destination = home.write("companies/acme/company.toml", "[company]\n");
    let vanished = home.path().join("companies/companies/acme/company.toml");

    assert!(
        !rename_or_already_moved(&vanished, &destination).expect("tolerated"),
        "a vanished source whose destination now exists did not move here",
    );

    // A NotFound with the destination *also* absent is a real failure and
    // still propagates, so this tolerance cannot mask a broken migration.
    let nowhere = home.path().join("companies/companies/globex/company.toml");
    assert!(rename_or_already_moved(&vanished, &nowhere).is_err());
}

/// The test above proves the *tolerance* — one call against a filesystem
/// already left half-moved by some other run. It does not prove the
/// *race itself* is safe, only its aftermath staged by hand. This drives
/// two real threads into [`migrate_legacy_nest`] over the same home at
/// once, synchronized with a barrier so both reach the rename at
/// (approximately) the same instant — `serve` booting while a hand-run
/// `export` migrates the same install, the scenario the module doc
/// names directly.
#[test]
fn two_concurrent_migrations_of_the_same_home_never_lose_or_duplicate_a_bundle() {
    // The barrier releases both threads before `migrate_legacy_nest`, which
    // scans before it renames — so a schedule where the winner finishes
    // before the loser scans never reaches the tolerated `NotFound` at all.
    // Repeating the race makes that interleaving near-certain rather than
    // lucky; every attempt asserts the same invariants, so a regression
    // fails on whichever attempt exposes it.
    for attempt in 0..24 {
        let home = TempHome::new(&format!("concurrent-race-{attempt}"));
        home.write(
            "companies/companies/acme/company.toml",
            "[company]\nname = \"Acme\"\n",
        );

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let path_a = home.path().to_path_buf();
        let barrier_a = barrier.clone();
        let handle_a = std::thread::spawn(move || {
            barrier_a.wait();
            migrate_legacy_nest(&path_a)
        });
        let path_b = home.path().to_path_buf();
        let barrier_b = barrier.clone();
        let handle_b = std::thread::spawn(move || {
            barrier_b.wait();
            migrate_legacy_nest(&path_b)
        });

        let result_a = handle_a.join().expect("thread a did not panic");
        let result_b = handle_b.join().expect("thread b did not panic");

        let migration_a =
            result_a.expect("the losing thread must not abort with the winner's NotFound");
        let migration_b =
            result_b.expect("the losing thread must not abort with the winner's NotFound");

        assert!(migration_a.collisions.is_empty(), "{migration_a:?}");
        assert!(migration_b.collisions.is_empty(), "{migration_b:?}");
        assert_eq!(
            migration_a.moved.len() + migration_b.moved.len(),
            1,
            "exactly one of the two racing calls may claim the move — the other \
                 must see it already done: a={migration_a:?} b={migration_b:?}"
        );

        assert_eq!(
            std::fs::read_to_string(home.path().join("companies/acme/company.toml")).unwrap(),
            "[company]\nname = \"Acme\"\n",
            "the bundle must land intact exactly once, never merged or truncated \
                 by the two renames overlapping"
        );
        assert!(
            !home.path().join("companies/companies/acme").exists(),
            "the loser must not have left a stale copy behind at the legacy path"
        );
    }
}

/// The sibling above races two whole migrations, so the interleaving it
/// needs is probable rather than certain: a schedule where the winner
/// finishes before the loser scans passes without ever reaching the
/// tolerated `NotFound`. This one barriers at the rename itself, leaving
/// nothing between release and syscall, so the contended path is the only
/// path it can take.
#[test]
fn two_threads_renaming_one_bundle_split_into_exactly_one_mover_and_one_no_op() {
    for attempt in 0..8 {
        let home = TempHome::new(&format!("rename-contention-{attempt}"));
        home.write(
            "companies/companies/acme/company.toml",
            "[company]\nname = \"Acme\"\n",
        );
        let legacy = home.path().join("companies/companies/acme");
        let destination = home.path().join("companies/acme");

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let race = |barrier: std::sync::Arc<std::sync::Barrier>| {
            let from = legacy.clone();
            let to = destination.clone();
            std::thread::spawn(move || {
                barrier.wait();
                rename_or_already_moved(&from, &to)
            })
        };
        let handle_a = race(barrier.clone());
        let handle_b = race(barrier.clone());

        let moved_a = handle_a
            .join()
            .expect("thread a did not panic")
            .expect("the losing thread must not surface the winner's NotFound as an error");
        let moved_b = handle_b
            .join()
            .expect("thread b did not panic")
            .expect("the losing thread must not surface the winner's NotFound as an error");

        assert_eq!(
            usize::from(moved_a) + usize::from(moved_b),
            1,
            "exactly one thread may claim the rename: a={moved_a} b={moved_b}"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("company.toml")).unwrap(),
            "[company]\nname = \"Acme\"\n",
            "the bundle must arrive intact, not merged by two overlapping renames"
        );
        assert!(!legacy.exists(), "nothing may remain at the legacy path");
    }
}

#[test]
fn an_empty_home_migrates_nothing() {
    let home = TempHome::new("empty");

    let migration = migrate_legacy_nest(home.path()).expect("nothing to do");

    assert!(migration.is_empty());
}
