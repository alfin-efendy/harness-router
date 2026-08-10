//! Apps screen commands. MCP server definitions persist in SQLite; `probe_app`
//! does a real stdio handshake (initialize → tools/list) or an HTTP
//! reachability check; enabled servers attach to agent sessions for real via
//! `SessionCtx.mcp_servers`. Moved verbatim (per the Move Recipe) from
//! `apps/cockpit/src-tauri/src/apps_cmd.rs`; that file keeps its own copy
//! until the proxy rewrite in Tasks 15-16.

use super::{ok, params, ApiError};
use crate::api::types::*;
use crate::control::ControlPlane;
use crate::domain::{McpServerSpec, McpTransport};
use crate::mcp::{self, McpServerRow};
use crate::serve::ApiState;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

pub(crate) const HANDLES: &[&str] = &[
    "list_apps",
    "add_app",
    "remove_app",
    "probe_app",
    "update_app_scope",
    "set_app_tool_perm",
    "toggle_app_agent",
    "begin_mcp_connect",
    "complete_mcp_connect",
    "disconnect_mcp",
];

#[derive(Deserialize)]
struct InputP {
    input: AddAppInput,
}
#[derive(Deserialize)]
struct IdP {
    id: String,
}
#[derive(Deserialize)]
struct UpdateScopeP {
    id: String,
    scope: String,
    scope_gateways: Vec<String>,
}
#[derive(Deserialize)]
struct ToolPermP {
    id: String,
    tool: String,
    perm: String,
}
#[derive(Deserialize)]
struct ToggleAgentP {
    id: String,
    agent_id: String,
    allowed: bool,
}
#[derive(Deserialize)]
struct McpCompleteConnectP {
    id: String,
    code: String,
    verifier: String,
}

pub(crate) async fn dispatch(state: &ApiState, method: &str, p: Value) -> Result<Value, ApiError> {
    let cp = &state.cp;
    match method {
        "list_apps" => ok(assemble(cp).await?),
        "add_app" => {
            let a: InputP = params(p)?;
            ok(add_app(state, a.input).await?)
        }
        "remove_app" => {
            let a: IdP = params(p)?;
            mcp::remove_server(cp.store(), &a.id).await?;
            ok(assemble(cp).await?)
        }
        "probe_app" => {
            let a: IdP = params(p)?;
            ok(probe_app(state, a.id).await?)
        }
        "update_app_scope" => {
            let a: UpdateScopeP = params(p)?;
            ok(update_app_scope(state, a.id, a.scope, a.scope_gateways).await?)
        }
        "set_app_tool_perm" => {
            let a: ToolPermP = params(p)?;
            mcp::set_tool_perm(cp.store(), &a.id, &a.tool, &a.perm).await?;
            ok(assemble(cp).await?)
        }
        "toggle_app_agent" => {
            let a: ToggleAgentP = params(p)?;
            mcp::set_agent_access(cp.store(), &a.id, &a.agent_id, a.allowed).await?;
            ok(assemble(cp).await?)
        }
        "begin_mcp_connect" => {
            let a: IdP = params(p)?;
            ok(begin_mcp_connect(state, &a.id).await?)
        }
        "complete_mcp_connect" => {
            let a: McpCompleteConnectP = params(p)?;
            ok(complete_mcp_connect(state, &a.id, &a.code, &a.verifier).await?)
        }
        "disconnect_mcp" => {
            let a: IdP = params(p)?;
            ok(disconnect_mcp(state, &a.id).await?)
        }
        _ => Err(ApiError::not_found(format!("unknown method: {method}"))),
    }
}

fn slugify(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    slug.trim_matches('-').replace("--", "-")
}

/// Global Constraint: remote MCP server URLs MUST be `https://` — the MCP
/// spec requires it. Checked wherever a URL is about to be trusted for a
/// remote transport: `add_app` (so a bad URL never gets persisted) and
/// `begin_mcp_connect` (defense in depth — a row could in principle exist
/// with a stale `http://` URL from before this check, e.g. a plugin-synced
/// `[[mcp]]` row, which doesn't go through `add_app` at all).
fn require_https(url: &str) -> Result<(), ApiError> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "remote MCP server URLs must use https://",
        ))
    }
}

