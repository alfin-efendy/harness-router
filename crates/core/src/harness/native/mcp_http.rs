//! Native MCP client over Streamable HTTP — the remote counterpart of
//! [`super::mcp_client::McpConnection`]'s stdio transport.
//!
//! Kept in its own module rather than grown into `mcp_client.rs`: the stdio
//! connection owns a child process and newline framing, this one owns an HTTP
//! client, an optional session id and a credential. Different lifetimes,
//! different failure modes. Both implement [`McpCaller`], so a remote server's
//! tools reach a session through exactly the same path.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::mcp_client::{build_call_request, McpCaller, McpToolDef, MCP_PROTOCOL_VERSION};
use super::mcp_oauth;
use crate::domain::{McpServerSpec, McpTransport};
use crate::stdio_jsonrpc;
use crate::store::Store;

/// A live remote MCP server connection.
pub struct McpHttpConnection {
    http: reqwest::Client,
    url: String,
    /// Static headers from the server spec (a manifest-resolved API token or
    /// injected OAuth bearer), plus the `Authorization` this connection was
    /// opened with, if any. `Mutex`-wrapped (not merely for interior
    /// mutability parity with `session_id`): the refresh-on-401 path
    /// (`set_bearer`) rewrites the `Authorization` entry in place once a
    /// refreshed access token is minted.
    headers: Mutex<Vec<(String, String)>>,
    /// `Mcp-Session-Id` if the server issued one at `initialize`; echoed on
    /// every subsequent request.
    session_id: Mutex<Option<String>>,
    next_id: AtomicI64,
    pub server_name: String,
    pub tools: Vec<McpToolDef>,
    /// Set only when this connection's `Authorization` header was sourced
    /// from a STORE-managed MCP OAuth token — never for a manifest-supplied
    /// credential or an anonymous connection (see [`connect_http_with_store`]
    /// and `connect_mcp_tools`'s auth-precedence dispatch, Task 8 of the
    /// remote-MCP-OAuth plan). Enables the reactive refresh-on-401 path in
    /// `post`: a manifest credential is not this host's to refresh or mark
    /// `reconnect_required`, so a connection with `oauth_store: None` simply
    /// propagates a 401 as a plain error and never touches the store.
    oauth_store: Option<Arc<Store>>,
}

/// Open a remote MCP connection: handshake, then list its tools.
///
/// `bearer`, when present, is sent as `Authorization: Bearer <bearer>` and
/// OVERRIDES any `Authorization` already in the spec's headers — a token this
/// host just minted is always fresher than one baked into a manifest.
///
/// This connection never attempts an OAuth refresh on a 401 — use
/// [`connect_http_with_store`] for a connection whose `bearer` came from a
/// stored MCP OAuth token.
pub async fn connect_http(
    spec: &McpServerSpec,
    bearer: Option<&str>,
) -> anyhow::Result<McpHttpConnection> {
    connect_http_inner(spec, bearer, None).await
}

/// Like [`connect_http`], but wires the connection to a [`Store`] so a 401
/// triggers the reactive refresh-and-retry path in `post` instead of a plain
/// error. Callers MUST pass this only when `bearer` itself came from a
/// stored MCP OAuth token (never for a manifest-supplied credential) — see
/// `connect_mcp_tools`'s auth-precedence dispatch.
pub(crate) async fn connect_http_with_store(
    spec: &McpServerSpec,
    bearer: Option<&str>,
    store: Arc<Store>,
) -> anyhow::Result<McpHttpConnection> {
    connect_http_inner(spec, bearer, Some(store)).await
}

async fn connect_http_inner(
    spec: &McpServerSpec,
    bearer: Option<&str>,
    oauth_store: Option<Arc<Store>>,
) -> anyhow::Result<McpHttpConnection> {
    let McpTransport::Http { url, headers } = &spec.transport else {
        anyhow::bail!("mcp_http: not an HTTP transport");
    };
    // RFC 3986 makes the scheme case-insensitive, so `HTTPS://…` is just as
    // valid as `https://…` — lowercase before comparing rather than reject it.
    if !url.to_ascii_lowercase().starts_with("https://") && !cfg!(test) {
        anyhow::bail!("mcp: remote server {url} must use https");
    }
    let mut merged: Vec<(String, String)> = headers
        .iter()
        .filter(|(k, _)| bearer.is_none() || !k.eq_ignore_ascii_case("authorization"))
        .cloned()
        .collect();
    if let Some(token) = bearer {
        merged.push(("Authorization".to_string(), format!("Bearer {token}")));
    }
    let mut conn = McpHttpConnection {
        http: reqwest::Client::new(),
        url: url.clone(),
        headers: Mutex::new(merged),
        session_id: Mutex::new(None),
        next_id: AtomicI64::new(1),
        server_name: spec.name.clone(),
        tools: Vec::new(),
        oauth_store,
    };
    conn.handshake().await?;
    conn.tools = conn.list_tools().await?;
    Ok(conn)
}

