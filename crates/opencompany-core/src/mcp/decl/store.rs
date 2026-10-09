//! Where a company's MCP declarations, credentials and health records live in
//! the [`SecretStore`](crate::ports::SecretStore), and the resolution that
//! turns them into effective, credentialed servers.

use super::*;

/// Loads the runtime server index from the secret store. A missing/empty key
/// yields an empty vec; a malformed index is a store error (surfaced, not
/// silently dropped, so corruption is visible).
pub async fn load_runtime_index(
    company: &CompanyId,
    secrets: &dyn SecretStore,
) -> Result<Vec<McpServer>> {
    let Some(SecretValue(raw)) = secrets.get(company, RUNTIME_INDEX_KEY).await? else {
        return Ok(Vec::new());
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&raw)
        .map_err(|e| OpenCompanyError::Store(format!("mcp runtime index is not valid JSON: {e}")))
}

/// Persists the runtime server index.
pub async fn save_runtime_index(
    company: &CompanyId,
    secrets: &dyn SecretStore,
    index: &[McpServer],
) -> Result<()> {
    let raw = serde_json::to_string(index)
        .map_err(|e| OpenCompanyError::Store(format!("serializing mcp runtime index: {e}")))?;
    secrets
        .set(company, RUNTIME_INDEX_KEY, SecretValue(raw))
        .await
}

/// Reads a server's stored credential and resolves it to [`AuthMaterial`].
///
/// The canonical [`auth_key`] (`mcp/{name}/auth`) is tried first — the API
/// (`PUT /mcp/servers/{name}`) writes rotated tokens there. When the canonical
/// key is empty/missing, `override_key` (a manifest server's `auth_secret`)
/// is the fallback for the initial commit-time credential. If neither holds a
/// non-empty value, the result is [`AuthMaterial::None`].
pub async fn load_auth(
    company: &CompanyId,
    name: &str,
    secrets: &dyn SecretStore,
    override_key: Option<&str>,
) -> Result<AuthMaterial> {
    let canonical = auth_key(name);
    // Try the canonical key first — the API writes rotated tokens there.
    let mut raw = None;
    if let Some(SecretValue(r)) = secrets.get(company, &canonical).await?
        && !r.trim().is_empty()
    {
        raw = Some(r);
    }
    // Fall back to the manifest's override key when the canonical key is cold.
    if raw.is_none()
        && let Some(ov) = override_key
        && let Some(SecretValue(r)) = secrets.get(company, ov).await?
    {
        raw = Some(r);
    }
    let Some(raw) = raw else {
        return Ok(AuthMaterial::None);
    };
    if raw.trim().is_empty() {
        return Ok(AuthMaterial::None);
    }
    let stored: StoredAuth = serde_json::from_str(&raw)
        .map_err(|e| OpenCompanyError::Store(format!("mcp auth for `{name}` is not valid: {e}")))?;
    Ok(stored.into())
}

/// Whether a server currently has a credential configured — the non-secret
/// status surfaced by the read APIs. Never returns the value.
pub async fn auth_configured(
    company: &CompanyId,
    server: &McpServer,
    secrets: &dyn SecretStore,
) -> Result<bool> {
    let material = load_auth(
        company,
        &server.name,
        secrets,
        server.auth_secret.as_deref(),
    )
    .await?;
    Ok(material.is_configured())
}

/// Writes a server's outbound credential (write-only intake). The credential is
/// serialized to the canonical [`auth_key`] and never read back out over any
/// API — only [`load_auth`] (harness-build + probe) crosses that boundary.
pub async fn store_auth(
    company: &CompanyId,
    name: &str,
    material: &AuthMaterial,
    secrets: &dyn SecretStore,
) -> Result<()> {
    let stored = match material {
        AuthMaterial::None => {
            // Nothing to store — clear instead so the read-back is "unset".
            return clear_auth(company, name, secrets).await;
        }
        AuthMaterial::Bearer(token) => StoredAuth::Bearer {
            token: token.clone(),
        },
        AuthMaterial::Header { name, value } => StoredAuth::Header {
            name: name.clone(),
            value: value.clone(),
        },
        AuthMaterial::QueryParam { name, value } => StoredAuth::QueryParam {
            name: name.clone(),
            value: value.clone(),
        },
        AuthMaterial::OAuth {
            access_token,
            refresh_token,
            client_id,
            client_secret,
            token_endpoint,
            expires_at,
        } => StoredAuth::Oauth {
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
            client_id: client_id.clone(),
            client_secret: client_secret.clone(),
            token_endpoint: token_endpoint.clone(),
            expires_at: *expires_at,
        },
    };
    let raw = serde_json::to_string(&stored)
        .map_err(|e| OpenCompanyError::Store(format!("serializing mcp auth: {e}")))?;
    secrets
        .set(company, &auth_key(name), SecretValue(raw))
        .await
}

