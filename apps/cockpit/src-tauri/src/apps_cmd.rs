//! Apps screen commands: thin proxies to the engine daemon's apps RPC
//! family. MCP server definitions persist in SQLite; `probe_app` does a real
//! stdio handshake (initialize → tools/list) or an HTTP reachability check;
//! enabled servers attach to agent sessions for real via
//! `SessionCtx.mcp_servers`.
//!
//! `begin_mcp_connect` is NOT a thin proxy like the rest of this file — Task
//! 9's plan correction (established by Task 7): the loopback OAuth callback
//! listener binds in Cockpit's own process, not the daemon's, because the
//! registered `redirect_uri` (`127.0.0.1:8976` on the USER's machine) must be
//! reachable even when the daemon is remote. The daemon owns discovery / DCR
//! / PKCE state and the token exchange; Cockpit only binds the port, awaits
//! the callback, validates `state` locally, and hands the code back to
//! `complete_mcp_connect` alongside the verifier, issuer token endpoint and
//! client id it stashed from `begin_mcp_connect`'s response — `begin_mcp_connect`
//! already selected the authorization server, so nothing on this path
//! rediscovers it (a second discovery run between authorize and completion
//! could resolve a different authorization server than the one that issued
//! the code). Mirrors `plugins_cmd.rs`'s
//! `plugin_profile_begin_pkce` / `plugin_profile_complete_pkce` exactly,
//! minus the `redirect_uri` round-trip (the daemon computes it itself from
//! the server id via `mcp_oauth::mcp_redirect_uri`, so there is nothing for
//! Cockpit to pass in).

use crate::engine::EngineClient;
use crate::engine_manager::EngineManager;
use crate::error::CmdError;
use ryuzi_core::oauth_loopback;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::State;
use tokio::sync::oneshot;

// `AgentAccessInfo`/`ToolInfo` are only reachable transitively (as fields of
// `AppInfo`) but are re-exported by name anyway for a complete, documented
// DTO surface; specta still emits them via the type graph either way.
#[allow(unused_imports)]
pub use ryuzi_core::api::types::{
    AddAppInput, AgentAccessInfo, AppInfo, McpConnectStart, ToolInfo,
};

type R<T> = Result<T, CmdError>;
type Engine<'a> = State<'a, Arc<EngineManager>>;

#[tauri::command]
#[specta::specta]
pub async fn list_apps(engine: Engine<'_>, runner_id: Option<String>) -> R<Vec<AppInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client.rpc("list_apps", serde_json::json!({})).await
}

#[tauri::command]
#[specta::specta]
pub async fn add_app(
    engine: Engine<'_>,
    runner_id: Option<String>,
    input: AddAppInput,
) -> R<Vec<AppInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc("add_app", serde_json::json!({ "input": input }))
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn remove_app(
    engine: Engine<'_>,
    runner_id: Option<String>,
    id: String,
) -> R<Vec<AppInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc("remove_app", serde_json::json!({ "id": id }))
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn probe_app(
    engine: Engine<'_>,
    runner_id: Option<String>,
    id: String,
) -> R<Vec<AppInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc("probe_app", serde_json::json!({ "id": id }))
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn update_app_scope(
    engine: Engine<'_>,
    runner_id: Option<String>,
    id: String,
    scope: String,
    scope_gateways: Vec<String>,
) -> R<Vec<AppInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "update_app_scope",
            serde_json::json!({ "id": id, "scope": scope, "scope_gateways": scope_gateways }),
        )
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn set_app_tool_perm(
    engine: Engine<'_>,
    runner_id: Option<String>,
    id: String,
    tool: String,
    perm: String,
) -> R<Vec<AppInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "set_app_tool_perm",
            serde_json::json!({ "id": id, "tool": tool, "perm": perm }),
        )
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn toggle_app_agent(
    engine: Engine<'_>,
    runner_id: Option<String>,
    id: String,
    agent_id: String,
    allowed: bool,
) -> R<Vec<AppInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "toggle_app_agent",
            serde_json::json!({ "id": id, "agent_id": agent_id, "allowed": allowed }),
        )
        .await
}

