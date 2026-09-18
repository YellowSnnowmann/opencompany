use super::*;

#[test]
fn verify_token_matches_typescript_fold() {
    // Reproduces domain.ts::verifyToken for a concrete domain. The fold of
    // "acme.com" under (hash*31 + code) | 0 is deterministic; a change here
    // would drift the console and host apart.
    let token = verify_token("acme.com");
    assert_eq!(token.len(), 8);
    let mut hash: i32 = 0;
    for unit in "acme.com".encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(unit as i32);
    }
    assert_eq!(token, format!("{:08x}", hash.unsigned_abs()));
}

#[test]
fn records_are_deterministic_and_five() {
    let a = dns_records("acme.com");
    let b = dns_records("acme.com");
    assert_eq!(a, b);
    assert_eq!(a.len(), 5);
    assert_eq!(a[0].record_type, "TXT");
    assert_eq!(a[0].name, "_opencompany.acme.com");
    assert!(a[0].value.starts_with("oc-verify="));
    assert_eq!(a[1].record_type, "CNAME");
    assert_eq!(a[1].value, PLATFORM_TARGET);
}

#[test]
fn empty_domain_yields_no_records() {
    assert!(dns_records("   ").is_empty());
    assert!(dns_records("").is_empty());
}

#[test]
fn trailing_dot_is_stripped() {
    assert_eq!(dns_records("acme.com."), dns_records("acme.com"));
}

#[test]
fn record_serializes_type_key() {
    let record = &dns_records("acme.com")[0];
    let json = serde_json::to_value(record).unwrap();
    assert_eq!(json["type"], "TXT");
    assert_eq!(json["ttl"], "3600");
}

#[tokio::test]
async fn verify_passes_when_all_records_present() {
    let resolver = StaticDnsResolver::fully_verifying("acme.com");
    let status = verify("acme.com", &resolver).await.unwrap();
    assert!(status.verified);
    assert_eq!(status.checks.as_ref().unwrap().len(), 5);
    assert!(status.checks.unwrap().iter().all(|c| c.found));
}

#[tokio::test]
async fn verify_fails_when_a_record_missing() {
    // Seed everything but the verification TXT.
    let mut resolver = StaticDnsResolver::new();
    for record in dns_records("acme.com").into_iter().skip(1) {
        match record.record_type.as_str() {
            "TXT" => resolver = resolver.with_txt(record.name, record.value),
            "CNAME" => resolver = resolver.with_cname(record.name, record.value),
            _ => {}
        }
    }
    let status = verify("acme.com", &resolver).await.unwrap();
    assert!(!status.verified);
    let checks = status.checks.unwrap();
    assert!(!checks[0].found);
}

#[tokio::test]
async fn verify_tolerates_trailing_dot_on_cname() {
    let resolver = StaticDnsResolver::fully_verifying("acme.com")
        .with_cname("acme.com", "mail.opencompany.host.");
    let status = verify("acme.com", &resolver).await.unwrap();
    let cname_check = status
        .checks
        .unwrap()
        .into_iter()
        .find(|c| c.name == "acme.com" && c.record_type == "CNAME")
        .unwrap();
    assert!(cname_check.found);
}
