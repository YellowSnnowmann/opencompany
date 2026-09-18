//! The `#[tauri::command]` surface the console calls.
//!
//! Thin by design: every one of these delegates to [`crate::proxy`] or
//! [`crate::embedded`], which are plain Rust and testable without a webview.
//! Logic that lives in a command is logic that can only be exercised by
//! starting a GUI.
//!
//! **Every command takes an explicit `connection_id`.** None of them reads an
//! "active connection" from application state — that single-valued field is
//! exactly what stops block/buzz from holding more than one workspace at a
//! time, and a command that defaulted it would reintroduce the limit invisibly.

use tauri::State;
use tauri::ipc::Channel;

use crate::local::LocalInstanceInfo;
use crate::proxy::{
    Connection, Credential, ProxyRequest, ProxyResponse, SharedProxy, may_carry_a_credential,
};

/// What the console needs to construct a connection record.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedInfo {
    pub base_url: String,
    pub data_dir: String,
    /// Who is answering there, as opposed to where.
    ///
    /// Carried because `base_url` holds an ephemeral port and so cannot be an
    /// identity: keyed on the address, the console reads every launch as a
    /// first meeting and leaves the previous launch's row behind, dead (#615).
    pub instance_id: String,
}

/// Registers (or re-registers) a host this client talks to.
///
/// **Takes no device token.** The console cannot supply one, because it has
/// never seen one: a paired device's session is resolved from the keychain by
/// `connection_id`. That is the difference between "the webview does not
/// normally hold the secret" and "the webview cannot hold the secret", and only
/// the second survives a script injected into rendered agent markdown.
#[tauri::command]
pub async fn oc_connect(
    proxy: State<'_, SharedProxy>,
    connection_id: String,
    base_url: String,
    platform_token: Option<String>,
) -> Result<(), String> {
    // Device first: a paired device is a *person* on this machine, and the
    // journal records their name. A platform bearer is a machine credential
    // that writes anonymously, so preferring it would silently un-attribute
    // every write the desktop makes.
    let credential = match (
        crate::keychain::device_session(&connection_id),
        platform_token,
    ) {
        (Some(session), _) => Credential::Device(session),
        (None, Some(token)) => Credential::Platform(token),
        (None, None) => Credential::None,
    };
    proxy
        .upsert(
            connection_id,
            Connection {
                base_url,
                credential,
            },
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn oc_disconnect(
    proxy: State<'_, SharedProxy>,
    connection_id: String,
) -> Result<(), String> {
    proxy.remove(&connection_id).await;
    Ok(())
}

#[tauri::command]
pub async fn oc_connections(proxy: State<'_, SharedProxy>) -> Result<Vec<String>, String> {
    Ok(proxy.ids().await)
}

/// One HTTP request against a named connection.
#[tauri::command]
pub async fn oc_request(
    proxy: State<'_, SharedProxy>,
    connection_id: String,
    request: ProxyRequest,
) -> Result<ProxyResponse, String> {
    proxy
        .request(&connection_id, request)
        .await
        .map_err(|error| error.to_string())
}

/// Subscribes to a connection's event stream, pushing payloads down `channel`.
///
/// One channel per subscription rather than one shared bus: a chatty company's
/// turn events must not be able to starve another connection's, and dropping
/// the channel is how the console unsubscribes.
#[tauri::command]
pub async fn oc_subscribe(
    proxy: State<'_, SharedProxy>,
    connection_id: String,
    path: String,
    channel: Channel<String>,
) -> Result<(), String> {
    let proxy = proxy.inner().clone();
    tokio::spawn(async move {
        let result = proxy
            .subscribe(&connection_id, &path, |event| {
                // A send failure means the console dropped the channel, i.e.
                // unsubscribed. Not an error worth reporting.
                let _ = channel.send(event);
            })
            .await;
        if let Err(error) = result {
            tracing::debug!(%error, "event stream ended");
        }
    });
    Ok(())
}

/// What the console learns after pairing. Carries no secret.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    pub company: String,
    pub device_id: String,
    pub expires_at_millis: u64,
}

/// What the host answers a claim with. The token half never leaves this module.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimedDevice {
    token: String,
    company: String,
    device_id: String,
    expires_at_millis: u64,
}