// ---------- Task 9: remote MCP server OAuth connect ----------

/// The MCP OAuth loopback callback port. Must equal the value baked into the
/// daemon's `mcp_oauth::mcp_redirect_uri` (`crates/core/src/harness/native/
/// mcp_oauth.rs`) — duplicated here rather than shared, for the same reason
/// `plugins_cmd.rs`'s `PLUGIN_OAUTH_CALLBACK_PORT` is: the daemon and
/// Cockpit are separate processes with no shared Rust module boundary to
/// draw a single canonical constant from. It also happens to be the SAME
/// port the plugin install wizard's callback listener binds — never live at
/// the same time as an in-flight MCP connect, since both are one-shot,
/// user-initiated flows.
const MCP_OAUTH_CALLBACK_PORT: u16 = 8976;

fn mcp_oauth_callback_path(server_id: &str) -> String {
    format!("/mcp-oauth/{server_id}/callback")
}

fn mcp_oauth_flow_key(server_id: &str, state_token: &str) -> String {
    format!("{server_id}:{state_token}")
}

/// Cancellation handles for pending local loopback callback servers, keyed
/// by `{server_id}:{state_token}` — the MCP-connect analogue of
/// `plugins_cmd.rs`'s `PLUGIN_INSTALL_CANCELS` (a separate map: an MCP
/// server id and a plugin id are different namespaces, and mixing them would
/// let a same-named plugin and MCP server cancel each other's flow).
static MCP_OAUTH_CANCELS: OnceLock<Mutex<HashMap<String, oneshot::Sender<()>>>> = OnceLock::new();

fn mcp_oauth_cancels() -> &'static Mutex<HashMap<String, oneshot::Sender<()>>> {
    MCP_OAUTH_CANCELS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Shut down a live local callback server for `server_id`, if any — fired on
/// a same-server re-begin (Retry) before re-binding the fixed port.
fn cancel_pending_mcp_flow(server_id: &str) {
    let prefix = format!("{server_id}:");
    let mut cancels = mcp_oauth_cancels()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let keys: Vec<String> = cancels
        .keys()
        .filter(|k| k.starts_with(&prefix))
        .cloned()
        .collect();
    for key in keys {
        if let Some(tx) = cancels.remove(&key) {
            let _ = tx.send(());
        }
    }
}

