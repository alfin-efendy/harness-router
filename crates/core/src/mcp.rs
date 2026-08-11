//! Apps (MCP servers) domain: persisted server definitions with per-tool
//! permissions and per-agent access, a real stdio JSON-RPC probe
//! (initialize → tools/list), and the bridge that attaches enabled servers to
//! agent sessions through `SessionCtx.mcp_servers`.

use crate::domain::{McpServerSpec, McpTransport};
use crate::llm_router::secrets::{decrypt_field, encrypt_field};
use crate::stdio_jsonrpc::{self, ReadError};
use crate::store::Store;
use rusqlite::{params, OptionalExtension};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Debug, Clone, PartialEq)]
pub struct McpServerRow {
    /// Slug id — also the MCP server name agents see (`mcp__<id>__<tool>`).
    pub id: String,
    pub name: String,
    pub kind: String,
    pub color: String,
    pub description: String,
    /// stdio | http
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub url: Option<String>,
    /// global | select
    pub scope: String,
    pub scope_gateways: Vec<String>,
    pub version: Option<String>,
    pub publisher: Option<String>,
    /// connected | error | unknown
    pub status: String,
    pub status_detail: Option<String>,
    pub auth_kind: String,
    pub auth_detail: Option<String>,
    /// The plugin that owns this row (Task 7's `[[mcp]]` sync), or `None` for
    /// a user-added-via-Apps-screen server. Set once at sync time and never
    /// user-editable; `servers_for_session` reads it to exclude a disabled
    /// plugin's rows, and `harness::native` reads it (via a fresh
    /// `list_servers` join, not this struct directly) to attribute a plugin
    /// server's tools to their owning plugin in approval prompts.
    pub plugin_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpToolRow {
    pub name: String,
    pub description: String,
    /// allow | ask | deny
    pub perm: String,
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

const SERVER_COLS: &str = "id,name,kind,color,description,transport,command,args,env,url,scope,scope_gateways,version,publisher,status,status_detail,auth_kind,auth_detail,plugin_id";

fn server_from(r: &rusqlite::Row) -> rusqlite::Result<McpServerRow> {
    let args: String = r.get(7)?;
    let env: String = r.get(8)?;
    let scope_gateways: String = r.get(11)?;
    Ok(McpServerRow {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        color: r.get(3)?,
        description: r.get(4)?,
        transport: r.get(5)?,
        command: r.get(6)?,
        args: serde_json::from_str(&args).unwrap_or_default(),
        env: serde_json::from_str::<std::collections::BTreeMap<String, String>>(&env)
            .map(|m| m.into_iter().collect())
            .unwrap_or_default(),
        url: r.get(9)?,
        scope: r.get(10)?,
        scope_gateways: serde_json::from_str(&scope_gateways).unwrap_or_default(),
        version: r.get(12)?,
        publisher: r.get(13)?,
        status: r.get(14)?,
        status_detail: r.get(15)?,
        auth_kind: r.get(16)?,
        auth_detail: r.get(17)?,
        plugin_id: r.get(18)?,
    })
}

pub async fn list_servers(store: &Store) -> anyhow::Result<Vec<McpServerRow>> {
    store
        .with_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {SERVER_COLS} FROM mcp_servers ORDER BY created_at"
            ))?;
            let rows = stmt
                .query_map([], server_from)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

pub async fn get_server(store: &Store, id: &str) -> anyhow::Result<Option<McpServerRow>> {
    let id = id.to_string();
    store
        .with_conn(move |c| {
            c.query_row(
                &format!("SELECT {SERVER_COLS} FROM mcp_servers WHERE id=?1"),
                params![id],
                server_from,
            )
            .optional()
        })
        .await
}

pub async fn upsert_server(store: &Store, row: McpServerRow) -> anyhow::Result<()> {
    let args = serde_json::to_string(&row.args)?;
    let env_map: std::collections::BTreeMap<_, _> = row.env.iter().cloned().collect();
    let env = serde_json::to_string(&env_map)?;
    let scope_gateways = serde_json::to_string(&row.scope_gateways)?;
    let now = crate::paths::now_ms();
    store
        .with_conn(move |c| {
            c.execute(
                &format!(
                    "INSERT INTO mcp_servers({SERVER_COLS},created_at) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20) \
                     ON CONFLICT(id) DO UPDATE SET \
                       name=excluded.name, kind=excluded.kind, color=excluded.color, \
                       description=excluded.description, transport=excluded.transport, \
                       command=excluded.command, args=excluded.args, env=excluded.env, \
                       url=excluded.url, scope=excluded.scope, scope_gateways=excluded.scope_gateways, \
                       version=excluded.version, publisher=excluded.publisher, status=excluded.status, \
                       status_detail=excluded.status_detail, auth_kind=excluded.auth_kind, \
                       auth_detail=excluded.auth_detail, plugin_id=excluded.plugin_id"
                ),
                params![
                    row.id, row.name, row.kind, row.color, row.description, row.transport,
                    row.command, args, env, row.url, row.scope, scope_gateways,
                    row.version, row.publisher, row.status, row.status_detail,
                    row.auth_kind, row.auth_detail, row.plugin_id, now
                ],
            )
            .map(|_| ())
        })
        .await
}

/// Encode a resolved HTTP header set into the JSON `mcp_servers.headers_json`
/// stores, encrypting each header VALUE with [`encrypt_field`] — never the
/// header NAME (`Authorization`, `X-Api-Key`, ...), which is not a secret and
/// stays greppable, and never the array as a single opaque blob (that would
/// make it impossible to encrypt/decrypt values independently the way
/// `store.rs`'s `upsert_mcp_oauth_token_json`/`decode_mcp_oauth_token` do for
/// `mcp_oauth_tokens.token_json` — the pattern this mirrors).
fn encode_server_headers(headers: &[(String, String)]) -> anyhow::Result<String> {
    let array: Vec<serde_json::Value> = headers
        .iter()
        .map(|(name, value)| serde_json::json!({ "name": name, "value": encrypt_field(value) }))
        .collect();
    Ok(serde_json::to_string(&serde_json::Value::Array(array))?)
}

/// Inverse of [`encode_server_headers`]: decrypt each header value back to
/// plaintext. A malformed row (not an array, or a header missing a field)
/// is a hard error — silently dropping a header a caller expects to reach the
/// wire would recreate exactly the kind of silent-auth-failure gap this
/// column exists to close.
fn decode_server_headers(raw: &str) -> anyhow::Result<Vec<(String, String)>> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let array = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("mcp server headers_json must be a JSON array"))?;
    array
        .iter()
        .map(|item| {
            let name = item
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("mcp server header missing name"))?;
            let value = item
                .get("value")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("mcp server header missing value"))?;
            Ok((name.to_string(), decrypt_field(value)?))
        })
        .collect()
}