/// Redeems a pairing code against a host, and returns what it answered.
///
/// Split out of [`oc_pair_device`] so it can be tested at all: a command takes
/// `State<'_, SharedProxy>`, which needs a Tauri application to construct, and
/// the module note above says why that matters — logic reachable only by
/// starting a GUI is logic nothing checks. Everything here is the part with a
/// rule in it, and the command below is the keychain write and the
/// re-registration around it.
///
/// **The one exchange in which a session token is created rather than
/// replayed.** The pairing code goes out in the request and the token comes
/// back in the response body, so this is where an unencrypted wire costs the
/// most — and it is not covered by `ProxyRegistry::upsert`, because it never
/// goes through the registry (#731).
async fn claim(base_url: &str, code: &str, label: Option<&str>) -> Result<ClaimedDevice, String> {
    if !may_carry_a_credential(base_url) {
        // The console shows this verbatim (`device-pairing.tsx`), so it is
        // written for the person reading it rather than for a log.
        return Err(format!(
            "{base_url} is not encrypted, so pairing would send this device's session in the clear. Use https, or a host on this machine."
        ));
    }
    let base = base_url.trim_end_matches('/');
    let response = reqwest::Client::builder()
        // As `ProxyRegistry`'s client does, and here for a sharper reason: the
        // default policy follows up to ten redirects, and a 307 from an https
        // base to an http one re-sends this request — pairing code and all —
        // over the wire the check above just refused. A check on the first url
        // is worth nothing if the client will walk to a second.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())?
        .post(format!("{base}/api/v1/devices/claim"))
        .json(&serde_json::json!({ "code": code, "label": label }))
        .send()
        .await
        .map_err(|error| error.to_string())?;

    if !response.status().is_success() {
        // The host's own wording, which is deliberately one indistinguishable
        // message for every way a claim can fail. Passing it through keeps that
        // property instead of inventing a more specific one here.
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("pairing failed with {status}"));
        return Err(message);
    }

    response.json().await.map_err(|error| error.to_string())
}

/// Redeems a pairing code, keeping the session token out of the webview.
///
/// The whole flow lives in Rust for one reason: the token exists for exactly
/// one HTTP response, and the console must not be on the path it takes. So this
/// command performs the claim, writes the result to the keychain, re-registers
/// the connection with the resolved credential, and returns only what a person
/// needs to see — which company, which device, how long it lasts.
///
/// Deliberately does its own request rather than going through
/// `ProxyRegistry::request`: this runs *before* the connection has a credential
/// worth attaching, and routing it through the proxy would mean a code path
/// where the claim response body — the one place a raw token appears — passes
/// through the same machinery that serialises bodies back to the webview.
///
/// Which is also why the transport rule has to be repeated here. Doing its own
/// request means doing its own checking: the registry never sees this url, so
/// `upsert`'s refusal does not cover the one exchange where the token is not
/// merely replayed but *handed over* — the code goes out in the request and the
/// session comes back in the response, both in the clear on a plain-HTTP host.
#[tauri::command]
pub async fn oc_pair_device(
    proxy: State<'_, SharedProxy>,
    connection_id: String,
    base_url: String,
    code: String,
    label: Option<String>,
) -> Result<PairedDevice, String> {
    let claimed = claim(&base_url, &code, label.as_deref()).await?;
    // `<company>.<token>` is the header carrier's form, and the only form
    // anything downstream needs.
    crate::keychain::remember_device(
        &connection_id,
        &format!("{}.{}", claimed.company, claimed.token),
    )
    .map_err(|error| error.to_string())?;

    // Re-register so the credential takes effect without waiting for a reload.
    // The console cannot do this itself — it has nothing to pass.
    //
    // The session is read back from the keychain rather than reused from the
    // claim: what matters is what the store will hand out on the *next* boot,
    // so a write that did not survive surfaces here rather than as a mysterious
    // 401 later. A miss is `Credential::None`, never `Device("")` — an empty
    // session header is a credential that authenticates as nobody while looking
    // like one to every check that only asks whether a device is paired.
    if let Ok(base_url) = proxy.base_url(&connection_id).await {
        let credential = match crate::keychain::device_session(&connection_id) {
            Some(session) => Credential::Device(session),
            None => Credential::None,
        };
        // Infallible in practice: `base_url` was just read back out of the
        // registry, so it is one `upsert` already accepted. Surfaced rather
        // than swallowed anyway — a pairing that reported success while the
        // credential never took effect is the worst of the three outcomes.
        proxy
            .upsert(
                connection_id.clone(),
                Connection {
                    base_url,
                    credential,
                },
            )
            .await
            .map_err(|error| error.to_string())?;
    }

    Ok(PairedDevice {
        company: claimed.company,
        device_id: claimed.device_id,
        expires_at_millis: claimed.expires_at_millis,
    })
}