/// Writes a server's bearer token (write-only intake). Thin back-compat wrapper
/// over [`store_auth`]; new callers should build an [`AuthMaterial`] and use
/// [`store_auth`] directly so custom-header / query-param intake share one path.
pub async fn store_bearer(
    company: &CompanyId,
    name: &str,
    token: &str,
    secrets: &dyn SecretStore,
) -> Result<()> {
    store_auth(
        company,
        name,
        &AuthMaterial::Bearer(token.to_string()),
        secrets,
    )
    .await
}

/// Clears a server's stored credential (best-effort — the store has no delete,
/// so an empty value reads back as "not configured").
pub async fn clear_auth(company: &CompanyId, name: &str, secrets: &dyn SecretStore) -> Result<()> {
    secrets
        .set(company, &auth_key(name), SecretValue(String::new()))
        .await
}

/// Clears a server's stored health (best-effort — an empty value reads back as
/// "never probed"). Called when a runtime server is deleted so a later server of
/// the same name never inherits a stale badge.
pub async fn clear_health(
    company: &CompanyId,
    name: &str,
    secrets: &dyn SecretStore,
) -> Result<()> {
    secrets
        .set(company, &health_key(name), SecretValue(String::new()))
        .await
}

/// Loads a server's last recorded health, or `None` when it has never been
/// probed (missing/empty key). A malformed record degrades to `None` rather than
/// erroring — a stale badge is never worth bricking a status read.
pub async fn load_health(
    company: &CompanyId,
    name: &str,
    secrets: &dyn SecretStore,
) -> Result<Option<McpHealth>> {
    let Some(SecretValue(raw)) = secrets.get(company, &health_key(name)).await? else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(serde_json::from_str(&raw).ok())
}

/// Persists a server's probe outcome under [`health_key`].
///
/// The caller is responsible for having scrubbed `health.message` first; this
/// function does not re-scrub (the scrubber needs the known-secret set, which
/// lives at the probe seam). Nothing secret should ever reach here.
pub async fn save_health(
    company: &CompanyId,
    name: &str,
    health: &McpHealth,
    secrets: &dyn SecretStore,
) -> Result<()> {
    let raw = serde_json::to_string(health)
        .map_err(|e| OpenCompanyError::Store(format!("serializing mcp health: {e}")))?;
    secrets
        .set(company, &health_key(name), SecretValue(raw))
        .await
}

/// The company's effective MCP servers with credentials resolved.
///
/// Merges defaults ∪ manifest ∪ runtime index, then fills each decl's
/// [`AuthMaterial`] from its stored secret. This is the single seam the harness
/// builder and the ops discovery route both use so agent-facing resolution and
/// console discovery stay identical.
///
/// `defaults` is the install-wide `[[default_mcp_server]]` list (issue #527),
/// reached at call sites as
/// [`CompanyRuntime::default_mcp_servers`](crate::company::runtime::CompanyRuntime::default_mcp_servers).
/// An install that configures none passes an empty slice, which leaves
/// resolution byte-identical to the two-layer behaviour.
pub async fn resolve_effective(
    company: &CompanyId,
    defaults: &[McpServer],
    manifest: &[McpServer],
    secrets: &dyn SecretStore,
) -> Result<Vec<McpServerDecl>> {
    let runtime = load_runtime_index(company, secrets).await?;
    let mut decls = effective_mcp_servers(defaults, manifest, &runtime);
    for decl in &mut decls {
        // A manifest server may name a custom auth_secret key; runtime servers
        // always use the canonical per-server key. Defaults never carry one —
        // `normalize_default_servers` rejects any entry with a credential-shaped
        // field — so they fall through to the canonical key, which is where the
        // console writes a token the operator adds for a default server later.
        let override_key = manifest
            .iter()
            .find(|m| m.name.trim() == decl.name)
            .and_then(|m| m.auth_secret.clone());
        decl.auth = load_auth(company, &decl.name, secrets, override_key.as_deref()).await?;
        // Never `?`: a caller treats an error out of here as "this company gets
        // no MCP servers", so one unreadable policy key would strip every
        // server from every agent. The gate's face degrades that one server to
        // all-park instead.
        let stored = crate::mcp::policy::load_tool_policies(
            company,
            secrets,
            &crate::mcp::policy::tool_policies_key(&decl.name),
        )
        .await;
        decl.tool_policies = crate::mcp::policy::effective_policies(&decl.read_only_tools, stored);
        // Threaded here, before any route can store a tier default. Without it
        // a tier default resolves against nothing: the tool is never
        // enumerated, so the operator's decision is silently not enforced.
        decl.tool_inventory = crate::mcp::policy::load_tool_inventory(
            company,
            secrets,
            &crate::mcp::policy::tool_inventory_key(&decl.name),
        )
        .await;
    }
    Ok(decls)
}
