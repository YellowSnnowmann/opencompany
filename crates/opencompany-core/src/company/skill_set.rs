//! A company's skill set, assembled from its layers in one place.
//!
//! [`skill_effective::resolve`] folds whatever layers it is handed. This module
//! is where a company's layers are named — the global baseline, the bundle
//! under its source directory, the host's shared library — so every reader
//! hands the fold the same ones.

use std::path::{Path, PathBuf};

use crate::company::SkillDoc;
use crate::company::skill_effective::{self, EffectiveSkill, SkillLayers};
use crate::error::Result;
use crate::ports::skills_state::SkillState;

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
