//! [`LifecycleScheduler`]: the week-1 "save your first workflow" nudge
//! (issue #1845).
//!
//! Part of the OpenCompany pre-launch retention strategy's fix for the 14%
//! churn cause "never saved a workflow": a signup who never saves one has
//! nothing to come back to. This scheduler is what turns that observation
//! into an actual nudge — one email plus one durable in-app notification, at
//! most once per user, at the day-7 boundary after signup.
//!
//! # Shaped like the existing schedulers, on purpose
//!
//! Modeled on [`WorkflowScheduler`](super::WorkflowScheduler) and
//! [`CompanyScheduler`](super::scheduler::CompanyScheduler): one
//! process-global task (not one per company — a hosted tenant can be
//! registered after boot, same reasoning as `WorkflowScheduler`'s own docs),
//! an injectable [`Clock`], the same tick-then-sleep loop shape. What differs
//! is what "due" means: there is no cron expression to match. [`Self::tick`]
//! walks every registered company's users once and, for each user past their
//! day-7 boundary, decides — once — whether they earned a nudge.
//!
//! # The idempotency ledger IS the notification row
//!
//! There is no separate "who have we nudged" table. [`Self::tick`] asks
//! [`NotificationStore::list`](crate::ports::notifications::NotificationStore::list)
//! for this user's own notifications and checks whether one already carries
//! [`week1_nudge::NUDGE_KIND`](crate::company::week1_nudge::NUDGE_KIND); if
//! so, the decision was already made (nudged, or the row would not exist) and
//! this tick does nothing further for them. This is a **best-effort**
//! check-then-act, not a durable claim — unlike
//! [`ScheduleFireStore::claim_fire`](crate::ports::ScheduleFireStore::claim_fire),
//! `NotificationStore` has no compare-and-swap primitive. Two REPLICAS
//! ticking at the same instant for the same user could theoretically both
//! pass the check and both file a row (and, worst case, both send an email).
//! Accepted for v1: a single duplicate nudge is a mildly annoying email, not
//! a correctness defect, and this is the same bar the issue's own idempotency
//! ask sets ("use a `NotificationStore` row as the idempotency ledger") — a
//! durable cross-replica claim would need a new store primitive this issue
//! does not scope.
//!
//! # The deploy cutoff sidesteps the attribution gap
//!
//! `WorkflowCreated.by` was `None` on every create path before issue #1843.
//! A user who signed up before this scheduler's process started may have
//! saved a workflow through one of those unattributed paths, and there is no
//! way to tell that user apart from one who truly never saved anything — so
//! this scheduler never nudges them at all, rather than risk a false-positive
//! nag. [`Self::cutoff_millis`] is stamped once — the first time
//! [`load_or_create_cutoff_millis`] ever runs against a given data root, not
//! re-stamped on every boot — and only users created at or after it are ever
//! considered. It has to be pinned rather than re-derived from "now" at each
//! boot: a deploy restarts the process, and re-stamping would move the
//! cutoff forward on every restart, permanently disqualifying anyone who
//! signed up in between. See [`crate::company::week1_nudge`]'s module docs
//! for the same attribution gap from the query's side.
//!
//! # Email is primary, in-app is the substrate that always lands
//!
//! The in-app [`Notification`] row is written **first**, unconditionally —
//! it is both the idempotency ledger and the durable half issue #1845 scope
//! item 4 asks for. Email is attempted only after that row exists, exactly
//! the transport [`crate::server::users::routes::deliver_code`] sends the
//! login link through: host-level [`MailSender`] + [`MailCredentials`]
//! (`OPENCOMPANY_MAIL_*`, wired only when the binary is built with the
//! `smtp` feature). Missing either — no feature, no host mail configured, or
//! a user whose login identity carries no mailbox (wallet/local auth) —
//! degrades LOUDLY (one `info!` line naming which reason) rather than
//! panicking or silently trying to send. Email failing at the transport
//! (`MailSender::send` returning `Err`) is logged and swallowed the same way
//! `deliver_code`'s login mail is: the in-app row already landed, so the
//! nudge is not lost, only quieter than it should have been.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::company::runtime::CompanyRuntime;
use crate::company::week1_nudge::{NUDGE_KIND, SEVEN_DAYS_MILLIS, user_saved_workflow_in_week1};
use crate::ports::notifications::{Notification, Subject, SubjectKind};
use crate::ports::now_millis;
use crate::ports::types::CompanyId;
use crate::ports::users::{UserRecord, UserStatus};
use crate::runtime::CompanyRegistry;
use crate::runtime::scheduler::Clock;
use crate::server::ops::mailer::{MailCredentials, MailSender, OutboundEmail};

