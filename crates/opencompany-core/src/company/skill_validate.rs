//! The one validator every skill document passes through.
//!
//! [`parse_skill_md`] answers whether a document is *shaped* like a SKILL.md.
//! This answers whether it is one the product will accept: the slug is a safe
//! directory name **and** a bounded one, the description fits the budget every
//! agent pays for it on every turn, and the frontmatter block cannot grow
//! without limit behind a small body.
//!
//! The Agent Skills spec also requires `name` to equal the skill's directory.
//! Here `name` is a display string — every skill this repo ships has one
//! (`Web Research` for `web-research`) — so enforcing it would reject the whole
//! baseline. It is reported as a [`SpecDelta`] instead: a warning that travels
//! with the validated document, never a refusal.
//!
//! Document *size* is bounded separately, on the assembled `SKILL.md`, by the
//! write plane's own ceiling; this module adds no second bound for it.

use super::skill_file::{SkillDoc, parse_skill_md};
use crate::error::OpenCompanyError;
use tinyskills::{
    PunctuationRule, SlugError, SlugRules, check_frontmatter_size, validate_description_chars,
};

/// The longest a skill slug may be, in characters.
///
/// The Agent Skills spec's limit. A slug is also a directory name in every
/// agent's scratch tree, so it is bounded by what a filesystem will take long
/// before it is bounded by taste.
pub const MAX_SLUG_CHARS: usize = 64;

/// The longest a skill description may be, in characters.
///
/// The spec's limit, and a prompt budget: a description is catalogue text, read
/// by every agent on every turn, so it is a one-liner by contract.
pub const MAX_DESCRIPTION_CHARS: usize = 1024;

/// The largest a skill's frontmatter block may be, in bytes.
///
/// Frontmatter carries four short scalars. A cap an order of magnitude above
/// the largest shipped block leaves room for keys we have not invented while
/// keeping an unbounded field out of the metadata the console renders.
pub const MAX_FRONTMATTER_BYTES: usize = 4 * 1024;

/// A rule the spec states that this validator records rather than enforces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecDelta {
    /// The spec requires `name` to equal the skill's directory name. Here it is
    /// a display string, so the mismatch is reported and the document accepted.
    NameIsNotSlug {
        /// The frontmatter's `name`.
        name: String,
        /// The directory name the document lives under.
        slug: String,
    },
}

impl SpecDelta {
    /// The delta in operator-facing language.
    pub fn message(&self) -> String {
        match self {
            Self::NameIsNotSlug { name, slug } => format!(
                "skill `{slug}` has the display name `{name}`. The Agent Skills spec expects \
                 `name` to be the slug; this host treats `name` as a display string and accepts \
                 the difference."
            ),
        }
    }
}

/// A document that passed validation, with whatever it diverges from the spec
/// on.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidSkill {
    /// The parsed document.
    pub doc: SkillDoc,
    /// Spec divergences worth reporting, none of which refused the document.
    pub deltas: Vec<SpecDelta>,
}
/// Slugs the skill routes cannot address, because a static route already holds
/// that path.
///
/// `/skills/draft`, `/skills/upload` and `/skills/registry` sit at the same
/// depth as `/skills/{slug}`, and a static segment wins, so a skill stored
/// under one of these names answers 405 to every toggle rather than falling
/// through — created, and then unreachable for the rest of its life. The names
/// are refused at the point one would be created instead.
///
/// `skills::tests_scan` walks this list against the router, so a route added or
/// renamed without updating it fails rather than reopening the hole.
pub const RESERVED_SLUGS: &[&str] = &["draft", "upload", "registry"];

/// The slug rules for deriving and accepting skill slugs: a safe
/// `[a-z0-9][a-z0-9-]*` name within [`MAX_SLUG_CHARS`], off
/// [`RESERVED_SLUGS`]. Deriving folds every run of other characters to one
/// `-`, cuts a long name rather than refusing it, and falls back to `skill`.
pub const SLUG_RULES: SlugRules<'static> = SlugRules::new()
    .max_chars(MAX_SLUG_CHARS)
    .reserved(RESERVED_SLUGS)
    .truncate(true)
    .punctuation(PunctuationRule::Separator)
    .fallback("skill");

