use std::sync::Arc;

use async_trait::async_trait;

use super::*;
use crate::ports::CompanyStore;
use crate::ports::types::{CompanyId, CompanyRecord};

struct RecordStore(Option<CompanyRecord>);

#[async_trait]
impl CompanyStore for RecordStore {
    async fn load(&self, _id: &CompanyId) -> crate::Result<Option<CompanyRecord>> {
        Ok(self.0.clone())
    }
    async fn save(&self, _record: &CompanyRecord) -> crate::Result<()> {
        unreachable!("resolve only reads")
    }
    async fn list(&self) -> crate::Result<Vec<CompanySummary>> {
        unreachable!("resolve only reads")
    }
    async fn append_ledger(
        &self,
        _id: &CompanyId,
        _entry: crate::ports::types::LedgerEntry,
    ) -> crate::Result<()> {
        unreachable!("resolve only reads")
    }
}

use crate::ports::types::CompanySummary;

fn record_with_group_chat(id: &str, name: &str) -> CompanyRecord {
    let manifest = toml::from_str(&format!(
        r#"
[company]
name = "Acme"

[policy]
mode = "full"

[[agent]]
id = "ceo"
role = "Chief Executive"
description = "Sets direction."

[[group_chat]]
id = "{id}"
name = "{name}"
"#,
    ))
    .expect("valid manifest");
    CompanyRecord {
        overlay_desk_hive: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        overlay_tool_grants: None,
        name_confirmed: false,
        activation_completed_at: None,
        created_at_millis: None,
        id: CompanyId::new("acme"),
        manifest,
        ledger: Vec::new(),
        lifecycle: "running".to_string(),
        setup: None,
        overlay_agents: Vec::new(),
        overlay_desk_members: Vec::new(),
        overlay_desk_order: Vec::new(),
        overlay_desks: Vec::new(),
        overlay_workflows: Vec::new(),
        overlay_budgets: Vec::new(),
        overlay_policy: None,
        overlay_desk_tools: Default::default(),
        disabled_workflows: Vec::new(),
        template_provenance: None,
    }
}

async fn resolve(store: RecordStore, chat_id: Option<&str>) -> (String, String) {
    let store: Arc<dyn CompanyStore> = Arc::new(store);
    resolve_seed_desk(&store, &CompanyId::new("acme"), chat_id).await
}

/// A desk created from the console is a desk.
///
/// It lives in `overlay_desks` and never in the manifest, so a lookup that
/// reads only `group_chats` fell through to the verbatim selector — and
/// every line journaled under the desk's *other* spelling was orphaned from
/// the thread index, `read_thread` and the seed alike (coderabbit + codex
/// on #1972).
#[test]
fn an_overlay_desk_resolves_by_either_spelling() {
    let mut record = record_with_group_chat("growth_desk", "Growth");
    record.overlay_desks.push(crate::ports::types::OverlayDesk {
        id: "ops_desk".to_string(),
        name: "Operations".to_string(),
        description: None,
        members: Vec::new(),
        responder: crate::ports::types::ResponderMode::default(),
        hive: Default::default(),
    });
    for spelling in ["ops_desk", "Operations"] {
        assert_eq!(
            desk_aliases(&record, Some(spelling)),
            ("ops_desk".to_string(), "Operations".to_string()),
            "{spelling:?} is the console-created desk"
        );
    }
}

/// An exact id beats another desk's display name.
///
/// Desk creation enforces unique ids but **not** unique names, so
/// `{id: "ops_desk", name: "sales"}` is valid and can sit ahead of
/// `{id: "sales", …}`. A single pass matching `id == key || name == key`
/// answers with whichever came first, so asking for the desk `sales` got
/// `ops_desk` — and since this returns a *pair*, the damage is worse than a
/// miss: `owns` would be handed one desk's id and another's name, merging
/// two conversations that have nothing to do with each other.
///
/// The precedence itself is `CompanyRecord::resolve_desk_id`'s, which this
/// now defers to rather than keeping a second, laxer copy of.
#[test]
fn an_exact_id_wins_over_an_earlier_desks_display_name() {
    let mut record = record_with_group_chat("ops_desk", "sales");
    record
        .manifest
        .group_chats
        .push(toml::from_str("id = \"sales\"\nname = \"Sales\"").expect("a desk"));
    assert_eq!(
        desk_aliases(&record, Some("sales")).0,
        "sales",
        "the desk whose id is `sales` owns that key"
    );
}

#[tokio::test]
async fn resolve_none_is_the_general_desk() {
    assert_eq!(
        resolve(RecordStore(None), None).await,
        (GENERAL_DESK.to_string(), GENERAL_DESK.to_string())
    );
}

#[tokio::test]
async fn resolve_general_spelling_short_circuits_without_a_store_read() {
    // The store would panic on `save`/`list`, but a General spelling must not
    // even reach `load` — it returns `(chat, chat)`, which owns folds.
    assert_eq!(
        resolve(RecordStore(None), Some("main")).await,
        ("main".to_string(), "main".to_string())
    );
}

#[tokio::test]
async fn resolve_named_desk_by_id_returns_the_manifest_name() {
    // Addressed by id; the seed must carry the name too, or a line journaled
    // under the name would be missed. This is the exact "looks fixed but seeds
    // nothing" trap the resolution guards against.
    let store = RecordStore(Some(record_with_group_chat("eng-123", "Engineering")));
    assert_eq!(
        resolve(store, Some("eng-123")).await,
        ("eng-123".to_string(), "Engineering".to_string())
    );
}

#[tokio::test]
async fn resolve_unmatched_selector_passes_through_verbatim() {
    let store = RecordStore(Some(record_with_group_chat("eng-123", "Engineering")));
    assert_eq!(
        resolve(store, Some("ad-hoc-thread")).await,
        ("ad-hoc-thread".to_string(), "ad-hoc-thread".to_string())
    );
}