/// How far past the day-7 boundary a tick still attempts a nudge.
///
/// Bounds the daily scan the same way
/// [`CATCHUP_WINDOW_MINUTES`](super::scheduler::CATCHUP_WINDOW_MINUTES)
/// bounds the cron schedulers' restart catch-up: without a ceiling, a user
/// who is neither nudged nor activated would be re-evaluated on every tick
/// for the rest of the process's life. Fourteen days — twice the nudge
/// window itself — is generous enough that a daily tick can never miss the
/// boundary, while still being a bound.
const LOOKBACK_MILLIS: u64 = 14 * 24 * 60 * 60 * 1000;

/// How often [`LifecycleScheduler::spawn`] ticks in production. A daily cron
/// per the issue's own spec — this is a day-granularity decision ("has a
/// week passed"), not a minute-granularity one, so it needs none of the
/// per-minute matching the cron schedulers do.
const TICK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The file under the host's data root that pins the deploy cutoff.
const CUTOFF_FILE: &str = "week1-nudge-cutoff";

/// Reads the deploy cutoff for `home`, minting and persisting one on first
/// use.
///
/// The module docs' "deploy cutoff" section explains why a cutoff exists;
/// this is why it now survives a restart. Before this, the production caller
/// passed `now_millis()` straight into [`LifecycleScheduler::new`] on every
/// boot, so a restart moved the cutoff forward to that boot's instant —  and
/// [`LifecycleScheduler::tick`] treats "signed up before the cutoff" as
/// unanswerable and never nudges that user again, for the rest of their
/// account's life. A deploy restarting mid-week for an already-eligible
/// signup therefore permanently disqualified them. Pinning the value the
/// first time this scheduler ever runs against a given `home`, the same way
/// [`crate::app::instance::load_or_create`] pins the host's instance id,
/// makes every later boot reuse it instead of moving the goalposts.
///
/// Never fails: an unwritable `home` mints a fresh value for this process and
/// logs the degradation rather than aborting boot over a nudge timestamp —
/// the same trade-off `instance::load_or_create` makes for the same reason.
pub fn load_or_create_cutoff_millis(home: &std::path::Path) -> u64 {
    let path = home.join(CUTOFF_FILE);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if let Ok(parsed) = existing.trim().parse::<u64>() {
            return parsed;
        }
        tracing::warn!(
            path = %path.display(),
            "week1-nudge-cutoff file is not a well-formed timestamp; minting a replacement"
        );
    }
    let minted = now_millis();
    if let Err(error) = std::fs::write(&path, minted.to_string()) {
        tracing::warn!(
            path = %path.display(),
            %error,
            "could not persist the week-1 nudge cutoff; every restart before this is fixed \
             will move the cutoff forward and can permanently exclude eligible signups"
        );
    }
    minted
}

/// The [`Subject::id`] every week-1 nudge notification carries.
///
/// Not a real workflow id — there is no workflow yet, which is the entire
/// point of the nudge — but [`Subject`] is a closed `{kind, id}` pair and
/// [`SubjectKind::Workflow`] is the closest existing tag for "this is about
/// creating one". A fixed constant rather than the user or company id:
/// [`Notification::audience`] and the company scope already carry those, so
/// reusing either here would make `id` redundant with a field the row
/// already has.
const NUDGE_SUBJECT_ID: &str = "week1-first-workflow";

/// Drives the week-1 "save your first workflow" nudge across every
/// registered company. See the module docs for the shape and the guarantees.
pub struct LifecycleScheduler {
    registry: CompanyRegistry,
    clock: Arc<dyn Clock>,
    /// Host-level mail sender (real under the `smtp` feature; `None` in the
    /// default build or when no host mail is configured). Absence degrades
    /// the nudge to in-app only — see the module docs.
    mail: Option<Arc<dyn MailSender>>,
    /// Host-level mail credentials (`OPENCOMPANY_MAIL_*`) — platform mail,
    /// the same scope the login-link email is sent under, never a company's
    /// own SMTP secret.
    mail_credentials: Option<MailCredentials>,
    /// The base URL the nudge email's link is built against
    /// ([`AppConfig::host_base_url`](crate::app::types::AppConfig::host_base_url)).
    host_base_url: String,
    /// Only a user created at or after this instant is ever considered — see
    /// the module docs' "deploy cutoff" section.
    cutoff_millis: u64,
}

