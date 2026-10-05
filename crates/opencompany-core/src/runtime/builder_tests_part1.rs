use super::tests_core::*;

/// A manifest upgrade that widens the allow-list into a BYO namespace must
/// not hand billing to persisted teammates whose grant was left unstated
/// (#788). Their pre-upgrade scope is frozen into an explicit line instead.
#[test]
fn an_upgrade_into_chargebee_preserves_the_pre_upgrade_scope_of_empty_lines() {
    let old: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n\
         [tools]\n\
         allow = [\"*\", \"workspace.*\", \"workspace.write\", \"media\", \"composio\", \
         \"search\", \"mcp:*\"]\n",
    )
    .expect("old manifest");
    let new: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n\
         [tools]\n\
         allow = [\"*\", \"workspace.*\", \"workspace.write\", \"media\", \"composio\", \
         \"search\", \"mcp:*\", \"chargebee\"]\n",
    )
    .expect("new manifest");
    let overlay_agents = vec![
        OverlayAgent {
            provider: None,
            id: "clerk".to_string(),
            name: "Clerk".to_string(),
            role: "Data Entry".to_string(),
            description: None,
            // `None` = inherit (tracks the allow-list) — the state that freezes.
            tools: None,
            skills: None,
            model: None,
            harness: None,
        },
        OverlayAgent {
            provider: None,
            id: "finance_help".to_string(),
            name: "Finance Help".to_string(),
            role: "Assistant".to_string(),
            description: None,
            tools: Some(vec!["docs.*".to_string()]),
            skills: None,
            model: None,
            harness: None,
        },
    ];

    let (migrated, _) = RuntimeBuilder::preserve_pre_upgrade_grant_scope(
        overlay_agents,
        Vec::new(),
        Some(&old),
        &new,
    );

    assert_eq!(
        migrated[0].tools,
        Some(old.tools.allow.clone()),
        "an absent (inherit) line is frozen to its pre-upgrade scope rather than silently inheriting chargebee"
    );
    assert_eq!(
        migrated[1].tools,
        Some(vec!["docs.*".to_string()]),
        "a stated grant is untouched"
    );
}

/// The console's per-agent edit half carries the same inherit-freeze rule.
/// Since #1804 `AgentOverride.tools` is a double-option: `Some(None)` is the
/// "reset this teammate to the company's standard grant" spelling (the
/// inherit state), so an upgrade into a BYO namespace must freeze it to the
/// previous allow-list as well — otherwise the override, copied across the
/// rebuild verbatim, replaces the new manifest's explicit non-billing
/// `tools` line with the widened list. `Some(Some([]))` (deny-all) and
/// `Some(Some(globs))` (narrow) state their own scope and are left untouched.
#[test]
fn an_upgrade_into_chargebee_freezes_an_empty_agent_override_scope() {
    let old: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n\
         [tools]\n\
         allow = [\"*\", \"workspace.*\", \"workspace.write\", \"media\", \"composio\", \
         \"search\", \"mcp:*\"]\n",
    )
    .expect("old manifest");
    let new: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n\
         [tools]\n\
         allow = [\"*\", \"workspace.*\", \"workspace.write\", \"media\", \"composio\", \
         \"search\", \"mcp:*\", \"chargebee\"]\n",
    )
    .expect("new manifest");
    let edits = vec![
        AgentOverride {
            agent_id: "tax_preparer".to_string(),
            // The stored spelling of "reset to the company's standard grant"
            // since #1804 — `Some(None)`, the inherit state. (An empty list
            // is `Some(Some(vec![]))`, a deny-all, and is NOT frozen.)
            tools: Some(None),
            ..Default::default()
        },
        AgentOverride {
            agent_id: "brand_strategist".to_string(),
            tools: Some(Some(vec!["docs.*".to_string()])),
            ..Default::default()
        },
    ];

    let (_, migrated) =
        RuntimeBuilder::preserve_pre_upgrade_grant_scope(Vec::new(), edits, Some(&old), &new);

    assert_eq!(
        migrated[0].tools,
        Some(Some(old.tools.allow.clone())),
        "an inherit override (Some(None)) is frozen to its pre-upgrade scope rather than silently inheriting chargebee"
    );
    assert_eq!(
        migrated[1].tools,
        Some(Some(vec!["docs.*".to_string()])),
        "a stated override grant is untouched"
    );
}

