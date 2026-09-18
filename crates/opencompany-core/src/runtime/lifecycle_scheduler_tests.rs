use std::sync::Mutex;

use async_trait::async_trait;

use super::*;
use crate::company::CompanyManifest;
use crate::error::OpenCompanyError;
use crate::ports::types::{Actor, ActorKind, CompanyEvent};
use crate::ports::users::{UserRole, UserStatus};
use crate::runtime::{FakeClock, RuntimeBuilder};
use crate::server::ops::smtp::{SmtpCredentials, SmtpSecurity};

fn tmp_home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-lifecycle-")
        .tempdir()
        .expect("tempdir")
}

fn manifest() -> CompanyManifest {
    toml::from_str("[company]\nname = \"Acme\"\n[[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n")
        .expect("valid manifest")
}

async fn seed_user(runtime: &CompanyRuntime, id: &CompanyId, uid: &str, created_at: u64) {
    runtime
        .users()
        .upsert_user(
            id,
            &UserRecord {
                id: uid.to_string(),
                email: format!("{uid}@example.test"),
                display_name: None,
                avatar: None,
                role: UserRole::Member,
                status: UserStatus::Active,
                password_hash: None,
                must_change_password: false,
                created_at_millis: created_at,
                last_seen_at_millis: None,
                updated_at_millis: created_at,
            },
        )
        .await
        .expect("seed user");
}

async fn seed_suspended_user(runtime: &CompanyRuntime, id: &CompanyId, uid: &str, created_at: u64) {
    runtime
        .users()
        .upsert_user(
            id,
            &UserRecord {
                id: uid.to_string(),
                email: format!("{uid}@example.test"),
                display_name: None,
                avatar: None,
                role: UserRole::Member,
                status: UserStatus::Suspended,
                password_hash: None,
                must_change_password: false,
                created_at_millis: created_at,
                last_seen_at_millis: None,
                updated_at_millis: created_at,
            },
        )
        .await
        .expect("seed suspended user");
}

async fn create_workflow_for(runtime: &CompanyRuntime, id: &CompanyId, uid: &str) {
    runtime
        .events()
        .append(
            id,
            CompanyEvent::WorkflowCreated {
                workflow_id: "wf-1".to_string(),
                name: "My workflow".to_string(),
                by: Some(Actor {
                    kind: ActorKind::User,
                    id: uid.to_string(),
                }),
            },
        )
        .await
        .expect("journal create");
}

/// A recording [`MailSender`] double: never touches the network, records
/// every send.
struct RecordingMail {
    sent: Mutex<Vec<OutboundEmail>>,
    fail: bool,
}

impl RecordingMail {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            sent: Mutex::new(Vec::new()),
            fail: false,
        })
    }
    fn failing() -> Arc<Self> {
        Arc::new(Self {
            sent: Mutex::new(Vec::new()),
            fail: true,
        })
    }
}

#[async_trait]
impl MailSender for RecordingMail {
    async fn send(
        &self,
        _creds: &MailCredentials,
        email: &OutboundEmail,
    ) -> Result<(), OpenCompanyError> {
        if self.fail {
            return Err(OpenCompanyError::Config("mail refused".to_string()));
        }
        self.sent.lock().unwrap().push(email.clone());
        Ok(())
    }
}

fn creds() -> MailCredentials {
    MailCredentials::Smtp(SmtpCredentials {
        host: "smtp.example.test".to_string(),
        port: 587,
        security: SmtpSecurity::Starttls,
        username: "user".to_string(),
        password: crate::ports::types::SecretValue("secret".to_string()),
        from_name: "OpenCompany".to_string(),
        from_email: "nudge@example.test".to_string(),
    })
}

const SEVEN_DAYS: u64 = SEVEN_DAYS_MILLIS;

/// A real "now" for anchoring signup/clock offsets in these tests.
///
/// `EventLog::append` always stamps `at_millis` with the real wall clock
/// (`crate::ports::now_millis`) — it is not driven by the [`FakeClock`]
/// injected into the scheduler, which only stands in for the scheduler's
/// own "what time is it" question. A test that journals a
/// `WorkflowCreated` (via [`create_workflow_for`]) therefore has to anchor
/// its `created_at_millis` / clock offsets to THIS, not to an arbitrary
/// small epoch like `0` — otherwise the journaled event's real timestamp
/// falls nowhere near the test's small-number "week-1 window" and the
/// activation query answers `false` for a create that, by the test's own
/// story, happened well inside the window.
fn real_now() -> u64 {
    crate::ports::now_millis()
}

