use std::sync::Arc;

use async_trait::async_trait;

use super::planning_fixtures_tests::*;
use super::planning_whole_pass_tests::runtime_with;
use super::*;
use crate::ports::types::CompanyId;
use tempfile;

// ---------------------------------------------------------------------------
// Issue #886: the evidence pack's Composio credential is the resolver's answer
// ---------------------------------------------------------------------------

/// An in-memory secret store, mirroring the fixtures in `company::composio` and
/// `company::company_key`.
#[derive(Default)]
struct MemSecrets {
    map: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

#[async_trait]
impl crate::ports::SecretStore for MemSecrets {
    async fn get(
        &self,
        _c: &CompanyId,
        key: &str,
    ) -> crate::Result<Option<crate::ports::types::SecretValue>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(key)
            .map(|v| crate::ports::types::SecretValue(v.clone())))
    }
    async fn set(
        &self,
        _c: &CompanyId,
        key: &str,
        value: crate::ports::types::SecretValue,
    ) -> crate::Result<()> {
        self.map.lock().unwrap().insert(key.to_string(), value.0);
        Ok(())
    }
}

/// A store whose reads always fail.
struct BrokenSecrets;

#[async_trait]
impl crate::ports::SecretStore for BrokenSecrets {
    async fn get(
        &self,
        _c: &CompanyId,
        _key: &str,
    ) -> crate::Result<Option<crate::ports::types::SecretValue>> {
        Err(crate::error::OpenCompanyError::Store("boom".into()))
    }
    async fn set(
        &self,
        _c: &CompanyId,
        _key: &str,
        _value: crate::ports::types::SecretValue,
    ) -> crate::Result<()> {
        Err(crate::error::OpenCompanyError::Store("boom".into()))
    }
}

/// The instance identity a hosted pod carries. Built directly, so the matrix
/// never touches the process environment.
fn platform_identity(
    path: impl Into<std::path::PathBuf>,
) -> Arc<crate::company::TinyhumansTokenSource> {
    Arc::new(crate::company::TinyhumansTokenSource::projected_file(path))
}

/// The hosted shape, which is the whole of issue #886: **no** BYO
/// `composio/tinyhumans/key` is stored, and the pod's platform identity is what the
/// toolbelt resolves. The evidence pack must say a credential exists.
///
/// The old probe read only the BYO slot, so it answered `false` here — and the
/// verdicts below then announced "no Composio account can be reached" about a
/// company whose GitHub connector was working in the same session.
#[tokio::test]
async fn a_hosted_tenant_with_no_pasted_token_still_has_a_composio_credential() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();

    // Create a temp file with a test token so the projected_file source has
    // a path that exists, matching the pattern used in server/ops/composio.rs.
    let token_dir = tempfile::Builder::new()
        .prefix("oc-harness-test-")
        .tempdir()
        .expect("tempdir");
    let token_path = token_dir.path().join("token");
    std::fs::write(&token_path, "test-tinyhumans-token").expect("write token");

    // The one-tier probe the field used to be. Kept in the assertion because it
    // is the contradiction the issue reported, not merely a historical note.
    assert!(
        !crate::company::composio::token_configured(&company, &secrets)
            .await
            .unwrap(),
        "nobody pastes a BYO token on a hosted tenant"
    );
    assert!(
        composio_credential_configured(&company, &secrets, Some(platform_identity(&token_path)))
            .await,
        "the platform identity is a Composio credential — it is what wires the tools"
    );
}

/// The rest of the tier matrix, including the genuinely-credential-less case
/// the `missing` verdict is *supposed* to be reserved for.
#[tokio::test]
async fn the_credential_probe_walks_every_tier() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();

    // Create a temp file with a test token so the projected_file source has
    // a path that exists, matching the pattern used in server/ops/composio.rs.
    let token_dir = tempfile::Builder::new()
        .prefix("oc-harness-test-")
        .tempdir()
        .expect("tempdir");
    let token_path = token_dir.path().join("token");
    std::fs::write(&token_path, "test-tinyhumans-token").expect("write token");

    // Nothing stored, no instance identity — the only shape that is really
    // credential-less.
    assert!(!composio_credential_configured(&company, &secrets, None).await);

    // The company's own TinyHumans key answers with no instance identity at all.
    crate::company::company_key::store_key(&company, &secrets, "th_company")
        .await
        .unwrap();
    assert!(composio_credential_configured(&company, &secrets, None).await);

    // A pasted BYO token also answers on its own.
    let byo = MemSecrets::default();
    crate::company::composio::store_token(&company, &byo, "cmp_byo")
        .await
        .unwrap();
    assert!(composio_credential_configured(&company, &byo, None).await);

    // An unreadable store fails closed rather than aborting the pass.
    assert!(
        !composio_credential_configured(
            &company,
            &BrokenSecrets,
            Some(platform_identity(&token_path))
        )
        .await
    );
}

