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
    "list_manual_mcp_oauth_clients",
    "set_manual_mcp_oauth_client",
    "delete_manual_mcp_oauth_client",
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
    /// Carried forward verbatim from the matching `begin_mcp_connect` call's
    /// `McpConnectStart.issuer_token_endpoint` — see `complete_mcp_connect`'s
    /// doc comment for why this must not be rediscovered here.
    issuer_token_endpoint: String,
    /// Carried forward verbatim from `McpConnectStart.client_id`.
    client_id: String,
}
#[derive(Deserialize)]
struct ManualClientP {
    issuer: String,
    client_id: String,
}
#[derive(Deserialize)]
struct IssuerP {
    issuer: String,
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
            ok(complete_mcp_connect(
                state,
                &a.id,
                &a.code,
                &a.verifier,
                &a.issuer_token_endpoint,
                &a.client_id,
            )
            .await?)
        }
        "disconnect_mcp" => {
            let a: IdP = params(p)?;
            ok(disconnect_mcp(state, &a.id).await?)
        }
        "list_manual_mcp_oauth_clients" => ok(manual_oauth_clients(cp).await?),
        "set_manual_mcp_oauth_client" => {
            let a: ManualClientP = params(p)?;
            ok(set_manual_oauth_client(cp, &a.issuer, &a.client_id).await?)
        }
        "delete_manual_mcp_oauth_client" => {
            let a: IssuerP = params(p)?;
            ok(delete_manual_oauth_client(cp, &a.issuer).await?)
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
    // RFC 3986 makes the scheme case-insensitive, so `HTTPS://…` is a valid
    // https URL. Comparing case-sensitively here rejected a URL the form had
    // already accepted (it lowercases first) and that `mcp_http::require_https`
    // accepts too — a bounce with the exact opaque error the form exists to
    // pre-empt, not a security gate.
    if url.to_ascii_lowercase().starts_with("https://") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "remote MCP server URLs must use https://",
        ))
    }
}

/// Does `url` point at the loopback interface? Used only to carve plain-http
/// loopback out of the `https://` requirement on an OAuth token endpoint —
/// RFC 8252 §8.3's reasoning (a loopback request never leaves the machine, so
/// there is no transport to intercept), and the same carve-out
/// `automation::validate_webhook_url` already makes for outbound HTTP in this
/// crate. Only literal loopback IPs and `localhost` qualify; unlike
/// `automation`'s check this does not re-resolve `localhost` to confirm it
/// lands on a loopback address, because the registered-endpoint gate in
/// [`require_registered_token_endpoint`] — not this one — is what actually
/// decides whether an endpoint may be POSTed to.
fn is_loopback_url(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Gate an `issuer_token_endpoint` before the daemon POSTs an authorization
/// code, a PKCE verifier and a client id to it.
///
/// `complete_mcp_connect` takes that endpoint as an RPC PARAMETER, and it is a
/// registered Tauri command — so the webview, or anything else that reaches
/// the control API, chooses the URL. Used verbatim it is both an SSRF lever
/// (the daemon fetches an attacker-named host) and a credential-exfiltration
/// one (`code` + `code_verifier` + `client_id` arrive there in the request
/// body), followed by credential INJECTION: whatever `access_token` comes back
/// lands in `mcp_oauth_tokens` for this server id, and the session path then
/// sends it as the bearer to the real MCP server. Two gates:
///
/// 1. Transport — `https://`, per [`require_https`], with the loopback
///    carve-out [`is_loopback_url`] documents.
/// 2. Binding — the endpoint must EQUAL the `token_endpoint` recorded on some
///    `mcp_oauth_clients` row, and `client_id` must be that row's client id.
///    Only `mcp_oauth::begin_mcp_connect` writes those rows, and only from the
///    RFC 8414 metadata of an authorization server it reached through the MCP
///    server's own RFC 9728 document — so this is what ties the endpoint back
///    to a real discovery run instead of to the caller's say-so. This is the
///    gate doing the real work; the transport one is hygiene.
///
/// The binding used to be checked by DERIVING candidate issuers from the
/// endpoint URL's own path prefixes (bare origin, then each segment) and
/// looking those up. That silently assumed a `token_endpoint` lives under its
/// issuer's path. RFC 8414 requires no such thing, and Atlassian's hosted MCP
/// endpoint is the live counterexample: issuer
/// `https://auth.atlassian.com/<resource-id>`, `token_endpoint`
/// `https://auth.atlassian.com/oauth/token` at the bare origin. No derived
/// candidate could match the registered row, so every completion was refused
/// and the browser flow hung to its five-minute deadline. Comparing against
/// the endpoint the flow actually recorded is both correct for a path-scoped
/// issuer and STRICTER than what it replaces: a caller can no longer name any
/// endpoint that merely shares a prefix with a registered issuer
/// (`https://as.example/tenant1/../../anything` no longer being the point —
/// `https://as.example/token` used to pass on a bare-origin row alone).
async fn require_registered_token_endpoint(
    store: &crate::store::Store,
    issuer_token_endpoint: &str,
    client_id: &str,
) -> Result<(), ApiError> {
    let parsed = url::Url::parse(issuer_token_endpoint).map_err(|e| {
        ApiError::bad_request(format!(
            "invalid OAuth token endpoint {issuer_token_endpoint}: {e}"
        ))
    })?;
    if !is_loopback_url(&parsed) {
        require_https(issuer_token_endpoint)
            .map_err(|_| ApiError::bad_request("the OAuth token endpoint must use https://"))?;
    }
    let registered = store
        .mcp_oauth_client_ids_for_token_endpoint(issuer_token_endpoint)
        .await?;
    if registered.is_empty() {
        return Err(ApiError::bad_request(
            "the OAuth token endpoint does not belong to an authorization server this \
             server has a registered client for",
        ));
    }
    // Any match, not the first row's: several path-scoped issuers at one
    // provider share a token endpoint while each holds its own client id.
    if registered.iter().any(|registered| registered == client_id) {
        return Ok(());
    }
    Err(ApiError::bad_request(
        "the supplied client_id is not the one registered with that authorization server",
    ))
}

/// Validate an `(issuer, client_id)` pair a USER typed, before it becomes a
/// `mcp_oauth_clients` row. Pure and unit-tested — the Tauri command and the
/// RPC handler stay thin, and the rules live where a test can reach them.
///
/// The issuer is NOT normalized beyond trimming surrounding whitespace. It is
/// the PRIMARY KEY `mcp_oauth::begin_mcp_connect` looks the client id up by,
/// and the key that lookup uses is the issuer string the MCP server's own
/// protected-resource metadata listed, VERBATIM. Lowercasing a host or dropping
/// a trailing slash here would produce a row discovery can never find, and the
/// user would be told to supply a client id they had already supplied.
///
/// No token endpoint is accepted, here or anywhere on this path — see
/// [`require_registered_token_endpoint`] for why that is the whole point.
fn validate_manual_oauth_client(
    issuer: &str,
    client_id: &str,
) -> Result<(String, String), ApiError> {
    let issuer = issuer.trim();
    let client_id = client_id.trim();
    if issuer.is_empty() {
        return Err(ApiError::bad_request(
            "an authorization server issuer URL is required",
        ));
    }
    if client_id.is_empty() {
        return Err(ApiError::bad_request("a client id is required"));
    }
    if client_id.len() > 512 {
        return Err(ApiError::bad_request(
            "that client id is too long to be a client id",
        ));
    }
    if client_id
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(ApiError::bad_request(
            "a client id cannot contain spaces or control characters",
        ));
    }
    let parsed = url::Url::parse(issuer)
        .map_err(|e| ApiError::bad_request(format!("invalid issuer URL {issuer}: {e}")))?;
    if !is_loopback_url(&parsed) {
        require_https(issuer).map_err(|_| {
            ApiError::bad_request("an authorization server issuer must use https://")
        })?;
    }
    if parsed.fragment().is_some() || parsed.query().is_some() {
        return Err(ApiError::bad_request(
            "an issuer URL must carry no query string and no fragment",
        ));
    }
    Ok((issuer.to_string(), client_id.to_string()))
}