async fn scheduler_with_mail(
    home: &std::path::Path,
    manifest: CompanyManifest,
    mail: Arc<RecordingMail>,
    cutoff_millis: u64,
    clock_millis: u64,
) -> (LifecycleScheduler, Arc<CompanyRuntime>, CompanyId) {
    let rt = Arc::new(
        RuntimeBuilder::new(home.to_path_buf(), manifest)
            .build()
            .await
            .unwrap(),
    );
    let id = rt.id().clone();
    let registry = CompanyRegistry::new();
    registry.insert(id.clone(), rt.clone());
    let clock = Arc::new(FakeClock::new(clock_millis));
    let scheduler = LifecycleScheduler::new(
        registry,
        clock,
        Some(mail as Arc<dyn MailSender>),
        Some(creds()),
        "https://acme.example".to_string(),
        cutoff_millis,
    );
    (scheduler, rt, id)
}

#[test]
fn cutoff_survives_a_simulated_restart() {
    // Before the fix, the production caller passed `now_millis()`
    // straight into `LifecycleScheduler::new` on every boot, so a
    // restart moved the cutoff forward. `load_or_create_cutoff_millis`
    // is the fix: two "boots" against the same home must agree.
    let home = tmp_home();
    let first_boot = load_or_create_cutoff_millis(home.path());
    // A real restart would also have `now_millis()` tick forward, but
    // the bug this guards is exactly that a *later* value would win if
    // re-minted — so proving equality (not merely "close") is the point.
    let second_boot = load_or_create_cutoff_millis(home.path());
    assert_eq!(
        first_boot, second_boot,
        "the cutoff must be pinned on first use and reused on every later boot, \
             not re-derived from `now` each time"
    );
}

#[test]
fn cutoff_persists_to_disk_and_survives_a_fresh_process_view() {
    // A stronger version of the above: read the persisted value back
    // with a completely independent call (as a real second process
    // would), not just a second in-process call.
    let home = tmp_home();
    let minted = load_or_create_cutoff_millis(home.path());
    let path = home.path().join("week1-nudge-cutoff");
    let on_disk: u64 = std::fs::read_to_string(&path)
        .expect("cutoff file must exist after first use")
        .trim()
        .parse()
        .expect("cutoff file must hold a plain integer");
    assert_eq!(on_disk, minted);
}

#[tokio::test]
async fn a_silent_signup_past_day_seven_gets_nudged_once() {
    let home = tmp_home();
    let mail = RecordingMail::new();
    let signup = real_now();
    let now = signup + SEVEN_DAYS; // exactly the boundary
    let (mut scheduler, rt, id) =
        scheduler_with_mail(home.path(), manifest(), mail.clone(), 0, now).await;
    seed_user(&rt, &id, "user-1", signup).await;

    assert_eq!(scheduler.tick().await, 1, "one nudge dispatched");
    assert_eq!(mail.sent.lock().unwrap().len(), 1, "email sent once");

    let feed = rt.notifications().list(&id, "user-1").await.unwrap();
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].notification.kind, NUDGE_KIND);
    assert_eq!(
        feed[0].notification.audience.as_deref(),
        Some(&["user-1".to_string()][..])
    );
}

/// PR #1878 review finding: `maybe_nudge` stamps the notification's
/// `created_at` with the real wall clock (`crate::ports::now_millis()`)
/// instead of `now` — this tick's own evaluation instant, already
/// threaded in from `self.clock.now_millis()` for exactly this reason.
/// `a_silent_signup_past_day_seven_gets_nudged_once` never catches this
/// because its `FakeClock` happens to be parked near real wall-clock time
/// (`real_now() + SEVEN_DAYS`); this test parks the fake clock far from
/// real time instead, so a wrong-clock stamp cannot hide behind the two
/// values coincidentally agreeing.
#[tokio::test]
async fn notification_created_at_uses_the_injected_clock_not_the_wall_clock() {
    let home = tmp_home();
    let mail = RecordingMail::new();
    // 1970-01-12 — nowhere near the real wall clock at test-run time.
    let fake_signup: u64 = 1_000_000_000;
    let fake_now = fake_signup + SEVEN_DAYS;
    let (mut scheduler, rt, id) =
        scheduler_with_mail(home.path(), manifest(), mail.clone(), 0, fake_now).await;
    seed_user(&rt, &id, "user-1", fake_signup).await;

    assert_eq!(scheduler.tick().await, 1, "one nudge dispatched");

    let feed = rt.notifications().list(&id, "user-1").await.unwrap();
    assert_eq!(feed.len(), 1);
    assert_eq!(
        feed[0].notification.created_at, fake_now,
        "created_at must come from the tick's own injected-clock instant, \
             not the real wall clock"
    );
}

