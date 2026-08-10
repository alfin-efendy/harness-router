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

use super::mcp_client::{build_call_request, McpCaller, McpToolDef};
use crate::domain::{McpServerSpec, McpTransport};
use crate::stdio_jsonrpc;

/// The MCP protocol version this client speaks. Same value the stdio
/// handshake sends — there is deliberately only one in the crate.
const PROTOCOL_VERSION: &str = "2025-06-18";

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
    if !url.starts_with("https://") && !cfg!(test) {
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
                "protocolVersion": PROTOCOL_VERSION,
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
    /// Task 2 extends this to read an SSE response body.
    async fn post(&self, message: &Value) -> anyhow::Result<(Value, Option<String>)> {
        let mut request = self
            .http
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION);
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
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("mcp: HTTP {status}");
        }
        if body.trim().is_empty() {
            return Ok((Value::Null, session));
        }
        Ok((serde_json::from_str(&body)?, session))
    }
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

    /// Minimal in-test MCP server: answers `initialize`, `tools/list` and
    /// `tools/call` with plain JSON (no SSE — Task 2 covers that path).
    ///
    /// Built on `axum::Router` + `tokio::net::TcpListener` + `axum::serve` —
    /// the same in-process test-server pattern `MockUpstream` already uses in
    /// `plugins/wasm_provider_conformance.rs:164-178`, one file over from this
    /// one. `axum` and `tokio`'s `net` feature are already direct dependencies
    /// of `ryuzi-core` (Cargo.toml), so this needs no new dependency — do NOT
    /// reach for `hyper`/`hyper-util`/`http-body-util` directly, none of the
    /// three is a direct dependency of this crate today.
    async fn spawn_json_server() -> (String, tokio::task::JoinHandle<()>) {
        use axum::extract::Json;
        use axum::http::StatusCode;
        use axum::routing::post;
        use axum::Router;

        async fn handle(
            Json(msg): Json<Value>,
        ) -> (StatusCode, [(&'static str, &'static str); 1], String) {
            let id = msg["id"].clone();
            let result = match msg["method"].as_str().unwrap_or_default() {
                "initialize" => json!({"protocolVersion": "2025-06-18", "capabilities": {}}),
                "tools/list" => json!({"tools": [{
                    "name": "ping",
                    "description": "ping it",
                    "inputSchema": {"type": "object"}
                }]}),
                "tools/call" => json!({"content": [{"type": "text", "text": "pong"}]}),
                other => panic!("unexpected method {other}"),
            };
            let payload = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                payload,
            )
        }

        let app = Router::new().route("/", post(handle));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle_task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), handle_task)
    }

    fn spec(url: &str) -> McpServerSpec {
        McpServerSpec {
            name: "test-remote".to_string(),
            transport: McpTransport::Http {
                url: url.to_string(),
                headers: vec![],
            },
        }
    }

    #[tokio::test]
    async fn connect_handshakes_and_lists_tools_over_a_json_response() {
        let (url, _server) = spawn_json_server().await;
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
        let (url, _server) = spawn_json_server().await;
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
}
