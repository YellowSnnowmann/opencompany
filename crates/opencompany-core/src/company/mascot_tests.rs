//! Unit tests for `mascot.rs`.

use super::*;

#[test]
fn valid_mode_parses() {
    assert_eq!(parse_mode("static").unwrap(), "static");
    assert_eq!(parse_mode(" animated ").unwrap(), "animated");
}

#[test]
fn invalid_mode_names_the_accepted_set() {
    let err = parse_mode("paused").unwrap_err();
    let message = err.to_string();
    assert!(message.contains("static"));
    assert!(message.contains("animated"));
}

#[test]
fn every_costume_parses() {
    for costume in MASCOT_COSTUMES {
        assert_eq!(parse_costume(costume).unwrap(), costume);
    }
}

#[test]
fn unknown_costume_is_refused() {
    assert!(parse_costume("face_mask").is_err());
    assert!(parse_costume("").is_err());
}

#[test]
fn costume_numbers_pair_one_to_one_with_costumes() {
    assert_eq!(MASCOT_COSTUMES.len(), MASCOT_COSTUME_NUMBERS.len());
    // Every number is distinct — two costumes must never collide on one
    // `mascotAnimationNumber`, or choosing one would silently render the
    // other.
    let mut numbers = MASCOT_COSTUME_NUMBERS.to_vec();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), MASCOT_COSTUME_NUMBERS.len());
}

#[test]
fn default_costume_is_in_the_closed_list() {
    assert!(MASCOT_COSTUMES.contains(&DEFAULT_MASCOT_COSTUME));
}

#[test]
fn every_skin_color_parses_and_has_a_hex() {
    assert_eq!(MASCOT_SKIN_COLORS.len(), MASCOT_SKIN_COLOR_HEXES.len());
    for color in MASCOT_SKIN_COLORS {
        assert_eq!(parse_skin_color(color).unwrap(), color);
    }
    for hex in MASCOT_SKIN_COLOR_HEXES {
        assert_eq!(hex.len(), 6);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}

#[test]
fn every_hand_color_parses_and_has_a_hex() {
    assert_eq!(MASCOT_HAND_COLORS.len(), MASCOT_HAND_COLOR_HEXES.len());
    for color in MASCOT_HAND_COLORS {
        assert_eq!(parse_hand_color(color).unwrap(), color);
    }
    for hex in MASCOT_HAND_COLOR_HEXES {
        assert_eq!(hex.len(), 6);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}

#[test]
fn unknown_colors_are_refused() {
    assert!(parse_skin_color("chartreuse").is_err());
    assert!(parse_hand_color("chartreuse").is_err());
}

#[test]
fn default_swatches_are_named_entries() {
    assert!(MASCOT_SKIN_COLORS.contains(&"default"));
    assert!(MASCOT_HAND_COLORS.contains(&"default"));
}

#[test]
fn a_full_set_of_choices_parses_and_is_trimmed() {
    let choices = parse_choices(
        Some(" static "),
        Some("habibi"),
        Some("mint"),
        Some(" charcoal "),
    )
    .unwrap();
    assert_eq!(choices.mode.as_deref(), Some("static"));
    assert_eq!(choices.costume.as_deref(), Some("habibi"));
    assert_eq!(choices.skin_color.as_deref(), Some("mint"));
    assert_eq!(choices.hand_color.as_deref(), Some("charcoal"));
    assert!(!choices.is_empty());
}

#[test]
fn absent_and_blank_choices_are_no_choice_never_a_stored_empty_string() {
    assert!(parse_choices(None, None, None, None).unwrap().is_empty());
    let blank = parse_choices(Some(""), Some("   "), None, Some("\t")).unwrap();
    assert!(blank.is_empty(), "{blank:?}");
    // A partial look keeps only what was chosen.
    let partial = parse_choices(None, Some("cap"), None, None).unwrap();
    assert_eq!(partial.costume.as_deref(), Some("cap"));
    assert!(partial.mode.is_none() && partial.skin_color.is_none() && partial.hand_color.is_none());
}

#[test]
fn every_costume_and_swatch_the_patch_route_accepts_is_accepted_here_too() {
    for costume in MASCOT_COSTUMES {
        assert!(
            parse_choices(None, Some(costume), None, None).is_ok(),
            "{costume}"
        );
    }
    for color in MASCOT_SKIN_COLORS {
        assert!(
            parse_choices(None, None, Some(color), None).is_ok(),
            "{color}"
        );
    }
    for color in MASCOT_HAND_COLORS {
        assert!(
            parse_choices(None, None, None, Some(color)).is_ok(),
            "{color}"
        );
    }
    for mode in MASCOT_MODES {
        assert!(
            parse_choices(Some(mode), None, None, None).is_ok(),
            "{mode}"
        );
    }
}

#[test]
fn one_bad_choice_refuses_the_whole_set_and_names_the_accepted_values() {
    // The order of the arguments is mode, costume, skin, hand: each bad value
    // is refused with the sentence the `PATCH` route gives for that field.
    let cases: [(&str, [Option<&str>; 4], &str); 4] = [
        ("mode", [Some("paused"), Some("cap"), None, None], "static"),
        (
            "costume",
            [None, Some("face_mask"), None, None],
            "cardboard_mask",
        ),
        ("skin", [None, None, Some("chartreuse"), None], "mint"),
        (
            "hand",
            [None, Some("cap"), None, Some("#ff0000")],
            "charcoal",
        ),
    ];
    for (field, [mode, costume, skin, hand], accepted) in cases {
        let err = parse_choices(mode, costume, skin, hand).unwrap_err();
        let message = err.to_string();
        assert!(message.contains(accepted), "{field}: {message}");
        assert!(message.contains("Pick one of"), "{field}: {message}");
    }
}