#[tokio::test]
async fn a_second_tick_never_double_nudges() {
    let home = tmp_home();
    let mail = RecordingMail::new();
    let signup = real_now();
    let (mut scheduler, rt, id) = scheduler_with_mail(
        home.path(),
        manifest(),
        mail.clone(),
        0,
        signup + SEVEN_DAYS,
    )
    .await;
    seed_user(&rt, &id, "user-1", signup).await;

    assert_eq!(scheduler.tick().await, 1);
    assert_eq!(
        scheduler.tick().await,
        0,
        "the second tick finds the ledger row"
    );
    assert_eq!(
        mail.sent.lock().unwrap().len(),
        1,
        "exactly one email, not two"
    );

    let feed = rt.notifications().list(&id, "user-1").await.unwrap();
    assert_eq!(feed.len(), 1, "no duplicate row");
}

#[tokio::test]
async fn a_user_who_saved_shortly_after_signup_is_never_nudged() {
    let home = tmp_home();
    let mail = RecordingMail::new();
    let signup = real_now();
    let (mut scheduler, rt, id) = scheduler_with_mail(
        home.path(),
        manifest(),
        mail.clone(),
        0,
        signup + SEVEN_DAYS,
    )
    .await;
    seed_user(&rt, &id, "user-1", signup).await;
    // Journaled at the real wall clock (see `real_now`'s docs) — a few
    // milliseconds after `signup`, which is still well inside the week-1
    // window `[signup, signup + 7d)`.
    create_workflow_for(&rt, &id, "user-1").await;

    assert_eq!(
        scheduler.tick().await,
        0,
        "activated inside the window: no nudge"
    );
    assert!(mail.sent.lock().unwrap().is_empty());
    let feed = rt.notifications().list(&id, "user-1").await.unwrap();
    assert!(
        feed.is_empty(),
        "no ledger row for a user who never needed one"
    );
}

/// codex review finding (comment 3892534913): `tick` captures its single
/// process-wide `now` ONCE, at the very top, and threads that SAME value
/// into `maybe_nudge` -> `user_saved_workflow_in_week1`'s
/// `evaluated_at_millis` for every company and every user it walks. A
/// workflow saved for THIS user after `now` was captured but before this
/// user's own turn in the loop is journaled with a REAL wall-clock
/// timestamp (`EventLog::append` always stamps `crate::ports::now_millis`
/// — see `real_now`'s own doc) that lands AFTER the frozen `now`, so the
/// completeness check (`entry.at_millis <= evaluated_at_millis`) rejects
/// an event that, by the time this tick actually reaches the user, has
/// already happened. This test pins the injected clock to the exact
/// instant `create_workflow_for` is about to journal past — the fake
/// clock never advances on its own, so anything appended afterward reads
/// as "too late" under the frozen value, reproducing the tick-wide-`now`
/// staleness `8912e48d8` already fixed for `created_at` but not for this
/// eligibility check.
#[tokio::test]
async fn a_workflow_saved_during_the_tick_still_counts() {
    let home = tmp_home();
    let mail = RecordingMail::new();
    let anchor = real_now();
    let signup = anchor - SEVEN_DAYS - 5_000;
    let (mut scheduler, rt, id) =
        scheduler_with_mail(home.path(), manifest(), mail.clone(), 0, anchor).await;
    seed_user(&rt, &id, "user-1", signup).await;
    // Journaled AFTER `anchor` was captured — `EventLog::append` stamps
    // the real wall clock, which has moved on by the time this line
    // runs, so its `at_millis` is strictly greater than `anchor` (the
    // scheduler's frozen `now`).
    create_workflow_for(&rt, &id, "user-1").await;

    assert_eq!(
        scheduler.tick().await,
        0,
        "the user saved a workflow before this tick actually reached them; \
             must not be nudged just because the save landed after the tick's \
             own frozen `now` was captured"
    );
    assert!(
        mail.sent.lock().unwrap().is_empty(),
        "must not have emailed a user who already saved a workflow"
    );
}