/// When the upgrade does not newly confer a BYO namespace, empty lines keep
/// tracking the allow-list as they always have.
#[test]
fn an_upgrade_without_a_new_billing_namespace_leaves_empty_lines_tracking() {
    let old: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n\
         [tools]\n\
         allow = [\"*\", \"workspace.*\", \"media\"]\n",
    )
    .expect("old manifest");
    let new: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n\
         [tools]\n\
         allow = [\"*\", \"workspace.*\", \"media\", \"search\"]\n",
    )
    .expect("new manifest");
    let overlay_agents = vec![OverlayAgent {
        provider: None,
        id: "clerk".to_string(),
        name: "Clerk".to_string(),
        role: "Data Entry".to_string(),
        description: None,
        // `None` = inherit / tracks the allow-list.
        tools: None,
        skills: None,
        model: None,
        harness: None,
    }];

    let (migrated, _) = RuntimeBuilder::preserve_pre_upgrade_grant_scope(
        overlay_agents,
        Vec::new(),
        Some(&old),
        &new,
    );

    assert!(
        migrated[0].tools.is_none(),
        "no BYO namespace was newly conferred, so the inherit line keeps tracking (stays None)"
    );
}

/// Automatic Git checkpoints are opt-in and stay off unless the operator
/// flips the switch. The default is asserted here so a silent change to the
/// host default — which would start shelling out to `git` in every agent
/// workspace — cannot slip past.
#[test]
fn workspace_git_checkpoints_default_off_and_switchable() {
    let home = tmp_home("opencompany-workspace-git-");
    let manifest: CompanyManifest =
        toml::from_str("[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n")
            .expect("manifest");
    let builder = RuntimeBuilder::new(home.path().to_path_buf(), manifest);
    assert!(
        !builder.workspace_git_enabled,
        "workspace Git checkpoints must default to off"
    );
    let enabled = builder.with_workspace_git_enabled(true);
    assert!(enabled.workspace_git_enabled);
    assert!(
        !enabled
            .with_workspace_git_enabled(false)
            .workspace_git_enabled,
        "the switch must also be able to turn checkpoints back off"
    );
}

/// Issue #1781 review (Codex P1): `register_company`'s `serve` boot loop
/// always loads through `CompanyManifest::from_path_for_reload`, which
/// grandfathers a `RESERVED_AGENT_IDS` collision so an already-running
/// company survives a reservation rule that tightened after its
/// `company.toml` was written (`b80c45e2c`, `76c6cacdf`). That relaxation
/// was applied unconditionally, so a manifest hand-authored *after*
/// `operator` became reserved — one this store has never seen — booted
/// exactly as quietly as a genuine legacy one. `build()` now refuses this
/// case: `existing.is_none()` (no persisted record for this id) plus a
/// manifest that only clears the relaxed loader, never the strict one, is
/// not a restart to grandfather — it is a fresh authoring mistake.
#[tokio::test]
async fn first_boot_refuses_a_fresh_manifest_claiming_the_reserved_operator_agent_id() {
    let home = tmp_home("opencompany-first-boot-reserved-id-");
    let manifest: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n\n[[agent]]\nid = \"operator\"\nrole = \"Chief of Staff\"\n",
    )
    .expect("manifest");

    let err = RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .build()
        .await
        .expect_err("a first-ever boot must not grandfather a brand new `operator` agent");
    match err {
        crate::OpenCompanyError::ManifestInvalid { problems, .. } => {
            assert!(
                problems.iter().any(|p| p.contains("operator")),
                "expected a reserved-id problem, got: {problems:?}"
            );
        }
        other => panic!("expected ManifestInvalid, got {other}"),
    }
}

