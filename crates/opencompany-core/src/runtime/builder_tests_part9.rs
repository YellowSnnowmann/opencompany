use super::tests_core::*;

/// Issue #1925: approvals are explicit-only in production — the
/// manifest-`[policy]` HITL gate is deliberately dead weight, disabled at
/// the one construction site nothing else reaches
/// (`RuntimeBuilder::build`'s default, uninjected gate). Nothing else in
/// the type system pins that wiring: `with_policy_hitl_disabled` is a
/// plain builder call on `ManifestApprovalGate`, so deleting it would
/// compile clean and silently resurrect policy-driven parking in every
/// company that never explicitly injects a gate. Pinned here so that
/// deletion instead breaks this test.
#[tokio::test]
async fn the_default_uninjected_gate_ships_with_policy_hitl_disabled() {
    let dir = tmp_home("oc-policy-hitl-default-");
    let manifest = parse(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n\
         [policy]\nmode = \"supervised\"\n",
    );
    let runtime = RuntimeBuilder::new(dir.path().to_path_buf(), manifest)
        .build()
        .await
        .unwrap();
    assert!(
        !runtime.approval_gate.policy_hitl_enabled(),
        "the production default build must disable the manifest-policy HITL gate; a \
         company that injects no gate of its own must never fall back to it"
    );
}