impl McpHttpConnection {
    async fn handshake(&self) -> anyhow::Result<()> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let init = stdio_jsonrpc::build_request(
            id,
            "initialize",
            Some(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "ryuzi-native", "version": env!("CARGO_PKG_VERSION") }
            })),
        );
        let (resp, session) = self.post(&init).await?;
        if let Some(err) = resp.get("error") {
            anyhow::bail!("mcp initialize error: {err}");
        }
        if let Some(session) = session {
            *self.session_id.lock().await = Some(session);
        }
        let initialized = stdio_jsonrpc::build_notification("notifications/initialized", None);
        // A notification has no id and no response body to await.
        let _ = self.post(&initialized).await;
        Ok(())
    }

    async fn list_tools(&self) -> anyhow::Result<Vec<McpToolDef>> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = stdio_jsonrpc::build_request(id, "tools/list", None);
        let (resp, _) = self.post(&req).await?;
        Ok(resp
            .pointer("/result/tools")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        Some(McpToolDef {
                            name: t.get("name").and_then(Value::as_str)?.to_string(),
                            description: t
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            input_schema: t
                                .get("inputSchema")
                                .cloned()
                                .unwrap_or_else(|| json!({ "type": "object" })),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// POST one JSON-RPC message and return `(response, new session id)`.
    ///
    /// A Streamable HTTP server may answer either with a bare
    /// `application/json` body (Task 1) or with a `text/event-stream` body
    /// carrying one or more SSE events, of which at most one is the reply to
    /// THIS message (Task 2) — `sse_message_for_id` picks that one out and
    /// treats a stream that never carries it as a transport failure.
    ///
    /// On an HTTP 401, delegates to [`Self::refresh_and_retry`] (Task 8):
    /// a store-managed connection gets ONE refresh-and-retry; any other
    /// non-success status (including a 401 the retry itself produces) is a
    /// plain error, same as before this task.
    async fn post(&self, message: &Value) -> anyhow::Result<(Value, Option<String>)> {
        match self.post_once(message).await? {
            PostOutcome::Ok(value, session) => Ok((value, session)),
            PostOutcome::Unauthorized(www_authenticate) => {
                self.refresh_and_retry(message, www_authenticate).await
            }
        }
    }

    /// One POST attempt, no retry. A 401 is reported as
    /// `PostOutcome::Unauthorized` (carrying `WWW-Authenticate`, if any)
    /// rather than an error — only `post`'s caller decides whether that's
    /// retryable.
    async fn post_once(&self, message: &Value) -> anyhow::Result<PostOutcome> {
        let mut request = self
            .http
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION);
        let headers_snapshot = self.headers.lock().await.clone();
        for (key, value) in &headers_snapshot {
            request = request.header(key, value);
        }
        if let Some(session) = self.session_id.lock().await.as_deref() {
            request = request.header("Mcp-Session-Id", session);
        }
        let response = tokio::time::timeout(Duration::from_secs(120), request.json(message).send())
            .await
            .map_err(|_| anyhow::anyhow!("mcp: request timed out"))??;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let www_authenticate = response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            return Ok(PostOutcome::Unauthorized(www_authenticate));
        }
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("mcp: HTTP {status}");
        }
        if body.trim().is_empty() {
            return Ok(PostOutcome::Ok(Value::Null, session));
        }
        if content_type.contains("text/event-stream") {
            let want = message.get("id").cloned();
            return Ok(PostOutcome::Ok(
                sse_message_for_id(&body, want.as_ref())?,
                session,
            ));
        }
        Ok(PostOutcome::Ok(serde_json::from_str(&body)?, session))
    }

    /// Handle a 401 seen by `post_once`: a connection with no `oauth_store`
    /// (manifest-authenticated or anonymous) simply reports it as a plain
    /// error — refreshing or reconnect-marking a credential this host
    /// doesn't own would be wrong. A store-managed connection gets exactly
    /// ONE refresh-and-retry (the plan's precedence rule): if the refresh
    /// itself fails, or the retry ALSO 401s, the server is marked
    /// `reconnect_required` in the store and the error names it, so the UI
    /// can prompt a reconnect instead of retrying a dead credential forever.
    async fn refresh_and_retry(
        &self,
        message: &Value,
        www_authenticate: Option<String>,
    ) -> anyhow::Result<(Value, Option<String>)> {
        let Some(store) = self.oauth_store.as_deref() else {
            anyhow::bail!("mcp: {} returned 401 Unauthorized", self.server_name);
        };
        if let Err(e) = self.refresh_stored_token(store, www_authenticate).await {
            let _ = store
                .mark_mcp_oauth_reconnect_required(&self.server_name)
                .await;
            anyhow::bail!(
                "mcp: {} needs reconnecting — token refresh failed: {e}",
                self.server_name
            );
        }
        match self.post_once(message).await? {
            PostOutcome::Ok(value, session) => Ok((value, session)),
            PostOutcome::Unauthorized(_) => {
                let _ = store
                    .mark_mcp_oauth_reconnect_required(&self.server_name)
                    .await;
                anyhow::bail!(
                    "mcp: {} needs reconnecting — still unauthorized after a token refresh",
                    self.server_name
                );
            }
        }
    }

    /// Exchange the stored refresh token for a new access token and persist
    /// it, updating this connection's in-memory `Authorization` header on
    /// success. The token endpoint is re-discovered from THIS 401's
    /// `WWW-Authenticate` header (the same RFC 9728 → RFC 8414 chain
    /// `mcp_oauth::begin_mcp_connect` uses to connect in the first place) —
    /// nothing about the issuer or token endpoint is persisted anywhere
    /// keyed by server name, so rediscovering here is the only way back to
    /// it; `mcp_oauth_clients` (keyed by issuer, not server) already caches
    /// the client id from that original connect.
    async fn refresh_stored_token(
        &self,
        store: &Store,
        www_authenticate: Option<String>,
    ) -> anyhow::Result<()> {
        let current = store
            .get_mcp_oauth_token(&self.server_name)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no stored token to refresh"))?;
        let refresh_token = current
            .refresh_token
            .clone()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| anyhow::anyhow!("no refresh token available"))?;
        let header = www_authenticate
            .ok_or_else(|| anyhow::anyhow!("401 carried no WWW-Authenticate header"))?;
        let metadata_url = crate::plugins::oauth::parse_www_authenticate_resource(&header)
            .ok_or_else(|| anyhow::anyhow!("WWW-Authenticate names no resource metadata"))?;
        let issuers = mcp_oauth::protected_resource_metadata(&self.http, &metadata_url).await?;
        let (issuer, metadata) =
            mcp_oauth::select_authorization_server(&self.http, &issuers).await?;
        let client_id = store
            .get_mcp_oauth_client(&issuer)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no registered client id for {issuer}"))?;
        let resource = mcp_oauth::canonical_resource_uri(&self.url)?;
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", client_id.as_str()),
            ("resource", resource.as_str()),
        ];
        let response = self
            .http
            .post(&metadata.token_endpoint)
            .form(&form)
            .send()
            .await?;
        if !response.status().is_success() {
            anyhow::bail!("token endpoint returned HTTP {}", response.status());
        }
        let body: Value = response.json().await?;
        let access_token = body
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("refresh response carried no access_token"))?
            .to_string();
        let new_token = crate::store::McpOauthToken {
            access_token: access_token.clone(),
            refresh_token: body
                .get("refresh_token")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(Some(refresh_token)),
            token_type: body
                .get("token_type")
                .and_then(Value::as_str)
                .unwrap_or("Bearer")
                .to_string(),
            expires_at: body
                .get("expires_in")
                .and_then(Value::as_i64)
                .map(|secs| crate::paths::now_ms() + secs * 1000),
            scopes: body
                .get("scope")
                .and_then(Value::as_str)
                .map(|s| s.split_whitespace().map(str::to_string).collect())
                .unwrap_or_else(|| current.scopes.clone()),
            reconnect_required: false,
        };
        store
            .upsert_mcp_oauth_token(&self.server_name, &new_token)
            .await?;
        self.set_bearer(&access_token).await;
        Ok(())
    }

    /// Replace this connection's `Authorization` header in place — used only
    /// by [`Self::refresh_stored_token`] once a refresh succeeds, so the
    /// retried request (and every request after it) carries the new token.
    async fn set_bearer(&self, token: &str) {
        let mut headers = self.headers.lock().await;
        headers.retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
        headers.push(("Authorization".to_string(), format!("Bearer {token}")));
    }
}

