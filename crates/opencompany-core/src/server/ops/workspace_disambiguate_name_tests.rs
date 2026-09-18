use super::disambiguate_name;
use crate::company::workspace_names::{MAX_NAME_BYTES, is_kebab_name, kebab_name_or};

/// The tag always lands between stem and extension, whatever the name
/// carried.
#[test]
fn inserts_the_tag_before_the_extension() {
    assert_eq!(disambiguate_name("image.png", "01j8ab"), "image-01j8ab.png");
    assert_eq!(disambiguate_name("notes", "01j8ab"), "notes-01j8ab");
    assert_eq!(
        disambiguate_name("page.compiled.mjs", "01j8ab"),
        "page.compiled-01j8ab.mjs"
    );
}

/// The regression the budget guard exists for: a name that already fills
/// the whole workspace-name budget must stay within it once the tag lands,
/// or the next normalization truncates the tag back off and the name maps
/// onto the very collision the retry exists to escape.
#[test]
fn a_full_budget_name_stays_within_the_budget_and_keeps_the_tag() {
    let stem = "a".repeat(92); // `a…a.png` fills the 96-byte budget exactly.
    let name = format!("{stem}.png");
    assert_eq!(name.len(), MAX_NAME_BYTES);
    assert_eq!(kebab_name_or(&name, &name), name);

    let disambiguated = disambiguate_name(&name, "ab12cd");
    assert!(
        disambiguated.len() <= MAX_NAME_BYTES,
        "disambiguated name is {} bytes, over the {MAX_NAME_BYTES} budget",
        disambiguated.len()
    );
    // The whole tag survives — truncation must never cut it off.
    assert!(disambiguated.contains("-ab12cd"), "{disambiguated}");
    // And the result is canonical, so the repair pass leaves it alone.
    assert!(is_kebab_name(&disambiguated), "{disambiguated}");

    // Re-running the sanitizer is a fixed point: the tag is not a casualty
    // of the budget, which is what would re-collide with the original name.
    assert_eq!(kebab_name_or(&disambiguated, &disambiguated), disambiguated);
}

/// A near-full name with a short stem survives whole; the tag still fits.
#[test]
fn a_short_stem_is_never_truncated() {
    assert_eq!(disambiguate_name("image.png", "01j8ab"), "image-01j8ab.png");
}
