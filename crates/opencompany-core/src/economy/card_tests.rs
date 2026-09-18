use super::*;

fn manifest_with_two_skills() -> CompanyManifest {
    let toml_src = r#"
        [company]
        name = "Acme SEO"
        output = "SEO audits and content"
        handle = "acme"

        [place]
        discoverable = true
        skills = [
            { id = "seo.audit", price_usd = "25.00", description = "Full site audit" },
            { id = "seo.brief", price_usd = "10.00" },
        ]
    "#;
    toml::from_str(toml_src).expect("valid manifest")
}

#[test]
fn projects_priced_skills_deterministically() {
    let manifest = manifest_with_two_skills();
    let card = build_agent_card(&manifest, "https://host.example");

    assert_eq!(card.handle, "acme");
    assert_eq!(card.name, "Acme SEO");
    assert_eq!(card.description, "SEO audits and content");
    assert_eq!(card.actor_type, "agent");
    assert_eq!(card.endpoint, "https://host.example/a2a/acme");
    assert_eq!(card.supported_interfaces, vec!["a2a-jsonrpc"]);
    assert_eq!(card.skills, vec!["seo.audit", "seo.brief"]);
    assert_eq!(card.capabilities, card.skills);
    assert_eq!(card.tags, card.skills);

    assert_eq!(card.payment_requirements.len(), 2);
    let audit = &card.payment_requirements[0];
    assert_eq!(audit.skill_id, "seo.audit");
    assert_eq!(audit.price, "25.00");
    assert_eq!(audit.asset, "USDC");
    assert_eq!(audit.network, "solana");

    // Determinism: a second projection is byte-identical.
    assert_eq!(build_agent_card(&manifest, "https://host.example"), card);
}

#[test]
fn endpoint_trims_trailing_slash_on_base() {
    let manifest = manifest_with_two_skills();
    let card = build_agent_card(&manifest, "https://host.example/");
    assert_eq!(card.endpoint, "https://host.example/a2a/acme");
}

#[test]
fn description_falls_back_to_name_when_output_absent() {
    let manifest: CompanyManifest =
        toml::from_str("[company]\nname = \"Solo\"\nhandle = \"solo\"\n").expect("manifest");
    let card = build_agent_card(&manifest, "https://h");
    assert_eq!(card.description, "Solo");
    assert!(card.skills.is_empty());
    assert!(card.payment_requirements.is_empty());
}

#[test]
fn skill_md_lists_every_priced_skill() {
    let manifest = manifest_with_two_skills();
    let card = build_agent_card(&manifest, "https://host.example");
    let md = render_skill_md(&card);
    assert!(md.contains("# Acme SEO"));
    assert!(md.contains("`seo.audit` — 25.00 USDC (solana)"));
    assert!(md.contains("`seo.brief` — 10.00 USDC (solana)"));
}