/// The outcome of one [`McpHttpConnection::post_once`] attempt.
enum PostOutcome {
    /// A non-401 response: the parsed JSON-RPC message, plus a session id if
    /// the server issued one on this response.
    Ok(Value, Option<String>),
    /// An HTTP 401, carrying `WWW-Authenticate` if the server sent one.
    Unauthorized(Option<String>),
}

/// Pull the JSON-RPC message whose `id` matches `want` out of an SSE body.
///
/// Per the SSE spec, one event may carry MULTIPLE `data:` lines — a server
/// that pretty-prints its JSON, or that simply splits a large payload
/// across lines, sends exactly this shape. Those lines are not independent
/// JSON documents: they are joined with `\n` into a single payload and
/// parsed once, at the blank line that terminates the event. A single
/// leading space after the `data:` colon is part of the field syntax, not
/// the value, and is stripped (`data: x` and `data:x` both yield `x`).
///
/// Any other message on the stream — a notification, or a server-initiated
/// request this client does not implement — is skipped rather than mistaken
/// for the answer. A stream that ends without the wanted id is a TRANSPORT
/// ERROR, not an empty result: silently resolving to `Value::Null` here would
/// make a truncated or broken upstream response indistinguishable from a tool
/// that legitimately returned nothing.
fn sse_message_for_id(body: &str, want: Option<&Value>) -> anyhow::Result<Value> {
    let mut skipped = 0usize;
    let mut data_lines: Vec<&str> = Vec::new();

    // `.chain(once(""))` appends a synthetic trailing blank line so the
    // final event still gets dispatched even when the body itself doesn't
    // end with one — a POST response arrives whole, not streamed
    // field-by-field, so a trailing blank line isn't guaranteed the way it
    // would be on a live `EventSource` connection.
    for line in body.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            // A blank line terminates the event: join and parse whatever
            // `data:` lines it accumulated, then reset for the next event —
            // resetting unconditionally (matched, skipped, or unparseable)
            // is what keeps one event's lines from leaking into the next.
            if data_lines.is_empty() {
                continue;
            }
            let payload = data_lines.join("\n");
            data_lines.clear();
            let Ok(message) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            match (want, message.get("id")) {
                (Some(want), Some(got)) if want == got => return Ok(message),
                _ => skipped += 1,
            }
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.strip_prefix(' ').unwrap_or(data));
        }
    }
    anyhow::bail!(
        "mcp: event stream ended without a response for the pending request ({skipped} other message(s) seen)"
    )
}

