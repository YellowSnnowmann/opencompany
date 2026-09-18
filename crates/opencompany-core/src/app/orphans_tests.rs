use super::*;

fn company(id: &str) -> CompanySummary {
    CompanySummary {
        id: CompanyId::new(id.to_string()),
        name: format!("{id} Inc"),
        lifecycle: "active".to_string(),
    }
}

fn owner(id: &str, tenant: &str) -> (CompanyId, String) {
    (CompanyId::new(id.to_string()), tenant.to_string())
}

/// The finding this issue exists for: a company the store holds that no
/// owner row claims. Its tenant gets a 403 from `authorize_address` and no
/// explanation, so nothing but this report can tell anyone it is there.
#[test]
fn a_company_with_no_owner_row_is_reported() {
    let report = find(&[company("acme")], &[]);

    assert_eq!(report.unowned.len(), 1);
    assert_eq!(report.unowned[0].id.as_ref(), "acme");
    assert!(report.dangling.is_empty());
    assert!(!report.is_empty());
}

/// The other half of the same assertion, and the one that stops this being
/// a function that reports everything. A healthy deployment must produce
/// silence.
#[test]
fn a_company_with_an_owner_row_is_not_reported() {
    let report = find(&[company("acme")], &[owner("acme", "tenant-a")]);

    assert!(report.is_empty(), "{report:?}");
    assert_eq!(report.to_text(), "");
}

/// An owner row naming a company the store does not have. Benign, and
/// reported separately from the unowned direction because the two mean
/// opposite things to an operator: one hides data, the other is litter.
#[test]
fn an_owner_row_naming_no_company_is_reported_as_dangling() {
    let report = find(&[], &[owner("ghost", "tenant-b")]);

    assert!(report.unowned.is_empty());
    assert_eq!(report.dangling.len(), 1);
    assert_eq!(report.dangling[0].id.as_ref(), "ghost");
    assert_eq!(report.dangling[0].tenant, "tenant-b");
}

/// Both directions at once, which is the state an operator reconciling a
/// shared database actually finds.
#[test]
fn both_directions_are_reported_together() {
    let report = find(
        &[company("acme"), company("beta")],
        &[owner("beta", "tenant-a"), owner("ghost", "tenant-b")],
    );

    assert_eq!(
        report
            .unowned
            .iter()
            .map(|c| c.id.as_ref())
            .collect::<Vec<_>>(),
        vec!["acme"]
    );
    assert_eq!(
        report
            .dangling
            .iter()
            .map(|r| r.id.as_ref())
            .collect::<Vec<_>>(),
        vec!["ghost"]
    );
}

/// Presence is the question, NOT agreement. A row that claims the company
/// for a different tenant than the one asking still means the company is
/// owned, and reporting it here would flood the report on every
/// multi-tenant deployment — the normal state of the collection this reads.
#[test]
fn a_row_owned_by_another_tenant_is_still_owned() {
    let report = find(&[company("acme")], &[owner("acme", "some-other-tenant")]);

    assert!(report.is_empty(), "{report:?}");
}

/// The tenant is reported exactly as persisted, un-canonicalised.
///
/// `canonical_tenant` maps `tenant:acme` and bare `acme` together, and
/// hydration needs that because it compares two tenant strings. This does
/// not compare anything, and an operator about to go and fix a row needs to
/// see the bytes that are actually in it.
#[test]
fn the_dangling_tenant_is_reported_verbatim() {
    let report = find(&[], &[owner("ghost", "tenant:acme")]);

    assert_eq!(report.dangling[0].tenant, "tenant:acme");
}

/// Output order does not track the order the two backends happened to
/// return rows in, so two boots of an unchanged deployment produce
/// identical text and a diff between them means something moved.
#[test]
fn findings_are_ordered_independently_of_the_input_order() {
    let forward = find(
        &[company("zeta"), company("alpha")],
        &[owner("z-ghost", "t"), owner("a-ghost", "t")],
    );
    let reversed = find(
        &[company("alpha"), company("zeta")],
        &[owner("a-ghost", "t"), owner("z-ghost", "t")],
    );

    assert_eq!(forward, reversed);
    assert_eq!(
        forward
            .unowned
            .iter()
            .map(|c| c.id.as_ref())
            .collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
    assert_eq!(
        forward
            .dangling
            .iter()
            .map(|r| r.id.as_ref())
            .collect::<Vec<_>>(),
        vec!["a-ghost", "z-ghost"]
    );
}

/// A duplicate id from the company store is reported once, not twice.
#[test]
fn a_duplicated_company_id_is_reported_once() {
    let report = find(&[company("acme"), company("acme")], &[]);

    assert_eq!(report.unowned.len(), 1);
}

/// The text names every finding. A report that says "3 companies" without
/// saying which is not actionable, and the ids are the whole point.
#[test]
fn the_text_names_every_finding() {
    let report = find(&[company("acme")], &[owner("ghost", "tenant-b")]);
    let text = report.to_text();

    assert!(text.contains("acme"), "{text}");
    assert!(text.contains("ghost"), "{text}");
    assert!(text.contains("tenant-b"), "{text}");
}

/// The boot filter keeps only this tenant's unowned companies — identified
/// by the `<tenant>--` id prefix `namespace_company_id` writes — and drops
/// the rest, so tenant B's company ids never reach tenant A's boot log.
#[test]
fn the_boot_filter_keeps_only_this_tenants_companies() {
    let report = find(&[company("tenant-a--acme"), company("tenant-b--beta")], &[]);
    let filtered = filter_to_tenant(report, "tenant-a");

    let ids: Vec<&str> = filtered.unowned.iter().map(|c| c.id.as_ref()).collect();
    assert_eq!(ids, vec!["tenant-a--acme"]);
    assert!(filtered.dangling.is_empty());
}

/// A company with no tenant prefix is nobody's in the shared database, so
/// the boot filter drops it too. Such a company is addressable with
/// platform scope, not orphaned from a tenant.
#[test]
fn the_boot_filter_drops_unprefixed_companies() {
    let report = find(&[company("acme")], &[]);
    let filtered = filter_to_tenant(report, "tenant-a");

    assert!(filtered.is_empty(), "{filtered:?}");
}

/// Dangling rows are matched by their persisted tenant, compared
/// canonically (`tenant:acme` and `acme` are one tenant), so the boot
/// filter keeps this tenant's litter and drops everyone else's.
#[test]
fn the_boot_filter_keeps_this_tenants_dangling_rows() {
    let report = find(
        &[],
        &[
            owner("tenant-a--ghost", "tenant:acme"),
            owner("tenant-b--ghost", "tenant-b"),
        ],
    );
    let filtered = filter_to_tenant(report, "acme");

    let ids: Vec<&str> = filtered.dangling.iter().map(|r| r.id.as_ref()).collect();
    assert_eq!(ids, vec!["tenant-a--ghost"]);
    assert!(filtered.unowned.is_empty());
}