#[tokio::test]
async fn not_yet_due_is_left_alone() {
    let home = tmp_home();
    let mail = RecordingMail::new();
    let signup = real_now();
    // Only three days old.
    let (mut scheduler, rt, id) = scheduler_with_mail(
        home.path(),
        manifest(),
        mail.clone(),
        0,
        signup + 3 * 24 * 60 * 60 * 1000,
    )
    .await;
    seed_user(&rt, &id, "user-1", signup).await;

    assert_eq!(scheduler.tick().await, 0);
    assert!(mail.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_pre_deploy_signup_is_never_nudged() {
    // The attribution-gap sidestep: cutoff is AFTER this user's signup.
    let home = tmp_home();
    let mail = RecordingMail::new();
    let signup = real_now();
    let cutoff = signup + 1_000_000;
    let (mut scheduler, rt, id) = scheduler_with_mail(
        home.path(),
        manifest(),
        mail.clone(),
        cutoff,
        cutoff + SEVEN_DAYS,
    )
    .await;
    seed_user(&rt, &id, "user-1", signup).await; // signed up before the cutoff

    assert_eq!(
        scheduler.tick().await,
        0,
        "pre-deploy signups are never nudged"
    );
    assert!(mail.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_suspended_user_past_day_seven_is_never_nudged() {
    // UserStatus::Suspended: "retained for attribution, but refused at
    // login and on every request" — a suspended user can neither read
    // the in-app row nor act on the email, so a due-for-nudge suspended
    // user must be skipped, not emailed and notified into a void.
    let home = tmp_home();
    let mail = RecordingMail::new();
    let signup = real_now();
    let (mut scheduler, rt, id) = scheduler_with_mail(
        home.path(),
        manifest(),
        mail.clone(),
        0,
        signup + SEVEN_DAYS,
    )
    .await;
    seed_suspended_user(&rt, &id, "user-1", signup).await;

    assert_eq!(
        scheduler.tick().await,
        0,
        "a suspended user must never be nudged"
    );
    assert!(mail.sent.lock().unwrap().is_empty());
    let feed = rt.notifications().list(&id, "user-1").await.unwrap();
    assert!(
        feed.is_empty(),
        "no in-app row should be filed for a suspended user either"
    );
}

#[tokio::test]
async fn past_the_lookback_window_is_left_alone() {
    let home = tmp_home();
    let mail = RecordingMail::new();
    let signup = real_now();
    let far_future = signup + SEVEN_DAYS + LOOKBACK_MILLIS + 1;
    let (mut scheduler, rt, id) =
        scheduler_with_mail(home.path(), manifest(), mail.clone(), 0, far_future).await;
    seed_user(&rt, &id, "user-1", signup).await;

    assert_eq!(
        scheduler.tick().await,
        0,
        "outside the bounded catch-up window"
    );
}

#[tokio::test]
async fn no_mail_transport_still_files_the_in_app_row() {
    let home = tmp_home();
    let rt = Arc::new(
        RuntimeBuilder::new(home.path().to_path_buf(), manifest())
            .build()
            .await
            .unwrap(),
    );
    let id = rt.id().clone();
    let registry = CompanyRegistry::new();
    registry.insert(id.clone(), rt.clone());
    let clock = Arc::new(FakeClock::new(SEVEN_DAYS));
    // No mail sender, no credentials: the `smtp`-absent degradation path.
    let mut scheduler = LifecycleScheduler::new(
        registry,
        clock,
        None,
        None,
        "https://acme.example".to_string(),
        0,
    );
    seed_user(&rt, &id, "user-1", 0).await;

    assert_eq!(
        scheduler.tick().await,
        1,
        "the in-app row is still filed with no transport wired"
    );
    let feed = rt.notifications().list(&id, "user-1").await.unwrap();
    assert_eq!(feed.len(), 1);
}

#[tokio::test]
async fn a_failing_transport_does_not_lose_the_in_app_row_or_panic() {
    let home = tmp_home();
    let mail = RecordingMail::failing();
    let (mut scheduler, rt, id) =
        scheduler_with_mail(home.path(), manifest(), mail.clone(), 0, SEVEN_DAYS).await;
    seed_user(&rt, &id, "user-1", 0).await;

    // Must not panic, and the row must still land even though the send
    // failed.
    assert_eq!(scheduler.tick().await, 1);
    let feed = rt.notifications().list(&id, "user-1").await.unwrap();
    assert_eq!(feed.len(), 1);
}
