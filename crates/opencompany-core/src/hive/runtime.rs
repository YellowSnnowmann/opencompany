//! One company's hive: the Coordinator, the OpenHuman host its agents are
//! registered on, the projector, and the tasks that run them (OC-2).
//!
//! # One per company, not per pool or per process
//!
//! A company with several named `built_in` harnesses has one pool per
//! harness, each building only the agents it serves — but the company has one
//! hive, one durable transcript and one set of desks, so every pool registers
//! its agents into the same [`CompanyHive`]. And it is one per *company*, not
//! per process: `hivemind_list_agents` would otherwise list another tenant's
//! roster. [`for_company`] keys the live hives by company and by the identity
//! of the `HiveStore` they persist to — the one store every pool of a company
//! is built over — so two test fixtures that reuse a company id over separate
//! stores never share a hive.
//!
//! # Lifecycle
//!
//! [`for_company`] starts the hive on first use: it loads the Coordinator's
//! state (marking any turn a dead process left running as interrupted),
//! configures the adapter — the [`HiveHooks`], the reach policy, the turn
//! wall — reconciles the projector's cursor from the journal, and spawns the
//! Coordinator's run loop and the projector's tail. Pools then
//! [`register`](CompanyHive::register) their agents, and every roster rebuild
//! goes through [`replace`](CompanyHive::replace), which rebuilds an agent's
//! handle in place so its continuing session and queued work carry over.
//! [`sync`](CompanyHive::sync) brings the hives in line with the company's
//! desks. Dropping the last handle shuts the Coordinator down and stops both
//! tasks.
//!
//! The options the Coordinator was started with (round width, turn walls,
//! retention, the adapter's turn timeout) are fixed for the life of the hive;
//! a `[group_chat.routing]` edit reaches them on the next start. Retention is
//! bounded (`routing::RETAINED_*`, `PENDING_PER_AGENT`): settled episodes,
//! delivered and interrupted records are pruned from the state row once the
//! projector has journaled them, and a send to an agent whose inbox is full
//! is refused (`InboxFull`) as backpressure.
//!
//! # One writer per store
//!
//! Starting a Coordinator claims its store (a fencing epoch): an older
//! Coordinator over the same store is refused every write from then on with
//! `Error::Fenced`. This process runs exactly one Coordinator per company
//! (`for_company`), and a run loop that is fenced stops for good — another
//! process owns the company — rather than retrying.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};
use std::time::Duration;

use tinyhivemind_hives::{Coordinator, CoordinatorOptions, HiveInfo};
use tinyhivemind_openhuman::OpenHumanHost;

use super::policy::ReachPolicy;
use super::projector::{HiveRoster, Projector, TurnMetaBoard};
use super::storage::PortStorage;
use crate::error::{OpenCompanyError, Result};
use crate::harness::built_in::hive_hooks::{HiveAgents, HiveHooks, HiveSeat};
use crate::ports::events::EventLog;
use crate::ports::general_channel::{GENERAL_CHANNEL_ID, GENERAL_CHANNEL_NAME};
use crate::ports::hive::HiveStore;
use crate::ports::types::{CompanyId, CompanyRecord};

/// How long a failed run loop or projection waits before it tries again.
const RETRY_AFTER: Duration = Duration::from_secs(1);

/// What a company hive is started with.
pub struct HiveConfig {
    /// The company.
    pub company: CompanyId,
    /// The process-wide OpenHuman runtime's identity.
    pub runtime_id: String,
    /// Where the Coordinator's state and transcript persist.
    pub store: Arc<dyn HiveStore>,
    /// The company journal the projector writes to.
    pub events: Arc<dyn EventLog>,
    /// The Coordinator's options (`routing::coordinator_options`).
    pub options: CoordinatorOptions,
    /// The wall on one agent turn (`routing::turn_timeout`).
    pub turn_timeout: Duration,
}

/// One company's hive. See the module docs.
pub struct CompanyHive {
    company: CompanyId,
    host: OpenHumanHost,
    projector: Arc<Projector>,
    roster: Arc<HiveRoster>,
    policy: Arc<ReachPolicy>,
    agents: Arc<HiveAgents>,
    registered: Mutex<HashSet<String>>,
    sync_lock: tokio::sync::Mutex<()>,
    tasks: Vec<tokio::task::AbortHandle>,
}

