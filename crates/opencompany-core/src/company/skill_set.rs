//! A company's skill set, assembled from its layers in one place.
//!
//! [`skill_effective::resolve`] folds whatever layers it is handed. This module
//! is where a company's layers are named — the global baseline, the bundle
//! under its source directory, the host's shared library — and where its
//! deltas are read, so every reader hands the fold the same inputs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::company::SkillDoc;
use crate::company::runtime::CompanyRuntime;
use crate::company::skill_effective::{self, EffectiveSkill, SkillLayers, globals_skill_disables};
use crate::error::Result;
use crate::ports::CompanyId;
use crate::ports::skills_state::{SkillState, SkillStateStore};

/// Whose skill set is being loaded: the key the deltas are stored under.
#[derive(Clone, Copy, Debug)]
pub struct SkillOwner<'a> {
    /// The company the deltas belong to.
    pub company: &'a CompanyId,
}

/// A loaded skill set: the deltas it was folded from, the fold, and the
/// library snapshot it was healed against.
#[derive(Clone, Debug)]
pub struct SkillSet {
    /// The stored deltas plus the disables `[globals].disable` synthesizes.
    pub deltas: Vec<SkillState>,
    /// The effective set, disabled entries included, ordered by slug.
    pub effective: Vec<EffectiveSkill>,
    /// The shared library the set was resolved against.
    pub library: Arc<[SkillDoc]>,
}

/// The deltas a skill set folds: the owner's stored rows, then a disabling
/// delta for every `skill:` entry in `globals_disable`.
///
/// `store` is `None` for a host wired to no delta store, which then folds only
/// the synthesized disables.
pub async fn load_skill_deltas(
    store: Option<&dyn SkillStateStore>,
    owner: SkillOwner<'_>,
    globals_disable: &[String],
) -> Result<Vec<SkillState>> {
    let mut deltas = match store {
        Some(store) => store.list(owner.company).await?,
        None => Vec::new(),
    };
    deltas.extend(globals_skill_disables(globals_disable));
    Ok(deltas)
}

/// Loads an owner's deltas and resolves its effective set over the global
/// baseline, the bundle under `bundle_root`, and `library`.
pub async fn load_skill_set(
    store: &dyn SkillStateStore,
    owner: SkillOwner<'_>,
    globals_disable: &[String],
    bundle_root: Option<&Path>,
    library: Arc<[SkillDoc]>,
) -> Result<SkillSet> {
    let deltas = load_skill_deltas(Some(store), owner, globals_disable).await?;
    let effective = skill_effective::resolve(&company_layers(bundle_root, &library), &deltas)?;
    Ok(SkillSet {
        deltas,
        effective,
        library,
    })
}

/// [`load_skill_set`] for a running company: its store, its persisted
/// `[globals].disable`, and its source directory's bundle.
pub async fn load_runtime_skill_set(
    runtime: &CompanyRuntime,
    library: Arc<[SkillDoc]>,
) -> Result<SkillSet> {
    let root = bundle_root(runtime.source_dir());
    load_skill_set(
        runtime.skills().as_ref(),
        SkillOwner {
            company: runtime.id(),
        },
        &runtime.globals_disable().await?,
        root.as_deref(),
        library,
    )
    .await
}

/// The directory a company's committed skill bundles live in:
/// `<source_dir>/skills`, or `None` for a company with no source directory.
pub fn bundle_root(source_dir: Option<&Path>) -> Option<PathBuf> {
    source_dir.map(|dir| dir.join("skills"))
}

/// A company's effective skill set: the global baseline, the bundle under
/// `source_dir`, and `library`, folded with `deltas`.
pub fn resolve_company(
    source_dir: Option<&Path>,
    library: &[SkillDoc],
    deltas: &[SkillState],
) -> Result<Vec<EffectiveSkill>> {
    let root = bundle_root(source_dir);
    skill_effective::resolve(&company_layers(root.as_deref(), library), deltas)
}

/// One agent's slice of [`resolve_company`], narrowed by its scope.
pub fn resolve_company_for_agent(
    source_dir: Option<&Path>,
    library: &[SkillDoc],
    deltas: &[SkillState],
    agent: &str,
    agent_skills: Option<&[String]>,
) -> Result<Vec<EffectiveSkill>> {
    let root = bundle_root(source_dir);
    skill_effective::resolve_for_agent(
        &company_layers(root.as_deref(), library),
        deltas,
        agent,
        agent_skills,
    )
}

fn company_layers<'a>(bundle_root: Option<&'a Path>, library: &'a [SkillDoc]) -> SkillLayers<'a> {
    SkillLayers {
        baseline: crate::globals::skills(),
        bundle_root,
        library,
    }
}