/// Validate a captured MCP OAuth loopback callback against the `state` this
/// flow's [`begin_mcp_connect`] issued, returning the authorization code to
/// exchange.
///
/// `state` is not decoration, and nothing upstream of this checks it.
/// `oauth_loopback::handle_profile_callback` computes a `validation_ok` solely
/// to pick which HTML page to serve and forwards the `CallbackResult` down the
/// channel either way (by design — otherwise the awaiting side hangs), so the
/// consumer is the ONLY thing between a forged loopback request and a token
/// exchange. The listener holds a FIXED port (8976) on a guessable path
/// (`/mcp-oauth/{server_id}/callback`, and a server id is just
/// `{plugin_id}-{mcp.name}`) for up to five minutes, so any local process — or
/// any page the user has open — can spend its one-shot slot with
/// `?code=<attacker's>&state=anything`. Unchecked, that either silently kills
/// the connect flow (an authorization server that binds PKCE strictly rejects
/// the mismatched verifier) or stores the ATTACKER's token for this server,
/// which every later agent turn then reads and writes through.
///
/// This lives in the daemon crate, and is `pub`, for a specific reason: the
/// loopback listener runs in COCKPIT's process (`apps/cockpit/src-tauri/src/
/// apps_cmd.rs` — the registered `redirect_uri` must be reachable even when
/// the daemon is remote), so the comparison has to happen there, but a rule
/// deciding whether an authorization code is trusted must not sit in a crate
/// no test suite reaches: `cargo test -p ryuzi-cockpit` cannot start at all on
/// Windows (tauri#13419, STATUS_ENTRYPOINT_NOT_FOUND) and CI runs
/// `cargo test` for `ryuzi-core`/`ryuzi-control`/`ryuzi-plugin-sdk` only.
/// Defined here it is covered by this module's tests; Cockpit calls it.
pub fn validate_mcp_oauth_callback(
    callback: &crate::oauth_loopback::CallbackResult,
    expected_state: &str,
) -> Result<String, String> {
    let Some(code) = callback
        .code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty())
    else {
        return Err("the callback carried no `code` parameter".into());
    };
    let Some(state) = callback.state.as_deref() else {
        return Err("the callback carried no `state` parameter".into());
    };
    if state != expected_state {
        return Err(
            "the callback's `state` does not match the state this flow issued — discarding it"
                .into(),
        );
    }
    Ok(code.to_string())
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

/// The exact `McpServerSpec` — URL *and credential* — a native session would
/// open for this HTTP row.
///
/// Both of this module's HTTP paths take their answer from here, and that is
/// the point: [`probe_and_persist`], so a probe result predicts the session it
/// claims to describe, and [`assemble`]'s `oauth_connect_available`, so the
/// Connection card offers OAuth connect only when a token connected through it
/// would actually be used.
///
/// Sourced from [`mcp::servers_for_session`] whenever that function attaches
/// the row, because THAT is what decides what a session's transport carries —
/// the row's persisted headers today, plus (for a plugin-owned row) the owning
/// plugin's live OAuth bearer. Re-deriving any of that here is how the two
/// drift apart. A row it does NOT attach (out of scope, agent access revoked,
/// owning plugin disabled) has no session behaviour to agree with, so it falls
/// back to the row's persisted headers — which is that function's own starting
/// point for the moment the row becomes attachable again.
///
/// `attachable` is `servers_for_session`'s output, passed in rather than
/// fetched per row so `assemble` pays for it once per `list_apps`.
///
/// `None` means the row's stored headers could not be DECODED — a rotated or
/// unavailable `llm_router::secrets` key, or a hand-edited row.
/// `get_server_headers` hard-errors on that, and propagating it would fail the
/// whole of `assemble`, i.e. blank the entire Apps screen over one bad row; so
/// this is log-and-skip, exactly as `mcp::servers_for_session` treats the same
/// failure (it drops that one row and attaches every other server). Both
/// callers then fail CLOSED: no OAuth affordance and a red probe with the
/// decode error, which is honest — a session cannot attach this server either.
async fn http_session_spec(
    store: &crate::store::Store,
    row: &McpServerRow,
    attachable: &[McpServerSpec],
) -> Option<McpServerSpec> {
    if let Some(spec) = attachable.iter().find(|spec| spec.name == row.id) {
        return Some(spec.clone());
    }
    match mcp::get_server_headers(store, &row.id).await {
        Ok(headers) => Some(McpServerSpec {
            name: row.id.clone(),
            transport: McpTransport::Http {
                url: row.url.clone().unwrap_or_default(),
                headers,
            },
        }),
        Err(error) => {
            tracing::warn!(
                server = %row.id,
                "apps: this server's stored headers could not be decoded (rotated/unavailable \
                 secret key, or a hand-edited row): {error}"
            );
            None
        }
    }
}

/// The settings key holding the last OAuth connect failure for a server, in
/// the same `<namespace>.<id>.<field>` shape
/// `crate::plugins::qualified_setting_key` uses for a plugin's own settings.
/// A settings row rather than a column on `mcp_servers`: `status`/
/// `status_detail` there describe the last PROBE and are overwritten by the
/// re-probe that every connect attempt ends with, which would erase this the
/// moment it was written.
fn connect_error_key(server_id: &str) -> String {
    format!("mcp.{server_id}.connect_error")
}

/// Record why a connect attempt failed, so `assemble` can hand it to the card
/// that is polling. Best-effort by construction: this runs on an error path
/// that is already being returned to the caller, and losing the annotation
/// must never replace that error with a storage one.
async fn note_connect_failure(cp: &ControlPlane, server_id: &str, reason: &str) {
    if let Err(e) = cp
        .store()
        .set_setting_raw(&connect_error_key(server_id), reason)
        .await
    {
        tracing::warn!("apps: could not record the connect failure for {server_id}: {e:#}");
    }
}

async fn clear_connect_failure(cp: &ControlPlane, server_id: &str) -> Result<(), ApiError> {
    cp.store()
        .delete_setting_raw(&connect_error_key(server_id))
        .await?;
    Ok(())
}

async fn assemble(cp: &ControlPlane) -> anyhow::Result<Vec<AppInfo>> {
    let mut out = Vec::new();
    // Fetched once, not per row — see `http_session_spec`.
    let attachable = mcp::servers_for_session(cp.store(), "native").await?;
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
        // Whether THIS host owns the server's credential, decided by the one
        // predicate the session path branches on
        // (`harness::native::mcp_http_credential`) applied to the spec a
        // session would really open. `transport == "http"` used to stand in
        // for it in the UI, which merely CORRELATES: a row authenticating
        // with a manifest `Authorization` header (atlassian-rovo's
        // `Basic ${setting:…}`) got the whole OAuth Connect block, and the
        // token a completed consent stored was then never used by anything.
        let oauth_connect_available = if row.transport == "http" {
            http_session_spec(cp.store(), &row, &attachable)
                .await
                .and_then(|spec| crate::harness::native::mcp_http_credential_of(&spec))
                .is_some_and(crate::harness::native::McpHttpCredential::host_managed)
        } else {
            false
        };
        let oauth_connect_error = cp
            .store()
            .get_setting_raw(&connect_error_key(&row.id))
            .await?;
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
            oauth_connect_available,
            oauth_token_stored,
            oauth_reconnect_required,
            oauth_connect_error,
            tools,
            agent_access,
            plugin_id: row.plugin_id,
        });
    }
    Ok(out)
}

