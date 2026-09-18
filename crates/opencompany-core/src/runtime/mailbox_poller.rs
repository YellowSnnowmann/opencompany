//! Per-company IMAP poller. Structured like [`CompanyScheduler`]
//! (`crate::runtime::scheduler`): an injectable interval loop that, per tick,
//! fetches new mail via a [`MailReceiver`] and files it through the shared
//! [`file_and_notify`]. Skips while the company is asleep (scale-to-zero:
//! unseen mail waits in Stalwart and is picked up on wake).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::company::runtime::CompanyRuntime;
use crate::ports::inbox::EmailRecord;
use crate::ports::{generate_id, now_millis};
use crate::server::ops::imap::ImapCredentials;
use crate::server::ops::inbox::file_and_notify;
use crate::server::ops::mailer::MailReceiver;
use crate::server::ops::smtp::local_part;

/// Drives one company's IMAP mailbox: on each tick, fetches new mail and files
/// it into the addressed inbox via [`file_and_notify`].
pub struct MailboxPoller {
    runtime: Arc<CompanyRuntime>,
    /// When set, the runtime to file into is looked up here every tick so a
    /// runtime swap (issue #290) reaches inbound mail. `None` keeps the boot
    /// snapshot.
    registry: Option<crate::runtime::CompanyRegistry>,
    receiver: Arc<dyn MailReceiver>,
    creds: ImapCredentials,
    address: String,
    interval: Duration,
}

impl MailboxPoller {
    /// Binds a poller to `runtime`'s mailbox `address`, fetched every
    /// `interval_secs` seconds (clamped to at least 1).
    pub fn new(
        runtime: Arc<CompanyRuntime>,
        receiver: Arc<dyn MailReceiver>,
        creds: ImapCredentials,
        address: String,
        interval_secs: u64,
    ) -> Self {
        Self {
            runtime,
            registry: None,
            receiver,
            creds,
            address,
            interval: Duration::from_secs(interval_secs.max(1)),
        }
    }

    /// Issue #290: re-read `registry` for this company on every tick, instead of
    /// driving the `Arc<CompanyRuntime>` snapshotted at boot.
    ///
    /// Without this, a runtime swap never reaches this loop: it keeps driving a
    /// runtime that has been replaced and quiesced, so every tick fails. Opted
    /// into by the boot path, so existing callers keep the snapshot behaviour.
    pub fn following(mut self, registry: crate::runtime::CompanyRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    /// The runtime to drive this tick: whatever is registered now, else the
    /// snapshot taken at construction.
    fn runtime(&self) -> Arc<CompanyRuntime> {
        self.registry
            .as_ref()
            .and_then(|registry| registry.get(self.runtime.id()))
            .unwrap_or_else(|| self.runtime.clone())
    }

    /// Fetches new mail and files each message. Returns the count filed. Skips
    /// (returning `Ok(0)`) when the company is not running — scale-to-zero
    /// leaves unseen mail parked in the mailbox until the next tick after wake.
    ///
    /// Messages are fetched without being marked `\Seen` (see
    /// [`MailReceiver::fetch_new`]); only once a message is durably filed does
    /// its UID get queued for [`MailReceiver::mark_seen`], and that ack is sent
    /// after the whole batch has been attempted. A storage failure on one
    /// message is logged and skipped rather than aborting the batch — both
    /// choices mean one bad message can never mark itself (or any message
    /// after it) seen without having been filed, so nothing is silently lost.
    pub async fn tick(&self) -> crate::Result<usize> {
        let runtime = self.runtime();
        if runtime.ensure_running().await.is_err() {
            return Ok(0);
        }
        let messages = self.receiver.fetch_new(&self.creds).await?;
        let mut filed_uids = Vec::with_capacity(messages.len());
        for m in messages {
            let record = EmailRecord {
                id: generate_id(),
                inbox: local_part(&self.address),
                from_name: m.email.from_name,
                from_email: m.email.from_email,
                subject: m.email.subject,
                body: m.email.body,
                at_millis: now_millis(),
                read: false,
                outbound: false,
            };
            match file_and_notify(&runtime, &self.address, record).await {
                Ok(()) => filed_uids.push(m.uid),
                Err(err) => {
                    tracing::warn!(
                        company = %self.runtime.id(),
                        uid = m.uid,
                        %err,
                        "failed to file inbound email; leaving unseen for retry"
                    );
                }
            }
        }
        let filed = filed_uids.len();
        if !filed_uids.is_empty()
            && let Err(err) = self.receiver.mark_seen(&self.creds, &filed_uids).await
        {
            tracing::warn!(company = %self.runtime.id(), %err, "failed to mark filed emails seen");
        }
        Ok(filed)
    }

    /// Spawns the interval loop; stops on `shutdown`. Mirrors
    /// [`CompanyScheduler::spawn`](crate::runtime::scheduler::CompanyScheduler::spawn).
    pub fn spawn(self, shutdown: Arc<Notify>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.notified() => break,
                    _ = tokio::time::sleep(self.interval) => {
                        if let Err(err) = self.tick().await {
                            tracing::warn!(company = %self.runtime.id(), %err, "mailbox poll failed");
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
#[path = "mailbox_poller_tests.rs"]
mod tests;
