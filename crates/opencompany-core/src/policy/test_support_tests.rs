use super::*;
use crate::policy::Standing;
#[cfg(feature = "openhuman")]
use crate::policy::consequence::Reach;
use crate::policy::consequence::{COMPOSIO_EXECUTE, consequence_of};
use crate::ports::types::EffectGroup;

/// The guarantee the whole module exists for: a fixture reaches the
/// classifier's action key. Asserted against the constant *and* against the
/// literal, because agreeing with `COMPOSIO_ACTION_KEY` while both drift off
/// the tool's declared schema would reproduce #470 with the helper in place.
#[test]
fn a_fixture_names_its_action_under_the_key_the_tool_declares() {
    let args = composio_send_args();
    assert_eq!(
        args.get(COMPOSIO_ACTION_KEY).and_then(|v| v.as_str()),
        Some(COMPOSIO_SEND_SLUG)
    );
    assert_eq!(
        args.get("tool").and_then(|v| v.as_str()),
        Some(COMPOSIO_SEND_SLUG),
        "`tool` is what `composio_execute`'s schema requires; if this fails \
         the wire key moved and the tool's schema has to move with it"
    );
    assert!(
        args.get("tool_slug").is_none(),
        "the key #470 was about must not come back"
    );
}

/// The action's own parameters ride under `arguments`, which is the only
/// other property the tool's schema admits — a fixture that spread them at
/// the top level would be rejected by `additionalProperties: false`.
#[test]
fn action_parameters_ride_under_arguments() {
    let args = composio_args_with(COMPOSIO_SEND_SLUG, serde_json::json!({ "to": "a@b.test" }));
    assert_eq!(
        args.get("tool").and_then(|v| v.as_str()),
        Some(COMPOSIO_SEND_SLUG)
    );
    assert_eq!(args["arguments"]["to"], "a@b.test");
    assert_eq!(
        args.as_object().map(|o| o.len()),
        Some(2),
        "`tool` and `arguments` are the whole of the declared schema"
    );
}

/// The send fixture is a send *through the catalogue*. True in both lanes,
/// but for different reasons — which is why the read fixture below is
/// pinned separately per lane.
#[test]
fn the_send_fixture_classifies_as_a_send() {
    for args in [
        composio_send_args(),
        composio_args(COMPOSIO_OTHER_SEND_SLUG),
    ] {
        let verdict = consequence_of(COMPOSIO_EXECUTE, &args);
        assert_eq!(verdict.group, EffectGroup::Send, "{args}");
        assert_eq!(verdict.standing, Standing::PerCall, "{args}");
    }
}

/// The fixture the fallback tests want: an action nobody has classified,
/// asked for on purpose. Same verdict in both lanes.
#[test]
fn the_unclassified_fixture_falls_back_to_a_send() {
    for args in [
        composio_unclassified_args(),
        composio_unclassified_args_numbered(0),
        composio_unclassified_args_numbered(7),
    ] {
        let verdict = consequence_of(COMPOSIO_EXECUTE, &args);
        assert_eq!(verdict.group, EffectGroup::Send, "{args}");
        assert_eq!(verdict.standing, Standing::PerCall, "{args}");
    }
}

/// The one that #470 was really about: with the catalogue linked in, the
/// read fixture is classified as a read — by the lookup, not by luck.
///
/// This is the assertion the old `tool_slug` fixtures could not have made.
/// Break the catalogue lookup and this fails, which is the property the
/// negative control in #559 is built on.
#[test]
#[cfg(feature = "openhuman")]
fn the_read_fixture_classifies_as_a_read() {
    let verdict = consequence_of(COMPOSIO_EXECUTE, &composio_read_args());
    assert_eq!(verdict.group, EffectGroup::Other);
    assert_eq!(verdict.standing, Standing::Grantable);
    // Issue #559 moved the read off `Consequence`, which is what parks. The
    // two assertions above are the ones that did not move.
    assert_eq!(verdict.reach, Reach::ExternalRead);
}

/// And the other lane, stated rather than left to be discovered: without
/// the harness feature the catalogue is not linked in, so the read fixture
/// classifies as a send. A test that wants the read verdict must be gated.
#[test]
#[cfg(not(feature = "openhuman"))]
fn without_the_catalogue_the_read_fixture_is_a_send() {
    let verdict = consequence_of(COMPOSIO_EXECUTE, &composio_read_args());
    assert_eq!(verdict.group, EffectGroup::Send);
    assert_eq!(verdict.standing, Standing::PerCall);
}
