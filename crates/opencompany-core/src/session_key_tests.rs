use super::*;

#[test]
fn a_session_is_named_for_its_company_and_its_teammate() {
    let key = openhuman_session_key(&CompanyId::new("acme"), "designer");
    assert_eq!(key, "acme:designer");
}

#[test]
fn two_teammates_of_one_company_are_two_sessions() {
    let company = CompanyId::new("acme");
    assert_ne!(
        openhuman_session_key(&company, "designer"),
        openhuman_session_key(&company, "engineer"),
        "one company's teammates must not share a session id — the whole \
         point is telling their turns apart on the bus"
    );
}

#[test]
fn one_teammate_id_in_two_companies_is_two_sessions() {
    // The process is multi-tenant and `agent_id` is unique only within a
    // company, so the company has to be in the key or two tenants' turns
    // arrive on the bus indistinguishable.
    assert_ne!(
        openhuman_session_key(&CompanyId::new("acme"), "designer"),
        openhuman_session_key(&CompanyId::new("globex"), "designer"),
    );
}

#[test]
fn the_key_is_stable_for_the_same_pair() {
    let company = CompanyId::new("acme");
    assert_eq!(
        openhuman_session_key(&company, "designer"),
        openhuman_session_key(&company, "designer"),
        "a roster rebuild must not rename a live session"
    );
}

#[test]
fn the_channel_is_not_the_builders_unlabelled_default() {
    assert_ne!(
        SESSION_CHANNEL, "internal",
        "`internal` is what openhuman calls a session nobody named"
    );
}