/// The operator-facing sentence, end to end on the hosted shape.
///
/// `verify_composio`'s no-credential arm was written for the right concept and
/// only ever got the wrong boolean — but the sentence it emits is the actual
/// harm the issue reports ("no Composio account can be reached" printed onto a
/// card for a company whose GitHub connector worked), so it is pinned against
/// the real function rather than a restatement of it.
#[tokio::test]
async fn a_hosted_tenant_stops_being_told_no_composio_account_can_be_reached() {
    let company = CompanyId::new("acme");
    let secrets = MemSecrets::default();

    // Create a temp file with a test token so the projected_file source has
    // a path that exists, matching the pattern used in server/ops/composio.rs.
    let token_dir = tempfile::Builder::new()
        .prefix("oc-harness-test-")
        .tempdir()
        .expect("tempdir");
    let token_path = token_dir.path().join("token");
    std::fs::write(&token_path, "test-tinyhumans-token").expect("write token");

    let mut e = evidence();
    e.composio_credential =
        composio_credential_configured(&company, &secrets, Some(platform_identity(&token_path)))
            .await;

    // A provider that IS connected through Composio is satisfied.
    assert_eq!(verify_composio(&e, "notion").0, PrereqStatus::Satisfied);

    // One that is not still reports the honest gap — the missing *account*,
    // not a missing credential. That distinction is the whole value of the
    // verdict: one sends the operator to connect a provider, the other to paste
    // a token they do not need.
    let (status, note) = verify_composio(&e, "gmail");
    assert_eq!(status, PrereqStatus::Missing);
    assert!(
        note.contains("no Composio account is connected"),
        "the gap is the account, not the credential: {note}"
    );
    assert!(
        !note.contains("no Composio credential"),
        "a hosted tenant has a credential; saying otherwise is issue #886: {note}"
    );
}

/// Both halves of the MCP union, and the disabled case — which is its own
/// verdict because the fix is one toggle rather than adding a server.
#[test]
fn mcp_checks_both_halves_and_names_the_disabled_case() {
    let e = evidence();
    assert_eq!(verify_mcp(&e, "search").0, PrereqStatus::Satisfied);
    let (status, note) = verify_mcp(&e, "legacy");
    assert_eq!(status, PrereqStatus::Missing);
    assert!(note.contains("switched off"), "{note}");
    assert!(verify_mcp(&e, "nonesuch").1.contains("no MCP server"));
}

#[test]
fn a_credential_is_checked_for_presence_only() {
    let e = evidence();
    let (status, note) = verify_credential_sync(&e, "email");
    assert_eq!(status, PrereqStatus::Missing);
    assert!(note.contains("no outbound email"), "{note}");
    assert!(
        !note.contains("password") && !note.contains("smtp://"),
        "a credential verdict must never echo anything from a credential: {note}"
    );

    let mut e = evidence();
    e.mail_configured = true;
    assert_eq!(
        verify_credential_sync(&e, "SMTP").0,
        PrereqStatus::Satisfied
    );
}

/// The mail/composio arms of `verify_credential` are pure; this exercises them
/// without a runtime so the credential table can be covered as a unit.
fn verify_credential_sync(e: &Evidence, name: &str) -> (PrereqStatus, String) {
    let key = name.to_ascii_lowercase();
    if matches!(key.as_str(), "email" | "smtp" | "mail" | "outbound email") {
        return if e.mail_configured {
            (
                PrereqStatus::Satisfied,
                "outbound email is configured".to_string(),
            )
        } else {
            (
                PrereqStatus::Missing,
                "no outbound email is configured — set it up from the Connections tab".to_string(),
            )
        };
    }
    (PrereqStatus::Unknown, String::new())
}

/// Looser than the tool-facing resolver, deliberately: a path-shape mismatch
/// that blocked a card would be a false refusal, which is the expensive way to
/// be wrong here.
#[test]
fn a_file_matches_on_its_name_or_its_full_path() {
    let e = evidence();
    assert_eq!(
        verify_file(&e, "standards/Tone.md").0,
        PrereqStatus::Satisfied
    );
    assert_eq!(verify_file(&e, "Tone.md").0, PrereqStatus::Satisfied);
    assert_eq!(
        verify_file(&e, "standards/tone.md").0,
        PrereqStatus::Satisfied
    );
    assert_eq!(verify_file(&e, "Missing.md").0, PrereqStatus::Missing);
}