#[async_trait]
impl McpCaller for McpHttpConnection {
    async fn call(&self, tool: &str, arguments: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = build_call_request(id, tool, &arguments);
        let (resp, _) = self.post(&req).await?;
        if let Some(err) = resp.get("error") {
            anyhow::bail!("{}", err);
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::domain::{McpServerSpec, McpTransport};

    use axum::extract::{Json, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::Router;

    /// One request the in-test MCP server received: the headers this file's
    /// assertions care about, plus the raw JSON-RPC body. A client that
    /// dropped the version header, sent a too-narrow `Accept`, lied about
    /// its `clientInfo`, or failed to echo back an issued session id would
    /// still get a response from the handler below — the only way to catch
    /// that is to record what actually arrived and assert on it here, not on
    /// what the server chose to reply.
    #[derive(Debug, Clone)]
    pub(crate) struct SeenRequest {
        protocol_version: Option<String>,
        accept: Option<String>,
        /// EVERY `Authorization` value on this request, in arrival order. A
        /// well-behaved client sends at most one; capturing the full list
        /// (rather than only the first, the way `HeaderMap::get` would) is
        /// what makes header duplication visible instead of silently hidden.
        authorization: Vec<String>,
        /// The `Mcp-Session-Id` this request carried, or `None` if it had
        /// none. Task 2's session-echo test reads this off every captured
        /// request, in order.
        session_id: Option<String>,
        body: Value,
    }

    pub(crate) type Seen = std::sync::Arc<std::sync::Mutex<Vec<SeenRequest>>>;

    /// Whether the in-test server encodes a JSON-RPC reply as a bare
    /// `application/json` body (Task 1's path), or wraps it in a
    /// `text/event-stream` body carrying a decoy notification and a decoy
    /// unrelated-id message ahead of the real answer, and issues an
    /// `Mcp-Session-Id` (Task 2's path). Same route, same `SeenRequest`
    /// capture, same computed result either way — only the wire encoding
    /// differs, so this is one server with a switch, not a second server.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ResponseMode {
        Json,
        Sse,
    }

    #[derive(Clone)]
    struct ServerState {
        sink: Seen,
        mode: ResponseMode,
    }

    /// Minimal in-test MCP server: answers `initialize`, `tools/list` and
    /// `tools/call`, and records every request it receives so tests can
    /// assert on what the client sent, not just what it got back.
    ///
    /// Built on `axum::Router` + `tokio::net::TcpListener` + `axum::serve` —
    /// the same in-process test-server pattern `MockUpstream` already uses in
    /// `plugins/wasm_provider_conformance.rs:164-178`, one file over from this
    /// one. `axum` and `tokio`'s `net` feature are already direct dependencies
    /// of `ryuzi-core` (Cargo.toml), so this needs no new dependency — do NOT
    /// reach for `hyper`/`hyper-util`/`http-body-util` directly, none of the
    /// three is a direct dependency of this crate today.
    async fn handle(
        State(state): State<ServerState>,
        headers: HeaderMap,
        Json(msg): Json<Value>,
    ) -> axum::response::Response {
        state.sink.lock().unwrap().push(SeenRequest {
            protocol_version: headers
                .get("mcp-protocol-version")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            accept: headers
                .get("accept")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            authorization: headers
                .get_all("authorization")
                .iter()
                .filter_map(|v| v.to_str().ok().map(str::to_string))
                .collect(),
            session_id: headers
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            body: msg.clone(),
        });
        let id = msg["id"].clone();
        let result = match msg["method"].as_str().unwrap_or_default() {
            "initialize" => json!({"protocolVersion": "2025-06-18", "capabilities": {}}),
            "tools/list" => json!({"tools": [{
                "name": "ping",
                "description": "ping it",
                "inputSchema": {"type": "object"}
            }]}),
            "tools/call" => {
                // A distinct string per mode makes it obvious, from the
                // assertion text alone, which wire path actually produced a
                // given result.
                let text = if state.mode == ResponseMode::Sse {
                    "ok"
                } else {
                    "pong"
                };
                json!({"content": [{"type": "text", "text": text}]})
            }
            // A notification: nothing to compute, and the client
            // discards whatever comes back (see `handshake`'s `let _ =`).
            "notifications/initialized" => Value::Null,
            other => panic!("unexpected method {other}"),
        };
        match state.mode {
            ResponseMode::Json => (
                StatusCode::OK,
                [("content-type", "application/json")],
                json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
            )
                .into_response(),
            ResponseMode::Sse => {
                // A notification, then an unrelated message, then the real
                // answer — the client must skip the first two and take only
                // the event whose id matches its own pending request.
                let payload = format!(
                    "event: message\ndata: {}\n\nevent: message\ndata: {}\n\nevent: message\ndata: {}\n\n",
                    json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}),
                    json!({"jsonrpc": "2.0", "id": 9999, "result": {"unrelated": true}}),
                    json!({"jsonrpc": "2.0", "id": id, "result": result}),
                );
                (
                    StatusCode::OK,
                    [
                        ("content-type", "text/event-stream"),
                        ("mcp-session-id", "sess-123"),
                    ],
                    payload,
                )
                    .into_response()
            }
        }
    }

