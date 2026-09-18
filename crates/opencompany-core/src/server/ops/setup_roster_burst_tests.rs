use super::*;

#[test]
fn roster_burst_registry_is_bounded_and_evicts_only_idle_entries() {
    let now = Instant::now();
    let mut registry = HashMap::new();
    for index in 0..MAX_ROSTER_BURST_ENTRIES {
        let burst = Arc::new(RosterBurst::default());
        burst.calls.lock().unwrap().push_back(now);
        registry.insert(CompanyId::new(format!("active-{index}")), burst);
    }

    let refused = roster_burst_for_in(&mut registry, &CompanyId::new("overflow"), now).is_err();
    assert_eq!(
        usize::from(refused),
        1,
        "a full registry of active bursts must refuse a new key"
    );
    assert_eq!(
        registry.len(),
        MAX_ROSTER_BURST_ENTRIES,
        "company churn must not grow the registry past its cap"
    );

    let held = Arc::clone(registry.values().next().unwrap());
    for burst in registry.values() {
        burst.calls.lock().unwrap().clear();
    }
    let inserted = roster_burst_for_in(&mut registry, &CompanyId::new("replacement"), now)
        .expect("idle entries make room for a new company");
    assert!(registry.values().any(|burst| Arc::ptr_eq(burst, &held)));
    assert!(registry.values().any(|burst| Arc::ptr_eq(burst, &inserted)));
    assert_eq!(registry.len(), 2);
}