fn initial_of(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into())
}

/// Parse `KEY=VALUE` lines into pairs. Lines without a `=` (including blank
/// lines) are dropped, keys and values are whitespace-trimmed, and the value
/// keeps any further `=` characters.
fn parse_env_lines(lines: &[String]) -> Vec<(String, String)> {
    lines
        .iter()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// Classify how the app authenticates: any env var means "env", with the
/// detail listing variable names only (never values); no env vars means
/// "none" with no detail.
fn derive_auth(env: &[(String, String)]) -> (&'static str, Option<String>) {
    let auth_kind = if env.is_empty() { "none" } else { "env" };
    let auth_detail = (!env.is_empty()).then(|| {
        env.iter()
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>()
            .join(", ")
    });
    (auth_kind, auth_detail)
}

async fn assemble(cp: &ControlPlane) -> anyhow::Result<Vec<AppInfo>> {
    let mut out = Vec::new();
    for row in mcp::list_servers(cp.store()).await? {
        let tools = mcp::list_tools(cp.store(), &row.id)
            .await?
            .into_iter()
            .map(|t| ToolInfo {
                name: t.name,
                desc: t.description,
                perm: t.perm,
            })
            .collect();
        // Native-only: "native" is the only agent id.
        let agent_access = vec![AgentAccessInfo {
            agent_id: "native".to_string(),
            allowed: mcp::agent_allowed(cp.store(), &row.id, "native").await?,
        }];
        // A stored MCP OAuth token exists independently of `auth_kind` (that
        // field only ever describes an `env`-derived credential) — looked up
        // per row so a stdio server (never issued one) just reads `None`.
        let mcp_token = cp.store().get_mcp_oauth_token(&row.id).await?;
        let (oauth_token_stored, oauth_reconnect_required) = match &mcp_token {
            Some(t) => (true, t.reconnect_required),
            None => (false, false),
        };
        out.push(AppInfo {
            initial: initial_of(&row.name),
            id: row.id,
            name: row.name,
            kind: row.kind,
            color: row.color,
            desc: row.description,
            transport: row.transport,
            command: row.command,
            args: row.args,
            url: row.url,
            scope: row.scope,
            scope_gateways: row.scope_gateways,
            status: row.status,
            status_detail: row.status_detail,
            version: row.version,
            publisher: row.publisher,
            auth_kind: row.auth_kind,
            auth_detail: row.auth_detail,
            oauth_token_stored,
            oauth_reconnect_required,
            tools,
            agent_access,
            plugin_id: row.plugin_id,
        });
    }
    Ok(out)
}

/// Probe one server and persist status/version/tools.
async fn probe_and_persist(cp: &ControlPlane, id: &str) -> anyhow::Result<()> {
    let Some(mut row) = mcp::get_server(cp.store(), id).await? else {
        anyhow::bail!("unknown app: {id}");
    };
    if row.transport == "http" {
        let url = row.url.clone().unwrap_or_default();
        let ok = match reqwest::Client::builder().timeout(Duration::from_secs(8)).build() {
            Ok(client) => client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .body(
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": {
                            "protocolVersion": "2025-06-18",
                            "capabilities": {},
                            "clientInfo": { "name": "ryuzi-cockpit", "version": env!("CARGO_PKG_VERSION") }
                        }
                    })
                    .to_string(),
                )
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false),
            Err(_) => false,
        };
        row.status = if ok { "connected" } else { "error" }.into();
        row.status_detail = (!ok).then(|| "HTTP initialize failed — check the URL".to_string());
        mcp::upsert_server(cp.store(), row).await?;
        return Ok(());
    }

    let command = row.command.clone().unwrap_or_default();
    let result = mcp::probe_stdio(&command, &row.args, &row.env).await;
    row.status = if result.ok { "connected" } else { "error" }.into();
    row.status_detail = result.error.clone();
    if let Some(v) = &result.server_version {
        row.version = Some(v.clone());
    }
    let tools = result.tools.clone();
    mcp::upsert_server(cp.store(), row).await?;
    if result.ok {
        mcp::replace_tools(cp.store(), id, tools).await?;
    }
    Ok(())
}

