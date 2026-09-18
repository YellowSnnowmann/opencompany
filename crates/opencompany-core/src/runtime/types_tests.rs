use super::CompanyStatus;

/// **Replay compatibility (issue #86).** A status payload serialized before
/// the kill switch existed has no `emergency_paused` key, and must read as
/// `false` — the company is not stopped — so an old snapshot or a synthetic
/// payload keeps working against a new host. This is the `#[serde(default)]`
/// contract on the field, asserted directly rather than only through the
/// live API responses.
#[test]
fn a_status_without_emergency_paused_deserializes_as_not_stopped() {
    let payload = serde_json::json!({
        "id": "acme",
        "name": "Acme",
        "lifecycle": "running",
        "pending_approvals": 3,
    });
    let status: CompanyStatus =
        serde_json::from_value(payload).expect("a pre-stop status payload still deserializes");
    assert!(!status.emergency_paused);
}