impl std::fmt::Debug for CompanyHive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompanyHive")
            .field("company", &self.company)
            .finish_non_exhaustive()
    }
}

type HiveKey = (CompanyId, usize);

/// The live hives, by company and store identity.
static HIVES: LazyLock<Mutex<HashMap<HiveKey, Weak<CompanyHive>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Serialises starting a hive, so two pools warming one company at once
/// share the hive rather than each starting a Coordinator over one store.
static STARTING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn store_identity(store: &Arc<dyn HiveStore>) -> usize {
    Arc::as_ptr(store).cast::<()>() as usize
}

fn hive_error(error: impl std::fmt::Display) -> OpenCompanyError {
    OpenCompanyError::Harness(format!("company hive: {error}"))
}

/// The company's live hive over `config.store`, started on first use.
///
/// # Errors
///
/// The Coordinator's state could not be loaded, the adapter refused its
/// configuration, or the projector could not read the journal.
pub async fn for_company(config: HiveConfig) -> Result<Arc<CompanyHive>> {
    let key = (config.company.clone(), store_identity(&config.store));
    if let Some(live) = live(&key) {
        return Ok(live);
    }
    let _starting = STARTING.lock().await;
    if let Some(live) = live(&key) {
        return Ok(live);
    }
    let hive = Arc::new(CompanyHive::start(config).await?);
    HIVES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, Arc::downgrade(&hive));
    Ok(hive)
}

fn live(key: &HiveKey) -> Option<Arc<CompanyHive>> {
    let mut hives = HIVES.lock().unwrap_or_else(PoisonError::into_inner);
    hives.retain(|_, hive| hive.strong_count() > 0);
    hives.get(key).and_then(Weak::upgrade)
}

impl CompanyHive {
    async fn start(config: HiveConfig) -> Result<Self> {
        let HiveConfig {
            company,
            runtime_id,
            store,
            events,
            options,
            turn_timeout,
        } = config;
        let storage = Arc::new(PortStorage::new(company.clone(), store));
        let coordinator = Coordinator::new(runtime_id.clone(), storage, options)
            .await
            .map_err(hive_error)?;
        let roster = Arc::new(HiveRoster::default());
        let meta = Arc::new(TurnMetaBoard::default());
        let agents = Arc::new(HiveAgents::default());
        let policy = Arc::new(ReachPolicy::new());
        let hooks = Arc::new(HiveHooks::new(
            company.clone(),
            Arc::clone(&agents),
            Arc::clone(&roster),
            Arc::clone(&meta),
        ));
        let host = OpenHumanHost::new(runtime_id, coordinator.clone())
            .and_then(|host| host.with_hooks(hooks))
            .and_then(|host| host.with_send_policy(Arc::clone(&policy) as _))
            .and_then(|host| host.with_turn_timeout(turn_timeout))
            .map_err(hive_error)?;
        let projector = Arc::new(Projector::new(
            company.clone(),
            events,
            coordinator.clone(),
            Arc::clone(&roster),
            meta,
        ));
        projector.reconcile().await?;
        let tail = {
            let projector = Arc::clone(&projector);
            let mut changes = coordinator.subscribe();
            let company = company.clone();
            tokio::spawn(async move {
                loop {
                    if let Err(error) = projector.project().await {
                        tracing::warn!(
                            %company,
                            %error,
                            "[hive] projecting the company hive failed; retrying on the next commit"
                        );
                        tokio::time::sleep(RETRY_AFTER).await;
                    }
                    if changes.changed().await.is_err() {
                        break;
                    }
                }
            })
            .abort_handle()
        };
        let run = {
            let coordinator = coordinator.clone();
            let company = company.clone();
            tokio::spawn(async move {
                loop {
                    match coordinator.run().await {
                        Ok(()) => break,
                        // Another process claimed this company's store (a
                        // newer Coordinator fenced this one out): its writes
                        // are refused from here on, so retrying would only
                        // spin. Stop, and say so.
                        Err(error @ tinyhivemind_hives::Error::Fenced { .. }) => {
                            tracing::error!(
                                %company,
                                %error,
                                "[hive] another process owns this company's hive; this one stops \
                                 running it"
                            );
                            coordinator.shutdown();
                            break;
                        }
                        Err(error) => {
                            tracing::error!(
                                %company,
                                %error,
                                "[hive] the company hive's run loop stopped; restarting it"
                            );
                            tokio::time::sleep(RETRY_AFTER).await;
                        }
                    }
                }
            })
            .abort_handle()
        };
        tracing::info!(%company, "[hive] company hive started");
        Ok(Self {
            company,
            host,
            projector,
            roster,
            policy,
            agents,
            registered: Mutex::new(HashSet::new()),
            sync_lock: tokio::sync::Mutex::new(()),
            tasks: vec![tail, run],
        })
    }

