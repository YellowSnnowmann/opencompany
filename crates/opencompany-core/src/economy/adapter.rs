//! [`TinyplaceEconomy`]: the [`AgentEconomy`] adapter over a [`TinyplaceClient`].
//!
//! This is the commerce brain of the tiny.place seam. It:
//!
//! - claims a `@handle` only after the operator opts in (the `going_public`
//!   flag standing in for the Identity approval checkpoint) and funding covers
//!   the registry fee — catching the `402` challenge, budget-checking, then
//!   completing the paid registration;
//! - publishes the Agent Card, parking it in the [`Outbox`] for the attached
//!   replayer when tiny.place is unreachable — and erroring when no replayer is
//!   attached, because then nothing would ever send it;
//! - sends outbound A2A tasks, paying an x402 challenge under budget and
//!   journaling the spend, and **failing** rather than queuing when offline;
//! - quotes and pays firm requirements, **failing closed** the instant a
//!   payment would exceed either the caller's [`BudgetScope`] or the company's
//!   monthly ceiling, and journaling every in/out movement to the ledger.
//!
//! Every spend path is budget-fail-closed and ledger-journaled, so budget and
//! audit are self-contained and unit-testable offline against
//! [`MockTinyplaceClient`](super::client::MockTinyplaceClient).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use crate::Result;
use crate::economy::client::{JsonRpcRequest, PaidOutcome, TinyplaceClient, now_secs};
use crate::economy::outbox::{Outbox, OutboxAction};
use crate::economy::signer::LocalSigner;
use crate::economy::x402::{self, X402Challenge};
use crate::error::OpenCompanyError;
use crate::ports::AgentEconomy;
use crate::ports::store::CompanyStore;
use crate::ports::types::{
    A2aTask, A2aTaskHandle, AgentAddr, AgentCard, BudgetScope, CompanyId, CompanyIdentity,
    LedgerEntry, PaymentReceipt, PaymentRequirement, Quote, RegistrationState,
};
use crate::ports::{generate_id, now_millis};

/// The settlement asset used when a firm quote is paid.
const PAY_ASSET: &str = "USDC";
/// The settlement network used when a firm quote is paid.
const PAY_NETWORK: &str = "solana";

/// How often an attached replayer retries the queued Agent Card.
///
/// There is no connectivity signal in the [`TinyplaceClient`] seam — no
/// reconnect event, no health stream — so the first tick after the network
/// returns *is* the reconnect drain. Inventing a listener to sharpen that is out
/// of scope for #454; a card that is at most one interval stale is the whole
/// point of a degrade path.
pub const OUTBOX_REPLAY_INTERVAL: Duration = Duration::from_secs(30);

/// The [`AgentEconomy`] over a [`TinyplaceClient`].
pub struct TinyplaceEconomy {
    client: Arc<dyn TinyplaceClient>,
    signer: Arc<LocalSigner>,
    store: Arc<dyn CompanyStore>,
    company: CompanyId,
    monthly_cap: Option<f64>,
    going_public: bool,
    outbox: Arc<Outbox>,
    /// Whether a background replayer is attached to this economy — i.e. whether
    /// anything will ever send what [`Self::publish_card`] queues.
    ///
    /// Set by exactly one function, [`spawn_outbox_replayer`], and never
    /// unset. That is what lets the offline publish path answer *"is my degrade
    /// honest?"* instead of assuming it. A constructor that forgets to attach a
    /// replayer leaves this `false`, and the publish then errors in the caller's
    /// face rather than dropping the card in silence.
    replayer: AtomicBool,
}

impl TinyplaceEconomy {
    /// Builds an economy for `company`. `going_public` starts `false`: the
    /// adapter never spends the master key on registration until the operator
    /// opts in via [`Self::going_public`].
    pub fn new(
        client: Arc<dyn TinyplaceClient>,
        signer: Arc<LocalSigner>,
        store: Arc<dyn CompanyStore>,
        company: CompanyId,
        monthly_cap: Option<f64>,
    ) -> Self {
        Self {
            client,
            signer,
            store,
            company,
            monthly_cap,
            going_public: false,
            outbox: Arc::new(Outbox::new()),
            replayer: AtomicBool::new(false),
        }
    }

    /// Sets the going-public flag. `true` encodes the Identity approval
    /// checkpoint plus funding: only then will [`Self::ensure_registered`]
    /// claim (and pay for) the `@handle`.
    pub fn going_public(mut self, approved: bool) -> Self {
        self.going_public = approved;
        self
    }

    /// The outbox holding the card deferred while tiny.place was unreachable.
    pub fn outbox(&self) -> &Arc<Outbox> {
        &self.outbox
    }