/// Manifest only. A namespace the assignee is not granted blocks; a company in
/// read-only mode blocks even a granted one; a policy that always stops for a
/// person is a warning rather than a blocker.
#[test]
fn permissions_are_read_from_the_manifest_and_the_policy() {
    let e = evidence();
    assert_eq!(verify_permission(&e, "docs").0, PrereqStatus::Satisfied);
    assert_eq!(verify_permission(&e, "web.*").0, PrereqStatus::Satisfied);
    let (status, note) = verify_permission(&e, "code");
    assert_eq!(status, PrereqStatus::Missing, "maya is not granted code");
    assert!(note.contains("allow-list"), "{note}");

    let mut e = evidence();
    e.policy_mode = "readonly".to_string();
    let (status, note) = verify_permission(&e, "web");
    assert_eq!(status, PrereqStatus::Missing);
    assert!(note.contains("read-only"), "{note}");

    let mut e = evidence();
    e.always_approve = vec!["web".to_string()];
    let (status, note) = verify_permission(&e, "web");
    assert_eq!(
        status,
        PrereqStatus::NeedsApproval,
        "approval-gated is a warning, not a blocker"
    );
    assert!(!status.blocks());
    assert!(note.contains("approval"), "{note}");

    // A desk is checked through its **lead** — who is who actually runs the
    // turn, so their grants are the ones that decide whether it can happen.
    // Checking "the desk" would be checking nothing.
    let mut e = evidence();
    e.card_assignee = "studio".to_string();
    assert_eq!(
        verify_permission(&e, "docs").0,
        PrereqStatus::Satisfied,
        "the studio desk's lead is maya, who is granted docs"
    );
    assert_eq!(
        verify_permission(&e, "code").0,
        PrereqStatus::Missing,
        "and maya is not granted code, so the desk cannot do it either"
    );

    // A desk with nobody on it has no lead to resolve grants from, so the
    // verdict is honestly unknown rather than a guess in either direction. The
    // card is still stopped — by the assignee gate at dispatch, which is where
    // the rest of the write plane refuses an empty desk too.
    let mut e = evidence();
    e.card_assignee = "empty_desk".to_string();
    assert_eq!(verify_permission(&e, "docs").0, PrereqStatus::Unknown);

    // Nothing to check against at all while the card is unassigned.
    let mut e = evidence();
    e.card_assignee = String::new();
    assert_eq!(verify_permission(&e, "docs").0, PrereqStatus::Unknown);
}

#[test]
fn an_assignee_is_checked_against_the_whole_roster() {
    let e = evidence();
    assert_eq!(verify_assignee(&e, "maya").0, PrereqStatus::Satisfied);
    assert_eq!(verify_assignee(&e, "studio").0, PrereqStatus::Satisfied);
    let (status, note) = verify_assignee(&e, "empty_desk");
    assert_eq!(status, PrereqStatus::Missing);
    assert!(note.contains("no members"), "{note}");
    assert_eq!(verify_assignee(&e, "nobody").0, PrereqStatus::Missing);
}

/// A kind this host cannot check is reported as unchecked, and it does not
/// block. The alternative — treating an unrecognised kind as missing — would
/// let a model invent a word and stop a card for a reason nobody can act on.
#[tokio::test]
async fn an_unknown_kind_is_reported_unchecked_and_does_not_block() {
    let (_home, runtime) = runtime_with(ScriptedModel::replying(CLEAN_PLAN)).await;
    let verified = verify_prerequisites(
        &runtime,
        &evidence(),
        &[claim(PrereqKind::Other, "quantum flux capacitor")],
    )
    .await;
    assert_eq!(verified.len(), 1);
    assert_eq!(verified[0].status, PrereqStatus::Unknown);
    assert!(!verified[0].status.blocks());
}

/// Duplicates and blanks are dropped, and the list is bounded — a model that
/// repeats itself must not fill the card with the same badge twelve times.
#[tokio::test]
async fn prerequisites_are_deduplicated_and_bounded() {
    let (_home, runtime) = runtime_with(ScriptedModel::replying(CLEAN_PLAN)).await;
    let mut claims = vec![
        claim(PrereqKind::Connection, "github"),
        claim(PrereqKind::Connection, "GITHUB"),
        claim(PrereqKind::Connection, "  "),
    ];
    for i in 0..40 {
        claims.push(claim(PrereqKind::Mcp, &format!("server-{i}")));
    }
    let verified = verify_prerequisites(&runtime, &evidence(), &claims).await;
    assert!(verified.len() <= MAX_PREREQUISITES, "{}", verified.len());
    assert_eq!(
        verified
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case("github"))
            .count(),
        1,
        "a repeated claim is one badge"
    );
    assert!(verified.iter().all(|p| !p.name.trim().is_empty()));
}

/// The model's `why` is kept as context but never leads: the host's finding is
/// the actionable half and the half that is true.
#[tokio::test]
async fn the_hosts_finding_leads_and_the_models_reason_follows() {
    let (_home, runtime) = runtime_with(ScriptedModel::replying(CLEAN_PLAN)).await;
    let verified = verify_prerequisites(
        &runtime,
        &evidence(),
        &[PrereqClaim {
            kind: PrereqKind::Connection,
            name: "slack".to_string(),
            why: "the announcement is posted there".to_string(),
        }],
    )
    .await;
    let note = &verified[0].note;
    assert!(note.starts_with("slack is not connected"), "{note}");
    assert!(note.contains("needed because: the announcement"), "{note}");
}