    async fn spawn_server(mode: ResponseMode) -> (String, Seen, tokio::task::JoinHandle<()>) {
        let sink: Seen = Default::default();
        let state = ServerState {
            sink: sink.clone(),
            mode,
        };
        let app = Router::new().route("/", post(handle)).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle_task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), sink, handle_task)
    }

    /// Task 1's plain-JSON response path. `pub(crate)`: Task 3's
    /// `harness::native::tests::an_http_mcp_server_contributes_its_tools_to_the_session`
    /// spins up the same in-test server to exercise `connect_mcp_tools`'s
    /// HTTP dispatch, rather than duplicating this fixture.
    pub(crate) async fn spawn_json_server() -> (String, Seen, tokio::task::JoinHandle<()>) {
        spawn_server(ResponseMode::Json).await
    }

    /// Task 2's SSE response path: every reply is wrapped in a
    /// `text/event-stream` body ahead of decoy messages, and the server
    /// issues session id `sess-123` at `initialize`.
    async fn spawn_sse_server() -> (String, Seen, tokio::task::JoinHandle<()>) {
        spawn_server(ResponseMode::Sse).await
    }

    fn spec(url: &str) -> McpServerSpec {
        spec_with_headers(url, vec![])
    }

    fn spec_with_headers(url: &str, headers: Vec<(String, String)>) -> McpServerSpec {
        McpServerSpec {
            name: "test-remote".to_string(),
            transport: McpTransport::Http {
                url: url.to_string(),
                headers,
            },
        }
    }

    #[tokio::test]
    async fn connect_handshakes_and_lists_tools_over_a_json_response() {
        let (url, _seen, _server) = spawn_json_server().await;
        let conn = connect_http(&spec(&url), None)
            .await
            .expect("connect must succeed");

        assert_eq!(conn.server_name, "test-remote");
        assert_eq!(
            conn.tools.len(),
            1,
            "the server advertises exactly one tool"
        );
        assert_eq!(conn.tools[0].name, "ping");
        assert_eq!(conn.tools[0].description, "ping it");
    }

    #[tokio::test]
    async fn call_returns_the_mcp_result_value() {
        let (url, _seen, _server) = spawn_json_server().await;
        let conn = connect_http(&spec(&url), None).await.unwrap();

        let result = conn
            .call("ping", json!({}))
            .await
            .expect("call must succeed");

        let (text, is_error) = crate::harness::native::mcp_client::render_tool_result(&result);
        assert_eq!(text, "pong");
        assert!(
            !is_error,
            "a successful call must not be flagged as an error"
        );
    }

    /// Locks down everything the client is contractually required to SEND,
    /// not just what it does with the response. Existing tests only checked
    /// the latter, so a client that dropped `MCP-Protocol-Version`, narrowed
    /// `Accept` to exclude `text/event-stream`, sent the wrong
    /// `protocolVersion`/`clientInfo.name`, or never confirmed the handshake
    /// with `notifications/initialized` would still pass them unchanged.
    #[tokio::test]
    async fn the_handshake_sends_the_protocol_version_header_a_dual_accept_header_and_correct_initialize_params(
    ) {
        let (url, seen, _server) = spawn_json_server().await;
        connect_http(&spec(&url), None)
            .await
            .expect("connect must succeed");

        let requests = seen.lock().unwrap().clone();
        let init = requests
            .first()
            .expect("the client must send an initialize request before anything else");

        assert_eq!(
            init.protocol_version.as_deref(),
            Some(MCP_PROTOCOL_VERSION),
            "a missing or wrong MCP-Protocol-Version header means a real server can't tell which \
             protocol revision this client speaks and may answer with an incompatible one"
        );

        let accept = init.accept.as_deref().expect(
            "a missing Accept header means a real Streamable HTTP server can't tell this client \
             is willing to receive either response shape it's allowed to send",
        );
        assert!(
            accept.contains("application/json"),
            "Accept must admit application/json, or a JSON-only server has nothing it can answer with: {accept}"
        );
        assert!(
            accept.contains("text/event-stream"),
            "Accept must admit text/event-stream, or an SSE-preferring server can't reply and Task 2's SSE path becomes unreachable in practice: {accept}"
        );

        assert_eq!(
            init.body["params"]["protocolVersion"], MCP_PROTOCOL_VERSION,
            "the initialize body must declare the same protocol version as the header — a \
             mismatch would let a permissive server negotiate a revision this client doesn't \
             actually implement"
        );
        assert_eq!(
            init.body["params"]["clientInfo"]["name"], "ryuzi-native",
            "servers key per-client quirks and rate limits off clientInfo.name — losing this \
             silently breaks that with nothing here to catch it"
        );

        let initialized = requests.get(1).expect(
            "the client must send notifications/initialized right after a successful initialize, \
             or a strict server may refuse to serve tools/list on the same connection",
        );
        assert_eq!(
            initialized.body["method"], "notifications/initialized",
            "the second request the server sees must be the initialized notification — anything \
             else means the handshake confirmation was dropped"
        );
    }

    /// `connect_http`'s doc comment promises a `bearer` argument OVERRIDES
    /// any `Authorization` already in the spec's headers and is not merely
    /// appended alongside it. Neither test above ever passes `Some(..)`, and
    /// per the plan's Task 8, `connect_mcp_tools` only supplies a bearer when
    /// the spec carries none — so this both-present merge path would
    /// otherwise never run under any test at all.
    #[tokio::test]
    async fn a_bearer_argument_overrides_a_manifest_authorization_header_without_duplicating_it() {
        let (url, seen, _server) = spawn_json_server().await;
        let manifest_spec = spec_with_headers(
            &url,
            vec![(
                "Authorization".to_string(),
                "Bearer manifest-token".to_string(),
            )],
        );

        connect_http(&manifest_spec, Some("host-token"))
            .await
            .expect("connect must succeed");

        let requests = seen.lock().unwrap().clone();
        let init = &requests[0];
        assert_eq!(
            init.authorization.len(),
            1,
            "exactly one Authorization header must reach the server — sending both the manifest \
             token and the host-minted one would leak the stale manifest credential alongside \
             the fresh one: {:?}",
            init.authorization
        );
        assert_eq!(
            init.authorization[0], "Bearer host-token",
            "the host-minted bearer must REPLACE the manifest's Authorization value, not just \
             happen to be the one the assertion above picked out"
        );
    }

    /// The property Task 2 exists to protect: on an SSE response body the
    /// client must skip a notification and a message carrying someone
    /// else's id, and resolve the call using only the event whose id
    /// matches the request it actually sent.
    #[tokio::test]
    async fn an_sse_body_yields_the_message_matching_the_pending_id() {
        let (url, _seen, _server) = spawn_sse_server().await;
        let conn = connect_http(&spec(&url), None)
            .await
            .expect("connect over SSE must succeed");

        let result = conn.call("anything", json!({})).await.unwrap();

        let (text, _) = crate::harness::native::mcp_client::render_tool_result(&result);
        assert_eq!(
            text, "ok",
            "the client must skip the notification and the id-9999 message on the SSE stream and \
             resolve the call using only the event matching its own pending id — a wrong value here \
             means it grabbed the first or last event instead of the matching one"
        );
    }

    #[tokio::test]
    async fn the_session_id_is_echoed_on_every_request_after_initialize() {
        let (url, seen, _server) = spawn_sse_server().await;
        let conn = connect_http(&spec(&url), None)
            .await
            .expect("connect over SSE must succeed");
        conn.call("anything", json!({})).await.unwrap();

        let echoed: Vec<Option<String>> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.session_id.clone())
            .collect();
        assert_eq!(
            echoed.first(),
            Some(&None),
            "the very first request (initialize) cannot echo a session id the server has not \
             issued yet: {echoed:?}"
        );
        assert!(
            echoed.len() >= 3,
            "expected at least initialize, notifications/initialized and tools/call to have been \
             captured, got {echoed:?}"
        );
        assert!(
            echoed
                .iter()
                .skip(1)
                .all(|v| v.as_deref() == Some("sess-123")),
            "every request after initialize must carry the session id the server issued at \
             initialize — a stdio-style client that never echoes it would send None here forever \
             instead of sess-123: {echoed:?}"
        );
    }

    /// The single hardest guarantee `sse_message_for_id` makes: a stream
    /// that never carries the pending request's id must be a transport
    /// error, not an empty/`Value::Null` result. Verified directly against
    /// the parser (not through a full connection) so this stays exact and
    /// fast: a body containing only a notification and an unrelated id
    /// (id 7 is never present) must fail rather than resolve.
    #[test]
    fn sse_stream_without_the_pending_id_is_a_transport_error_not_an_empty_result() {
        let pending = json!(7);
        let body = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{\"unrelated\":true}}\n\n",
        );

        let result = sse_message_for_id(body, Some(&pending));

        assert!(
            result.is_err(),
            "a stream that ends without id 7 must be reported as a transport error — resolving to \
             Ok(Value::Null) here would make a truncated or broken upstream response \
             indistinguishable from a tool call that legitimately returned nothing: {result:?}"
        );
    }

    /// `post()` passes `want = None` for a notification (it has no `id` to
    /// wait for). If `sse_message_for_id` ever treated a `None` pending id
    /// as satisfied by ANY message — in particular one whose own `id` is
    /// `null` — a notification post would silently grab that message as if
    /// it were an answer nobody asked for.
    #[test]
    fn no_pending_id_never_opportunistically_matches_a_null_id_message() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":null,\"result\":{}}\n\n";

        let result = sse_message_for_id(body, None);

        assert!(
            result.is_err(),
            "with nothing pending, the stream must end in error rather than matching a null-id \
             message: {result:?}"
        );
    }

    /// The SSE spec lets one event carry MULTIPLE `data:` lines, which a
    /// client must concatenate with `\n` into a single payload before
    /// parsing — a server that pretty-prints its JSON, or that simply
    /// splits a large payload across lines, sends exactly this shape.
    /// Treating each `data:` line as its own JSON document (the pre-fix
    /// behaviour) fails to parse every single line here, so the whole
    /// response goes missing and the call fails with "event stream ended
    /// without a response" even though the server behaved legally.
    #[test]
    fn sse_multi_line_data_lines_within_one_event_are_joined_before_parsing() {
        let pending = json!(3);
        let body = concat!(
            "event: message\n",
            "data: {\n",
            "data:   \"jsonrpc\": \"2.0\",\n",
            "data:   \"id\": 3,\n",
            "data:   \"result\": {}\n",
            "data: }\n",
            "\n",
        );

        let result = sse_message_for_id(body, Some(&pending)).expect(
            "a legally pretty-printed multi-line SSE event must still parse into the pending \
             response — treating each data: line as its own JSON document silently drops it",
        );

        assert_eq!(result["id"], json!(3));
        assert_eq!(result["result"], json!({}));
    }

    /// Naive concatenation across EVENT boundaries (instead of resetting
    /// the accumulator at the blank line the SSE spec defines as the event
    /// terminator) is worse than the plain miss above: it risks silently
    /// splicing fields from an unrelated event into the one being matched.
    /// Two multi-line events — the first carrying a different id, the
    /// second carrying the pending one — must resolve to exactly the
    /// second event's own payload, not something blended with the first.
    #[test]
    fn a_multi_line_event_for_a_different_id_does_not_leak_into_the_pending_events_payload() {
        let pending = json!(3);
        let body = concat!(
            "event: message\n",
            "data: {\n",
            "data:   \"jsonrpc\": \"2.0\",\n",
            "data:   \"id\": 9999,\n",
            "data:   \"result\": {\"marker\": \"unrelated\"}\n",
            "data: }\n",
            "\n",
            "event: message\n",
            "data: {\n",
            "data:   \"jsonrpc\": \"2.0\",\n",
            "data:   \"id\": 3,\n",
            "data:   \"result\": {\"marker\": \"pending\"}\n",
            "data: }\n",
            "\n",
        );

        let result = sse_message_for_id(body, Some(&pending)).expect(
            "the second multi-line event carries the pending id and must be parsed and returned",
        );

        assert_eq!(result["id"], json!(3));
        assert_eq!(
            result["result"]["marker"], "pending",
            "must resolve to the SECOND event's own payload, not one spliced together with the \
             first (unrelated-id) event's data lines: {result:?}"
        );
    }

    // -----------------------------------------------------------------
    // Task 8: refresh-on-401
    // -----------------------------------------------------------------

    /// What `spawn_refresh_fixture` hands back: the two origins it bound,
    /// plus every token-request body the AS actually received, so a test
    /// can assert on what reached the wire.
    struct RefreshFixture {
        mcp_url: String,
        as_url: String,
        token_request_bodies: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    /// Binds a two-origin OAuth-refresh fixture: an MCP resource server and
    /// a SEPARATE authorization server — the same two-origin shape
    /// `mcp_oauth.rs`'s own `spawn_oauth_fixture` uses, since a real
    /// deployment never colocates the two.
    ///
    /// The MCP server 401s any request whose `Authorization` header is not
    /// exactly `Bearer <accepted_bearer>`, and points every 401 at its own
    /// protected-resource metadata; when `accepted_bearer` is `None` it
    /// 401s UNCONDITIONALLY, no matter what token this client ever presents
    /// — the shape the reconnect-required test below needs (a refresh that
    /// succeeds at the AS but doesn't actually fix access, e.g. because the
    /// underlying grant was revoked). The AS's `/token` endpoint accepts a
    /// `refresh_token` grant and always mints `new_access_token`.
    async fn spawn_refresh_fixture(
        accepted_bearer: Option<&'static str>,
        new_access_token: &'static str,
    ) -> RefreshFixture {
        use axum::extract::{Json, State};
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::IntoResponse;
        use axum::routing::{get, post};
        use axum::Router;

        #[derive(Clone)]
        struct AsState {
            as_url: String,
            new_access_token: &'static str,
            token_request_bodies: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        }

        async fn handle_as_metadata(State(state): State<AsState>) -> axum::Json<Value> {
            let as_url = &state.as_url;
            axum::Json(json!({
                "issuer": as_url,
                "authorization_endpoint": format!("{as_url}/authorize"),
                "token_endpoint": format!("{as_url}/token"),
            }))
        }

        async fn handle_token(
            State(state): State<AsState>,
            body: axum::body::Bytes,
        ) -> axum::Json<Value> {
            state
                .token_request_bodies
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&body).into_owned());
            axum::Json(json!({
                "access_token": state.new_access_token,
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "rotated-refresh",
            }))
        }

        let as_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let as_addr = as_listener.local_addr().unwrap();
        let as_url = format!("http://{as_addr}");
        let token_request_bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let as_state = AsState {
            as_url: as_url.clone(),
            new_access_token,
            token_request_bodies: token_request_bodies.clone(),
        };
        let as_app = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(handle_as_metadata),
            )
            .route("/token", post(handle_token))
            .with_state(as_state);
        tokio::spawn(async move {
            axum::serve(as_listener, as_app).await.unwrap();
        });

        #[derive(Clone)]
        struct McpState {
            accepted_bearer: Option<&'static str>,
            as_url: String,
            mcp_url: std::sync::Arc<std::sync::Mutex<String>>,
        }

        async fn handle_mcp(
            State(state): State<McpState>,
            headers: HeaderMap,
            Json(msg): Json<Value>,
        ) -> axum::response::Response {
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let authorized = match (state.accepted_bearer, auth.as_deref()) {
                (Some(want), Some(got)) => got == format!("Bearer {want}"),
                _ => false,
            };
            if !authorized {
                let mcp_url = state.mcp_url.lock().unwrap().clone();
                let www_auth = format!(
                    "Bearer resource_metadata=\"{mcp_url}/.well-known/oauth-protected-resource\""
                );
                return axum::response::Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header(axum::http::header::WWW_AUTHENTICATE, www_auth)
                    .body(axum::body::Body::empty())
                    .unwrap()
                    .into_response();
            }
            let id = msg["id"].clone();
            let result = match msg["method"].as_str().unwrap_or_default() {
                "initialize" => json!({"protocolVersion": "2025-06-18", "capabilities": {}}),
                "tools/list" => json!({"tools": []}),
                _ => Value::Null,
            };
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
            )
                .into_response()
        }

        async fn handle_prm(State(state): State<McpState>) -> axum::Json<Value> {
            axum::Json(json!({
                "resource": "placeholder",
                "authorization_servers": [state.as_url],
            }))
        }

        let mcp_url_slot = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let mcp_state = McpState {
            accepted_bearer,
            as_url: as_url.clone(),
            mcp_url: mcp_url_slot.clone(),
        };
        let mcp_app = Router::new()
            .route("/", post(handle_mcp))
            .route("/.well-known/oauth-protected-resource", get(handle_prm))
            .with_state(mcp_state);
        let mcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mcp_addr = mcp_listener.local_addr().unwrap();
        let mcp_url = format!("http://{mcp_addr}");
        *mcp_url_slot.lock().unwrap() = mcp_url.clone();
        tokio::spawn(async move {
            axum::serve(mcp_listener, mcp_app).await.unwrap();
        });

        RefreshFixture {
            mcp_url,
            as_url,
            token_request_bodies,
        }
    }

    fn stale_token_with_refresh() -> crate::store::McpOauthToken {
        crate::store::McpOauthToken {
            access_token: "stale-token".to_string(),
            refresh_token: Some("refresh-1".to_string()),
            token_type: "Bearer".to_string(),
            expires_at: None,
            scopes: vec![],
            reconnect_required: false,
        }
    }

    /// PROPERTY: a 401 on a store-managed connection triggers exactly ONE
    /// refresh, and the RETRY actually carries the refreshed token — proven
    /// by the MCP server only accepting `Bearer fresh-token`, so a bug that
    /// retried with the stale bearer (or never updated it at all) would
    /// leave the connect failing instead of succeeding.
    #[tokio::test]
    async fn a_401_is_refreshed_once_and_the_retry_succeeds_with_the_new_token() {
        crate::llm_router::secrets::use_test_key_file();
        let fixture = spawn_refresh_fixture(Some("fresh-token"), "fresh-token").await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(crate::store::Store::open(tmp.path()).await.unwrap());
        // Keyed by "test-remote" — the server name `spec()` bakes in below.
        // A mismatch here would make `refresh_stored_token`'s store lookup
        // fail with "no stored token to refresh" before it ever reaches the
        // AS, which would make this test fail for the WRONG reason.
        store
            .upsert_mcp_oauth_token("test-remote", &stale_token_with_refresh())
            .await
            .unwrap();
        store
            .upsert_mcp_oauth_client(&fixture.as_url, "client-1")
            .await
            .unwrap();
        let target_spec = spec(&fixture.mcp_url);

        let conn = connect_http_with_store(&target_spec, Some("stale-token"), store.clone())
            .await
            .expect(
                "the refresh-then-retry must let connect succeed even though the FIRST attempt \
                 (using the stale stored token) 401s",
            );

        assert_eq!(conn.server_name, "test-remote");
        let bodies = fixture.token_request_bodies.lock().unwrap().clone();
        assert_eq!(
            bodies.len(),
            1,
            "exactly one refresh request must have been sent — a bug that refreshed on every \
             request (not just the first 401) would drive this above 1: {bodies:?}"
        );
        assert!(
            bodies[0].contains("grant_type=refresh_token"),
            "the refresh must use the refresh_token grant: {}",
            bodies[0]
        );

        let stored = store
            .get_mcp_oauth_token("test-remote")
            .await
            .unwrap()
            .expect("the refreshed token must still be stored under the server's name");
        assert_eq!(
            stored.access_token, "fresh-token",
            "the store must hold the NEW access token after a successful refresh, not the stale one"
        );
        assert!(
            !stored.reconnect_required,
            "a successful refresh must not leave reconnect_required set"
        );
    }

    /// PROPERTY: when the retry ALSO 401s (a refresh that talks to the AS
    /// successfully but doesn't actually restore access), the server must be
    /// marked `reconnect_required` in the STORE — not merely reported in the
    /// error text. A test that only checked the error message would miss a
    /// regression that failed loudly but never actually persisted the flag,
    /// silently leaving a future connect attempt free to retry the same dead
    /// credential forever.
    #[tokio::test]
    async fn a_401_that_persists_through_a_refresh_marks_the_server_reconnect_required() {
        crate::llm_router::secrets::use_test_key_file();
        // accepted_bearer: None => the MCP server 401s unconditionally, no
        // matter what token this client ever presents.
        let fixture = spawn_refresh_fixture(None, "fresh-token").await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(crate::store::Store::open(tmp.path()).await.unwrap());
        store
            .upsert_mcp_oauth_token("test-remote", &stale_token_with_refresh())
            .await
            .unwrap();
        store
            .upsert_mcp_oauth_client(&fixture.as_url, "client-1")
            .await
            .unwrap();
        let target_spec = spec(&fixture.mcp_url);

        // `.expect_err(..)`/`.unwrap_err(..)` would require `McpHttpConnection`
        // (the `Ok` type) to implement `Debug`, which it deliberately does
        // not — match explicitly instead.
        let err =
            match connect_http_with_store(&target_spec, Some("stale-token"), store.clone()).await {
                Err(e) => e,
                Ok(_) => panic!(
                "a server that keeps 401ing after a refresh must fail the connect, not silently \
                 succeed"
            ),
            };

        assert!(
            err.to_string().contains("test-remote"),
            "the error must name the server so the UI can point the user at the right reconnect \
             control: {err}"
        );

        let stored = store
            .get_mcp_oauth_token("test-remote")
            .await
            .unwrap()
            .expect("the token row must still exist after a failed reconnect attempt");
        assert!(
            stored.reconnect_required,
            "a 401 that survives a refresh-and-retry must mark the server reconnect_required in \
             the store"
        );
    }
}