impl LifecycleScheduler {
    /// Builds a scheduler over every company in `registry`, driven by
    /// `clock`. `cutoff_millis` is normally
    /// [`load_or_create_cutoff_millis`] read once at process boot — a value
    /// pinned to the host's data root the first time this scheduler ever
    /// runs, not `clock.now_millis()` recomputed on every boot (see that
    /// function's docs for why); tests pass a fixed value so a seeded user
    /// can be placed on either side of it.
    pub fn new(
        registry: CompanyRegistry,
        clock: Arc<dyn Clock>,
        mail: Option<Arc<dyn MailSender>>,
        mail_credentials: Option<MailCredentials>,
        host_base_url: String,
        cutoff_millis: u64,
    ) -> Self {
        Self {
            registry,
            clock,
            mail,
            mail_credentials,
            host_base_url,
            cutoff_millis,
        }
    }

    /// Runs one tick: for every registered, running company, walks its
    /// users and nudges each one that is due. Returns how many nudges were
    /// dispatched.
    ///
    /// A company whose `ensure_running` guard rejects (paused or archived)
    /// contributes nothing this tick — the same skip
    /// [`WorkflowScheduler::tick`](super::WorkflowScheduler::tick) makes, so
    /// a paused company's users simply wait for the next tick after resume
    /// rather than being nudged while nobody would see it land.
    pub async fn tick(&mut self) -> usize {
        let now = self.clock.now_millis();
        let mut nudged = 0;
        for company in self.registry.list() {
            let Some(runtime) = self.registry.get(&company) else {
                continue; // removed between listing and lookup
            };
            if runtime.ensure_running().await.is_err() {
                continue;
            }
            let users = match runtime.users().list_users(&company).await {
                Ok(users) => users,
                Err(err) => {
                    tracing::warn!(
                        %company,
                        %err,
                        "lifecycle scheduler: could not list users; skipping this company this tick"
                    );
                    continue;
                }
            };
            for user in users {
                if user.status != UserStatus::Active {
                    // Retained for attribution but refused at login and on
                    // every request (`UserStatus::Suspended`'s own docs) —
                    // the same bar `workflows::delivery`'s admin-recipient
                    // filter and `server::ops::mentions`'s advertised-user
                    // filter both hold notification recipients to. A
                    // suspended user can neither read the in-app row nor
                    // act on the email, so nudging them is pure noise.
                    continue;
                }
                if user.created_at_millis < self.cutoff_millis {
                    // Pre-deploy signup: the attribution gap makes "never
                    // saved a workflow" unanswerable for them. Never nudge.
                    continue;
                }
                let elapsed = now.saturating_sub(user.created_at_millis);
                if elapsed < SEVEN_DAYS_MILLIS {
                    continue; // not due yet
                }
                if elapsed >= SEVEN_DAYS_MILLIS + LOOKBACK_MILLIS {
                    continue; // past the bounded catch-up window; see LOOKBACK_MILLIS
                }
                match self.maybe_nudge(&company, &runtime, &user, now).await {
                    Ok(true) => nudged += 1,
                    Ok(false) => {}
                    Err(err) => {
                        tracing::warn!(
                            %company,
                            user = %user.id,
                            %err,
                            "lifecycle scheduler: week-1 nudge check failed for this user"
                        );
                    }
                }
            }
        }
        nudged
    }

    /// Decides and, if owed, dispatches one user's week-1 nudge. Returns
    /// whether a nudge was actually filed.
    ///
    /// `now` is this tick's own evaluation instant (from [`Self::tick`]'s
    /// `self.clock.now_millis()`) — passed through to
    /// [`user_saved_workflow_in_week1`] so a save that lands after the
    /// nominal week-1 window but before this tick actually runs still
    /// suppresses the nudge; see that function's docs.
    async fn maybe_nudge(
        &self,
        company: &CompanyId,
        runtime: &CompanyRuntime,
        user: &UserRecord,
        now: u64,
    ) -> crate::Result<bool> {
        // The idempotency ledger: has this user already been nudged?
        let existing = runtime.notifications().list(company, &user.id).await?;
        if existing
            .iter()
            .any(|view| view.notification.kind == NUDGE_KIND)
        {
            return Ok(false);
        }
        // codex review finding (comment 3892534913): `now` is `tick`'s
        // single process-wide instant, captured once at the top and shared
        // across every company and user this loop walks — deliberately so
        // for `elapsed`'s day-7 boundary math and this notification's own
        // `created_at` (see `8912e48d8`, which fixed the SAME staleness for
        // `created_at` specifically). But `EventLog::append` always stamps a
        // journaled event with the real wall clock (`crate::ports::now_millis`,
        // never `self.clock` — see this test module's own `real_now` doc), so
        // a workflow saved by this user after `now` was captured but before
        // this iteration reaches them journals with a timestamp LATER than
        // the frozen `now`, and the completeness check below would reject an
        // event that, by the actual instant this code runs, has already
        // happened. A freshly-read real clock — not `now`, not `self.clock`,
        // which in tests is a `FakeClock` decoupled from the journal's real
        // timestamps entirely — is what this specific comparison needs: it is
        // checking against journal timestamps that are always real time,
        // regardless of what clock the scheduler itself is running on.
        if user_saved_workflow_in_week1(
            company,
            runtime.events(),
            &user.id,
            user.created_at_millis,
            crate::ports::now_millis(),
        )
        .await?
        {
            return Ok(false); // earned activation by the time this tick ran: no nudge owed
        }

        // The in-app row lands FIRST and unconditionally — it is both the
        // ledger a later tick reads and scope item 4's own substrate. Email
        // is attempted only once this exists.
        let notification = Notification {
            id: crate::ports::generate_id(),
            kind: NUDGE_KIND.to_string(),
            subject: Subject {
                kind: SubjectKind::Workflow,
                id: NUDGE_SUBJECT_ID.to_string(),
            },
            created_at: now,
            title: "Save your first workflow".to_string(),
            audience: Some(vec![user.id.clone()]),
            context: None,
        };
        runtime
            .notifications()
            .append(company, &notification)
            .await?;

        self.send_email(company, runtime, user).await;

        Ok(true)
    }