    /// Whether a background replayer is attached — see [`spawn_outbox_replayer`].
    pub fn has_replayer(&self) -> bool {
        self.replayer.load(Ordering::SeqCst)
    }

    /// Replays the queued Agent Card, if there is one.
    ///
    /// Empty outbox is a silent success — the replayer calls this on a timer, so
    /// "nothing to do" is the normal case. On continued unreachability the card
    /// goes back into the slot (unless a newer one landed meanwhile: see
    /// [`Outbox::requeue`]) and the error is returned, so the next tick tries
    /// again.
    ///
    /// A **rejection** — a `4xx` the server actually answered — is not requeued.
    /// Retrying it every interval forever would be a hot loop against a card the
    /// directory has already refused; the error is surfaced, and the next real
    /// `publish_card` queues a fresh card if one is warranted.
    pub async fn flush_outbox(&self) -> Result<()> {
        let Some(OutboxAction::PublishCard(card)) = self.outbox.take() else {
            return Ok(());
        };
        match self.client.put_agent(&self.signer.agent_id(), &card).await {
            Ok(()) => Ok(()),
            Err(err) => {
                if matches!(&err, OpenCompanyError::Tinyplace { code, .. } if code == "unreachable")
                {
                    self.outbox.requeue(OutboxAction::PublishCard(card));
                }
                Err(err)
            }
        }
    }

    /// Journals a negative (outflow) ledger movement.
    async fn ledger_out(&self, kind: &str, amount: f64, memo: String) -> Result<()> {
        self.store
            .append_ledger(
                &self.company,
                LedgerEntry {
                    at_millis: now_millis(),
                    kind: kind.to_string(),
                    amount_usd: -amount,
                    memo,
                },
            )
            .await
    }

    /// The remaining monthly budget: the cap minus the sum of ledger outflows.
    /// Fails open to `+∞` when no cap is set or no record exists yet.
    async fn remaining_budget(&self) -> Result<f64> {
        let Some(cap) = self.monthly_cap else {
            return Ok(f64::INFINITY);
        };
        let spent: f64 = match self.store.load(&self.company).await? {
            Some(record) => record
                .ledger
                .iter()
                .filter(|entry| entry.amount_usd < 0.0)
                .map(|entry| -entry.amount_usd)
                .sum(),
            None => 0.0,
        };
        Ok(cap - spent)
    }

    /// Parses a decimal challenge amount, rejecting a malformed string.
    fn parse_amount(raw: &str) -> Result<f64> {
        raw.trim().parse::<f64>().map_err(|_| {
            OpenCompanyError::tinyplace(
                "bad_amount",
                format!("challenge amount `{raw}` is not a number"),
            )
        })
    }

    /// Enforces both the monthly ceiling for `amount`, returning
    /// [`OpenCompanyError::BudgetExceeded`] when it would be crossed.
    async fn enforce_monthly(&self, amount: f64, what: &str) -> Result<()> {
        let remaining = self.remaining_budget().await?;
        if amount > remaining {
            return Err(OpenCompanyError::BudgetExceeded(format!(
                "{what} needs ${amount:.2} but only ${remaining:.2} remains this month"
            )));
        }
        Ok(())
    }
}

impl std::fmt::Debug for TinyplaceEconomy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TinyplaceEconomy")
            .field("company", &self.company)
            .field("agent_id", &self.signer.agent_id())
            .field("monthly_cap", &self.monthly_cap)
            .field("going_public", &self.going_public)
            .field("outbox_len", &self.outbox.len())
            .field("replayer", &self.has_replayer())
            .finish_non_exhaustive()
    }
}