/// How long the HTTP probe may take in total.
///
/// The probe shares `harness::native::mcp_http`'s connection path, whose own
/// deadline is sized for an agent turn (120 s). This is a button a human is
/// watching, so it is bounded far tighter: an unreachable or wedged server has
/// to become a red dot in seconds, not minutes.
const HTTP_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Probe one server and persist status/version/tools.
async fn probe_and_persist(cp: &ControlPlane, id: &str) -> anyhow::Result<()> {
    let Some(mut row) = mcp::get_server(cp.store(), id).await? else {
        anyhow::bail!("unknown app: {id}");
    };
    if row.transport == "http" {
        // The real client, with the real credential: this used to POST a bare
        // `initialize` with NO `Authorization` header at all — not the
        // manifest-resolved one, not the stored OAuth token — so every
        // auth-gated remote server reported `error` / "HTTP initialize failed
        // — check the URL" for a URL that was correct, and (because it also
        // never called `replace_tools`) no remote server ever got `mcp_tools`
        // rows, which is what per-tool permissions and the Tools tab need.
        // `http_session_spec` + `open_http_mcp` mean the probe now
        // authenticates exactly as the session it is predicting does.
        let attachable = mcp::servers_for_session(cp.store(), "native").await?;
        let (ok, detail, tools) = match http_session_spec(cp.store(), &row, &attachable).await {
            Some(spec) => {
                let opened = tokio::time::timeout(
                    HTTP_PROBE_TIMEOUT,
                    crate::harness::native::open_http_mcp(cp.store(), &spec),
                )
                .await;
                match opened {
                    // `open_http_mcp` has already done `initialize` +
                    // `tools/list`.
                    Ok(Ok(conn)) => {
                        let tools = conn
                            .tools
                            .iter()
                            .map(|t| (t.name.clone(), t.description.clone()))
                            .collect::<Vec<_>>();
                        // The probe is finished with this server the moment it
                        // has the tool list. Without this DELETE every Probe
                        // click — and every re-probe after an OAuth connect —
                        // leaves a session allocated on the remote server
                        // until that server's own idle timeout expires.
                        conn.terminate().await;
                        (true, None, tools)
                    }
                    // `{e:#}` — the whole error chain. The single-line "check
                    // the URL" this replaced was actively misleading for the
                    // failure it saw most: a 401 from a server whose URL was
                    // fine.
                    Ok(Err(e)) => (false, Some(format!("{e:#}")), Vec::new()),
                    Err(_) => (
                        false,
                        Some(format!(
                            "the server did not answer within {}s",
                            HTTP_PROBE_TIMEOUT.as_secs()
                        )),
                        Vec::new(),
                    ),
                }
            }
            // Undecodable stored headers — see `http_session_spec`. A session
            // cannot attach this server either, so say so instead of probing
            // it without the credential it is supposed to carry.
            None => (
                false,
                Some(
                    "this server's stored headers could not be decoded — the secret key that \
                     encrypted them is unavailable or has been rotated"
                        .to_string(),
                ),
                Vec::new(),
            ),
        };
        row.status = if ok { "connected" } else { "error" }.into();
        row.status_detail = detail;
        mcp::upsert_server(cp.store(), row).await?;
        if ok {
            // Same call the stdio path makes, and the reason per-tool
            // permissions exist for a remote server at all. `replace_tools`
            // preserves the perm of every tool that survives.
            mcp::replace_tools(cp.store(), id, tools).await?;
        }
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
    // A new attempt starts clean — otherwise the card would render the
    // previous attempt's failure over a flow that is only just beginning.
    clear_connect_failure(cp, id).await?;
    let spec = McpServerSpec {
        name: row.id.clone(),
        transport: McpTransport::Http {
            url,
            headers: vec![],
        },
    };
    // Hardened, not bare: this client is what `select_authorization_server` and
    // `register_oauth_client` borrow, and both would otherwise follow redirects
    // with no deadline. See `hardened_http_client`.
    let http = crate::harness::native::mcp_http::hardened_http_client()?;
    // Annotated onto the row, not merely returned: the reason a connect could
    // not even START is often "this authorization server does not do dynamic
    // registration, record a client id for <issuer>" — and the issuer is a URL
    // the user has to copy. Returned alone it is a toast that disappears;
    // recorded here it renders on the card (`AppInfo::oauth_connect_error`),
    // the same way a completion failure already does.
    let start = match crate::harness::native::mcp_oauth::begin_mcp_connect(cp.store(), &http, &spec)
        .await
    {
        Ok(start) => start,
        Err(e) => {
            let reason = format!("{e:#}");
            note_connect_failure(cp, id, &reason).await;
            return Err(ApiError::bad_request(reason));
        }
    };
    Ok(McpConnectStart {
        authorize_url: start.url,
        state: start.state,
        verifier: start.verifier,
        // Carried forward so `complete_mcp_connect` never has to rediscover
        // the authorization server this flow selected.
        issuer_token_endpoint: start.issuer_token_endpoint,
        client_id: start.client_id,
    })
}

/// Complete a remote MCP server's OAuth connect flow: Cockpit's loopback
/// callback captured `code`, and hands it back here with the `verifier`,
/// `issuer_token_endpoint` and `client_id` it stashed from
/// `begin_mcp_connect`'s `McpConnectStart` response.
///
/// This deliberately does NOT rediscover the authorization server. An
/// earlier version of this handler re-ran the full RFC 9728 → RFC 8414
/// discovery chain here to recover the issuer and token endpoint, because
/// `begin_mcp_connect`'s response didn't carry them. That reintroduced the
/// exact hazard `harness::native::mcp_oauth::complete_mcp_connect`'s design
/// was meant to avoid: discovery run a second time, minutes later, from a
/// separate request, could resolve a DIFFERENT authorization server than the
/// one that actually issued the code (e.g. a PRM document listing several,
/// the first becoming reachable or unreachable in between) — and it cost an
/// extra round of read-only HTTP requests on every connect completion.
/// `McpConnectStart` now carries `issuer_token_endpoint`/`client_id`
/// forward from the exact authorization server `begin_mcp_connect` selected;
/// use those, not a fresh lookup.
///
/// Carried forward is not the same as trusted, though: both values arrive as
/// RPC parameters on a registered Tauri command, so
/// [`require_registered_token_endpoint`] has to tie them back to a real
/// discovery run before anything is POSTed anywhere.
async fn complete_mcp_connect(
    state: &ApiState,
    id: &str,
    code: &str,
    verifier: &str,
    issuer_token_endpoint: &str,
    client_id: &str,
) -> Result<Vec<AppInfo>, ApiError> {
    let cp = &state.cp;
    let row = mcp::get_server(cp.store(), id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("unknown app: {id}")))?;
    let url = row
        .url
        .clone()
        .ok_or_else(|| ApiError::bad_request(format!("{id} has no URL configured")))?;
    // Before anything leaves the machine: the caller named this endpoint.
    //
    // Every failure below is annotated onto the server row before it is
    // returned. The sole production caller is a spawned Cockpit task that
    // discards this Result (`complete_local_mcp_callback`), so the returned
    // error reaches no user on its own — see `AppInfo::oauth_connect_error`.
    if let Err(e) = require_registered_token_endpoint(cp.store(), issuer_token_endpoint, client_id)
        .await
        .map_err(|e| {
            // The endpoint is named because a refusal here is nearly always a
            // discovery/registration mismatch, and without it the message is
            // unactionable.
            ApiError::bad_request(format!("{}: {issuer_token_endpoint}", e.message))
        })
    {
        note_connect_failure(cp, id, &e.message).await;
        return Err(e);
    }
    // Hardened, not bare — the token exchange POSTs `code`/`code_verifier` in a
    // form body, which redirect header-stripping cannot protect at all.
    let http = crate::harness::native::mcp_http::hardened_http_client()?;
    if let Err(e) = crate::harness::native::mcp_oauth::complete_mcp_connect(
        cp.store(),
        &http,
        id,
        &url,
        issuer_token_endpoint,
        client_id,
        code,
        verifier,
    )
    .await
    {
        // `{e:#}` — the whole chain. The outer layer here is usually just
        // "token exchange failed", with the HTTP status underneath it.
        note_connect_failure(cp, id, &format!("{e:#}")).await;
        return Err(e.into());
    }
    clear_connect_failure(cp, id).await?;
    // Re-probe with the token that was just stored, before returning the list
    // this RPC's caller renders. Without it a successful connect left the card
    // showing whatever the last (unauthenticated, therefore failed) probe
    // wrote: a red Error dot and an empty Tools tab sitting next to an "OAuth
    // connected" pill. Best-effort — the token IS stored and usable, so a
    // server that is momentarily unreachable must not turn a completed connect
    // into a failed RPC; the row simply keeps its previous status.
    if let Err(e) = probe_and_persist(cp, id).await {
        tracing::warn!("apps: re-probe after OAuth connect failed for {id}: {e:#}");
    }
    Ok(assemble(cp).await?)
}

/// Drop a remote MCP server's stored OAuth token — the Disconnect action
/// `OauthProfileConnections.tsx`'s pattern offers for a plugin profile,
/// mirrored here for a remote server.
async fn disconnect_mcp(state: &ApiState, id: &str) -> Result<Vec<AppInfo>, ApiError> {
    let cp = &state.cp;
    cp.store().delete_mcp_oauth_token(id).await?;
    // A deliberate disconnect ends whatever the last attempt's failure was
    // saying, so it must not survive to caption the "Not connected" state.
    clear_connect_failure(cp, id).await?;
    Ok(assemble(cp).await?)
}

/// Every client id the user recorded by hand, for the Cockpit surface that
/// lists and removes them.
async fn manual_oauth_clients(cp: &ControlPlane) -> Result<Vec<ManualMcpOauthClient>, ApiError> {
    Ok(cp
        .store()
        .list_user_asserted_mcp_oauth_clients()
        .await?
        .into_iter()
        .map(|(issuer, client_id)| ManualMcpOauthClient { issuer, client_id })
        .collect())
}

/// Record a client id for an authorization server that offers no dynamic
/// registration. `mcp_oauth::begin_mcp_connect` reuses a stored client id for
/// an issuer verbatim, so recording one here is exactly what unblocks the next
/// Connect — no other wiring is needed for it to take effect.
async fn set_manual_oauth_client(
    cp: &ControlPlane,
    issuer: &str,
    client_id: &str,
) -> Result<Vec<ManualMcpOauthClient>, ApiError> {
    let (issuer, client_id) = validate_manual_oauth_client(issuer, client_id)?;
    cp.store()
        .upsert_user_asserted_mcp_oauth_client(&issuer, &client_id)
        .await?;
    manual_oauth_clients(cp).await
}

