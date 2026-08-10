//! Native MCP client over Streamable HTTP — the remote counterpart of
//! [`super::mcp_client::McpConnection`]'s stdio transport.
//!
//! Kept in its own module rather than grown into `mcp_client.rs`: the stdio
//! connection owns a child process and newline framing, this one owns an HTTP
//! client, an optional session id and a credential. Different lifetimes,
//! different failure modes. Both implement [`McpCaller`], so a remote server's
//! tools reach a session through exactly the same path.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::mcp_client::{build_call_request, McpCaller, McpToolDef, MCP_PROTOCOL_VERSION};
use crate::domain::{McpServerSpec, McpTransport};
use crate::stdio_jsonrpc;

/// A live remote MCP server connection.
pub struct McpHttpConnection {
    http: reqwest::Client,
    url: String,
    /// Static headers from the server spec (a manifest-resolved API token or
    /// injected OAuth bearer), plus the `Authorization` this connection was
    /// opened with, if any.
    headers: Vec<(String, String)>,
    /// `Mcp-Session-Id` if the server issued one at `initialize`; echoed on
    /// every subsequent request.
    session_id: Mutex<Option<String>>,
    next_id: AtomicI64,
    pub server_name: String,
    pub tools: Vec<McpToolDef>,
}

/// Open a remote MCP connection: handshake, then list its tools.
///
/// `bearer`, when present, is sent as `Authorization: Bearer <bearer>` and
/// OVERRIDES any `Authorization` already in the spec's headers — a token this
/// host just minted is always fresher than one baked into a manifest.
pub async fn connect_http(
    spec: &McpServerSpec,
    bearer: Option<&str>,
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
        headers: merged,
        session_id: Mutex::new(None),
        next_id: AtomicI64::new(1),
        server_name: spec.name.clone(),
        tools: Vec::new(),
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
    async fn post(&self, message: &Value) -> anyhow::Result<(Value, Option<String>)> {
        let mut request = self
            .http
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION);
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        if let Some(session) = self.session_id.lock().await.as_deref() {
            request = request.header("Mcp-Session-Id", session);
        }
        let response = tokio::time::timeout(Duration::from_secs(120), request.json(message).send())
            .await
            .map_err(|_| anyhow::anyhow!("mcp: request timed out"))??;
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
            return Ok((Value::Null, session));
        }
        if content_type.contains("text/event-stream") {
            let want = message.get("id").cloned();
            return Ok((sse_message_for_id(&body, want.as_ref())?, session));
        }
        Ok((serde_json::from_str(&body)?, session))
    }
}

/// Pull the JSON-RPC message whose `id` matches `want` out of an SSE body.
///
/// Any other message on the stream — a notification, or a server-initiated
/// request this client does not implement — is skipped rather than mistaken
/// for the answer. A stream that ends without the wanted id is a TRANSPORT
/// ERROR, not an empty result: silently resolving to `Value::Null` here would
/// make a truncated or broken upstream response indistinguishable from a tool
/// that legitimately returned nothing.
fn sse_message_for_id(body: &str, want: Option<&Value>) -> anyhow::Result<Value> {
    let mut skipped = 0usize;
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(message) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        match (want, message.get("id")) {
            (Some(want), Some(got)) if want == got => return Ok(message),
            _ => skipped += 1,
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
mod tests {
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
    struct SeenRequest {
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

    type Seen = std::sync::Arc<std::sync::Mutex<Vec<SeenRequest>>>;

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

    /// Task 1's plain-JSON response path.
    async fn spawn_json_server() -> (String, Seen, tokio::task::JoinHandle<()>) {
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
}
