//! The guidance layer may only ever *add* a description. These walk every tool
//! the belt actually wires and assert that, plus the two regressions that made
//! the layer necessary.

use super::*;

/// Every (provider, tool, is_canonical) the belt wires, as the catalogue names
/// them. Mirrors `live::canonical` and `live::extras`.
const WIRED: &[(&str, &str, bool)] = &[
    ("brave", "brave_web_search", true),
    ("brave", "brave_news_search", false),
    ("brave", "brave_image_search", false),
    ("brave", "brave_video_search", false),
    ("exa", "exa_search", true),
    ("exa", "exa_find_similar", false),
    ("exa", "exa_get_contents", false),
    ("querit", "querit_search", true),
    ("searxng", "searxng_search", true),
];

fn raw(provider: &str, tool: &str) -> ToolSpec {
    tinysearch_bus::provider_tool_specs()
        .get(provider)
        .unwrap_or_else(|| panic!("the catalogue publishes {provider}"))
        .iter()
        .find(|spec| spec.name == tool)
        .unwrap_or_else(|| panic!("{provider} publishes {tool}"))
        .clone()
}

#[test]
fn the_argument_set_is_never_widened_or_narrowed() {
    // The module validates with `additionalProperties: false` against its own
    // catalogue, so adding a property here would produce a call it rejects, and
    // dropping one would hide an argument it accepts.
    for (provider, tool, canonical) in WIRED {
        let before = raw(provider, tool);
        let after = documented(before.clone(), *canonical);
        let keys = |spec: &ToolSpec| -> Vec<String> {
            spec.parameters["properties"]
                .as_object()
                .expect("properties")
                .keys()
                .cloned()
                .collect()
        };
        assert_eq!(keys(&before), keys(&after), "{tool} argument set changed");
        assert_eq!(
            before.parameters["required"], after.parameters["required"],
            "{tool} required set changed",
        );
        assert_eq!(
            before.parameters["additionalProperties"], after.parameters["additionalProperties"],
            "{tool} stopped rejecting unknown arguments",
        );
    }
}

#[test]
fn a_property_keeps_its_type_bounds_and_enum() {
    for (provider, tool, canonical) in WIRED {
        let before = raw(provider, tool);
        let after = documented(before.clone(), *canonical);
        let properties = after.parameters["properties"].as_object().expect("props");
        for (name, property) in properties {
            let original = &before.parameters["properties"][name];
            for key in [
                "type",
                "enum",
                "minimum",
                "maximum",
                "minLength",
                "maxLength",
                "items",
            ] {
                assert_eq!(
                    original.get(key),
                    property.get(key),
                    "{tool}.{name} had its `{key}` changed",
                );
            }
        }
    }
}

#[test]
fn every_wired_argument_is_documented() {
    // The regression this layer exists for: an undocumented `freshness` is a
    // filter the model has to guess the format of.
    for (provider, tool, canonical) in WIRED {
        let spec = documented(raw(provider, tool), *canonical);
        for (name, property) in spec.parameters["properties"].as_object().expect("props") {
            let described = property
                .get("description")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty());
            assert!(described, "{tool}.{name} reaches the model undocumented");
        }
    }
}

#[test]
fn braves_freshness_says_which_values_the_provider_honours() {
    let spec = documented(raw("brave", "brave_web_search"), true);
    let text = spec.parameters["properties"]["freshness"]["description"]
        .as_str()
        .expect("documented");
    for code in ["pd", "pw", "pm", "py"] {
        assert!(text.contains(code), "freshness does not mention `{code}`");
    }
}

#[test]
fn every_wired_tool_has_a_description_worth_reading() {
    // "Search web with Brave" is four words. The managed tool it shares a belt
    // name with runs to several sentences; a model should not be briefed
    // differently because of which company it belongs to.
    for (provider, tool, canonical) in WIRED {
        let spec = documented(raw(provider, tool), *canonical);
        assert!(
            spec.description.split_whitespace().count() >= 20,
            "{tool} is described in {} words",
            spec.description.split_whitespace().count(),
        );
    }
}

#[test]
fn a_tool_that_returns_third_party_text_warns_about_obeying_it() {
    for (provider, tool, canonical) in WIRED {
        let spec = documented(raw(provider, tool), *canonical);
        if *tool == "brave_image_search" || *tool == "brave_video_search" {
            // Links to media, not prose a model would be tempted to follow.
            continue;
        }
        assert!(
            spec.description.contains("never obey"),
            "{tool} does not warn that results are not instructions",
        );
    }
}

#[test]
fn no_catalogue_tool_declares_both_count_spellings() {
    // `max_results` is read before `count`; that order may never matter.
    for (provider, tool, _) in WIRED {
        let spec = raw(provider, tool);
        let properties = spec.parameters["properties"].as_object().expect("props");
        assert!(
            !(properties.contains_key("max_results") && properties.contains_key("count")),
            "{tool} declares both, so which one is read is now a decision",
        );
    }
}