/// The twin of the test above, proving the fix does not regress
/// `b80c45e2c`'s grandfather case: a company whose **stored** record
/// already carries the reserved `operator` agent id — a collision that
/// predates the reservation rule, not one an operator just introduced —
/// must still boot.
///
/// Seeded by writing the `CompanyRecord` straight to the store rather
/// than via a first `build()`, because `build()` itself enforces the
/// strict, unrelaxed `validate()` whenever `existing.is_none()`
/// (`861a8fbad`) — a bare first boot with this manifest would already be
/// refused by `first_boot_refuses_a_fresh_manifest_claiming_the_reserved_operator_agent_id`
/// above, never reaching the grandfather case this test means to prove.
/// Writing the record directly is exactly how a real grandfathered
/// company got here in production: its `company.toml` was accepted, and
/// its record written, before the rule existed at all.
#[tokio::test]
async fn a_reboot_still_grandfathers_an_already_registered_operator_agent_id() {
    let home = tmp_home("opencompany-reboot-reserved-id-");
    let reserved: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\n\
         [[agent]]\nid = \"operator\"\nrole = \"Chief of Staff\"\n",
    )
    .expect("manifest");
    let id = company_id_from_name("Acme");
    FsCompanyStore::new(home.path())
        .save(&CompanyRecord {
            general_channel: Default::default(),
            id: id.clone(),
            manifest: reserved.clone(),
            ledger: Vec::new(),
            lifecycle: "running".to_string(),
            overlay_agents: Vec::new(),
            overlay_desk_members: Vec::new(),
            overlay_desk_order: Vec::new(),
            overlay_desks: Vec::new(),
            overlay_workflows: Vec::new(),
            overlay_budgets: Vec::new(),
            overlay_policy: None,
            overlay_tool_grants: None,
            overlay_desk_tools: Default::default(),
            overlay_retired_agents: Vec::new(),
            overlay_agent_edits: Vec::new(),
            overlay_desk_hive: Vec::new(),
            disabled_workflows: Vec::new(),
            template_provenance: None,
            setup: None,
            name_confirmed: false,
            activation_completed_at: None,
            created_at_millis: None,
        })
        .await
        .unwrap();

    RuntimeBuilder::new(home.path().to_path_buf(), reserved)
        .with_id(id)
        .build()
        .await
        .expect(
            "a company whose STORED record already carries this collision must still \
             reboot, even though the manifest being loaded only clears the relaxed loader",
        );
}

/// The twin of the test above from the other direction: a reboot whose
/// manifest *newly* adds a reserved-id agent — one the stored record does
/// not carry — must be refused exactly as a first boot would be. This is
/// the vulnerability Codex flagged on #1781: `existing.is_some()` used to
/// be the entire test, so an operator could edit `company.toml` between
/// two restarts to mint `operator` (or `system`/`main`/`general`) and the
/// very next `serve` boot excused it as if it had always been there.
#[tokio::test]
async fn a_reboot_refuses_a_newly_introduced_reserved_agent_id() {
    let home = tmp_home("opencompany-reboot-new-reserved-id-");
    let safe: CompanyManifest =
        toml::from_str("[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n")
            .expect("manifest");
    RuntimeBuilder::new(home.path().to_path_buf(), safe)
        .build()
        .await
        .expect("the first boot with a safe manifest must succeed and persist a record");

    let reserved: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\n\
         [[agent]]\nid = \"operator\"\nrole = \"Chief of Staff\"\n",
    )
    .expect("manifest");
    let err = RuntimeBuilder::new(home.path().to_path_buf(), reserved)
        .build()
        .await
        .expect_err(
            "editing company.toml to add a reserved id between two restarts must not be \
             excused just because a record already existed",
        );
    match err {
        crate::OpenCompanyError::ManifestInvalid { problems, .. } => {
            assert!(
                problems.iter().any(|p| p.contains("operator")),
                "expected a reserved-id problem, got: {problems:?}"
            );
        }
        other => panic!("expected ManifestInvalid, got {other}"),
    }
}