/// [`SLUG_RULES`]' shape rule alone, with no length cap and nothing reserved.
pub const SLUG_SHAPE_RULES: SlugRules<'static> = SlugRules::new().max_chars(usize::MAX);

/// The half of [`validate_slug`] that is about safety rather than about size.
///
/// A slug is a path segment, and this is what keeps it one. The length cap is
/// a rule about what authoring may *create*, so a route addressing a skill that
/// already exists asks only this: a row stored before the cap was introduced is
/// still a row its owner has to be able to reach.
pub fn validate_slug_shape(slug: &str) -> Result<(), String> {
    tinyskills::validate_slug(slug, &SLUG_SHAPE_RULES).map_err(|error| slug_problem(slug, &error))
}

/// Whether `slug` is one the product will accept under [`SLUG_RULES`]: a safe
/// directory name within [`MAX_SLUG_CHARS`] and not reserved.
///
/// Returns the operator-facing reason on refusal, so each caller can wrap it in
/// its own error type without restating the rule.
pub fn validate_slug(slug: &str) -> Result<(), String> {
    tinyskills::validate_slug(slug, &SLUG_RULES).map_err(|error| slug_problem(slug, &error))
}

fn slug_problem(slug: &str, error: &SlugError) -> String {
    match error {
        SlugError::TooLong { length, max } => format!(
            "that slug is {length} characters — a skill slug has to be {max} characters or fewer."
        ),
        SlugError::Reserved { .. } => format!(
            "`{slug}` is a reserved skill slug — the skill routes already use that path, so a \
             skill stored under it could never be switched off again."
        ),
        _ => format!(
            "`{slug}` is not a valid skill slug. Skills live under `skills/<slug>/`, so a slug \
             is `[a-z0-9][a-z0-9-]*`."
        ),
    }
}

/// Validates one `SKILL.md` source for the given slug.
///
/// This is the single place the rules live: registry install, the
/// empty-registry fallback and console authoring all call it, so the three
/// cannot disagree about what a skill is. Every problem found is reported at
/// once rather than one per round-trip.
pub fn validate_skill_md(slug: &str, src: &str) -> Result<ValidSkill, Vec<String>> {
    let mut problems = Vec::new();

    if let Err(problem) = validate_slug(slug) {
        problems.push(problem);
    }

    if let Err(too_large) = check_frontmatter_size(src, MAX_FRONTMATTER_BYTES) {
        problems.push(format!(
            "that skill's frontmatter block is {} bytes — a skill's frontmatter has to be under \
             {MAX_FRONTMATTER_BYTES} bytes.",
            too_large.bytes
        ));
    }

    let doc = match parse_skill_md(slug, src) {
        Ok(doc) => doc,
        Err(OpenCompanyError::DataInvalid {
            problems: found, ..
        }) => {
            problems.extend(found);
            return Err(problems);
        }
        Err(other) => {
            problems.push(other.to_string());
            return Err(problems);
        }
    };

    if let Err(too_long) = validate_description_chars(&doc.description, MAX_DESCRIPTION_CHARS) {
        problems.push(format!(
            "that skill's description is {} characters — a description has to be \
             {MAX_DESCRIPTION_CHARS} characters or fewer.",
            too_long.chars
        ));
    }

    if !problems.is_empty() {
        return Err(problems);
    }

    let mut deltas = Vec::new();
    if doc.name != doc.slug {
        deltas.push(SpecDelta::NameIsNotSlug {
            name: doc.name.clone(),
            slug: doc.slug.clone(),
        });
    }

    Ok(ValidSkill { doc, deltas })
}

/// Turns a display name into a slug the slug-bearing routes accept: a
/// filesystem-and-URL-safe name within [`MAX_SLUG_CHARS`], derived under
/// [`SLUG_RULES`].
///
/// Authoring and upload both derive a store key and a directory name from free
/// text, so whatever this returns has to pass [`validate_slug`]. Truncating
/// keeps a long name authorable; refusing it would leave the operator renaming
/// a skill to satisfy a limit they cannot see.
pub fn slugify(name: &str) -> String {
    tinyskills::slugify_with(name, &SLUG_RULES).unwrap_or_else(|_| "skill".to_string())
}

#[cfg(test)]
#[path = "skill_validate_tests.rs"]
mod tests;