/// Adopts a session a sign-in just returned, as this connection's credential.
///
/// The desktop's sign-in flow asks the host for the header carrier — there is
/// no cookie jar behind [`oc_request`], so the cookie the host would otherwise
/// set has nowhere to live — and the readable session it gets back is handed
/// here rather than kept in the webview. That is the property the proxy's
/// `RESERVED_HEADERS` exists to hold: the page never holds a credential, so a
/// script injected into rendered agent markdown cannot exfiltrate one, and it
/// cannot choose what a request authenticates as either, because the proxy
/// attaches the credential itself.
///
/// This is `oc_pair_device` minus the pairing ceremony. A sign-in's session and
/// a paired device's are the same thing to every layer below: the host renders
/// both as `<company>.<token>`, the keychain stores both under the connection
/// id, and [`Credential::Device`] carries both in the same header. The claim
/// step is the only difference, and the sign-in already did its own.
#[tauri::command]
pub async fn oc_adopt_session(
    proxy: State<'_, SharedProxy>,
    connection_id: String,
    session: String,
) -> Result<(), String> {
    adopt_session(&proxy, connection_id, session).await
}

/// The body of [`oc_adopt_session`], off the `State` extractor so a test can
/// drive it against a bare registry.
///
/// Ordered so that nothing durable happens until everything refusable has been
/// refused, and everything durable is undone when a later step fails anyway:
///
/// 1. The connection is looked up first — adopting a session for a connection
///    the core does not hold stores nothing.
/// 2. The transport gate runs BEFORE the keychain write. `upsert` would refuse
///    the credential afterwards, but by then the keychain would hold a session
///    the next launch's `oc_connect` dutifully presents — and its `upsert`
///    refuses the whole registration, leaving the connection unusable until
///    someone finds the hidden keychain entry. The precheck is the same
///    function `upsert` consults, so the two cannot disagree.
/// 3. A read-back miss is an error, not a quiet `Credential::None`. The
///    read-back exists to surface a write that did not survive; installing
///    nothing and reporting success would have the console record a credential
///    while every request runs anonymous — the exact silence this command was
///    added to end.
/// 4. An `upsert` refusal rolls the keychain entry back, best-effort, for the
///    same reason as (2): a credential the registry refused must not ambush
///    the next launch.
pub(crate) async fn adopt_session(
    proxy: &crate::proxy::ProxyRegistry,
    connection_id: String,
    session: String,
) -> Result<(), String> {
    // Refused before anything is stored: a session that authenticates as
    // nobody must not survive into the keychain, where the next launch would
    // dutifully present it and read the host's 401 as a revoked sign-in.
    if session.trim().is_empty() {
        return Err("a sign-in session cannot be empty".to_string());
    }
    let base_url = proxy
        .base_url(&connection_id)
        .await
        .map_err(|error| error.to_string())?;
    if !may_carry_a_credential(&base_url) {
        // The registry's own words for this refusal, so the sign-in screen and
        // a failed registration name the problem identically.
        return Err(crate::proxy::ProxyError::InsecureBaseUrl(base_url).to_string());
    }

    crate::keychain::remember_device(&connection_id, &session)
        .map_err(|error| error.to_string())?;

    // Read back from the store rather than reused from the argument: what
    // matters is what the store will hand out on the *next* boot, so a write
    // that did not survive surfaces here — as a failure, with the entry
    // removed — rather than as a mysterious 401 later.
    let Some(stored) = crate::keychain::device_session(&connection_id) else {
        let _ = crate::keychain::forget_device(&connection_id);
        return Err(
            "the session was stored but could not be read back from the keychain".to_string(),
        );
    };
    proxy
        .upsert(
            connection_id.clone(),
            Connection {
                base_url,
                credential: Credential::Device(stored),
            },
        )
        .await
        .map_err(|error| {
            // Best-effort: the refusal is the error worth reporting, and a
            // failed tidy-up must not replace it.
            let _ = crate::keychain::forget_device(&connection_id);
            error.to_string()
        })
}

