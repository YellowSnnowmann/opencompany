//! Tests for the console-chat ↔ hive-id mapping core's reserved desk
//! identities force (`hive_id_for_chat`, `chat_for_hive`, `hive_name`).

use super::*;

#[test]
fn general_runs_in_cores_default_desk_and_maps_back() {
    assert_eq!(hive_id_for_chat("general"), GENERAL_HIVE_ID);
    assert_eq!(chat_for_hive(GENERAL_HIVE_ID), "general");
}

#[test]
fn an_ordinary_desk_keeps_its_id_both_ways() {
    assert_eq!(hive_id_for_chat("engineering"), "engineering");
    assert_eq!(chat_for_hive("engineering"), "engineering");
}

#[test]
fn a_desk_id_core_reserves_is_prefixed_and_maps_back() {
    assert_eq!(hive_id_for_chat("main"), "desk-main");
    assert_eq!(chat_for_hive("desk-main"), "main");
    assert_eq!(
        chat_for_hive("desk-ops"),
        "desk-ops",
        "a desk really called `desk-ops` is left alone"
    );
}

#[test]
fn a_reserved_desk_name_gets_its_id_appended() {
    assert_eq!(hive_name("ops", "Main"), "Main (ops)");
    assert_eq!(hive_name("ops", "Operations"), "Operations");
    assert_eq!(hive_name(GENERAL_HIVE_ID, "General"), "General");
}