/// Attaches the background Agent-Card replayer to a freshly built economy, and
/// with it the promise that a queued card will actually be sent (issue #454).
///
/// **This is the only thing that sets the "a replayer is attached" flag**, which
/// is what makes the flag mean what it says. [`TinyplaceEconomy::publish_card`]
/// reads it to decide whether an offline publish may honestly report success, so
/// a construction path that does not come through here degrades to a visible
/// error instead of a silent drop. Call it on the concrete economy **before** it
/// is type-erased into `Arc<dyn AgentEconomy>` — after that the flush surface is
/// unreachable, which is exactly how the queue ended up with no drain.
///
/// # Why a weak reference
///
/// The task holds a [`Weak`](std::sync::Weak), not an `Arc`, and exits the first
/// time the upgrade fails. A runtime rebuild (issue #290) constructs a fresh
/// economy and drops the old one; with a strong reference the old economy — and
/// its timer — would live forever, so every rebuild would leak another flusher
/// publishing an ever-staler card over the live one. Weak makes the replayer's
/// lifetime exactly the economy's.
///
/// `every` is the retry period; production passes [`OUTBOX_REPLAY_INTERVAL`].
/// It is a parameter rather than a constant read inside so a test can exercise
/// *this* function — the real attachment path, flag and all — without waiting
/// out a production interval.
pub fn spawn_outbox_replayer(economy: &Arc<TinyplaceEconomy>, every: Duration) {
    economy.replayer.store(true, Ordering::SeqCst);
    let company = economy.company.clone();
    let weak = Arc::downgrade(economy);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        loop {
            ticker.tick().await;
            let Some(economy) = weak.upgrade() else {
                // The economy is gone (a rebuild, or shutdown): so is its queue.
                break;
            };
            if economy.outbox.is_empty() {
                continue;
            }
            match economy.flush_outbox().await {
                Ok(()) => tracing::info!(
                    company = %company,
                    "tiny.place: replayed the queued Agent Card; the directory entry is current"
                ),
                Err(err) => tracing::warn!(
                    company = %company,
                    error = %err,
                    "tiny.place: replaying the queued Agent Card failed"
                ),
            }
        }
    });
}

#[async_trait]
impl AgentEconomy for TinyplaceEconomy {
    async fn ensure_registered(&self, identity: &CompanyIdentity) -> Result<RegistrationState> {
        // If the handle already resolves to us, we are done.
        if let Ok(addr) = self.client.resolve(&identity.handle).await
            && addr.0 == self.signer.agent_id()
        {
            return Ok(RegistrationState::Registered { addr });
        }

        // A private company never spends its master key at boot.
        if !self.going_public {
            return Ok(RegistrationState::Unregistered);
        }

        match self.client.register_name(&identity.handle).await? {
            PaidOutcome::Done(receipt) => Ok(RegistrationState::Registered { addr: receipt.addr }),
            PaidOutcome::PaymentRequired(challenge) => {
                let fee = Self::parse_amount(&challenge.amount)?;
                self.enforce_monthly(fee, "registering a handle").await?;
                let auth = x402::authorize(&self.signer, &challenge, now_secs());
                let receipt = self
                    .client
                    .register_name_paid(&identity.handle, &auth)
                    .await?;
                self.ledger_out(
                    "registry.fee",
                    fee,
                    format!(
                        "claimed @{} (signer {})",
                        identity.handle,
                        self.signer.agent_id()
                    ),
                )
                .await?;
                Ok(RegistrationState::Registered { addr: receipt.addr })
            }
        }
    }