    /// The Coordinator: sends, releases, reads.
    #[must_use]
    pub fn coordinator(&self) -> &Coordinator {
        self.host.coordinator()
    }

    /// The projector, for a sender that wants a reply threaded before its own
    /// acceptance row lands.
    #[must_use]
    pub fn projector(&self) -> &Arc<Projector> {
        &self.projector
    }

    /// The Coordinator id of a manifest agent, when it is registered here.
    #[must_use]
    pub fn coordinator_id(&self, manifest_id: &str) -> Option<String> {
        self.roster.coordinator_id(manifest_id)
    }

    /// The manifest id behind a Coordinator id.
    #[must_use]
    pub fn manifest_id(&self, coordinator_id: &str) -> String {
        self.roster.manifest_id(&self.company, coordinator_id)
    }

    /// Whether `coordinator_id` has been registered with this hive.
    #[must_use]
    pub fn is_registered(&self, coordinator_id: &str) -> bool {
        self.registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(coordinator_id)
    }

    fn note_registered(&self, coordinator_id: &str, manifest_id: &str) {
        self.registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(coordinator_id.to_string());
        let mut map = self.roster.snapshot();
        map.insert(coordinator_id.to_string(), manifest_id.to_string());
        self.roster.install(map);
    }

    /// Registers a freshly built agent, continuing its one OpenHuman session
    /// (`openhuman_session_key`) across every hive and DM.
    ///
    /// A Coordinator that already bound the agent to a session the adapter
    /// returned on an earlier turn keeps that one: the registration falls back
    /// to an unbound one rather than refusing the agent.
    ///
    /// # Errors
    ///
    /// The adapter refused the handle (another runtime, a name collision with
    /// a permanent `hivemind_*` tool) or the registration could not commit.
    pub async fn register(&self, seat: HiveSeat) -> Result<()> {
        let coordinator_id = seat.agent.runtime_id.clone();
        let manifest_id = seat.agent.agent_id.clone();
        let session = crate::session_key::openhuman_session_key(&self.company, &manifest_id);
        let agent = seat.agent.runtime_agent().clone();
        self.agents.insert(coordinator_id.clone(), seat);
        let registered = match self
            .host
            .register_agent_in_session(agent.clone(), &session)
            .await
        {
            Err(tinyhivemind_openhuman::Error::Coordinator(
                tinyhivemind_hives::Error::SessionConflict(_),
            )) => self.host.register_agent(agent).await,
            other => other,
        };
        if let Err(error) = registered {
            self.agents.remove(&coordinator_id);
            return Err(hive_error(error));
        }
        self.note_registered(&coordinator_id, &manifest_id);
        Ok(())
    }

    /// Drops this hive's own clone of a registered agent's handle — the
    /// first half of a rebuild, before the pool's clones go too and
    /// [`replace`](Self::replace) builds the new one under the same id.
    pub fn release_handle(&self, coordinator_id: &str) {
        self.agents.remove(coordinator_id);
    }

    /// Rebuilds a registered agent's handle in place: waits for any turn it is
    /// running, drops the adapter's handle, then runs `build`. The session
    /// binding and queued work carry over; the `hivemind_*` tools are attached
    /// to the new handle.
    ///
    /// Every host-side clone of the old handle must be gone before this runs
    /// ([`release_handle`](Self::release_handle) and the pool's roster), or
    /// OpenHuman refuses the duplicate id. `build` may register the agent
    /// under a suffixed id when one survives anyway — an isolated turn still
    /// running on it, say; the seat is still returned for the pool, the
    /// Coordinator's agent is left without a handle until the next rebuild,
    /// and the error says so.
    ///
    /// # Errors
    ///
    /// `build` failed, or built an agent the adapter refused.
    pub async fn replace(
        &self,
        coordinator_id: &str,
        build: impl FnOnce() -> Result<HiveSeat>,
    ) -> std::result::Result<HiveSeat, (Option<HiveSeat>, OpenCompanyError)> {
        self.agents.remove(coordinator_id);
        let mut built: Option<HiveSeat> = None;
        let expected = coordinator_id.to_string();
        let replaced = self
            .host
            .replace_agent(coordinator_id, || {
                let seat = build().map_err(|error| {
                    tinyhivemind_openhuman::Error::Harness(anyhow::anyhow!(error.to_string()))
                })?;
                let agent = seat.agent.runtime_agent().clone();
                let id = seat.agent.runtime_id.clone();
                built = Some(seat);
                if id != expected {
                    return Err(tinyhivemind_openhuman::Error::AgentConflict(id));
                }
                Ok(agent)
            })
            .await;
        match (replaced, built) {
            (Ok(_), Some(seat)) => {
                self.agents.insert(coordinator_id.to_string(), seat.clone());
                Ok(seat)
            }
            (Ok(_), None) => Err((None, hive_error("the rebuild returned no agent"))),
            (Err(error), built) => Err((built, hive_error(error))),
        }
    }