    /// Best-effort email delivery for one nudge. Never returns an error —
    /// every refusal reason (no transport, no mailbox, transport failure) is
    /// logged and swallowed, because the in-app row filed by
    /// [`Self::maybe_nudge`] already makes the nudge durable; email is the
    /// primary *reach*, not the primary *record*.
    async fn send_email(&self, company: &CompanyId, runtime: &CompanyRuntime, user: &UserRecord) {
        let (Some(sender), Some(creds)) = (&self.mail, &self.mail_credentials) else {
            // Loud, per the issue's own "if smtp absent, degrade LOUDLY"
            // instruction — not merely a debug line nobody sees.
            tracing::info!(
                %company,
                user = %user.id,
                "lifecycle scheduler: no host mail transport wired (OPENCOMPANY_MAIL_* / \
                 `smtp` feature); week-1 nudge for this user stays in-app only"
            );
            return;
        };
        let Some(mailbox) = user.mailbox() else {
            tracing::info!(
                %company,
                user = %user.id,
                "lifecycle scheduler: user's login identity has no mailbox (wallet/local \
                 auth); week-1 nudge stays in-app only"
            );
            return;
        };
        let company_name = match runtime.store().load(company).await {
            Ok(Some(record)) => record.manifest.company.name,
            _ => company.as_ref().to_string(),
        };
        // Same shape as the login link (`deliver_code` /
        // `server::users::admin`'s invite mail): land on sign-in, carrying the
        // console's own `#/workflows` fragment so a signed-in click goes
        // straight to the empty state's "Create a workflow" CTA rather than
        // wherever the console last had them.
        let link = format!(
            "{}/login?company={}#/workflows",
            self.host_base_url,
            company.as_ref()
        );
        let mail = OutboundEmail {
            to: mailbox,
            subject: format!("Save your first workflow in {company_name}"),
            body: format!(
                "You joined {company_name} about a week ago and haven't saved a workflow \
                 yet — that's the thing {company_name} actually runs on a schedule or on \
                 demand, once you've described it.\n\n\
                 Describe one in plain words and the copilot drafts the graph for you to \
                 review:\n\n{link}\n\n\
                 If you've already got one you're happy with, there's nothing else to do — \
                 you won't hear about this again.\n"
            ),
        };
        if let Err(err) = sender.send(creds, &mail).await {
            // The error, never the message: consistent with `deliver_code`'s
            // own login mail — the address itself must never reach a log
            // line via an interpolated `detail`.
            tracing::warn!(
                %company,
                user = %user.id,
                "lifecycle scheduler: week-1 nudge email failed: {err}"
            );
        }
    }

    /// Spawns a background task that ticks once immediately, then every
    /// [`TICK_INTERVAL`], until `shutdown` is notified.
    ///
    /// Ticking once before the loop (rather than waiting a full day for the
    /// first pass) matters here specifically: at boot there may already be
    /// users well past their day-7 boundary, and making every one of them
    /// wait up to 24h for a scheduler that only just started would be its
    /// own small defect.
    pub fn spawn(mut self, shutdown: Arc<Notify>) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.tick().await;
            let notified = shutdown.notified();
            tokio::pin!(notified);
            loop {
                tokio::select! {
                    _ = &mut notified => break,
                    _ = tokio::time::sleep(TICK_INTERVAL) => {
                        self.tick().await;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "lifecycle_scheduler_tests.rs"]
mod tests;