    /// Publishes the Agent Card, degrading to the outbox when tiny.place is
    /// unreachable — **but only when something will actually drain it**
    /// (issue #454).
    ///
    /// This is the inversion the whole issue turns on. The old arm queued
    /// unconditionally and returned `Ok(())`, and nothing in the tree ever
    /// drained that queue: `drain()`'s only caller lived in its own test module,
    /// and the concrete economy is type-erased behind [`AgentEconomy`] the moment
    /// it is built, so no production code could reach it even in principle. Every
    /// card published during an outage was therefore dropped while the caller was
    /// told it had succeeded.
    ///
    /// So the `Ok(())` is now *earned*: it is returned only when
    /// [`spawn_outbox_replayer`] has attached a replayer, which makes the
    /// sentence "queued, and it will go out" true. With no replayer the original
    /// unreachable error propagates and nothing is queued — a constructor path
    /// written later that forgets to attach one inherits a visible error rather
    /// than the silent drop this issue is about. Fail-safe by construction, the
    /// same direction as `PublishDestination::Unclaimed` in the harness publish
    /// queue (issue #445).
    async fn publish_card(&self, _identity: &CompanyIdentity, card: &AgentCard) -> Result<()> {
        match self.client.put_agent(&self.signer.agent_id(), card).await {
            Ok(()) => Ok(()),
            Err(OpenCompanyError::Tinyplace { code, message }) if code == "unreachable" => {
                if !self.has_replayer() {
                    return Err(OpenCompanyError::tinyplace("unreachable", message));
                }
                // Offline, and a replayer is listening: park the newest card and
                // let it go stale rather than erroring.
                self.outbox.enqueue(OutboxAction::PublishCard(card.clone()));
                tracing::warn!(
                    company = %self.company,
                    handle = %card.handle,
                    "tiny.place is unreachable; the Agent Card is queued for replay and the \
                     directory entry is stale until it lands"
                );
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Sends an outbound A2A task, and **fails** when tiny.place is unreachable
    /// rather than deferring it.
    ///
    /// This is the deliberate other half of [`publish_card`](Self::publish_card)'s
    /// contract, and the split is about money (issue #454). A card publish
    /// degrades and replays: it is idempotent, it costs nothing, and the newest
    /// card is always the right one to send whenever the network returns. A task
    /// send does neither. It may carry an x402 payment, the budget scope that
    /// authorised it belongs to the caller's cycle and is gone by flush time, and
    /// a replay that lands after the caller already retried is a double-send —
    /// which here means a double-spend. So the error goes back to the caller, who
    /// is the only party holding the context to decide whether to retry it.
    ///
    /// Before #454 this arm did *both*: it pushed a copy onto the outbox **and**
    /// returned the error. Nothing ever drained that copy, so it was pure
    /// unreachable state; had anything drained it, it would have been a
    /// background double-send with no budget behind it.
    async fn send_a2a_task(&self, to: &AgentAddr, task: A2aTask) -> Result<A2aTaskHandle> {
        let params = serde_json::json!({
            "id": generate_id(),
            "skill": task.skill,
            "input": task.input,
        });
        let rpc = JsonRpcRequest::new("tasks/send", params);

        match self.client.send_task(&to.0, rpc.clone()).await {
            Ok(PaidOutcome::Done(response)) => Ok(handle_from_response(&response, &rpc.id)),
            Ok(PaidOutcome::PaymentRequired(challenge)) => {
                let amount = Self::parse_amount(&challenge.amount)?;
                self.enforce_monthly(amount, "hiring").await?;
                let auth = x402::authorize(&self.signer, &challenge, now_secs());
                let response = self
                    .client
                    .send_task_paid(&to.0, rpc.clone(), &auth)
                    .await?;
                self.ledger_out(
                    "x402.out",
                    amount,
                    format!(
                        "a2a tasks/send to {} for `{}` (signer {})",
                        to.0,
                        task.skill,
                        self.signer.agent_id()
                    ),
                )
                .await?;
                Ok(handle_from_response(&response, &rpc.id))
            }
            Err(OpenCompanyError::Tinyplace { code, message }) if code == "unreachable" => {
                // Offline: surface the error and queue nothing. The caller owns
                // the retry decision for a task that may cost money.
                Err(OpenCompanyError::tinyplace("unreachable", message))
            }
            Err(err) => Err(err),
        }
    }

    async fn quote(&self, requirement: &PaymentRequirement) -> Result<Quote> {
        // A firm quote equal to the requirement; no wire round-trip needed.
        Ok(Quote {
            quote_id: generate_id(),
            to: requirement.to.clone(),
            amount_usd: requirement.amount_usd,
        })
    }

    async fn pay(&self, quote: &Quote, budget: &BudgetScope) -> Result<PaymentReceipt> {
        // Fail closed against the caller's scope first — before any wire call.
        if quote.amount_usd > budget.remaining_usd {
            return Err(OpenCompanyError::BudgetExceeded(format!(
                "paying ${:.2} exceeds the {} scope's ${:.2}",
                quote.amount_usd, budget.label, budget.remaining_usd
            )));
        }
        // Then clamp against the monthly ceiling.
        self.enforce_monthly(quote.amount_usd, "paying").await?;

        let challenge = X402Challenge {
            amount: format!("{:.2}", quote.amount_usd),
            recipient: quote.to.0.clone(),
            asset: PAY_ASSET.to_string(),
            network: PAY_NETWORK.to_string(),
        };
        let auth = x402::authorize(&self.signer, &challenge, now_secs());

        let verified = self.client.payments_verify(&auth).await?;
        if !verified.ok {
            return Err(OpenCompanyError::tinyplace(
                "verify_failed",
                verified
                    .reason
                    .unwrap_or_else(|| "payment authorization did not verify".to_string()),
            ));
        }
        self.client.payments_settle(&auth).await?;

        self.ledger_out(
            "x402.out",
            quote.amount_usd,
            format!("paid quote {} to {}", quote.quote_id, quote.to.0),
        )
        .await?;

        Ok(PaymentReceipt {
            quote_id: quote.quote_id.clone(),
            amount_usd: quote.amount_usd,
            at_millis: now_millis(),
        })
    }
}

/// Extracts an [`A2aTaskHandle`] from a response, falling back to the request id.
fn handle_from_response(
    response: &crate::economy::client::JsonRpcResponse,
    fallback_id: &str,
) -> A2aTaskHandle {
    let id = response
        .result
        .as_ref()
        .and_then(|r| r.get("id").or_else(|| r.get("taskId")))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| fallback_id.to_string());
    A2aTaskHandle(id)
}

#[cfg(test)]
#[path = "adapter_tests.rs"]
mod tests;