/// Forgets this machine's stored session for a connection.
///
/// Local only. The session record on the host outlives it — revoking that is
/// the operator's action from the devices list, and doing both here would mean
/// removing a row from one machine silently cut off another.
#[tauri::command]
pub async fn oc_forget_device(
    proxy: State<'_, SharedProxy>,
    connection_id: String,
) -> Result<(), String> {
    crate::keychain::forget_device(&connection_id).map_err(|error| error.to_string())?;
    if let Ok(base_url) = proxy.base_url(&connection_id).await {
        // As in `oc_pair_device`: a url the registry already accepted.
        proxy
            .upsert(
                connection_id,
                Connection {
                    base_url,
                    credential: Credential::None,
                },
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Where the host rooted at the data dir is listening, if it is running.
///
/// Kept alongside [`oc_local_instances`], which supersedes it, because the two
/// halves of this application ship independently: a `pnpm dev` console built
/// before the roster existed calls only this, and a shell built before it
/// answers only this. Both degrade to the single-instance behaviour instead of
/// to an unhandled `no such command`.
#[tauri::command]
pub async fn oc_embedded(
    state: State<'_, crate::AppHandleState>,
) -> Result<Option<EmbeddedInfo>, String> {
    let local = state.local.lock().await;
    Ok(local.default_instance().and_then(|instance| {
        Some(EmbeddedInfo {
            base_url: instance.base_url?,
            data_dir: instance.data_dir,
            instance_id: instance.instance_id?,
        })
    }))
}

/// Every host this machine runs, listening or not.
///
/// The listing is the whole surface: creating, starting and stopping all
/// answer with the affected instance, and the console re-reads this rather
/// than keeping its own idea of the roster. One source of truth, on the side
/// that actually holds the sockets.
#[tauri::command]
pub async fn oc_local_instances(
    state: State<'_, crate::AppHandleState>,
) -> Result<Vec<LocalInstanceInfo>, String> {
    Ok(state.local.lock().await.list())
}

/// Adds a host over a fresh data root on this machine, and starts it.
///
/// Its own root, never a second process over an existing one: two hosts over
/// one root overwrite each other's companies, which is why `prepare_instance`
/// locks it in the first place.
#[tauri::command]
pub async fn oc_create_local_instance(
    state: State<'_, crate::AppHandleState>,
    label: String,
) -> Result<LocalInstanceInfo, String> {
    state.local.lock().await.create(&label).await
}

#[tauri::command]
pub async fn oc_start_local_instance(
    state: State<'_, crate::AppHandleState>,
    id: String,
) -> Result<LocalInstanceInfo, String> {
    state.local.lock().await.start(&id).await
}

/// Stops a host, freeing its port and — the part that matters — its data root,
/// so a terminal `opencompany serve` can take it.
#[tauri::command]
pub async fn oc_stop_local_instance(
    state: State<'_, crate::AppHandleState>,
    id: String,
) -> Result<LocalInstanceInfo, String> {
    state.local.lock().await.stop(&id)
}

#[tauri::command]
pub async fn oc_rename_local_instance(
    state: State<'_, crate::AppHandleState>,
    id: String,
    label: String,
) -> Result<LocalInstanceInfo, String> {
    state.local.lock().await.rename(&id, &label)
}

/// Opens a tunnel to a host on another machine, and answers with the loopback
/// address the console should use for it.
///
/// Idempotent per target: asking for a host that is already tunnelled hands
/// back the tunnel that is up. The console reopens every remembered `ssh`
/// connection at launch, and a second call must not mean a second child.
#[tauri::command]
pub async fn oc_open_ssh_tunnel(
    state: State<'_, crate::AppHandleState>,
    target: crate::ssh::SshTarget,
) -> Result<crate::ssh::SshTunnelInfo, String> {
    state.ssh.lock().await.open(target).await
}

/// Closes the tunnel to a target. Not an error when there is none — the
/// console closes on removal, and removal can arrive twice.
#[tauri::command]
pub async fn oc_close_ssh_tunnel(
    state: State<'_, crate::AppHandleState>,
    target: crate::ssh::SshTarget,
) -> Result<(), String> {
    state.ssh.lock().await.close(&target).await;
    Ok(())
}

/// Every tunnel, and which of them stopped forwarding.
///
/// The roster the console re-reads rather than keeping its own copy of, for
/// the same reason [`oc_local_instances`] is: one source of truth, on the side
/// that actually holds the processes.
#[tauri::command]
pub async fn oc_ssh_tunnels(
    state: State<'_, crate::AppHandleState>,
) -> Result<Vec<crate::ssh::SshTunnelInfo>, String> {
    Ok(state.ssh.lock().await.list())
}

/// Drops a host from the roster. **Leaves its data on disk** — see
/// [`crate::local::LocalHosts::forget`].
#[tauri::command]
pub async fn oc_forget_local_instance(
    state: State<'_, crate::AppHandleState>,
    id: String,
) -> Result<(), String> {
    let mut local = state.local.lock().await;
    // Stopping first is what makes the removal complete: a forgotten instance
    // whose host kept listening would hold its root against the terminal, and
    // stay reachable from a console row nothing lists any more.
    let _ = local.stop(&id);
    local.forget(&id)
}

/// Permanently deletes a desktop-created host and everything in its data root.
///
/// This is intentionally distinct from [`oc_forget_local_instance`], whose
/// recoverable contract leaves the data root intact.
#[tauri::command]
pub async fn oc_delete_local_instance(
    state: State<'_, crate::AppHandleState>,
    id: String,
) -> Result<(), String> {
    state.local.lock().await.delete(&id).await
}

/// Every coding harness this shell knows how to drive over ACP, and what the
/// **filesystem** says about each right now.
///
/// Takes no state and no connection id: unlike everything else in this file,
/// readiness is a property of *this machine*, not of a host it talks to.
///
/// Answers nothing on its own. Every harness comes back `checking`, because
/// nothing short of running the adapter can say whether it is installed,
/// working, and signed in — and this call runs nothing. It exists to paint the
/// list; [`oc_acp_confirm_harness`] is what settles each row.
#[tauri::command]
pub fn oc_acp_harnesses() -> Vec<crate::acp::discovery::HarnessStatus> {
    crate::acp::discovery::survey()
}

/// Actually starts one harness, resolving its `checking` state to `ready` or
/// `spawnFailed` — and returning the models it advertises.
///
/// Both answers come from one spawn because they come from the same call:
/// `session/new` is where an adapter both proves it can open a session and
/// lists what it can run. Asking twice would spawn twice for no more
/// information.
///
/// Split from [`oc_acp_harnesses`] rather than folded into it so the list
/// paints immediately and each row settles on its own: one slow CLI must not
/// hold up the others, and the operator sees "Checking…" rather than an empty
/// pane. Safe to call concurrently for every harness.
///
/// The subprocess is killed when the probe's client drops, so nothing is left
/// running whether it succeeded, failed, or timed out.
#[tauri::command]
pub async fn oc_acp_confirm_harness(
    state: tauri::State<'_, crate::AppHandleState>,
    id: String,
) -> Result<crate::acp::discovery::ConfirmedHarness, String> {
    // A dedicated empty directory, not the data root itself.
    //
    // Still stable and ordinary — an agent that inspects its working directory
    // on startup sees a real place, and nothing is left behind to clean up —
    // but no longer the root holding every company's journal, ledger and
    // derived state. These CLIs read their working directory on startup
    // looking for project configuration and repository markers, and pointing
    // one at the whole data root hands it that surface for no benefit the
    // probe actually needs.
    let cwd = state.data_dir.join("acp-probe");
    if let Err(error) = std::fs::create_dir_all(&cwd) {
        // Not fatal: the probe only needs *a* directory. Falling back keeps a
        // read-only or full disk from turning every harness into "won't start"
        // when the real answer has nothing to do with the harness.
        tracing::debug!(%error, "could not create the ACP probe directory");
        return Ok(crate::acp::discovery::confirm(&id, &state.data_dir).await);
    }
    Ok(crate::acp::discovery::confirm(&id, &cwd).await)
}

/// Installs (or updates) the ACP adapter this app owns for one harness.
///
/// The adapter is *our* dependency, not the operator's: they installed Claude
/// Code, and `@agentclientprotocol/claude-agent-acp` is the piece that makes it
/// speak this protocol. So the app fetches it, into its own directory, at the
/// version this build pins — never into the operator's global npm prefix.
///
/// **Explicit, never automatic.** It is a network fetch that writes
/// executables, and doing that unannounced on launch is not something an app
/// should decide for someone. The console offers a button; this is what the
/// button calls.
///
/// One install per harness at a time. Two concurrent `npm install --prefix`
/// runs against the same directory interleave their writes, and the failure
/// that produces is a half-populated `node_modules` that reads as a corrupt
/// install rather than as a collision.
#[tauri::command]
pub async fn oc_acp_install_harness(id: String) -> Result<(), String> {
    use tokio::sync::Mutex;

    /// Serialises **every** install, not one per harness.
    ///
    /// Both adapters install into the same `npm --prefix` root, so two
    /// concurrent `npm install` runs there interleave their writes to one
    /// `node_modules` and one lockfile. An earlier version of this guarded
    /// per-id so Claude's install would not block Codex's — which is exactly
    /// the case that corrupts the tree, since those are the two that share the
    /// prefix. The wait is seconds and the button is per-row, so the cost of
    /// serialising is a queue nobody notices.
    ///
    /// A `tokio::sync::Mutex` rather than the `std` one because it is held
    /// across an await. The guard also removes the need to un-register an id
    /// by hand: a cancelled or panicking install drops the guard and releases
    /// the lock, where the previous insert/remove pair leaked the id forever
    /// and made every later attempt report "already running".
    static INSTALLING: Mutex<()> = Mutex::const_new(());

    let harness = crate::acp::discovery::HARNESSES
        .iter()
        .find(|h| h.id == id)
        .ok_or_else(|| format!("`{id}` is not a harness this build knows"))?;

    let _guard = INSTALLING.lock().await;
    crate::acp::tools::install(harness).await
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;

/// Who is sitting at this machine, as the operating system already knows.
///
/// A **suggestion** for a profile nobody has filled in yet — see
/// [`crate::identity`] for why it is read rather than imported. Every field is
/// optional and a machine that knows nothing answers an empty record, which the
/// console reads as "ask them to type it".
///
/// Takes no `connection_id`, unlike every other command here: it is a fact about
/// this computer, not about a host — the same answer whichever workspace the
/// person is looking at.
#[tauri::command]
pub async fn oc_device_identity() -> Result<crate::identity::DeviceIdentity, String> {
    Ok(crate::identity::device_identity())
}

/// What one probe of the update endpoint told us.
///
/// `available: false` is the answer to every uninteresting outcome — up to
/// date, offline, endpoint down, this build carrying no signing key — because
/// the console does nothing different for any of them. See
/// [`oc_app_update_check`] for why a failure is not an error here.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    /// The version running right now, from `tauri.conf.json`.
    pub current_version: String,
    pub available: bool,
    /// What the endpoint is offering, when it is offering something.
    pub available_version: Option<String>,
    /// The manifest's release notes, if it carried any.
    pub notes: Option<String>,
}

impl AppUpdateInfo {
    fn nothing(current_version: String) -> Self {
        Self {
            current_version,
            available: false,
            available_version: None,
            notes: None,
        }
    }
}

/// What a background download left behind.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateStaged {
    /// True when bytes are staged and `oc_app_update_install` can finish.
    pub ready: bool,
    pub version: Option<String>,
    pub notes: Option<String>,
}

impl AppUpdateStaged {
    fn nothing() -> Self {
        Self {
            ready: false,
            version: None,
            notes: None,
        }
    }
}

/// This build's configured minisign public key, or the empty string.
///
/// Read back out of the plugin config rather than kept as a second constant:
/// the config file is the only place the key is written, and a copy here would
/// be a copy that can disagree with the one the plugin actually verifies with.
fn updater_pubkey(app: &tauri::AppHandle) -> String {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|pubkey| pubkey.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Asks the update endpoint whether there is a newer shell than this one.
///
/// Downloads nothing and installs nothing. The console runs this on a timer, so
/// **every failure answers "no update"** rather than erroring: a laptop on a
/// train, a GitHub outage and a release that has not happened yet are all the
/// same fact to the person using the application, and none of them is worth a
/// banner. A build carrying the placeholder signing key answers the same way —
/// see [`crate::update::is_configured`], which is what stops a misconfigured
/// release offering an update that could never verify.
#[tauri::command]
pub async fn oc_app_update_check(app: tauri::AppHandle) -> Result<AppUpdateInfo, String> {
    use tauri_plugin_updater::UpdaterExt;

    let current_version = app.package_info().version.to_string();

    if !crate::update::is_configured(&updater_pubkey(&app)) {
        tracing::debug!("updates are not configured in this build; reporting none");
        return Ok(AppUpdateInfo::nothing(current_version));
    }

    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(error) => {
            tracing::warn!(%error, "the updater could not be built; reporting no update");
            return Ok(AppUpdateInfo::nothing(current_version));
        }
    };

    match updater.check().await {
        Ok(Some(update)) => {
            tracing::info!(
                current = %current_version,
                offered = %update.version,
                "a newer desktop build is available"
            );
            Ok(AppUpdateInfo {
                current_version,
                available: true,
                available_version: Some(update.version.clone()),
                notes: update.body.clone(),
            })
        }
        Ok(None) => Ok(AppUpdateInfo::nothing(current_version)),
        Err(error) => {
            tracing::warn!(%error, "the update check failed; reporting no update");
            Ok(AppUpdateInfo::nothing(current_version))
        }
    }
}

/// Downloads the offered bundle and holds it, verified, until someone restarts.
///
/// The counterpart to the check's silence: this one **does** report its
/// failures, because the console only calls it after a check said there is
/// something to fetch, and a download that keeps failing is worth one banner
/// with a retry on it.
///
/// A mid-stream network failure is retried up to
/// [`crate::update::MAX_DOWNLOAD_ATTEMPTS`] times; a signature failure is not
/// retried at all. The bytes are verified against the configured public key
/// *inside* this call, so what lands in the staging slot is already trusted and
/// the restart has nothing left to check.
#[tauri::command]
pub async fn oc_app_update_download(
    app: tauri::AppHandle,
    pending: State<'_, crate::update::PendingUpdate>,
) -> Result<AppUpdateStaged, String> {
    use tauri_plugin_updater::UpdaterExt;

    if !crate::update::is_configured(&updater_pubkey(&app)) {
        return Ok(AppUpdateStaged::nothing());
    }

    let updater = app
        .updater()
        .map_err(|error| format!("the updater could not be built: {error}"))?;

    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => return Ok(AppUpdateStaged::nothing()),
        Err(error) => return Err(format!("the update check failed: {error}")),
    };

    let version = update.version.clone();
    let notes = update.body.clone();
    tracing::info!(%version, "downloading a desktop update in the background");

    let mut attempt: u32 = 1;
    let bytes = loop {
        match update.download(|_, _| {}, || {}).await {
            Ok(bytes) => break bytes,
            Err(error) => {
                let transient = crate::update::is_transient(&error);
                match crate::update::classify(
                    attempt,
                    crate::update::MAX_DOWNLOAD_ATTEMPTS,
                    transient,
                ) {
                    crate::update::RetryDecision::Retry => {
                        tracing::warn!(%error, attempt, "the update download failed; retrying");
                        tokio::time::sleep(crate::update::backoff_for(attempt)).await;
                        attempt += 1;
                    }
                    crate::update::RetryDecision::GiveUp => {
                        tracing::error!(%error, attempt, "the update download failed");
                        return Err(format!("the update could not be downloaded: {error}"));
                    }
                }
            }
        }
    };

    tracing::info!(%version, "a desktop update is staged and waiting for a restart");
    *pending.0.lock().await = Some(crate::update::StagedUpdate {
        update,
        bytes,
        version: version.clone(),
    });

    Ok(AppUpdateStaged {
        ready: true,
        version: Some(version),
        notes,
    })
}

/// Applies the staged bytes and relaunches. Never returns on success.
///
/// Stops every listening host first, and that is the whole reason this command
/// touches [`crate::AppHandleState`] at all: `restart` spawns the replacement
/// process and *then* exits, so a host still holding its data root would still
/// be holding it when the new process reaches for the same root — and the
/// application would come back with its companies down and "held by another
/// process" against each one. [`crate::local::LocalHosts::quiesce`] releases
/// the locks without recording a decision the operator did not make.
///
/// A failed install puts them back: the bundle on disk was not replaced, so
/// this build keeps running and its hosts should keep serving.
#[tauri::command]
pub async fn oc_app_update_install(
    state: State<'_, crate::AppHandleState>,
    pending: State<'_, crate::update::PendingUpdate>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let staged = pending
        .0
        .lock()
        .await
        .take()
        .ok_or("no update has been downloaded yet")?;

    tracing::info!(version = %staged.version, "installing a desktop update");

    let mut local = state.local.lock().await;
    let quiesced = local.quiesce();

    if let Err(error) = staged.update.install(staged.bytes) {
        tracing::error!(%error, "the update could not be installed");
        for id in quiesced {
            if let Err(error) = local.start(&id).await {
                tracing::error!(%id, %error, "a host did not come back after a failed install");
            }
        }
        return Err(format!("the update could not be installed: {error}"));
    }

    tracing::info!("the update is installed; relaunching");
    app.restart();
}

#[cfg(test)]
#[path = "commands_adopt_session_tests.rs"]
mod adopt_session_tests;