async fn delete_manual_oauth_client(
    cp: &ControlPlane,
    issuer: &str,
) -> Result<Vec<ManualMcpOauthClient>, ApiError> {
    cp.store()
        .delete_user_asserted_mcp_oauth_client(issuer.trim())
        .await?;
    manual_oauth_clients(cp).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{dispatch, tests_support::state};
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

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
        // RFC 3986: the scheme is case-insensitive. A case-sensitive check here
        // bounced a URL the Add-server form had already accepted, since the form
        // lowercases before validating.
        assert!(
            require_https("HTTPS://mcp.example.com").is_ok(),
            "an uppercase scheme is still https"
        );
        assert!(
            require_https("HTTP://mcp.example.com").is_err(),
            "an uppercase plain-http scheme must still be rejected"
        );
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

    // ---------- the loopback callback's `state` must be compared ----------

    fn callback(code: Option<&str>, state: Option<&str>) -> crate::oauth_loopback::CallbackResult {
        crate::oauth_loopback::CallbackResult {
            code: code.map(str::to_string),
            state: state.map(str::to_string),
        }
    }

    /// PROPERTY: a callback whose `state` is not the state this flow issued
    /// must be REJECTED, not exchanged. This is the whole security value of
    /// generating a state at all on this path — `oauth_loopback`'s server
    /// forwards every `CallbackResult` down the channel regardless of what it
    /// thinks of it (it only varies the HTML page), so the consumer is the
    /// only check that exists.
    ///
    /// Verified by observed failure: deleting the `state != expected_state`
    /// arm from `validate_mcp_oauth_callback` turns the mismatch assertion
    /// red (a forged state comes back `Ok("attacker-code")`), and deleting the
    /// missing-`state` arm turns the `None` assertion red the same way.
    #[test]
    fn a_callback_state_that_does_not_match_the_issued_state_is_rejected() {
        assert_eq!(
            validate_mcp_oauth_callback(&callback(Some("real-code"), Some("s-1")), "s-1"),
            Ok("real-code".to_string()),
            "the authorization server's own redirect must still go through"
        );

        let forged =
            validate_mcp_oauth_callback(&callback(Some("attacker-code"), Some("anything")), "s-1")
                .expect_err("a mismatched state must not yield a code to exchange");
        assert!(forged.contains("does not match"), "{forged}");

        let stateless = validate_mcp_oauth_callback(&callback(Some("attacker-code"), None), "s-1")
            .expect_err("a callback with no state at all must not yield a code either");
        assert!(stateless.contains("no `state`"), "{stateless}");
    }

    /// A state match is exact — no prefix, suffix or case latitude, since a
    /// guessable-prefix acceptance would hand the whole check away.
    #[test]
    fn state_comparison_is_exact() {
        for wrong in ["s-1 ", " s-1", "s-11", "s-", "S-1", ""] {
            assert!(
                validate_mcp_oauth_callback(&callback(Some("c"), Some(wrong)), "s-1").is_err(),
                "state {wrong:?} must not be accepted for expected state \"s-1\""
            );
        }
    }

    /// `code` handling: absent, blank, or whitespace-only is nothing to
    /// exchange; a real code comes back trimmed (the trim used to happen at
    /// the call site, and moved in here with the rest of the validation).
    #[test]
    fn a_callback_without_a_usable_code_is_rejected_and_a_real_one_is_trimmed() {
        for empty in [None, Some(""), Some("   ")] {
            let err =
                validate_mcp_oauth_callback(&callback(empty, Some("s-1")), "s-1").unwrap_err();
            assert!(err.contains("no `code`"), "{err}");
        }
        assert_eq!(
            validate_mcp_oauth_callback(&callback(Some("  c-1\n"), Some("s-1")), "s-1"),
            Ok("c-1".to_string())
        );
    }

    // ---------- the token endpoint is caller-supplied, so it is gated ----------

    /// PROPERTY: a token endpoint the caller invented must be rejected on
    /// transport grounds before anything else — `complete_mcp_connect` is a
    /// registered Tauri command, so `http://attacker.test/token` is one
    /// webview call away, and the request body carries `code` +
    /// `code_verifier` + `client_id`.
    ///
    /// Verified by observed failure: dropping the `require_https` arm from
    /// `require_registered_token_endpoint` turns the first assertion's
    /// message check red (the URL then falls through to the binding gate and
    /// is rejected for the wrong reason).
    #[tokio::test]
    async fn a_plain_http_token_endpoint_is_rejected_unless_it_is_loopback() {
        let s = state().await;
        let store = s.cp.store();
        store
            .upsert_mcp_oauth_client("http://attacker.test", "c", "http://attacker.test/token")
            .await
            .unwrap();
        let err = require_registered_token_endpoint(store, "http://attacker.test/token", "c")
            .await
            .expect_err("a plain http:// token endpoint must not be POSTed to");
        assert_eq!(err.status, 400);
        assert!(
            err.message.contains("https://"),
            "the rejection must be the transport one — a registered row exists, so the \
             binding gate would have let this through: {}",
            err.message
        );

        // Loopback is the documented carve-out (and what every mock
        // authorization server in this crate binds).
        store
            .upsert_mcp_oauth_client("http://127.0.0.1:9", "c", "http://127.0.0.1:9/token")
            .await
            .unwrap();
        require_registered_token_endpoint(store, "http://127.0.0.1:9/token", "c")
            .await
            .expect("a registered loopback endpoint stays usable");
    }

    /// PROPERTY: the endpoint must belong to an authorization server this
    /// store has actually registered a client with, and `client_id` must be
    /// that registration's — a caller cannot name an arbitrary https host, nor
    /// swap in its own client id at a real one.
    #[tokio::test]
    async fn a_token_endpoint_with_no_registered_client_row_is_rejected() {
        let s = state().await;
        let store = s.cp.store();
        store
            .upsert_mcp_oauth_client(
                "https://as.example",
                "registered-client",
                "https://as.example/token",
            )
            .await
            .unwrap();

        let unknown = require_registered_token_endpoint(
            store,
            "https://evil.example/token",
            "registered-client",
        )
        .await
        .expect_err("an issuer with no client row must be rejected");
        assert_eq!(unknown.status, 400);
        assert!(
            unknown.message.contains("does not belong"),
            "{}",
            unknown.message
        );

        let wrong_client =
            require_registered_token_endpoint(store, "https://as.example/token", "attacker-client")
                .await
                .expect_err("a client_id that is not the registered one must be rejected");
        assert!(
            wrong_client.message.contains("client_id"),
            "{}",
            wrong_client.message
        );

        require_registered_token_endpoint(store, "https://as.example/token", "registered-client")
            .await
            .expect("the real pairing must still be accepted");
    }

    /// PROPERTY (the regression this gate shipped with): a `token_endpoint`
    /// that does NOT live under its issuer's path must still complete.
    ///
    /// These are Atlassian's real, live values. Its protected-resource
    /// metadata for `https://mcp.atlassian.com/v1/mcp` names authorization
    /// server `https://auth.atlassian.com/<resource-id>` — an issuer with a
    /// PATH — and that issuer's RFC 8414 document puts `token_endpoint` at
    /// `https://auth.atlassian.com/oauth/token`, on the bare origin. RFC 8414
    /// nowhere requires the endpoint to sit under the issuer, so the earlier
    /// gate — which derived candidate issuers from the endpoint's own path
    /// prefixes (`https://auth.atlassian.com`, `…/oauth`, `…/oauth/token`)
    /// and looked each up — could never match the row DCR had written under
    /// the resource-scoped issuer. Every completion was refused, and because
    /// the caller is a background task that discards the error, the card sat
    /// on "Waiting for you to finish signing in…" until its deadline.
    ///
    /// Verified by observed failure: this test was written FIRST and run
    /// against the prefix-matching gate, where it failed with "the OAuth
    /// token endpoint does not belong to an authorization server this server
    /// has a registered client for".
    #[tokio::test]
    async fn a_token_endpoint_outside_its_issuers_path_is_accepted() {
        const ISSUER: &str = "https://auth.atlassian.com/VCeDsk8ZHncYF1g234fKtc4lNipbBhu3";
        const TOKEN_ENDPOINT: &str = "https://auth.atlassian.com/oauth/token";
        let s = state().await;
        let store = s.cp.store();
        store
            .upsert_mcp_oauth_client(ISSUER, "atlassian-client", TOKEN_ENDPOINT)
            .await
            .unwrap();

        require_registered_token_endpoint(store, TOKEN_ENDPOINT, "atlassian-client")
            .await
            .expect("the endpoint this flow recorded must be accepted for its own client");
    }

    /// PROPERTY: one provider's several path-scoped issuers share a token
    /// endpoint while each holds its OWN registered client — Atlassian mints a
    /// per-resource issuer with its own `dcr/register` — so the lookup must
    /// consider every row at that endpoint, not just one.
    ///
    /// Verified by observed failure: narrowing
    /// `mcp_oauth_client_ids_for_token_endpoint` to `LIMIT 1` turns the second
    /// assertion red (the second server's client id never being the row the
    /// query happens to return first).
    #[tokio::test]
    async fn two_issuers_sharing_a_token_endpoint_each_authorize_their_own_client() {
        const TOKEN_ENDPOINT: &str = "https://auth.atlassian.com/oauth/token";
        let s = state().await;
        let store = s.cp.store();
        store
            .upsert_mcp_oauth_client("https://auth.atlassian.com/aaa", "client-a", TOKEN_ENDPOINT)
            .await
            .unwrap();
        store
            .upsert_mcp_oauth_client("https://auth.atlassian.com/bbb", "client-b", TOKEN_ENDPOINT)
            .await
            .unwrap();

        require_registered_token_endpoint(store, TOKEN_ENDPOINT, "client-a")
            .await
            .expect("the first issuer's client must be accepted");
        require_registered_token_endpoint(store, TOKEN_ENDPOINT, "client-b")
            .await
            .expect("the second issuer's client must be accepted at the shared endpoint");
        require_registered_token_endpoint(store, TOKEN_ENDPOINT, "client-c")
            .await
            .expect_err("a client id registered at neither issuer must still be refused");
    }

    /// PROPERTY: binding to the RECORDED endpoint is strictly stronger than
    /// the prefix matching it replaces. Under the old rule a registered
    /// bare-origin issuer authorized ANY path at that origin, so a caller
    /// could name an endpoint the flow never discovered and have the daemon
    /// POST `code` + `code_verifier` to it. It must now be refused.
    ///
    /// Verified by observed failure: this passes trivially under the new gate,
    /// and fails under the old one — `issuer_candidates` yields the bare
    /// origin for `https://as.example/not-the-token-endpoint`, whose row
    /// matches, so the old gate returned `Ok`.
    #[tokio::test]
    async fn an_endpoint_that_only_shares_a_prefix_with_a_registered_issuer_is_refused() {
        let s = state().await;
        let store = s.cp.store();
        store
            .upsert_mcp_oauth_client(
                "https://as.example",
                "registered-client",
                "https://as.example/oauth/token",
            )
            .await
            .unwrap();

        let err = require_registered_token_endpoint(
            store,
            "https://as.example/not-the-token-endpoint",
            "registered-client",
        )
        .await
        .expect_err("only the endpoint the flow recorded may be POSTed to");
        assert!(err.message.contains("does not belong"), "{}", err.message);

        require_registered_token_endpoint(
            store,
            "https://as.example/oauth/token",
            "registered-client",
        )
        .await
        .expect("the recorded endpoint itself stays usable");
    }

    /// A row written before the `token_endpoint` column existed (NULL) must
    /// authorize nothing — failing closed rather than matching every endpoint
    /// SQL's NULL comparison would otherwise leave undecided. `begin_mcp_connect`
    /// re-upserts on every connect, so such a row backfills on the next attempt.
    #[tokio::test]
    async fn a_client_row_predating_the_token_endpoint_column_authorizes_nothing() {
        let s = state().await;
        let store = s.cp.store();
        store
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO mcp_oauth_clients(issuer, client_id, created_at) \
                     VALUES ('https://legacy.example', 'legacy-client', 1)",
                    [],
                )
                .map(|_| ())
            })
            .await
            .unwrap();

        require_registered_token_endpoint(store, "https://legacy.example/token", "legacy-client")
            .await
            .expect_err("a row with no recorded token endpoint must authorize nothing");
    }

    #[test]
    fn manual_oauth_client_validation_rejects_what_cannot_be_an_issuer() {
        assert!(validate_manual_oauth_client("", "c").is_err());
        assert!(validate_manual_oauth_client("https://as.example", "   ").is_err());

        let err = validate_manual_oauth_client("http://as.example", "c")
            .expect_err("a plaintext issuer on a public host must be refused");
        assert!(err.message.contains("https://"), "{}", err.message);

        // The loopback carve-out `is_loopback_url` documents: a request that
        // never leaves the machine has no transport to intercept.
        assert!(validate_manual_oauth_client("http://127.0.0.1:9", "c").is_ok());

        assert!(validate_manual_oauth_client("https://as.example?x=1", "c").is_err());
        assert!(validate_manual_oauth_client("https://as.example#f", "c").is_err());
        assert!(validate_manual_oauth_client("https://as.example", "a b").is_err());

        // The trailing slash MUST survive. The issuer is matched verbatim
        // against the string the MCP server's protected-resource metadata
        // listed, so normalizing it here would write a row
        // `mcp_oauth::begin_mcp_connect` can never find — and the user would be
        // told to supply a client id they had already supplied.
        assert_eq!(
            validate_manual_oauth_client("  https://as.example/tenant/  ", " client-1 ").unwrap(),
            (
                "https://as.example/tenant/".to_string(),
                "client-1".to_string()
            )
        );
    }

    #[tokio::test]
    async fn manual_oauth_clients_round_trip_through_the_rpc() {
        let s = state().await;
        let store = s.cp.store();

        dispatch(
            &s,
            "set_manual_mcp_oauth_client",
            json!({ "issuer": "https://as.example", "client_id": "manual-client" }),
        )
        .await
        .expect("recording a client id must succeed");

        let listed = dispatch(&s, "list_manual_mcp_oauth_clients", json!({}))
            .await
            .unwrap();
        assert_eq!(
            listed,
            json!([{ "issuer": "https://as.example", "clientId": "manual-client" }])
        );

        assert_eq!(
            store
                .get_mcp_oauth_client("https://as.example")
                .await
                .unwrap()
                .as_deref(),
            Some("manual-client"),
            "`begin_mcp_connect` must be able to find the recorded row"
        );

        let after_delete = dispatch(
            &s,
            "delete_manual_mcp_oauth_client",
            json!({ "issuer": "https://as.example" }),
        )
        .await
        .unwrap();
        assert_eq!(after_delete, json!([]));
    }

    /// PROPERTY: the new user-facing write path widens
    /// [`require_registered_token_endpoint`] by exactly nothing. The user
    /// asserts a client id, never an endpoint, so the completion gate has
    /// nothing to bind to until a real discovery run records one.
    #[tokio::test]
    async fn a_user_recorded_client_id_authorizes_no_token_endpoint_on_its_own() {
        let s = state().await;
        let store = s.cp.store();

        dispatch(
            &s,
            "set_manual_mcp_oauth_client",
            json!({ "issuer": "https://as.example", "client_id": "manual-client" }),
        )
        .await
        .unwrap();

        require_registered_token_endpoint(store, "https://as.example/token", "manual-client")
            .await
            .expect_err(
                "a row the USER wrote carries no token endpoint, so it may authorize no exchange",
            );
        require_registered_token_endpoint(store, "https://evil.example/token", "manual-client")
            .await
            .expect_err("and certainly not an endpoint at some unrelated origin");

        // Stands in for the discovery write `mcp_oauth::begin_mcp_connect`
        // performs on the next connect, from the authorization server's own
        // RFC 8414 metadata.
        store
            .upsert_mcp_oauth_client(
                "https://as.example",
                "manual-client",
                "https://as.example/token",
            )
            .await
            .unwrap();

        require_registered_token_endpoint(store, "https://as.example/token", "manual-client")
            .await
            .expect("the endpoint became acceptable only because DISCOVERY recorded it");
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

    // ---------- complete_mcp_connect must not rediscover the AS ----------

    /// One authorization server: RFC 8414 metadata (whose availability the
    /// returned `Arc<AtomicBool>` controls), RFC 7591 registration, and a
    /// token endpoint that records every request body it receives. The
    /// token endpoint is deliberately NOT gated by the same flag — only the
    /// discovery *metadata* document goes dark when toggled off, mirroring
    /// an authorization server whose discovery endpoint flakes while a
    /// token endpoint a caller already knows about keeps working.
    async fn spawn_authorization_server() -> (String, Arc<AtomicBool>, Arc<Mutex<Vec<String>>>) {
        use axum::extract::{Json, State};
        use axum::routing::{get, post};
        use axum::Router;

        #[derive(Clone)]
        struct AsState {
            as_url: String,
            up: Arc<AtomicBool>,
            token_hits: Arc<Mutex<Vec<String>>>,
        }

        async fn handle_metadata(
            State(state): State<AsState>,
        ) -> (axum::http::StatusCode, axum::Json<Value>) {
            if state.up.load(Ordering::SeqCst) {
                let as_url = &state.as_url;
                (
                    axum::http::StatusCode::OK,
                    axum::Json(json!({
                        "issuer": as_url,
                        "authorization_endpoint": format!("{as_url}/authorize"),
                        "token_endpoint": format!("{as_url}/token"),
                        "registration_endpoint": format!("{as_url}/register"),
                    })),
                )
            } else {
                (axum::http::StatusCode::NOT_FOUND, axum::Json(Value::Null))
            }
        }

        async fn handle_register(Json(_req): Json<Value>) -> axum::Json<Value> {
            axum::Json(json!({ "client_id": "freshly-registered-client" }))
        }

        async fn handle_token(
            State(state): State<AsState>,
            body: axum::body::Bytes,
        ) -> axum::Json<Value> {
            state
                .token_hits
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&body).into_owned());
            axum::Json(
                json!({ "access_token": "issued-token", "token_type": "Bearer", "expires_in": 3600 }),
            )
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let as_url = format!("http://{addr}");
        let up = Arc::new(AtomicBool::new(true));
        let token_hits = Arc::new(Mutex::new(Vec::new()));
        let state = AsState {
            as_url: as_url.clone(),
            up: up.clone(),
            token_hits: token_hits.clone(),
        };
        let app = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(handle_metadata),
            )
            .route("/register", post(handle_register))
            .route("/token", post(handle_token))
            .with_state(state);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (as_url, up, token_hits)
    }

    /// An MCP resource server that 401s with a `WWW-Authenticate` header
    /// naming its own protected-resource metadata, which in turn lists BOTH
    /// authorization servers, in document order — `as1_url` first. Mirrors
    /// `mcp_oauth.rs`'s `spawn_mcp_401_with_prm`, generalized to two issuers
    /// so `select_authorization_server`'s document-order fallback has
    /// something real to fall back to.
    async fn spawn_mcp_with_two_authorization_servers(as1_url: String, as2_url: String) -> String {
        use axum::extract::State;
        use axum::routing::{get, post};
        use axum::Router;

        #[derive(Clone)]
        struct McpState {
            mcp_url: Arc<Mutex<String>>,
            as1_url: String,
            as2_url: String,
        }

        async fn handle_probe(State(state): State<McpState>) -> axum::response::Response {
            let mcp_url = state.mcp_url.lock().unwrap().clone();
            let www_auth = format!(
                "Bearer resource_metadata=\"{mcp_url}/.well-known/oauth-protected-resource\""
            );
            axum::response::Response::builder()
                .status(axum::http::StatusCode::UNAUTHORIZED)
                .header(axum::http::header::WWW_AUTHENTICATE, www_auth)
                .body(axum::body::Body::empty())
                .unwrap()
        }

        async fn handle_prm(State(state): State<McpState>) -> axum::Json<Value> {
            // The bound port is unknown when this router is built, and the
            // client now VERIFIES the `resource` claim against the configured
            // server URL — so it has to be the real URL by the time it is served.
            let mcp_url = state.mcp_url.lock().unwrap().clone();
            axum::Json(json!({
                "resource": mcp_url,
                "authorization_servers": [state.as1_url, state.as2_url],
            }))
        }

        let mcp_url_slot = Arc::new(Mutex::new(String::new()));
        let mcp_state = McpState {
            mcp_url: mcp_url_slot.clone(),
            as1_url,
            as2_url,
        };
        let app = Router::new()
            .route("/", post(handle_probe))
            .route("/.well-known/oauth-protected-resource", get(handle_prm))
            .with_state(mcp_state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mcp_url = format!("http://{addr}");
        *mcp_url_slot.lock().unwrap() = mcp_url.clone();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        mcp_url
    }

    /// PROPERTY: completion must use the authorization server
    /// `begin_mcp_connect` actually selected, not whatever a fresh discovery
    /// run would resolve to at completion time.
    ///
    /// Pinned by making AS-1 — first in the PRM's document order, and the
    /// one `begin_mcp_connect` selects because it is reachable at that
    /// point — go dark (its discovery *metadata* only; its token endpoint
    /// keeps working) strictly BETWEEN the begin and complete calls, while
    /// AS-2 already has a client id on file (as if some other MCP server
    /// once connected through the same authorization server). A completion
    /// handler that rediscovers here would fail over to AS-2, find a
    /// pre-existing client id waiting for it, and complete SILENTLY against
    /// the wrong authorization server — exactly the hazard the plan's Task 7
    /// commentary warns about. Verified by observed failure: see this test's
    /// module doc / the accompanying report for the manual before/after run
    /// that reintroduced the old rediscovery path and watched this go red.
    #[tokio::test]
    async fn complete_mcp_connect_uses_the_authorization_server_begin_selected_not_a_fresh_discovery(
    ) {
        let (as1_url, as1_up, as1_token_hits) = spawn_authorization_server().await;
        let (as2_url, _as2_up, as2_token_hits) = spawn_authorization_server().await;
        let mcp_url =
            spawn_mcp_with_two_authorization_servers(as1_url.clone(), as2_url.clone()).await;

        let s = state().await;
        mcp::upsert_server(s.cp.store(), http_row("remote", &mcp_url))
            .await
            .unwrap();
        s.cp.store()
            .upsert_mcp_oauth_client(
                &as2_url,
                "as2-preexisting-client",
                &format!("{as2_url}/token"),
            )
            .await
            .unwrap();

        // `begin_mcp_connect` is called directly rather than through the RPC
        // dispatch here, purely to sidestep the `https://`-only gate that
        // guards the RPC entrypoint (this fixture's mock servers are plain
        // http, same as every other discovery test in this crate) — that
        // gate is an unrelated concern. The values below are byte-identical
        // to what `dispatch(&s, "begin_mcp_connect", ...)` would have
        // returned: the RPC handler's `begin_mcp_connect` fn is a thin
        // wrapper around exactly this call. `complete_mcp_connect`, the
        // actual site of the defect this test pins, IS driven through the
        // real RPC dispatch below.
        let http = reqwest::Client::new();
        let spec = McpServerSpec {
            name: "remote".to_string(),
            transport: McpTransport::Http {
                url: mcp_url.clone(),
                headers: vec![],
            },
        };
        let start =
            crate::harness::native::mcp_oauth::begin_mcp_connect(s.cp.store(), &http, &spec)
                .await
                .expect("AS-1 is reachable at begin time, so begin_mcp_connect must succeed");
        assert_eq!(
            start.issuer_token_endpoint,
            format!("{as1_url}/token"),
            "document order + AS-1 reachable means begin_mcp_connect must select AS-1, not AS-2"
        );

        // AS-1 goes dark for discovery purposes strictly between authorize
        // and completion. Its token endpoint is untouched by this flag.
        as1_up.store(false, Ordering::SeqCst);

        let out = dispatch(
            &s,
            "complete_mcp_connect",
            json!({
                "id": "remote",
                "code": "the-code",
                "verifier": start.verifier,
                "issuer_token_endpoint": start.issuer_token_endpoint,
                "client_id": start.client_id,
            }),
        )
        .await
        .expect(
            "completion must succeed using the carried-forward values, without needing AS-1's \
             now-dead discovery endpoint at all",
        );
        let _apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();

        assert_eq!(
            as1_token_hits.lock().unwrap().len(),
            1,
            "the token exchange must land on AS-1 — the authorization server begin_mcp_connect \
             actually selected, and presumably the one that issued the code — regardless of \
             whether AS-1's discovery endpoint is reachable right now"
        );
        assert_eq!(
            as2_token_hits.lock().unwrap().len(),
            0,
            "AS-2 must never see a token request here: it was never the authorization server \
             that issued the code, it only LOOKS usable because a client id happens to already \
             be on file for it"
        );
    }

    /// PROPERTY: an `issuer_token_endpoint` the caller made up never receives
    /// a request AT ALL — the rejection has to happen before the POST, because
    /// the POST itself is the whole attack: `grant_type`/`code`/`code_verifier`
    /// /`client_id`/`resource` in the body, and then whatever `access_token`
    /// comes back gets stored as this server's bearer.
    ///
    /// Driven through the real RPC dispatch (`complete_mcp_connect` is a
    /// registered Tauri command, so the webview picks these arguments), and
    /// pointed at a LIVE authorization server that records every token hit —
    /// so this asserts on absence of traffic, not merely on an error being
    /// returned.
    ///
    /// Verified by observed failure: with
    /// `require_registered_token_endpoint`'s call removed from
    /// `complete_mcp_connect`, the dispatch succeeds instead of erroring, the
    /// hit count is 1 instead of 0, and `attacker-token` lands in
    /// `mcp_oauth_tokens` for `remote` — all three assertions go red.
    #[tokio::test]
    async fn complete_mcp_connect_never_posts_to_a_token_endpoint_the_caller_invented() {
        let (attacker_url, _up, attacker_token_hits) = spawn_authorization_server().await;
        let s = state().await;
        mcp::upsert_server(s.cp.store(), http_row("remote", "https://mcp.example.com"))
            .await
            .unwrap();

        let err = dispatch(
            &s,
            "complete_mcp_connect",
            json!({
                "id": "remote",
                "code": "victims-code",
                "verifier": "victims-verifier",
                "issuer_token_endpoint": format!("{attacker_url}/token"),
                "client_id": "attacker-client",
            }),
        )
        .await
        .expect_err("a token endpoint with no registered client row must be refused");
        assert_eq!(err.status, 400);

        assert!(
            attacker_token_hits.lock().unwrap().is_empty(),
            "the caller-named endpoint must never be contacted — it received: {:?}",
            attacker_token_hits.lock().unwrap()
        );
        assert!(
            s.cp.store()
                .get_mcp_oauth_token("remote")
                .await
                .unwrap()
                .is_none(),
            "and no token may be stored for the server from a refused exchange"
        );
    }

    // ---------- the http probe must authenticate, and persist tools ----------

    fn stored_token(access_token: &str) -> crate::store::McpOauthToken {
        crate::store::McpOauthToken {
            access_token: access_token.to_string(),
            refresh_token: None,
            token_type: "Bearer".into(),
            expires_at: None,
            scopes: vec![],
            reconnect_required: false,
        }
    }

    /// An MCP resource server that actually ENFORCES a bearer — the shape
    /// every real auth-gated remote server has, and the shape the old probe
    /// could not get past, because it sent no `Authorization` header at all
    /// and therefore only ever saw the 401.
    ///
    /// With `Authorization: Bearer <accepted>` it answers the full JSON-RPC
    /// handshake and advertises one tool (`ping`); anything else gets a 401
    /// carrying a `WWW-Authenticate` header that names its own
    /// protected-resource metadata, which in turn points at `as_url` — so the
    /// same fixture also drives a real `begin_mcp_connect` discovery run.
    async fn spawn_bearer_gated_mcp(accepted: &'static str, as_url: String) -> String {
        use axum::extract::State;
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::IntoResponse;
        use axum::routing::{get, post};
        use axum::Router;

        #[derive(Clone)]
        struct McpState {
            mcp_url: Arc<Mutex<String>>,
            as_url: String,
            accepted: &'static str,
        }

        async fn handle_rpc(
            State(state): State<McpState>,
            headers: HeaderMap,
            body: axum::body::Bytes,
        ) -> axum::response::Response {
            let presented = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if presented != format!("Bearer {}", state.accepted) {
                let mcp_url = state.mcp_url.lock().unwrap().clone();
                return axum::response::Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header(
                        axum::http::header::WWW_AUTHENTICATE,
                        format!(
                            "Bearer resource_metadata=\"{mcp_url}/.well-known/oauth-protected-resource\""
                        ),
                    )
                    .body(axum::body::Body::empty())
                    .unwrap();
            }
            let msg: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let id = msg["id"].clone();
            let result = match msg["method"].as_str().unwrap_or_default() {
                "initialize" => json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
                "tools/list" => json!({ "tools": [{
                    "name": "ping",
                    "description": "ping it",
                    "inputSchema": { "type": "object" }
                }] }),
                _ => Value::Null,
            };
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
            )
                .into_response()
        }

        async fn handle_prm(State(state): State<McpState>) -> axum::Json<Value> {
            // The bound port is unknown when this router is built, and the
            // client now VERIFIES the `resource` claim against the configured
            // server URL — so it has to be the real URL by the time it is served.
            let mcp_url = state.mcp_url.lock().unwrap().clone();
            axum::Json(json!({
                "resource": mcp_url,
                "authorization_servers": [state.as_url],
            }))
        }

        let mcp_url_slot = Arc::new(Mutex::new(String::new()));
        let app = Router::new()
            .route("/", post(handle_rpc))
            .route("/.well-known/oauth-protected-resource", get(handle_prm))
            .with_state(McpState {
                mcp_url: mcp_url_slot.clone(),
                as_url,
                accepted,
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mcp_url = format!("http://{addr}");
        *mcp_url_slot.lock().unwrap() = mcp_url.clone();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        mcp_url
    }

    /// PROPERTY: the http probe authenticates with the credential the SESSION
    /// would use, and persists the tools it discovers.
    ///
    /// Both halves matter and neither existed. The probe POSTed a bare
    /// `initialize` with no `Authorization` header, so every auth-gated remote
    /// server sat at `status = "error"` / "HTTP initialize failed — check the
    /// URL" with a URL that was perfectly correct; and it never called
    /// `replace_tools`, so an http server never got `mcp_tools` rows — which
    /// is what the Tools tab (`hasTools: app.tools.length > 0`) and per-tool
    /// permissions are built out of. Remote servers therefore had NO
    /// configurable per-tool permissions at all.
    ///
    /// Structured so it cannot pass on a server that accepts anything: the
    /// first probe runs with NO stored token and must FAIL with the server's
    /// real 401 surfaced, and only the second — after a token is stored —
    /// succeeds. Verified by observed failure: restoring the old bare
    /// `reqwest` POST turns the second half red (status `error`, no tools, and
    /// the `set_app_tool_perm` round-trip below has no row to write).
    #[tokio::test]
    async fn the_http_probe_authenticates_with_the_stored_token_and_persists_the_tools() {
        let mcp_url = spawn_bearer_gated_mcp("real-token", "http://127.0.0.1:9".into()).await;
        let s = state().await;
        mcp::upsert_server(s.cp.store(), http_row("remote", &mcp_url))
            .await
            .unwrap();

        let out = dispatch(&s, "probe_app", json!({ "id": "remote" }))
            .await
            .unwrap();
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        assert_eq!(
            apps[0].status, "error",
            "with no credential at all the server really is unusable"
        );
        assert!(
            apps[0]
                .status_detail
                .as_deref()
                .is_some_and(|d| d.contains("401")),
            "the detail must name what actually happened (a 401), not blame the URL: {:?}",
            apps[0].status_detail
        );
        assert!(apps[0].tools.is_empty());

        s.cp.store()
            .upsert_mcp_oauth_token("remote", &stored_token("real-token"))
            .await
            .unwrap();
        let out = dispatch(&s, "probe_app", json!({ "id": "remote" }))
            .await
            .unwrap();
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        assert_eq!(
            apps[0].status, "connected",
            "the probe must present the stored OAuth token, exactly as a session does: {:?}",
            apps[0].status_detail
        );
        assert_eq!(
            apps[0]
                .tools
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ping"],
            "the tools the handshake discovered must be persisted, or there is no Tools tab and \
             no per-tool permission to configure"
        );

        // The concrete consequence: a remote server's per-tool permission is
        // now configurable at all, because a `mcp_tools` row exists to carry it.
        let out = dispatch(
            &s,
            "set_app_tool_perm",
            json!({ "id": "remote", "tool": "ping", "perm": "allow" }),
        )
        .await
        .unwrap();
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        assert_eq!(apps[0].tools[0].perm, "allow");
    }

    /// PROPERTY: a completed OAuth connect leaves the card's status and its
    /// pill AGREEING, because completion re-probes with the token it just
    /// stored. Before this, `complete_mcp_connect` returned the app list
    /// untouched, so a fully successful connect rendered an "OAuth connected"
    /// pill next to the red Error dot and empty Tools tab that the earlier
    /// (unauthenticated, therefore failed) probe had written — and clicking
    /// Probe again could not fix it either.
    ///
    /// Drives the real RPC against a real bearer-enforcing MCP server and a
    /// real authorization server. Verified by observed failure: removing the
    /// `probe_and_persist` call from `complete_mcp_connect` leaves
    /// `status == "unknown"` with no tools, turning the last two assertions
    /// red while the token/pill assertions still pass — which is exactly the
    /// contradiction this pins.
    #[tokio::test]
    async fn complete_mcp_connect_reprobes_so_the_status_and_the_pill_agree() {
        let (as_url, _up, _hits) = spawn_authorization_server().await;
        // `spawn_authorization_server`'s token endpoint always mints
        // `issued-token`; the MCP server accepts exactly that bearer.
        let mcp_url = spawn_bearer_gated_mcp("issued-token", as_url.clone()).await;
        let s = state().await;
        mcp::upsert_server(s.cp.store(), http_row("remote", &mcp_url))
            .await
            .unwrap();

        // Same shortcut (and same reason) as
        // `complete_mcp_connect_uses_the_authorization_server_begin_selected_
        // not_a_fresh_discovery`: `begin_mcp_connect` is called directly to
        // sidestep the RPC's https-only gate, since this fixture — like every
        // discovery fixture in this crate — can only bind plaintext loopback.
        // `complete_mcp_connect`, the handler under test, goes through the
        // real dispatch.
        let http = reqwest::Client::new();
        let spec = McpServerSpec {
            name: "remote".to_string(),
            transport: McpTransport::Http {
                url: mcp_url.clone(),
                headers: vec![],
            },
        };
        let start =
            crate::harness::native::mcp_oauth::begin_mcp_connect(s.cp.store(), &http, &spec)
                .await
                .expect("the 401 + PRM + AS-metadata + DCR chain must resolve");

        // A previous attempt's failure, left over for this one to clear.
        note_connect_failure(&s.cp, "remote", "a stale failure from a previous attempt").await;

        let out = dispatch(
            &s,
            "complete_mcp_connect",
            json!({
                "id": "remote",
                "code": "the-code",
                "verifier": start.verifier,
                "issuer_token_endpoint": start.issuer_token_endpoint,
                "client_id": start.client_id,
            }),
        )
        .await
        .expect("the token exchange must succeed");
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        assert!(apps[0].oauth_token_stored, "the token must be stored");
        assert!(apps[0].oauth_connect_available);
        assert_eq!(
            apps[0].status, "connected",
            "the very response that reports a stored token must not still show the failed \
             pre-connect probe's status: {:?}",
            apps[0].status_detail
        );
        assert_eq!(
            apps[0]
                .tools
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            vec!["ping"],
            "and the tools the now-authenticated handshake found must be there, so the Tools tab \
             appears without the user hunting for the Probe button"
        );
        assert_eq!(
            apps[0].oauth_connect_error, None,
            "a completed connect must clear the previous attempt's failure, not leave the card \
             captioned with it"
        );
    }

    /// PROPERTY: a refused completion says WHY, on the server row the card is
    /// polling.
    ///
    /// The token exchange runs in a spawned Cockpit task that throws this
    /// handler's `Result` away — `complete_local_mcp_callback` only
    /// `eprintln!`s it, and `tracing` has no subscriber anywhere in this
    /// workspace, so neither reaches a user. The card polls `list_apps` for
    /// five minutes and then reports that the sign-in link expired. That is
    /// how a gate refusal in the first second (the whole
    /// path-scoped-issuer bug) presented as a hang: the failure was known
    /// immediately and stored nowhere.
    ///
    /// Verified by observed failure: removing the `note_connect_failure` call
    /// from `complete_mcp_connect`'s gate arm leaves `oauth_connect_error`
    /// `None` and turns the second assertion red — the RPC still returns its
    /// error either way, which is exactly the signal nobody was reading.
    #[tokio::test]
    async fn a_refused_completion_is_recorded_on_the_row_the_card_polls() {
        let s = state().await;
        mcp::upsert_server(s.cp.store(), http_row("remote", "https://mcp.example.com"))
            .await
            .unwrap();

        let err = dispatch(
            &s,
            "complete_mcp_connect",
            json!({
                "id": "remote",
                "code": "the-code",
                "verifier": "the-verifier",
                "issuer_token_endpoint": "https://auth.atlassian.com/oauth/token",
                "client_id": "some-client",
            }),
        )
        .await
        .expect_err("no client row is registered, so the exchange must be refused");
        assert_eq!(err.status, 400);

        let out = dispatch(&s, "list_apps", json!({})).await.unwrap();
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        let reason = apps[0]
            .oauth_connect_error
            .as_deref()
            .expect("the refusal must be readable from the list the card polls");
        assert!(reason.contains("does not belong"), "{reason}");
        assert!(
            reason.contains("https://auth.atlassian.com/oauth/token"),
            "the message must name the endpoint that was refused, or it is unactionable: {reason}"
        );

        // Disconnect ends the attempt, so the reason must not outlive it.
        dispatch(&s, "disconnect_mcp", json!({ "id": "remote" }))
            .await
            .unwrap();
        let out = dispatch(&s, "list_apps", json!({})).await.unwrap();
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        assert_eq!(apps[0].oauth_connect_error, None);
    }

    // ---------- the OAuth affordance must not lie about who owns the credential ----------

    /// PROPERTY: `AppInfo.oauth_connect_available` and what the session
    /// actually authenticates with are the SAME decision, and they flip
    /// TOGETHER.
    ///
    /// This is the anti-drift test. `transport === "http"` used to gate the
    /// UI's OAuth block, and it only correlates with credential ownership: for
    /// `atlassian-rovo` (`Authorization: Basic ${setting:…}`) the card said
    /// "Not connected", walked the user through a real Atlassian consent
    /// screen, flipped to "OAuth connected" — and the session went on sending
    /// the Basic header, ignoring the token forever.
    ///
    /// Both sides are observed for real, on one row, from one store: the card
    /// side through the `list_apps` RPC, the session side through
    /// `mcp::servers_for_session` → `harness::native::open_http_mcp` (the
    /// exact path `connect_mcp_tools` delegates to) with the assertion on the
    /// `Authorization` header the server RECEIVED. Then the manifest header is
    /// cleared and both sides must invert.
    ///
    /// Verified by observed failure: hardcoding `oauth_connect_available` to
    /// `true` in `assemble` turns the first assertion red; hardcoding it to
    /// `false` turns the third red; and making `open_http_mcp` prefer the
    /// stored token over a manifest header turns the second red.
    #[tokio::test]
    async fn oauth_connect_available_agrees_with_what_the_session_authenticates_with() {
        crate::llm_router::secrets::use_test_key_file();
        let (url, seen_auth) = crate::harness::native::tests::spawn_auth_echo_server().await;
        let s = state().await;
        let store = s.cp.store();
        mcp::upsert_server(store, http_row("remote", &url))
            .await
            .unwrap();
        store
            .upsert_mcp_oauth_token("remote", &stored_token("stored-token"))
            .await
            .unwrap();
        mcp::set_server_headers(
            store,
            "remote",
            &[(
                "Authorization".to_string(),
                "Basic manifest-creds".to_string(),
            )],
        )
        .await
        .unwrap();

        let session_auth = |seen: &Arc<Mutex<Vec<Vec<String>>>>| -> Vec<String> {
            seen.lock()
                .unwrap()
                .first()
                .cloned()
                .expect("the initialize request must have reached the server")
        };

        let apps: Vec<AppInfo> =
            serde_json::from_value(dispatch(&s, "list_apps", json!({})).await.unwrap()).unwrap();
        assert!(
            !apps[0].oauth_connect_available,
            "a server whose manifest supplies the credential must not offer an OAuth connect the \
             session would then ignore"
        );
        let specs = mcp::servers_for_session(store, "native").await.unwrap();
        let _ = crate::harness::native::open_http_mcp(store, &specs[0]).await;
        assert_eq!(
            session_auth(&seen_auth),
            ["Basic manifest-creds".to_string()],
            "and that is exactly what the session sends — the card's claim and the wire agree"
        );

        // Drop the manifest credential: the host now owns the slot, so BOTH
        // sides must invert in lockstep.
        mcp::set_server_headers(store, "remote", &[]).await.unwrap();
        seen_auth.lock().unwrap().clear();

        let apps: Vec<AppInfo> =
            serde_json::from_value(dispatch(&s, "list_apps", json!({})).await.unwrap()).unwrap();
        assert!(
            apps[0].oauth_connect_available,
            "with no manifest credential the connected token IS what authenticates, so connect is \
             a real offer"
        );
        let specs = mcp::servers_for_session(store, "native").await.unwrap();
        let _ = crate::harness::native::open_http_mcp(store, &specs[0]).await;
        assert_eq!(
            session_auth(&seen_auth),
            ["Bearer stored-token".to_string()],
            "the session now sends the connected token, matching the affordance the card offers"
        );
    }

    /// PROPERTY: one row whose stored headers cannot be DECODED costs exactly
    /// that row — `list_apps` must still return, and the row must fail CLOSED.
    ///
    /// `mcp::get_server_headers` hard-errors on an undecodable `headers_json`
    /// (a rotated or unavailable secret key, a hand-edited row), and
    /// `assemble` now reads headers where it previously read none — so a
    /// propagated error here would blank the ENTIRE Apps screen over one bad
    /// row, which is precisely what `mcp::servers_for_session` refuses to do
    /// for the identical failure. Verified by observed failure: making
    /// `http_session_spec` propagate the error with `?` instead of logging and
    /// returning `None` turns the first assertion red — `list_apps` errors
    /// instead of listing, and the healthy stdio row disappears with it.
    #[tokio::test]
    async fn a_row_with_undecodable_headers_costs_one_row_not_the_whole_list() {
        crate::llm_router::secrets::use_test_key_file();
        let s = state().await;
        let store = s.cp.store();
        mcp::upsert_server(store, http_row("broken", "https://mcp.example.com"))
            .await
            .unwrap();
        mcp::upsert_server(store, http_row("healthy", "https://other.example.com"))
            .await
            .unwrap();
        // Straight past `set_server_headers` (which would encrypt properly):
        // this is the hand-edited/rotated-key shape.
        store
            .with_conn(|c| {
                c.execute(
                    "UPDATE mcp_servers SET headers_json='not-json-at-all' WHERE id='broken'",
                    [],
                )
                .map(|_| ())
            })
            .await
            .unwrap();

        let out = dispatch(&s, "list_apps", json!({}))
            .await
            .expect("one undecodable row must not fail the whole list");
        let apps: Vec<AppInfo> = serde_json::from_value(out).unwrap();
        assert_eq!(apps.len(), 2, "every other server must still be listed");
        let broken = apps.iter().find(|a| a.id == "broken").unwrap();
        let healthy = apps.iter().find(|a| a.id == "healthy").unwrap();
        assert!(
            !broken.oauth_connect_available,
            "a credential this host cannot even read must not be advertised as connectable — a \
             session cannot attach this server at all"
        );
        assert!(
            healthy.oauth_connect_available,
            "and the failure must be scoped to the one row"
        );
    }

    /// PROPERTY: a stdio server never offers OAuth connect. `begin_mcp_connect`
    /// already refuses one (see
    /// `begin_mcp_connect_rejects_a_stdio_server`), so a card that offered the
    /// button could only ever produce an error toast. Verified by observed
    /// failure: dropping the `row.transport == "http"` arm from `assemble`'s
    /// derivation makes a stdio row report `true` (it has no headers, so the
    /// predicate alone reads as host-managed) and turns this red.
    #[tokio::test]
    async fn a_stdio_row_never_offers_oauth_connect() {
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

        let apps: Vec<AppInfo> =
            serde_json::from_value(dispatch(&s, "list_apps", json!({})).await.unwrap()).unwrap();
        assert!(!apps[0].oauth_connect_available);
    }
}
