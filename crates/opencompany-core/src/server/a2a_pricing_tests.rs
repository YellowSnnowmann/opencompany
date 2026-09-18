use super::*;

use super::test_support::*;

#[test]
fn a_priced_skill_is_charged_for() {
    let card = card_pricing(&[("seo.audit", "25.00")]);
    assert!(matches!(
        classify_skill(&card, "seo.audit"),
        SkillCharge::Priced(_)
    ));
}

#[test]
fn a_zero_price_is_deliberately_free() {
    let card = card_pricing(&[("seo.audit", "25.00"), ("seo.free", "0.00")]);
    assert!(matches!(
        classify_skill(&card, "seo.free"),
        SkillCharge::Free
    ));
}

#[test]
fn an_unparsable_price_is_still_free() {
    let card = card_pricing(&[("seo.audit", "25.00"), ("seo.odd", "gratis")]);
    assert!(matches!(
        classify_skill(&card, "seo.odd"),
        SkillCharge::Free
    ));
}

#[test]
fn an_unadvertised_skill_is_unknown_not_free() {
    let card = card_pricing(&[("seo.audit", "25.00"), ("seo.free", "0.00")]);
    assert!(matches!(
        classify_skill(&card, "seo.ghost"),
        SkillCharge::Unknown
    ));
}

#[test]
fn a_card_that_prices_nothing_charges_for_nothing() {
    // A company that never opted into pricing keeps serving every id,
    // including one it does not list — refusing here would take A2A away
    // from it.
    let card = card_pricing(&[("seo.free", "0.00")]);
    assert!(matches!(
        classify_skill(&card, "seo.ghost"),
        SkillCharge::Free
    ));
    assert!(matches!(
        classify_skill(&AgentCard::default(), "seo.ghost"),
        SkillCharge::Free
    ));
}

#[test]
fn a_duplicate_id_with_a_priced_entry_is_still_charged() {
    // Manifest validation now rejects this shape outright, but the lookup
    // itself must stay safe by construction: given both a free and a
    // priced entry under the same id, in either order, the priced one
    // must win. Letting the free entry win would waive a price the
    // company does charge for that skill.
    let free_first = card_pricing(&[("seo.audit", "0.00"), ("seo.audit", "25.00")]);
    assert!(matches!(
        classify_skill(&free_first, "seo.audit"),
        SkillCharge::Priced(_)
    ));

    let priced_first = card_pricing(&[("seo.audit", "25.00"), ("seo.audit", "0.00")]);
    assert!(matches!(
        classify_skill(&priced_first, "seo.audit"),
        SkillCharge::Priced(_)
    ));
}