/// Begins a remote MCP server's OAuth connect flow. The daemon
/// (`begin_mcp_connect`) discovers the server's authorization server via
/// RFC 9728, registers (or reuses) a client id, and builds the authorize
/// URL; this command then binds the fixed wizard port, spawns a one-shot
/// callback server at the exact path the daemon's `mcp_redirect_uri` used
/// when building that URL, and awaits the browser redirect in the
/// background. On a captured callback it hands the code + the verifier,
/// issuer token endpoint and client id (all stashed from the daemon's
/// response) to `complete_mcp_connect`.
#[tauri::command]
#[specta::specta]
pub async fn begin_mcp_connect(
    engine: Engine<'_>,
    runner_id: Option<String>,
    id: String,
) -> R<McpConnectStart> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    let start: McpConnectStart = client
        .rpc("begin_mcp_connect", serde_json::json!({ "id": id }))
        .await?;

    // A same-server re-begin (Retry) must shut its previous flow's local
    // callback server down before we try to bind the fixed port again.
    cancel_pending_mcp_flow(&id);

    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let flow_key = mcp_oauth_flow_key(&id, &start.state);
    mcp_oauth_cancels()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(flow_key.clone(), cancel_tx);

    // Bind-retry: a just-canceled previous flow's axum server shuts down
    // asynchronously, so the port can still be held for a moment.
    let mut bound = oauth_loopback::bind_fixed(MCP_OAUTH_CALLBACK_PORT).await;
    for _ in 0..2 {
        if bound.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        bound = oauth_loopback::bind_fixed(MCP_OAUTH_CALLBACK_PORT).await;
    }
    let listener = match bound {
        Ok(listener) => listener,
        Err(err) => {
            mcp_oauth_cancels()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(&flow_key);
            return Err(err.into());
        }
    };

    let (server, result_rx, shutdown_tx) = oauth_loopback::spawn_profile_callback_server(
        listener,
        &mcp_oauth_callback_path(&id),
        start.state.clone(),
    );
    let engine_client = client.clone();
    let task_id = id.clone();
    let verifier = start.verifier.clone();
    // Carried forward alongside the verifier — the authorization server
    // `begin_mcp_connect` actually selected, so the completion step below
    // never has to rediscover it (and risk resolving a different one).
    let issuer_token_endpoint = start.issuer_token_endpoint.clone();
    let connect_client_id = start.client_id.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = tokio::select! {
            res = oauth_loopback::await_callback(
                server,
                result_rx,
                shutdown_tx,
                std::time::Duration::from_secs(5 * 60),
            ) => Some(res),
            // Cancellation (view closed / re-begin): dropping the
            // await_callback future drops shutdown_tx, which shuts the axum
            // server down gracefully.
            _ = cancel_rx => None,
        };
        mcp_oauth_cancels()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&flow_key);
        // Canceled, or the callback capture itself failed (timeout / listener
        // closed): exit silently — the server just stays disconnected and
        // the user can retry, same as the profile flow's cancel path.
        if let Some(Ok(callback)) = outcome {
            complete_local_mcp_callback(
                &engine_client,
                &task_id,
                &verifier,
                &issuer_token_endpoint,
                &connect_client_id,
                callback,
            )
            .await;
        }
    });

    Ok(start)
}

/// Hands a captured loopback callback's `code` + the stashed `verifier` to
/// the daemon's `complete_mcp_connect` for the token exchange. There is no
/// dedicated event/toast for this background completion (unlike the plugin
/// install wizard's `PluginOauthCompletedMsg`) — Cockpit's polling loop
/// (mirroring `OauthProfileConnections.tsx`'s PKCE poll) is what notices the
/// server turn "connected"; a failure here is only logged.
async fn complete_local_mcp_callback(
    engine: &EngineClient,
    id: &str,
    verifier: &str,
    issuer_token_endpoint: &str,
    client_id: &str,
    callback: oauth_loopback::CallbackResult,
) {
    let Some(code) = callback.code else {
        eprintln!("[ryuzi] MCP OAuth callback for {id} did not include a `code` parameter");
        return;
    };
    if let Err(err) = engine
        .rpc::<Vec<AppInfo>>(
            "complete_mcp_connect",
            serde_json::json!({
                "id": id,
                "code": code.trim(),
                "verifier": verifier,
                "issuer_token_endpoint": issuer_token_endpoint,
                "client_id": client_id,
            }),
        )
        .await
    {
        eprintln!(
            "[ryuzi] MCP OAuth completion failed for {id}: {}",
            err.message
        );
    }
}

/// Exposed for symmetry with [`begin_mcp_connect`] (and for direct testing)
/// — the loopback callback task above is the only production caller.
#[tauri::command]
#[specta::specta]
pub async fn complete_mcp_connect(
    engine: Engine<'_>,
    runner_id: Option<String>,
    id: String,
    code: String,
    verifier: String,
    issuer_token_endpoint: String,
    client_id: String,
) -> R<Vec<AppInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "complete_mcp_connect",
            serde_json::json!({
                "id": id,
                "code": code,
                "verifier": verifier,
                "issuer_token_endpoint": issuer_token_endpoint,
                "client_id": client_id,
            }),
        )
        .await
}

/// Drop a remote MCP server's stored OAuth token.
#[tauri::command]
#[specta::specta]
pub async fn disconnect_mcp(
    engine: Engine<'_>,
    runner_id: Option<String>,
    id: String,
) -> R<Vec<AppInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc("disconnect_mcp", serde_json::json!({ "id": id }))
        .await
}