/// Store the resolved HTTP header set for `server_id` — e.g. the
/// `Authorization` a declarative plugin connector resolved at sync time
/// (`plugins::mcp_sync::sync_plugin_mcp`), or headers a user typed by hand
/// for a server added directly in Apps. A resolved header value is a
/// credential exactly like an MCP OAuth access token, so it is encrypted at
/// rest the same way (see [`encode_server_headers`]) — never written to
/// `headers_json` in the clear.
///
/// Called with the CURRENT full header set on every sync/save, not just the
/// changed entries, so a header a plugin update or user edit removes doesn't
/// linger — an empty slice clears whatever was stored. A no-op (not an
/// error) if `server_id` doesn't exist yet; callers upsert the row first.
pub async fn set_server_headers(
    store: &Store,
    server_id: &str,
    headers: &[(String, String)],
) -> anyhow::Result<()> {
    let server_id = server_id.to_string();
    let headers_json = encode_server_headers(headers)?;
    store
        .with_conn(move |c| {
            c.execute(
                "UPDATE mcp_servers SET headers_json=?2 WHERE id=?1",
                params![server_id, headers_json],
            )
            .map(|_| ())
        })
        .await
}

/// The decrypted HTTP headers stored for `server_id`. Empty for a stdio row,
/// for a server with none stored, and for every row that existed before the
/// `headers_json` column was added (`NULL` decodes to an empty list, not an
/// error) — never an error just because nothing was ever set.
pub async fn get_server_headers(
    store: &Store,
    server_id: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    let server_id = server_id.to_string();
    let raw: Option<Option<String>> = store
        .with_conn(move |c| {
            c.query_row(
                "SELECT headers_json FROM mcp_servers WHERE id=?1",
                params![server_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
        })
        .await?;
    match raw.flatten() {
        Some(raw) => decode_server_headers(&raw),
        None => Ok(Vec::new()),
    }
}

pub async fn remove_server(store: &Store, id: &str) -> anyhow::Result<()> {
    let id = id.to_string();
    store
        .with_conn(move |c| {
            c.execute("DELETE FROM mcp_tools WHERE server_id=?1", params![id])?;
            c.execute(
                "DELETE FROM mcp_agent_access WHERE server_id=?1",
                params![id],
            )?;
            c.execute("DELETE FROM mcp_servers WHERE id=?1", params![id])
                .map(|_| ())
        })
        .await
}

/// Delete every row owned by `plugin_id` (Task 7's `[[mcp]]` sync) — the
/// uninstall counterpart of [`crate::plugins::mcp_sync::sync_plugin_mcp`].
/// Reuses [`remove_server`] per row so tools/agent-access rows and the server
/// row itself all go together, identically to a user removing an Apps card
/// by hand. A no-op (not an error) when `plugin_id` owns no rows.
pub async fn remove_plugin_servers(store: &Store, plugin_id: &str) -> anyhow::Result<()> {
    let ids: Vec<String> = list_servers(store)
        .await?
        .into_iter()
        .filter(|r| r.plugin_id.as_deref() == Some(plugin_id))
        .map(|r| r.id)
        .collect();
    for id in ids {
        remove_server(store, &id).await?;
    }
    Ok(())
}

/// Delete every row `plugin_id` owns whose `id` is NOT in `keep_ids` — the
/// orphan-pruning counterpart of [`remove_plugin_servers`], used when a
/// plugin update's manifest no longer declares an `[[mcp]]` entry it
/// previously synced (F3: without this, a removed server's stale row —
/// perms, tools, agent-access — stuck around forever). Scoped to
/// `plugin_id` only — a row with a different `plugin_id` (including a
/// user-added server, where `plugin_id IS NULL`) is never touched
/// regardless of an id collision. Reuses [`remove_server`] per pruned row
/// so tools/agent-access rows go with it, identically to
/// [`remove_plugin_servers`].
pub async fn prune_plugin_servers(
    store: &Store,
    plugin_id: &str,
    keep_ids: &[String],
) -> anyhow::Result<usize> {
    let ids: Vec<String> = list_servers(store)
        .await?
        .into_iter()
        .filter(|r| r.plugin_id.as_deref() == Some(plugin_id) && !keep_ids.contains(&r.id))
        .map(|r| r.id)
        .collect();
    let pruned = ids.len();
    for id in ids {
        remove_server(store, &id).await?;
    }
    Ok(pruned)
}

pub async fn list_tools(store: &Store, server_id: &str) -> anyhow::Result<Vec<McpToolRow>> {
    let server_id = server_id.to_string();
    store
        .with_conn(move |c| {
            let mut stmt = c.prepare(
                "SELECT name, description, perm FROM mcp_tools WHERE server_id=?1 ORDER BY name",
            )?;
            let rows = stmt
                .query_map(params![server_id], |r| {
                    Ok(McpToolRow {
                        name: r.get(0)?,
                        description: r.get(1)?,
                        perm: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// Replace the discovered tool list, preserving perms for tools that survive.
pub async fn replace_tools(
    store: &Store,
    server_id: &str,
    tools: Vec<(String, String)>,
) -> anyhow::Result<()> {
    let existing = list_tools(store, server_id).await?;
    let server_id = server_id.to_string();
    store
        .with_conn(move |c| {
            c.execute("DELETE FROM mcp_tools WHERE server_id=?1", params![server_id])?;
            for (name, desc) in tools {
                let perm = existing
                    .iter()
                    .find(|t| t.name == name)
                    .map(|t| t.perm.clone())
                    .unwrap_or_else(|| "ask".to_string());
                c.execute(
                    "INSERT INTO mcp_tools(server_id, name, description, perm) VALUES (?1,?2,?3,?4)",
                    params![server_id, name, desc, perm],
                )?;
            }
            Ok(())
        })
        .await
}

pub async fn set_tool_perm(
    store: &Store,
    server_id: &str,
    tool: &str,
    perm: &str,
) -> anyhow::Result<()> {
    let server_id = server_id.to_string();
    let tool = tool.to_string();
    let perm = perm.to_string();
    store
        .with_conn(move |c| {
            c.execute(
                "UPDATE mcp_tools SET perm=?3 WHERE server_id=?1 AND name=?2",
                params![server_id, tool, perm],
            )
            .map(|_| ())
        })
        .await
}

pub async fn agent_access(store: &Store, server_id: &str) -> anyhow::Result<Vec<(String, bool)>> {
    let server_id = server_id.to_string();
    store
        .with_conn(move |c| {
            let mut stmt =
                c.prepare("SELECT agent_id, allowed FROM mcp_agent_access WHERE server_id=?1")?;
            let rows = stmt
                .query_map(params![server_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

pub async fn set_agent_access(
    store: &Store,
    server_id: &str,
    agent_id: &str,
    allowed: bool,
) -> anyhow::Result<()> {
    let server_id = server_id.to_string();
    let agent_id = agent_id.to_string();
    store
        .with_conn(move |c| {
            c.execute(
                "INSERT INTO mcp_agent_access(server_id, agent_id, allowed) VALUES (?1,?2,?3) \
                 ON CONFLICT(server_id, agent_id) DO UPDATE SET allowed=excluded.allowed",
                params![server_id, agent_id, allowed as i64],
            )
            .map(|_| ())
        })
        .await
}

/// Whether `agent_id` may use `server_id` (unset → allowed by default).
pub async fn agent_allowed(store: &Store, server_id: &str, agent_id: &str) -> anyhow::Result<bool> {
    Ok(agent_access(store, server_id)
        .await?
        .into_iter()
        .find(|(a, _)| a == agent_id)
        .map(|(_, allowed)| allowed)
        .unwrap_or(true))
}

// ---------------------------------------------------------------------------
// Session attachment
// ---------------------------------------------------------------------------

/// The MCP servers to attach to a new local session for `agent_id`: enabled
/// scope (global or explicitly including `local`), agent access allowed, and
/// — for a Task 7 plugin-synced row — its owning plugin currently enabled.
/// Disabling a plugin never deletes its rows (perms must survive a
/// disable/enable cycle — [`crate::plugins::mcp_sync::sync_plugin_mcp`]'s
/// doc), so this is the single gate that keeps a disabled plugin's servers
/// out of new sessions. A row's owning plugin is, by construction, always
/// the "connector-only" kind (`declarative_plugin` only ever builds a
/// connector from `manifest.mcp`), whose `PluginHost::is_enabled` default is
/// `false` for an absent key — mirrored here directly via a raw settings
/// read (no `PluginHost` dependency needed) so a missing/never-enabled key
/// excludes the row, matching that default exactly.
///
/// # Credentials
/// An HTTP row's persisted headers are decoded here, and a plugin-owned row
/// additionally gets its owning plugin's LIVE OAuth bearer layered on (see
/// [`with_live_plugin_oauth_bearer`]). Both are per-row best-effort: a row
/// whose stored headers cannot be decrypted is logged and SKIPPED rather than
/// failing the call, because failing the call drops every other server too
/// (the caller has no per-row information to fall back to).
pub async fn servers_for_session(
    store: &Store,
    agent_id: &str,
) -> anyhow::Result<Vec<McpServerSpec>> {
    let mut out = Vec::new();
    for row in list_servers(store).await? {
        let in_scope = row.scope == "global" || row.scope_gateways.iter().any(|g| g == "local");
        if !in_scope || !agent_allowed(store, &row.id, agent_id).await? {
            continue;
        }
        if let Some(plugin_id) = &row.plugin_id {
            let key = crate::plugins::host::qualified_setting_key(plugin_id, "enabled");
            let enabled =
                store.get_setting_raw(&key).await.ok().flatten().as_deref() == Some("true");
            if !enabled {
                continue;
            }
        }
        let transport = match row.transport.as_str() {
            // Task 13: read back whatever `set_server_headers` persisted for
            // this row (a plugin-resolved `Authorization`, or headers a user
            // typed by hand) instead of hardcoding an empty list — the gap
            // that made Task 8's manifest-auth precedence rule unreachable
            // for every plugin-supplied HTTP server. A stdio row never calls
            // this (no lookup, no cost) since only `McpTransport::Http`
            // carries headers at all.
            "http" => match &row.url {
                Some(url) => {
                    // LOG-AND-SKIP, never `?`: `decode_server_headers`
                    // deliberately hard-errors on a header it cannot decrypt
                    // (a rotated/unavailable `llm_router::secrets` key, or a
                    // hand-edited row), and propagating that here would fail
                    // the WHOLE call — which the only caller
                    // (`control::lifecycle`) turns into an empty server list,
                    // silently dropping every OTHER server, stdio ones
                    // included. One broken row must cost exactly one row.
                    let stored = match get_server_headers(store, &row.id).await {
                        Ok(headers) => headers,
                        Err(error) => {
                            tracing::warn!(
                                server = %row.id,
                                "mcp: skipping server — its stored headers could not be decoded \
                                 (rotated/unavailable secret key, or a hand-edited row): {error}"
                            );
                            continue;
                        }
                    };
                    McpTransport::Http {
                        url: url.clone(),
                        headers: with_live_plugin_oauth_bearer(store, &row, stored).await,
                    }
                }
                None => continue,
            },
            _ => match &row.command {
                Some(command) => McpTransport::Stdio {
                    command: command.clone(),
                    args: row.args.clone(),
                    env: row.env.clone(),
                },
                None => continue,
            },
        };
        out.push(McpServerSpec {
            name: row.id.clone(),
            transport,
        });
    }
    Ok(out)
}

/// Layer a plugin's LIVE OAuth bearer onto a plugin-owned HTTP row's stored
/// headers, resolved fresh at session start.
///
/// A plugin whose `[auth] kind = "oauth"` gets its `Authorization: Bearer …`
/// injected into every HTTP `[[mcp]]` spec by
/// `plugins::declarative`'s `build_spec`. That value is a short-lived access
/// token, so `plugins::mcp_sync` deliberately does NOT persist it (see
/// `mcp_sync::persistable_headers`) and it is re-read here instead — once per
/// session start, from the same `plugin_oauth_tokens` row the connector reads.
/// Three properties follow from resolving it here rather than snapshotting it
/// at sync time (which only runs on plugin enable/install/OAuth completion,
/// i.e. possibly days earlier):
///
/// - a session never starts with a bearer older than the session itself;
/// - `disconnect_plugin_oauth` deleting the token takes effect on the next
///   session with nothing to clean up — a revoked account cannot keep sending
///   its old bearer, structurally, rather than because some other handler
///   remembered to clear this column;
/// - a `reconnect_required` token is never used, matching
///   `resolve_http_oauth_bearer_token`'s refusal and `connect_mcp_tools`'s
///   identical filter on the MCP-scoped token store.
///
/// A header already persisted under the same name always wins and short-
/// circuits the lookup: that can only be a static manifest credential (a
/// `${setting:}`/`${env:}` value), which `mcp_sync` DOES persist, and Task 8's
/// precedence rule is that a manifest-supplied `Authorization` wins verbatim.
///
/// Never fails: a store error, a missing token (never connected, or
/// disconnected), or a `reconnect_required` one all leave `headers` exactly as
/// stored — refusing to attach the server over a credential problem would be
/// the same over-reaction the log-and-skip above exists to avoid.
///
/// **Known gap (needs `harness/native`):** an EXPIRED bearer is still sent,
/// with a warning. Refreshing needs the plugin's `Connector`/`ConnectorCtx`
/// (endpoint + client-id resolution + the store write-back), which this
/// module has no access to; and because the injected header then looks like
/// manifest auth, `connect_mcp_tools` passes no `Store` and the transport
/// refuses to refresh on a 401 either.
async fn with_live_plugin_oauth_bearer(
    store: &Store,
    row: &McpServerRow,
    mut headers: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let Some(plugin_id) = row.plugin_id.as_deref() else {
        return headers;
    };
    if headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    {
        return headers;
    }
    let token = match store.get_plugin_oauth_token(plugin_id).await {
        Ok(Some(token)) => token,
        Ok(None) => return headers,
        Err(error) => {
            tracing::warn!(
                server = %row.id,
                plugin = %plugin_id,
                "mcp: could not read this plugin's OAuth token; attaching the server without an \
                 Authorization header: {error}"
            );
            return headers;
        }
    };
    if token.reconnect_required {
        tracing::info!(
            server = %row.id,
            plugin = %plugin_id,
            "mcp: this plugin's OAuth connection needs to be re-established; attaching the \
             server without an Authorization header"
        );
        return headers;
    }
    if crate::plugins::oauth::needs_refresh(crate::paths::now_ms(), token.expires_at) {
        tracing::warn!(
            server = %row.id,
            plugin = %plugin_id,
            "mcp: this plugin's OAuth access token is expired or about to expire and cannot be \
             refreshed from here; the server may answer 401 until the plugin re-syncs"
        );
    }
    headers.push((
        "Authorization".to_string(),
        format!("Bearer {}", token.access_token),
    ));
    headers
}

// ---------------------------------------------------------------------------
// Tool-permission bridge (`mcp__<server>__<tool>` names)
// ---------------------------------------------------------------------------

/// Split a Claude-style MCP tool name into (server, tool).
pub fn mcp_tool_parts(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

/// The persisted per-tool permission (`"allow"`/`"ask"`/`"deny"`) for a
/// `mcp__<server>__<tool>` full name, if `full_name` parses as one and a
/// `mcp_tools` row exists for it — `None` otherwise (not a namespaced name,
/// or no row yet, e.g. a component server before Task 7's sync writes one).
/// Thin rename-wrap of [`tool_perm_for_title`]: same lookup, but this is the
/// name [`crate::harness::native::permission::evaluate`] calls at
/// enforcement time, so the "title" framing (display-only, Apps-UI-era)
/// doesn't leak into the permission gate's vocabulary.
pub async fn stored_tool_perm(store: &Store, full_name: &str) -> Option<String> {
    tool_perm_for_title(store, full_name).await
}

/// The persisted permission for an MCP tool title, if it is one.
pub async fn tool_perm_for_title(store: &Store, title: &str) -> Option<String> {
    let (server, tool) = mcp_tool_parts(title)?;
    let server = server.to_string();
    let tool = tool.to_string();
    store
        .with_conn(move |c| {
            c.query_row(
                "SELECT perm FROM mcp_tools WHERE server_id=?1 AND name=?2",
                params![server, tool],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
        .await
        .ok()
        .flatten()
}

// ---------------------------------------------------------------------------
// Stdio probe (newline-delimited JSON-RPC)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProbeResult {
    pub ok: bool,
    pub server_version: Option<String>,
    pub tools: Vec<(String, String)>,
    pub error: Option<String>,
}

/// Extract the JSON-RPC response with `id` from a line, if it is one.
pub use crate::stdio_jsonrpc::parse_response_line;

/// Pull `(name, description)` pairs out of a `tools/list` result.
pub fn parse_tools_result(v: &serde_json::Value) -> Vec<(String, String)> {
    v.pointer("/result/tools")
        .and_then(|t| t.as_array())
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| {
                    Some((
                        t.get("name")?.as_str()?.to_string(),
                        t.get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Spawn a stdio MCP server, run initialize → tools/list, and tear it down.
pub async fn probe_stdio(command: &str, args: &[String], env: &[(String, String)]) -> ProbeResult {
    match tokio::time::timeout(
        Duration::from_secs(25),
        probe_stdio_inner(command, args, env),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => ProbeResult {
            ok: false,
            error: Some("probe timed out after 25s".into()),
            ..Default::default()
        },
    }
}

async fn probe_stdio_inner(
    command: &str,
    args: &[String],
    env: &[(String, String)],
) -> ProbeResult {
    let fail = |error: String| ProbeResult {
        ok: false,
        error: Some(error),
        ..Default::default()
    };

    // .cmd shims (npx on Windows) must run through cmd.exe.
    let is_shim = cfg!(windows)
        && (command.to_ascii_lowercase().ends_with(".cmd")
            || command.to_ascii_lowercase().ends_with(".bat")
            || !command.contains(['/', '\\', '.']));
    let mut cmd = if is_shim {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(command).args(args);
        c
    } else {
        let mut c = tokio::process::Command::new(command);
        c.args(args);
        c
    };
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::process_util::no_window(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("failed to spawn: {e}")),
    };
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();

    let init = stdio_jsonrpc::build_request(
        1,
        "initialize",
        Some(serde_json::json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "ryuzi-cockpit", "version": env!("CARGO_PKG_VERSION") }
        })),
    );
    if let Err(e) = stdio_jsonrpc::write_line(&mut stdin, &init).await {
        return fail(format!("failed to write initialize: {e}"));
    }

    let init_resp = match stdio_jsonrpc::read_response(&mut lines, 1).await {
        Ok(v) => v,
        Err(ReadError::Closed) => return fail("server closed stdout during initialize".into()),
        Err(ReadError::Io(e)) => return fail(format!("read error: {e}")),
    };
    if let Some(err) = init_resp.get("error") {
        return fail(format!("initialize error: {err}"));
    }
    let server_version = init_resp
        .pointer("/result/serverInfo/version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let initialized = stdio_jsonrpc::build_notification("notifications/initialized", None);
    let _ = stdio_jsonrpc::write_line(&mut stdin, &initialized).await;

    let tools_req = stdio_jsonrpc::build_request(2, "tools/list", None);
    if let Err(e) = stdio_jsonrpc::write_line(&mut stdin, &tools_req).await {
        return fail(format!("failed to write tools/list: {e}"));
    }
    let tools_resp = match stdio_jsonrpc::read_response(&mut lines, 2).await {
        Ok(v) => v,
        Err(ReadError::Closed) => return fail("server closed stdout during tools/list".into()),
        Err(ReadError::Io(e)) => return fail(format!("read error: {e}")),
    };

    let tools = parse_tools_result(&tools_resp);
    let _ = child.kill().await;
    ProbeResult {
        ok: true,
        server_version,
        tools,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_mcp_tool_titles() {
        assert_eq!(
            mcp_tool_parts("mcp__github__create_pr"),
            Some(("github", "create_pr"))
        );
        assert_eq!(
            mcp_tool_parts("mcp__pg__query__nested"),
            Some(("pg", "query__nested"))
        );
        assert_eq!(mcp_tool_parts("Bash"), None);
        assert_eq!(mcp_tool_parts("mcp__justserver"), None);
    }

    #[test]
    fn parses_jsonrpc_frames_and_tools() {
        assert!(parse_response_line("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}", 1).is_some());
        assert!(parse_response_line("{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}", 1).is_none());
        assert!(parse_response_line("not json", 1).is_none());

        let v: serde_json::Value = serde_json::from_str(
            "{\"id\":2,\"result\":{\"tools\":[{\"name\":\"query\",\"description\":\"Run SQL\"},{\"name\":\"bare\"}]}}",
        )
        .unwrap();
        assert_eq!(
            parse_tools_result(&v),
            vec![
                ("query".to_string(), "Run SQL".to_string()),
                ("bare".to_string(), String::new())
            ]
        );
    }

    #[tokio::test]
    async fn server_rows_tools_and_access_roundtrip() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();

        upsert_server(
            &store,
            McpServerRow {
                id: "github".into(),
                name: "GitHub".into(),
                kind: "MCP server".into(),
                color: "#24292F".into(),
                description: "PRs and issues".into(),
                transport: "stdio".into(),
                command: Some("npx".into()),
                args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
                env: vec![("GITHUB_TOKEN".into(), "x".into())],
                url: None,
                scope: "global".into(),
                scope_gateways: vec![],
                version: Some("1.0.0".into()),
                publisher: Some("github".into()),
                status: "unknown".into(),
                status_detail: None,
                auth_kind: "env".into(),
                auth_detail: Some("GITHUB_TOKEN".into()),
                plugin_id: None,
            },
        )
        .await
        .unwrap();

        let rows = list_servers(&store).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].env,
            vec![("GITHUB_TOKEN".to_string(), "x".to_string())]
        );
        assert_eq!(
            rows[0].plugin_id, None,
            "user-added rows carry no plugin_id"
        );

        // Tool discovery keeps perms across refreshes.
        replace_tools(
            &store,
            "github",
            vec![("create_pr".into(), "Open a PR".into())],
        )
        .await
        .unwrap();
        set_tool_perm(&store, "github", "create_pr", "deny")
            .await
            .unwrap();
        replace_tools(
            &store,
            "github",
            vec![
                ("create_pr".into(), "Open a PR".into()),
                ("list_issues".into(), "List issues".into()),
            ],
        )
        .await
        .unwrap();
        let tools = list_tools(&store, "github").await.unwrap();
        assert_eq!(
            tools.iter().find(|t| t.name == "create_pr").unwrap().perm,
            "deny"
        );
        assert_eq!(
            tools.iter().find(|t| t.name == "list_issues").unwrap().perm,
            "ask"
        );

        // Perm lookup by mcp title.
        assert_eq!(
            tool_perm_for_title(&store, "mcp__github__create_pr")
                .await
                .as_deref(),
            Some("deny")
        );
        assert_eq!(tool_perm_for_title(&store, "Bash").await, None);

        // Agent access defaults to allowed until set.
        assert!(agent_allowed(&store, "github", "claude").await.unwrap());
        set_agent_access(&store, "github", "claude", false)
            .await
            .unwrap();
        assert!(!agent_allowed(&store, "github", "claude").await.unwrap());

        // Session attachment honors scope + access.
        let specs = servers_for_session(&store, "claude").await.unwrap();
        assert!(
            specs.is_empty(),
            "denied agent access must exclude the server"
        );
        set_agent_access(&store, "github", "claude", true)
            .await
            .unwrap();
        let specs = servers_for_session(&store, "claude").await.unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "github");

        remove_server(&store, "github").await.unwrap();
        assert!(list_servers(&store).await.unwrap().is_empty());
        assert!(list_tools(&store, "github").await.unwrap().is_empty());
    }

    fn plugin_owned_row(id: &str, plugin_id: &str) -> McpServerRow {
        McpServerRow {
            id: id.into(),
            name: id.into(),
            kind: "MCP server".into(),
            color: "#8B8B8B".into(),
            description: String::new(),
            transport: "stdio".into(),
            command: Some("acme-mcp".into()),
            args: vec![],
            env: vec![],
            url: None,
            scope: "global".into(),
            scope_gateways: vec![],
            version: None,
            publisher: None,
            status: "unknown".into(),
            status_detail: None,
            auth_kind: "none".into(),
            auth_detail: None,
            plugin_id: Some(plugin_id.into()),
        }
    }

    #[tokio::test]
    async fn plugin_id_round_trips_through_upsert_and_list() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();

        upsert_server(&store, plugin_owned_row("acme-main", "acme"))
            .await
            .unwrap();

        let row = get_server(&store, "acme-main").await.unwrap().unwrap();
        assert_eq!(row.plugin_id.as_deref(), Some("acme"));
        let rows = list_servers(&store).await.unwrap();
        assert_eq!(rows[0].plugin_id.as_deref(), Some("acme"));
    }

    #[tokio::test]
    async fn remove_plugin_servers_deletes_only_that_plugins_rows() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();

        upsert_server(&store, plugin_owned_row("acme-main", "acme"))
            .await
            .unwrap();
        upsert_server(&store, plugin_owned_row("other-main", "other"))
            .await
            .unwrap();
        replace_tools(&store, "acme-main", vec![("do_thing".into(), "".into())])
            .await
            .unwrap();

        remove_plugin_servers(&store, "acme").await.unwrap();

        let ids: Vec<_> = list_servers(&store)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec!["other-main".to_string()]);
        assert!(list_tools(&store, "acme-main").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn servers_for_session_excludes_a_disabled_plugins_rows() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();

        upsert_server(&store, plugin_owned_row("acme-main", "acme"))
            .await
            .unwrap();

        // Never enabled — connector-only plugins default to disabled.
        let specs = servers_for_session(&store, "native").await.unwrap();
        assert!(
            specs.is_empty(),
            "a never-enabled plugin's row must not attach, got: {specs:?}"
        );

        store
            .set_setting_raw("plugin.acme.enabled", "true")
            .await
            .unwrap();
        let specs = servers_for_session(&store, "native").await.unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "acme-main");

        store
            .set_setting_raw("plugin.acme.enabled", "false")
            .await
            .unwrap();
        let specs = servers_for_session(&store, "native").await.unwrap();
        assert!(
            specs.is_empty(),
            "an explicitly disabled plugin's row must not attach, got: {specs:?}"
        );

        // The row itself survives the disable — only session attachment is gated.
        assert!(get_server(&store, "acme-main").await.unwrap().is_some());
    }

    // ---------------------------------------------------------------------
    // Task 13: resolved HTTP headers persisted on the mcp_servers row
    // ---------------------------------------------------------------------

    fn plugin_owned_http_row(id: &str, plugin_id: &str, url: &str) -> McpServerRow {
        McpServerRow {
            id: id.into(),
            name: id.into(),
            kind: "MCP server".into(),
            color: "#8B8B8B".into(),
            description: String::new(),
            transport: "http".into(),
            command: None,
            args: vec![],
            env: vec![],
            url: Some(url.into()),
            scope: "global".into(),
            scope_gateways: vec![],
            version: None,
            publisher: None,
            status: "unchecked".into(),
            status_detail: None,
            auth_kind: "none".into(),
            auth_detail: None,
            plugin_id: Some(plugin_id.into()),
        }
    }

    /// The tag [`crate::llm_router::secrets`] writes in front of every
    /// ciphertext (its private `ENC_PREFIX`). Duplicated here rather than
    /// exported: a test that asserts the REAL scheme's marker must fail if
    /// that module's format changes, not silently follow it.
    const ENC_PREFIX: &str = "enc:v1:";

    /// The raw, undecoded `headers_json` column for `server_id` — the whole
    /// point of the encryption-at-rest tests below is to bypass
    /// [`get_server_headers`]'s decrypting accessor.
    async fn raw_headers_json(store: &Store, server_id: &str) -> String {
        let server_id = server_id.to_string();
        store
            .with_conn(move |c| {
                c.query_row(
                    "SELECT headers_json FROM mcp_servers WHERE id=?1",
                    params![server_id],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn set_and_get_server_headers_round_trip() {
        // Not because encode/decode could be key-sensitive (it is symmetric
        // under any key), but because the FIRST test in this binary to touch
        // the process-global cipher decides where its master key comes from:
        // without this, that would be the developer's real OS keychain, and
        // every later `use_test_key_file()` in the binary would be a no-op.
        crate::llm_router::secrets::use_test_key_file();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        upsert_server(
            &store,
            plugin_owned_http_row("acme-http", "acme", "https://acme.example.com/mcp"),
        )
        .await
        .unwrap();

        assert_eq!(
            get_server_headers(&store, "acme-http").await.unwrap(),
            Vec::<(String, String)>::new(),
            "a row with nothing stored yet must decode to an empty list, not error"
        );

        set_server_headers(
            &store,
            "acme-http",
            &[("Authorization".to_string(), "Basic creds".to_string())],
        )
        .await
        .unwrap();
        assert_eq!(
            get_server_headers(&store, "acme-http").await.unwrap(),
            vec![("Authorization".to_string(), "Basic creds".to_string())]
        );

        // Re-setting (what a plugin re-sync does every time) replaces the
        // full set rather than merging — a header a new manifest drops must
        // not linger.
        set_server_headers(&store, "acme-http", &[]).await.unwrap();
        assert_eq!(
            get_server_headers(&store, "acme-http").await.unwrap(),
            Vec::<(String, String)>::new(),
            "setting an empty header list must clear whatever was stored before"
        );
    }

    /// PROPERTY (the test that would have caught Task 13's gap): a
    /// plugin-resolved `Authorization` header, persisted the way
    /// `plugins::mcp_sync::sync_plugin_mcp` persists one, must reach
    /// `servers_for_session`'s output. Before this task `servers_for_session`
    /// hardcoded `headers: vec![]` for every HTTP row, so this exact
    /// assertion would have failed even though the header was sitting right
    /// there in the row.
    #[tokio::test]
    async fn servers_for_session_carries_a_plugin_resolved_authorization_header() {
        crate::llm_router::secrets::use_test_key_file();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        upsert_server(
            &store,
            plugin_owned_http_row(
                "rovo-main",
                "atlassian-rovo",
                "https://mcp.atlassian.com/v1/mcp",
            ),
        )
        .await
        .unwrap();
        set_server_headers(
            &store,
            "rovo-main",
            &[(
                "Authorization".to_string(),
                "Basic manifest-resolved-creds".to_string(),
            )],
        )
        .await
        .unwrap();
        store
            .set_setting_raw("plugin.atlassian-rovo.enabled", "true")
            .await
            .unwrap();

        let specs = servers_for_session(&store, "native").await.unwrap();

        assert_eq!(specs.len(), 1);
        let McpTransport::Http { headers, .. } = &specs[0].transport else {
            panic!("expected an Http transport, got {:?}", specs[0].transport);
        };
        assert_eq!(
            headers,
            &vec![(
                "Authorization".to_string(),
                "Basic manifest-resolved-creds".to_string()
            )],
            "the plugin-resolved Authorization header must reach the session spec, got {headers:?}"
        );
    }

    #[tokio::test]
    async fn server_headers_are_encrypted_at_rest() {
        // A regression that dropped the encrypt_field/decrypt_field calls
        // from encode_server_headers would still pass a plain
        // round-trip-equality test, because decode would simply hand back
        // whatever was written. This test reads the raw column, bypassing
        // the decode path, so a dropped encrypt call is caught directly —
        // same shape as store.rs's mcp_oauth_token_roundtrip_encrypts_at_rest.
        //
        // "not stored verbatim" is NOT the property, though: base64, rot13,
        // or any other reversible obfuscation satisfies that and satisfies
        // the round-trip below too. So the stored value must additionally
        // carry the real scheme's `enc:v1:` marker AND decrypt back under
        // `decrypt_field` — which no non-encryption stand-in can fake.
        crate::llm_router::secrets::use_test_key_file();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        upsert_server(
            &store,
            plugin_owned_http_row(
                "rovo-main",
                "atlassian-rovo",
                "https://mcp.atlassian.com/v1/mcp",
            ),
        )
        .await
        .unwrap();
        set_server_headers(
            &store,
            "rovo-main",
            &[(
                "Authorization".to_string(),
                "Basic super-secret-credential".to_string(),
            )],
        )
        .await
        .unwrap();

        let raw = raw_headers_json(&store, "rovo-main").await;
        assert!(
            !raw.contains("super-secret-credential"),
            "a resolved header value must not be written to disk in the clear: {raw}"
        );
        // The header NAME is not a secret and stays greppable/plain.
        assert!(
            raw.contains("Authorization"),
            "the header name is not a secret and should stay in the clear: {raw}"
        );

        // The stored value is the REAL cipher's output, not merely "something
        // other than the plaintext": it carries `encrypt_field`'s version tag
        // and `decrypt_field` recovers the plaintext from it.
        let stored: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let stored_value = stored[0]["value"].as_str().unwrap();
        assert!(
            stored_value.starts_with(ENC_PREFIX),
            "a stored header value must be tagged with the real scheme's {ENC_PREFIX} marker, \
             not merely obfuscated: {stored_value}"
        );
        assert_eq!(
            crate::llm_router::secrets::decrypt_field(stored_value).unwrap(),
            "Basic super-secret-credential",
            "the stored value must be this process cipher's ciphertext for the plaintext"
        );

        let roundtrip = get_server_headers(&store, "rovo-main").await.unwrap();
        assert_eq!(
            roundtrip,
            vec![(
                "Authorization".to_string(),
                "Basic super-secret-credential".to_string()
            )],
            "decoding must still recover the original plaintext"
        );
    }

    #[tokio::test]
    async fn a_stdio_rows_headers_are_unaffected() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        upsert_server(
            &store,
            McpServerRow {
                id: "github".into(),
                name: "GitHub".into(),
                kind: "MCP server".into(),
                color: "#24292F".into(),
                description: String::new(),
                transport: "stdio".into(),
                command: Some("npx".into()),
                args: vec![],
                env: vec![("GITHUB_TOKEN".into(), "x".into())],
                url: None,
                scope: "global".into(),
                scope_gateways: vec![],
                version: None,
                publisher: None,
                status: "unknown".into(),
                status_detail: None,
                auth_kind: "env".into(),
                auth_detail: Some("GITHUB_TOKEN".into()),
                plugin_id: None,
            },
        )
        .await
        .unwrap();

        // Nothing ever writes headers_json for a stdio row (mcp_sync only
        // calls set_server_headers for http transport), so the column stays
        // NULL and must decode to empty, not error.
        assert_eq!(
            get_server_headers(&store, "github").await.unwrap(),
            Vec::<(String, String)>::new()
        );

        let specs = servers_for_session(&store, "native").await.unwrap();
        assert_eq!(specs.len(), 1);
        assert!(
            matches!(specs[0].transport, McpTransport::Stdio { .. }),
            "a stdio row's transport shape must be unaffected by the headers_json column"
        );
    }

    // ---------------------------------------------------------------------
    // One undecryptable row must cost exactly that row
    // ---------------------------------------------------------------------

    /// Write `headers_json` DIRECTLY, bypassing [`encode_server_headers`] —
    /// the only way to stage a row whose stored header value THIS process
    /// cannot decrypt. Encrypting under a foreign key is the faithful
    /// simulation of the real failure mode (the `llm_router::secrets` master
    /// key rotated, or the keychain unavailable so a different key resolved):
    /// it fails inside `decrypt_field` itself, exactly where a real one would,
    /// rather than tripping the JSON-shape checks first.
    async fn write_undecryptable_headers_json(store: &Store, server_id: &str) {
        let ciphertext = crate::llm_router::secrets::SecretCipher::from_key([42u8; 32])
            .encrypt("Bearer minted-under-a-key-this-process-does-not-have");
        let raw = serde_json::json!([{ "name": "Authorization", "value": ciphertext }]).to_string();
        let server_id = server_id.to_string();
        store
            .with_conn(move |c| {
                c.execute(
                    "UPDATE mcp_servers SET headers_json=?2 WHERE id=?1",
                    params![server_id, raw],
                )
                .map(|_| ())
            })
            .await
            .unwrap();
    }

    async fn enable_plugin(store: &Store, plugin_id: &str) {
        store
            .set_setting_raw(&format!("plugin.{plugin_id}.enabled"), "true")
            .await
            .unwrap();
    }

    /// PROPERTY: a single row whose stored headers cannot be decrypted is
    /// skipped, and every OTHER server still attaches.
    ///
    /// `decode_server_headers` hard-errors by design (silently dropping a
    /// header a caller expects on the wire is the silent-auth-failure gap the
    /// column exists to close), but propagating that out of
    /// `servers_for_session` is strictly worse than the one dropped header:
    /// its only caller cannot recover per row, so ONE unreadable row used to
    /// remove every MCP server — stdio ones, which have no headers at all,
    /// included — from every new session.
    #[tokio::test]
    async fn one_undecryptable_row_does_not_strip_every_other_server() {
        crate::llm_router::secrets::use_test_key_file();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();

        // A stdio server: nothing ever writes headers_json for one, so
        // nothing about it can fail to decrypt — it is pure collateral damage.
        upsert_server(&store, plugin_owned_row("stdio-row", "acme"))
            .await
            .unwrap();
        enable_plugin(&store, "acme").await;
        // A healthy HTTP row with a decryptable header.
        upsert_server(
            &store,
            plugin_owned_http_row("http-ok", "acme", "https://ok.example.com/mcp"),
        )
        .await
        .unwrap();
        set_server_headers(
            &store,
            "http-ok",
            &[("Authorization".to_string(), "Basic good".to_string())],
        )
        .await
        .unwrap();
        // And the poisoned one.
        upsert_server(
            &store,
            plugin_owned_http_row("http-broken", "acme", "https://broken.example.com/mcp"),
        )
        .await
        .unwrap();
        write_undecryptable_headers_json(&store, "http-broken").await;

        let specs = servers_for_session(&store, "native")
            .await
            .expect("one undecryptable row must not fail the whole call");

        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"stdio-row"),
            "a stdio row has no headers to fail on and must always survive, got {names:?}"
        );
        assert!(
            names.contains(&"http-ok"),
            "a healthy http row must survive an unrelated row's decrypt failure, got {names:?}"
        );
        assert!(
            !names.contains(&"http-broken"),
            "the row whose headers cannot be decrypted must be skipped rather than attached \
             with a silently dropped credential, got {names:?}"
        );
    }

    // ---------------------------------------------------------------------
    // A plugin's OAuth bearer is resolved live, never snapshotted
    // ---------------------------------------------------------------------

    fn plugin_oauth_token(
        plugin_id: &str,
        access_token: &str,
    ) -> crate::plugins::oauth::PluginOauthToken {
        crate::plugins::oauth::PluginOauthToken {
            plugin_id: plugin_id.to_string(),
            access_token: access_token.to_string(),
            refresh_token: Some("refresh".into()),
            token_type: "Bearer".into(),
            // Comfortably outside `oauth::needs_refresh`'s 5-minute window so
            // this test never depends on the near-expiry warning path.
            expires_at: Some(crate::paths::now_ms() + 3_600_000),
            scopes: vec![],
            reconnect_required: false,
        }
    }

    /// EVERY `Authorization` header on the spec, not just the first — so a
    /// regression that appends a second one (rather than honoring the
    /// persisted header's precedence) is visible instead of hidden behind a
    /// `find`.
    fn authorizations_of(spec: &McpServerSpec) -> Vec<String> {
        let McpTransport::Http { headers, .. } = &spec.transport else {
            panic!("expected an Http transport, got {:?}", spec.transport);
        };
        headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.clone())
            .collect()
    }

    async fn only_spec(store: &Store) -> McpServerSpec {
        let mut specs = servers_for_session(store, "native").await.unwrap();
        assert_eq!(specs.len(), 1, "expected exactly one attached server");
        specs.remove(0)
    }

    /// PROPERTY: an OAuth plugin's bearer is read from `plugin_oauth_tokens`
    /// at SESSION START, so (a) a token rotated since the last plugin sync is
    /// used, and (b) `disconnect_plugin_oauth` — which deletes only that
    /// table — actually stops the credential, instead of leaving a snapshot in
    /// `headers_json` that keeps being sent from every new session (a local
    /// revocation bypass).
    #[tokio::test]
    async fn a_plugin_oauth_bearer_is_resolved_per_session_and_dies_with_the_token() {
        crate::llm_router::secrets::use_test_key_file();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        upsert_server(
            &store,
            plugin_owned_http_row(
                "acme-oauth-svc",
                "acme-oauth",
                "https://acme.example.com/mcp",
            ),
        )
        .await
        .unwrap();
        enable_plugin(&store, "acme-oauth").await;
        // What `mcp_sync` persists for an OAuth plugin: its static headers
        // only — never the bearer.
        set_server_headers(
            &store,
            "acme-oauth-svc",
            &[("Accept".to_string(), "application/json".to_string())],
        )
        .await
        .unwrap();
        store
            .upsert_plugin_oauth_token(&plugin_oauth_token("acme-oauth", "first-access-token"))
            .await
            .unwrap();

        let spec = only_spec(&store).await;
        assert_eq!(
            authorizations_of(&spec),
            vec!["Bearer first-access-token".to_string()],
            "the live plugin OAuth bearer must reach the session spec"
        );
        let McpTransport::Http { headers, .. } = &spec.transport else {
            unreachable!("checked by authorizations_of")
        };
        assert!(
            headers.contains(&("Accept".to_string(), "application/json".to_string())),
            "injecting the bearer must not drop the row's persisted static headers: {headers:?}"
        );

        // A refresh elsewhere (or a reconnect) rotated the token; no plugin
        // re-sync happened. The next session must use the NEW value.
        store
            .upsert_plugin_oauth_token(&plugin_oauth_token("acme-oauth", "rotated-access-token"))
            .await
            .unwrap();
        assert_eq!(
            authorizations_of(&only_spec(&store).await),
            vec!["Bearer rotated-access-token".to_string()],
            "a bearer rotated since the last plugin sync must be picked up at session start"
        );

        // Exactly what `plugins_api::disconnect_plugin_oauth` does — and
        // nothing else, in particular nothing to `headers_json`.
        store.delete_plugin_oauth_token("acme-oauth").await.unwrap();
        assert!(
            authorizations_of(&only_spec(&store).await).is_empty(),
            "after disconnect no new session may carry the revoked account's bearer"
        );
    }

    /// PROPERTY: a token the host has already marked unusable is not sent —
    /// mirroring `declarative`'s `resolve_http_oauth_bearer_token` refusal and
    /// `connect_mcp_tools`'s identical filter on the MCP-scoped token store.
    #[tokio::test]
    async fn a_reconnect_required_plugin_token_is_never_used_as_a_bearer() {
        crate::llm_router::secrets::use_test_key_file();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        upsert_server(
            &store,
            plugin_owned_http_row(
                "acme-stale-svc",
                "acme-stale",
                "https://acme.example.com/mcp",
            ),
        )
        .await
        .unwrap();
        enable_plugin(&store, "acme-stale").await;
        let mut token = plugin_oauth_token("acme-stale", "unusable-access-token");
        token.reconnect_required = true;
        store.upsert_plugin_oauth_token(&token).await.unwrap();

        assert!(
            authorizations_of(&only_spec(&store).await).is_empty(),
            "a reconnect_required token must never be turned into a bearer"
        );
    }

    /// PROPERTY: the persisted-header path wins and short-circuits the token
    /// lookup — Task 8's rule is that a manifest-supplied `Authorization`
    /// (which for a static `${setting:}` credential IS persisted) is used
    /// verbatim, so live resolution must never overwrite one.
    #[tokio::test]
    async fn a_persisted_static_authorization_is_not_overwritten_by_a_live_token() {
        crate::llm_router::secrets::use_test_key_file();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        upsert_server(
            &store,
            plugin_owned_http_row("acme-both-svc", "acme-both", "https://acme.example.com/mcp"),
        )
        .await
        .unwrap();
        enable_plugin(&store, "acme-both").await;
        set_server_headers(
            &store,
            "acme-both-svc",
            &[(
                "Authorization".to_string(),
                "Basic manifest-resolved-creds".to_string(),
            )],
        )
        .await
        .unwrap();
        store
            .upsert_plugin_oauth_token(&plugin_oauth_token("acme-both", "should-not-be-used"))
            .await
            .unwrap();

        assert_eq!(
            authorizations_of(&only_spec(&store).await),
            vec!["Basic manifest-resolved-creds".to_string()],
            "a persisted manifest credential must win over a live plugin OAuth token, and must \
             not be joined by a second Authorization header"
        );
    }
}