async fn add_app(state: &ApiState, input: AddAppInput) -> Result<Vec<AppInfo>, ApiError> {
    let cp = &state.cp;
    let id = input.id.clone().unwrap_or_else(|| slugify(&input.name));
    if id.is_empty() {
        return Err(ApiError::bad_request("app needs a name"));
    }
    // Global Constraint: remote MCP server URLs MUST be https:// — the MCP
    // spec requires it, and this RPC is the add-server boundary the plan
    // calls out to enforce it at (the Cockpit form is the OTHER enforcement
    // point, but a non-Cockpit caller of this RPC must not slip past it).
    if input.transport == "http" {
        require_https(input.url.as_deref().unwrap_or_default())?;
    }
    let env = parse_env_lines(&input.env);
    let (auth_kind, auth_detail) = derive_auth(&env);
    mcp::upsert_server(
        cp.store(),
        McpServerRow {
            id: id.clone(),
            name: input.name,
            kind: input.kind.unwrap_or_else(|| "MCP server".into()),
            color: input.color.unwrap_or_else(|| "#8B8B8B".into()),
            description: input.description,
            transport: input.transport,
            command: input.command,
            args: input.args,
            env,
            url: input.url,
            scope: "global".into(),
            scope_gateways: vec![],
            version: input.version,
            publisher: input.publisher,
            status: "unknown".into(),
            status_detail: None,
            auth_kind: auth_kind.into(),
            auth_detail,
            plugin_id: None,
        },
    )
    .await?;
    // Real handshake right away so the card shows a true status + tool list.
    probe_and_persist(cp, &id).await?;
    Ok(assemble(cp).await?)
}

async fn probe_app(state: &ApiState, id: String) -> Result<Vec<AppInfo>, ApiError> {
    let cp = &state.cp;
    probe_and_persist(cp, &id).await?;
    Ok(assemble(cp).await?)
}

async fn update_app_scope(
    state: &ApiState,
    id: String,
    scope: String,
    scope_gateways: Vec<String>,
) -> Result<Vec<AppInfo>, ApiError> {
    let cp = &state.cp;
    let mut row = mcp::get_server(cp.store(), &id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("unknown app: {id}")))?;
    row.scope = scope;
    row.scope_gateways = scope_gateways;
    mcp::upsert_server(cp.store(), row).await?;
    Ok(assemble(cp).await?)
}

/// Start a remote MCP server's OAuth connect flow (Task 9 — wraps
/// `harness::native::mcp_oauth::begin_mcp_connect`, Task 7). Cockpit calls
/// this, opens the returned `authorize_url` in the browser, and captures the
/// redirect itself (see the plan's Task 9 correction: the loopback callback
/// listener lives in Cockpit's own process, not here).
async fn begin_mcp_connect(state: &ApiState, id: &str) -> Result<McpConnectStart, ApiError> {
    let cp = &state.cp;
    let row = mcp::get_server(cp.store(), id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("unknown app: {id}")))?;
    if row.transport != "http" {
        return Err(ApiError::bad_request(format!(
            "{id} is not a remote (http) server — only a remote MCP server supports OAuth connect"
        )));
    }
    let url = row
        .url
        .clone()
        .ok_or_else(|| ApiError::bad_request(format!("{id} has no URL configured")))?;
    require_https(&url)?;
    let spec = McpServerSpec {
        name: row.id.clone(),
        transport: McpTransport::Http {
            url,
            headers: vec![],
        },
    };
    let http = reqwest::Client::new();
    let start =
        crate::harness::native::mcp_oauth::begin_mcp_connect(cp.store(), &http, &spec).await?;
    Ok(McpConnectStart {
        authorize_url: start.url,
        state: start.state,
        verifier: start.verifier,
    })
}