    /// Brings the hives in line with the company: one hive per desk plus
    /// `#general`, each holding the desk's registered roster members, and the
    /// reach policy re-read from `record`.
    ///
    /// A desk that disappeared keeps its hive (the Coordinator never deletes
    /// one, its transcript is permanent) with every member removed. A desk's
    /// renamed display name is not carried over: the hive keeps the name it
    /// was created with.
    ///
    /// # Errors
    ///
    /// A membership change could not commit.
    pub async fn sync(&self, record: &CompanyRecord) -> Result<()> {
        let _sync = self.sync_lock.lock().await;
        let map = self.roster.snapshot();
        self.policy.install(Arc::new(record.clone()), map.clone());
        let by_manifest: HashMap<&str, &str> = map
            .iter()
            .map(|(coordinator, manifest)| (manifest.as_str(), coordinator.as_str()))
            .collect();
        let mut desks = crate::runtime::delegation_tools::desk_ids(record);
        if !desks.iter().any(|desk| desk == GENERAL_CHANNEL_ID) {
            desks.push(GENERAL_CHANNEL_ID.to_string());
        }
        let coordinator = self.coordinator();
        let existing: HashMap<String, HiveInfo> = coordinator
            .list_hives()
            .map_err(hive_error)?
            .into_iter()
            .map(|hive| (hive.hive_id.clone(), hive))
            .collect();
        for desk in &desks {
            let members: Vec<String> = super::route::hive_members(record, desk)
                .iter()
                .filter_map(|member| by_manifest.get(member.as_str()))
                .map(|coordinator| (*coordinator).to_string())
                .collect();
            match existing.get(desk) {
                None => {
                    let name = if desk == GENERAL_CHANNEL_ID {
                        GENERAL_CHANNEL_NAME.to_string()
                    } else {
                        crate::server::chat_history::desk_display_name(record, desk)
                    };
                    coordinator
                        .create_hive(HiveInfo {
                            hive_id: desk.clone(),
                            name: if name.trim().is_empty() { desk.clone() } else { name },
                            description: None,
                            members,
                        })
                        .await
                        .map_err(hive_error)?;
                }
                Some(hive) => {
                    for member in members.iter().filter(|m| !hive.members.contains(m)) {
                        coordinator
                            .join_hive(desk, member)
                            .await
                            .map_err(hive_error)?;
                    }
                    for member in hive.members.iter().filter(|m| !members.contains(m)) {
                        coordinator
                            .leave_hive(desk, member)
                            .await
                            .map_err(hive_error)?;
                    }
                }
            }
        }
        for (hive_id, hive) in &existing {
            if desks.contains(hive_id) {
                continue;
            }
            for member in &hive.members {
                coordinator
                    .leave_hive(hive_id, member)
                    .await
                    .map_err(hive_error)?;
            }
        }
        Ok(())
    }

    /// Releases a parked agent with the operator's decisions as a note it
    /// reads at the top of its next turn. Answers whether the agent is
    /// registered here.
    pub async fn release(&self, manifest_id: &str, note: Option<String>) -> Result<bool> {
        let Some(coordinator_id) = self.coordinator_id(manifest_id) else {
            return Ok(false);
        };
        self.coordinator()
            .release_with(&coordinator_id, note)
            .await
            .map_err(hive_error)?;
        Ok(true)
    }
}

impl Drop for CompanyHive {
    fn drop(&mut self) {
        self.coordinator().shutdown();
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