/// Re-run the RFC 9728 → RFC 8414 discovery chain from scratch to recover
/// the issuer and its token endpoint for the OAuth-code exchange.
/// `begin_mcp_connect` (Task 7) already did this once — and cached the
/// resulting client id in `mcp_oauth_clients` — but it returns only the
/// built authorize URL, not the discovered issuer/metadata, so
/// `complete_mcp_connect` (called minutes later, from a separate request
/// after the browser round-trip) has nothing to recover the token endpoint
/// from except redoing the same deterministic, side-effect-free lookup: the
/// server's own discovery documents don't change between the two calls in
/// any realistic flow, and this performs no registration (client id comes
/// from the store, already persisted by `begin_mcp_connect`).
async fn discover_authorization_server(
    http: &reqwest::Client,
    url: &str,
) -> anyhow::Result<(String, crate::plugins::oauth::OauthServerMetadata)> {
    let probe = http
        .post(url)
        .header("content-type", "application/json")
        .header(
            "MCP-Protocol-Version",
            crate::harness::native::mcp_client::MCP_PROTOCOL_VERSION,
        )
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": crate::harness::native::mcp_client::MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "ryuzi-cockpit", "version": env!("CARGO_PKG_VERSION")}
            }
        }))
        .send()
        .await?;
    if probe.status() != reqwest::StatusCode::UNAUTHORIZED {
        anyhow::bail!(
            "expected a 401 challenge while completing the OAuth connect, got HTTP {}",
            probe.status()
        );
    }
    let header = probe
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("401 response carried no WWW-Authenticate header"))?;
    let metadata_url = crate::plugins::oauth::parse_www_authenticate_resource(header)
        .ok_or_else(|| anyhow::anyhow!("WWW-Authenticate names no protected-resource metadata"))?;
    let issuers =
        crate::harness::native::mcp_oauth::protected_resource_metadata(http, &metadata_url).await?;
    crate::harness::native::mcp_oauth::select_authorization_server(http, &issuers).await
}

/// Complete a remote MCP server's OAuth connect flow: Cockpit's loopback
/// callback captured `code`, and hands it back here with the `verifier` it
/// stashed from `begin_mcp_connect`'s response.
async fn complete_mcp_connect(
    state: &ApiState,
    id: &str,
    code: &str,
    verifier: &str,
) -> Result<Vec<AppInfo>, ApiError> {
    let cp = &state.cp;
    let row = mcp::get_server(cp.store(), id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("unknown app: {id}")))?;
    let url = row
        .url
        .clone()
        .ok_or_else(|| ApiError::bad_request(format!("{id} has no URL configured")))?;
    let http = reqwest::Client::new();
    let (issuer, metadata) = discover_authorization_server(&http, &url).await?;
    let client_id = cp
        .store()
        .get_mcp_oauth_client(&issuer)
        .await?
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "no OAuth client is registered for {issuer} yet — start Connect again"
            ))
        })?;
    crate::harness::native::mcp_oauth::complete_mcp_connect(
        cp.store(),
        &http,
        id,
        &url,
        &metadata.token_endpoint,
        &client_id,
        code,
        verifier,
    )
    .await?;
    Ok(assemble(cp).await?)
}

/// Drop a remote MCP server's stored OAuth token — the Disconnect action
/// `OauthProfileConnections.tsx`'s pattern offers for a plugin profile,
/// mirrored here for a remote server.
async fn disconnect_mcp(state: &ApiState, id: &str) -> Result<Vec<AppInfo>, ApiError> {
    let cp = &state.cp;
    cp.store().delete_mcp_oauth_token(id).await?;
    Ok(assemble(cp).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{dispatch, tests_support::state};
    use serde_json::json;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn env_lines_skip_blanks_and_lines_without_equals() {
        let parsed = parse_env_lines(&lines(&["FOO=bar", "", "no-separator", "BAZ=qux"]));
        assert_eq!(
            parsed,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string()),
            ]
        );
    }

    #[test]
    fn env_lines_trim_key_and_value() {
        let parsed = parse_env_lines(&lines(&[" API_KEY = secret value "]));
        assert_eq!(
            parsed,
            vec![("API_KEY".to_string(), "secret value".to_string())]
        );
    }

    #[test]
    fn env_lines_split_on_first_equals_only() {
        let parsed = parse_env_lines(&lines(&["TOKEN=abc=def"]));
        assert_eq!(parsed, vec![("TOKEN".to_string(), "abc=def".to_string())]);
    }

    #[test]
    fn no_env_means_no_auth() {
        assert_eq!(derive_auth(&[]), ("none", None));
    }

    #[test]
    fn env_auth_lists_variable_names_only() {
        let env = vec![
            ("API_KEY".to_string(), "secret".to_string()),
            ("ORG".to_string(), "acme".to_string()),
        ];
        assert_eq!(derive_auth(&env), ("env", Some("API_KEY, ORG".to_string())));
    }

    #[test]
    fn slugify_lowercases_and_dashes_non_alphanumerics() {
        assert_eq!(slugify("My App!"), "my-app");
        assert_eq!(slugify("sentry"), "sentry");
        assert_eq!(slugify("a  b"), "a-b");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn initial_is_uppercased_first_char_or_placeholder() {
        assert_eq!(initial_of("ryuzi"), "R");
        assert_eq!(initial_of("42nd"), "4");
        assert_eq!(initial_of(""), "?");
    }

    #[tokio::test]
    async fn list_apps_returns_empty_on_fresh_store_via_rpc() {
        let s = state().await;
        let out = dispatch(&s, "list_apps", json!({})).await.unwrap();
        assert_eq!(out, json!([]));
    }

    // ---------- Task 9: https:// enforcement, OAuth connect, reconnect surfacing ----------

    fn http_row(id: &str, url: &str) -> McpServerRow {
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
            status: "unknown".into(),
            status_detail: None,
            auth_kind: "none".into(),
            auth_detail: None,
            plugin_id: None,
        }
    }

    /// PROPERTY: `require_https` is the single gate both `add_app` and
    /// `begin_mcp_connect` call — this pins its exact behavior so a future
    /// edit that loosens the `starts_with` check (e.g. to `contains`, or
    /// dropped entirely) fails here first. Verified by observed failure:
    /// commenting out the `starts_with` check and returning `Ok(())`
    /// unconditionally turns the first assertion red.
    #[test]
    fn require_https_rejects_plain_http_and_accepts_https() {
        assert!(require_https("http://mcp.example.com").is_err());
        assert!(require_https("https://mcp.example.com").is_ok());
    }

    #[tokio::test]
    async fn add_app_rejects_a_plain_http_remote_url_via_rpc() {
        let s = state().await;
        let res = dispatch(
            &s,
            "add_app",
            json!({
                "input": {
                    "id": null,
                    "name": "Insecure",
                    "description": "",
                    "kind": null,
                    "transport": "http",
                    "command": null,
                    "args": [],
                    "env": [],
                    "url": "http://mcp.example.com",
                    "version": null,
                    "publisher": null,
                    "color": null
                }
            }),
        )
        .await;
        let err = res.expect_err("a plain http:// remote URL must be rejected, not silently added");
        assert_eq!(err.status, 400);
        assert!(
            err.message.contains("https://"),
            "the rejection must name the https:// requirement: {}",
            err.message
        );
        assert!(
            mcp::list_servers(s.cp.store()).await.unwrap().is_empty(),
            "a rejected add must not persist a row"
        );
    }

    /// PROPERTY: a stdio server (or any non-http row) must not offer OAuth
    /// connect at all. Verified by observed failure: relaxing the
    /// `row.transport != "http"` guard to always proceed turns this red (the
    /// RPC would then attempt a real network probe against a "stdio"
    /// transport's nonexistent URL and fail with a different, confusing
    /// error rather than this clear 400).
    #[tokio::test]
    async fn begin_mcp_connect_rejects_a_stdio_server() {
        let s = state().await;
        mcp::upsert_server(
            s.cp.store(),
            McpServerRow {
                id: "stdio-app".into(),
                name: "Stdio App".into(),
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
                plugin_id: None,
            },
        )
        .await
        .unwrap();

        let res = dispatch(&s, "begin_mcp_connect", json!({ "id": "stdio-app" })).await;
        let err = res.expect_err("a stdio server must not offer OAuth connect");
        assert_eq!(err.status, 400);
        // Assert the SPECIFIC transport-guard message, not merely "some 400"
        // — a stdio row also has no `url`, so the later "has no URL
        // configured" bad_request would produce a 400 too if the transport
        // check were ever removed, and a status-only assertion would not
        // catch that regression.
        assert!(
            err.message.contains("not a remote (http) server"),
            "expected the transport-guard message, got: {}",
            err.message
        );
    }

    /// Defense in depth: a persisted `http://` row (e.g. one that predates
    /// this check, or a plugin-synced `[[mcp]]` row that never went through
    /// `add_app`) must not be able to start a connect flow either — proven
    /// the same way as `add_app`'s rejection (see `require_https`'s test).
    #[tokio::test]
    async fn begin_mcp_connect_rejects_a_non_https_stored_url() {
        let s = state().await;
        mcp::upsert_server(s.cp.store(), http_row("insecure", "http://mcp.example.com"))
            .await
            .unwrap();

        let res = dispatch(&s, "begin_mcp_connect", json!({ "id": "insecure" })).await;
        let err = res.expect_err("a stored http:// URL must not be usable for OAuth connect");
        assert_eq!(err.status, 400);
        assert!(err.message.contains("https://"), "{}", err.message);
    }

    #[tokio::test]
    async fn begin_mcp_connect_rejects_an_unknown_server_id() {
        let s = state().await;
        let res = dispatch(&s, "begin_mcp_connect", json!({ "id": "nope" })).await;
        let err = res.expect_err("an unknown server id must not silently no-op");
        assert_eq!(err.status, 404);
    }

    /// PROPERTY: `list_apps` must surface a server's stored MCP OAuth token
    /// and its `reconnect_required` flag — the UI's ONLY way to know it
    /// should show "Reconnect" instead of "Connect" (Task 8 landed
    /// `reconnect_required` on the store; nothing read it back out to the
    /// API surface before this task). Verified by observed failure:
    /// reverting `assemble()`'s two new lines (leaving `oauth_token_stored`/
    /// `oauth_reconnect_required` hardcoded to `false`) turns the second and
    /// third assertions red — proving this test does not merely restate the
    /// zero-value default.
    #[tokio::test]
    async fn list_apps_surfaces_a_stored_mcp_oauth_token_and_its_reconnect_flag() {
        let s = state().await;
        mcp::upsert_server(s.cp.store(), http_row("remote", "https://mcp.example.com"))
            .await
            .unwrap();

        let out = dispatch(&s, "list_apps", json!({})).await.unwrap();
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        assert!(!apps[0].oauth_token_stored, "no token stored yet");
        assert!(!apps[0].oauth_reconnect_required);

        s.cp.store()
            .upsert_mcp_oauth_token(
                "remote",
                &crate::store::McpOauthToken {
                    access_token: "tok".into(),
                    refresh_token: None,
                    token_type: "Bearer".into(),
                    expires_at: None,
                    scopes: vec![],
                    reconnect_required: false,
                },
            )
            .await
            .unwrap();
        let out = dispatch(&s, "list_apps", json!({})).await.unwrap();
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        assert!(
            apps[0].oauth_token_stored,
            "a stored token must be surfaced"
        );
        assert!(!apps[0].oauth_reconnect_required);

        s.cp.store()
            .mark_mcp_oauth_reconnect_required("remote")
            .await
            .unwrap();
        let out = dispatch(&s, "list_apps", json!({})).await.unwrap();
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        assert!(
            apps[0].oauth_reconnect_required,
            "a tripped server must surface reconnect_required so the UI can offer Reconnect"
        );
    }

    #[tokio::test]
    async fn disconnect_mcp_clears_the_stored_token() {
        let s = state().await;
        mcp::upsert_server(s.cp.store(), http_row("remote", "https://mcp.example.com"))
            .await
            .unwrap();
        s.cp.store()
            .upsert_mcp_oauth_token(
                "remote",
                &crate::store::McpOauthToken {
                    access_token: "tok".into(),
                    refresh_token: None,
                    token_type: "Bearer".into(),
                    expires_at: None,
                    scopes: vec![],
                    reconnect_required: false,
                },
            )
            .await
            .unwrap();

        let out = dispatch(&s, "disconnect_mcp", json!({ "id": "remote" }))
            .await
            .unwrap();
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        assert!(
            !apps[0].oauth_token_stored,
            "disconnect must clear the stored token"
        );
        assert!(
            s.cp.store()
                .get_mcp_oauth_token("remote")
                .await
                .unwrap()
                .is_none(),
            "the token row itself must be gone, not merely hidden"
        );
    }
}
