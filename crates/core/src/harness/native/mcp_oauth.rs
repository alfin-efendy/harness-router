//! MCP-specific OAuth discovery (RFC 9728) on top of the OAuth primitives that
//! already ship for plugin profiles.
//!
//! Only three things here are new to this codebase: fetching the Protected
//! Resource Metadata document, choosing an authorization server from it, and
//! deriving the canonical resource URI that RFC 8707 requires on both the
//! authorize and token requests. RFC 8414 discovery, RFC 7591 registration and
//! PKCE all come from [`crate::plugins::oauth`].

use serde_json::Value;

use crate::plugins::oauth::{discover_oauth_server_metadata, OauthServerMetadata};

/// The canonical URI of an MCP server, as RFC 8707 §2 defines it and the MCP
/// specification requires: lowercase scheme and host, no fragment, and no
/// trailing slash unless the slash is semantically significant.
pub fn canonical_resource_uri(url: &str) -> anyhow::Result<String> {
    let parsed =
        url::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid MCP server URL {url}: {e}"))?;
    if parsed.fragment().is_some() {
        anyhow::bail!("MCP server URL must not contain a fragment: {url}");
    }
    if parsed.host_str().is_none() {
        anyhow::bail!("MCP server URL must be absolute: {url}");
    }
    let mut canonical = format!(
        "{}://{}",
        parsed.scheme().to_ascii_lowercase(),
        parsed.host_str().unwrap().to_ascii_lowercase()
    );
    if let Some(port) = parsed.port() {
        canonical.push_str(&format!(":{port}"));
    }
    let path = parsed.path().trim_end_matches('/');
    canonical.push_str(path);
    Ok(canonical)
}

/// The `authorization_servers` list of a PRM document, in document order.
pub fn authorization_servers_from_prm(doc: &Value) -> anyhow::Result<Vec<String>> {
    let servers: Vec<String> = doc
        .get("authorization_servers")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if servers.is_empty() {
        anyhow::bail!("the MCP server's protected-resource metadata names no authorization server");
    }
    Ok(servers)
}

/// Fetch and parse the RFC 9728 document at `metadata_url`.
pub async fn protected_resource_metadata(
    http: &reqwest::Client,
    metadata_url: &str,
) -> anyhow::Result<Vec<String>> {
    let response = http.get(metadata_url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!(
            "protected-resource metadata fetch failed: HTTP {}",
            response.status()
        );
    }
    authorization_servers_from_prm(&response.json::<Value>().await?)
}

/// Take the candidates IN DOCUMENT ORDER and use the first whose RFC 8414
/// metadata fetches and parses. RFC 9728 §7.6 leaves selection to the client;
/// any cleverer rule would be guessing on the user's behalf.
pub async fn select_authorization_server(
    http: &reqwest::Client,
    issuers: &[String],
) -> anyhow::Result<(String, OauthServerMetadata)> {
    let mut tried = Vec::new();
    for issuer in issuers {
        match discover_oauth_server_metadata(http, issuer).await {
            Ok(metadata) => return Ok((issuer.clone(), metadata)),
            Err(e) => tried.push(format!("{issuer}: {e}")),
        }
    }
    anyhow::bail!(
        "no usable authorization server — tried {}",
        tried.join("; ")
    )
}

/// A started PKCE authorization: the URL to open, plus the verifier and state
/// the callback must be matched against.
#[derive(Debug)]
pub struct McpAuthorizeStart {
    pub url: String,
    pub verifier: String,
    pub state: String,
}

/// Build the authorization URL. `resource` MUST be the canonical MCP server
/// URI — see [`canonical_resource_uri`] — and is sent whether or not the
/// authorization server advertises support for it, as the spec requires.
pub fn build_authorize_url(
    metadata: &OauthServerMetadata,
    client_id: &str,
    redirect_uri: &str,
    resource: &str,
    scopes: &[String],
) -> anyhow::Result<McpAuthorizeStart> {
    let verifier = crate::plugins::oauth::generate_pkce_verifier();
    let challenge = crate::plugins::oauth::pkce_challenge_s256(&verifier);
    let state = crate::plugins::oauth::generate_pkce_verifier();
    let mut url = url::Url::parse(&metadata.authorization_endpoint)
        .map_err(|e| anyhow::anyhow!("invalid authorization_endpoint: {e}"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", client_id);
        q.append_pair("redirect_uri", redirect_uri);
        q.append_pair("code_challenge", &challenge);
        q.append_pair("code_challenge_method", "S256");
        q.append_pair("state", &state);
        q.append_pair("resource", resource);
        if !scopes.is_empty() {
            q.append_pair("scope", &scopes.join(" "));
        }
    }
    Ok(McpAuthorizeStart {
        url: url.to_string(),
        verifier,
        state,
    })
}

/// The form body for the authorization-code exchange. `resource` appears here
/// too — RFC 8707 requires it on the token request as well, and a token minted
/// without it is not bound to this MCP server.
pub fn token_request_form(
    code: &str,
    verifier: &str,
    client_id: &str,
    redirect_uri: &str,
    resource: &str,
) -> Vec<(String, String)> {
    vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
        ("client_id".to_string(), client_id.to_string()),
        ("code_verifier".to_string(), verifier.to_string()),
        ("resource".to_string(), resource.to_string()),
    ]
}

/// The loopback redirect this client registers and uses for a given MCP
/// server. Port 8976 is the same fixed port the plugin install wizard's
/// OAuth flow binds (`PLUGIN_OAUTH_CALLBACK_PORT`,
/// `crates/core/src/api/plugins_api.rs`) — reusing it, rather than a second
/// port, keeps this to one thing to keep free and one thing to explain. The
/// literal is duplicated here rather than imported: that constant is
/// private to `plugins_api.rs`, and the equivalent Cockpit-side listener
/// (`apps/cockpit/src-tauri/src/plugins_cmd.rs`) already independently
/// redefines its own copy of the same port for the same reason — the daemon
/// and Cockpit are separate processes with no shared Rust module boundary
/// here to draw a single canonical constant from.
pub fn mcp_redirect_uri(server_name: &str) -> String {
    format!("http://127.0.0.1:8976/mcp-oauth/{server_name}/callback")
}

/// Probe the server with a credential-less request, discover its
/// authorization server from the resulting 401's `WWW-Authenticate` header,
/// ensure a client id is registered for it, and build the authorize URL.
///
/// The probe is deliberately not a full [`super::mcp_http::connect_http`]
/// handshake: only the 401 and its header matter here, so a lighter,
/// purpose-built request keeps this function usable before any bearer
/// exists to hand to a real connection.
pub async fn begin_mcp_connect(
    store: &crate::store::Store,
    http: &reqwest::Client,
    server: &crate::domain::McpServerSpec,
) -> anyhow::Result<McpAuthorizeStart> {
    let crate::domain::McpTransport::Http { url, .. } = &server.transport else {
        anyhow::bail!("{} is not a remote MCP server", server.name);
    };
    // A bare initialize with no credential: the 401 is the point — it is
    // what carries the WWW-Authenticate header this whole flow starts from.
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
                "clientInfo": {"name": "ryuzi-native", "version": env!("CARGO_PKG_VERSION")}
            }
        }))
        .send()
        .await?;
    if probe.status() != reqwest::StatusCode::UNAUTHORIZED {
        anyhow::bail!(
            "{} did not ask for authorization (HTTP {}) — it may need no credential, or a static one already in its manifest",
            server.name,
            probe.status()
        );
    }
    let header = probe
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} returned 401 without a WWW-Authenticate header — there is nothing to discover \
                 an authorization server from, so it cannot be connected via OAuth",
                server.name
            )
        })?;
    let metadata_url =
        crate::plugins::oauth::parse_www_authenticate_resource(header).ok_or_else(|| {
            anyhow::anyhow!(
            "{}'s WWW-Authenticate header names no protected-resource metadata to discover from",
            server.name
        )
        })?;

    let issuers = protected_resource_metadata(http, &metadata_url).await?;
    let (issuer, metadata) = select_authorization_server(http, &issuers).await?;

    let redirect_uri = mcp_redirect_uri(&server.name);
    // A client id already registered for this issuer (by this server or any
    // other one behind the same authorization server) is reused verbatim —
    // registering a second time would leave an orphaned client id at the AS
    // for no benefit, and some authorization servers rate-limit DCR.
    let client_id = match store.get_mcp_oauth_client(&issuer).await? {
        Some(existing) => existing,
        None => {
            let registration = metadata.registration_endpoint.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "{issuer} supports no dynamic client registration — a client id for it must be supplied manually before {} can connect",
                    server.name
                )
            })?;
            let id =
                crate::plugins::oauth::register_oauth_client(http, registration, &redirect_uri)
                    .await?;
            store.upsert_mcp_oauth_client(&issuer, &id).await?;
            id
        }
    };
    build_authorize_url(
        &metadata,
        &client_id,
        &redirect_uri,
        &canonical_resource_uri(url)?,
        &[],
    )
}

/// Exchange the callback's authorization code for a token and store it under
/// the server's name. The `resource` parameter — the same canonical MCP
/// server URI [`begin_mcp_connect`] put on the authorize request — rides
/// along via [`token_request_form`], so the minted token stays bound to this
/// server rather than silently losing its audience restriction.
///
/// `issuer_token_endpoint` and `client_id` are supplied by the caller rather
/// than rediscovered here: the caller already holds both from the matching
/// `begin_mcp_connect` call (the client id was also just persisted to
/// `mcp_oauth_clients` by that call, so a caller that isn't threading it
/// through by hand can re-read it with [`crate::store::Store::get_mcp_oauth_client`]).
/// Re-running discovery here would risk landing on a different authorization
/// server than the one that actually issued the code.
#[allow(clippy::too_many_arguments)]
pub async fn complete_mcp_connect(
    store: &crate::store::Store,
    http: &reqwest::Client,
    server_name: &str,
    server_url: &str,
    issuer_token_endpoint: &str,
    client_id: &str,
    code: &str,
    verifier: &str,
) -> anyhow::Result<()> {
    let form = token_request_form(
        code,
        verifier,
        client_id,
        &mcp_redirect_uri(server_name),
        &canonical_resource_uri(server_url)?,
    );
    let response = http.post(issuer_token_endpoint).form(&form).send().await?;
    if !response.status().is_success() {
        anyhow::bail!(
            "token exchange failed for {server_name}: HTTP {}",
            response.status()
        );
    }
    let body: serde_json::Value = response.json().await?;
    let token = crate::store::McpOauthToken {
        access_token: body["access_token"]
            .as_str()
            .ok_or_else(|| {
                anyhow::anyhow!("{server_name}'s token response carried no access_token")
            })?
            .to_string(),
        refresh_token: body["refresh_token"].as_str().map(str::to_string),
        token_type: body["token_type"].as_str().unwrap_or("Bearer").to_string(),
        expires_at: body["expires_in"]
            .as_i64()
            .map(|secs| crate::paths::now_ms() + secs * 1000),
        scopes: body["scope"]
            .as_str()
            .map(|s| s.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default(),
        reconnect_required: false,
    };
    store.upsert_mcp_oauth_token(server_name, &token).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_uri_lowercases_scheme_and_host_and_drops_a_trailing_slash() {
        assert_eq!(
            canonical_resource_uri("HTTPS://MCP.Example.COM/mcp/").unwrap(),
            "https://mcp.example.com/mcp",
            "RFC 8707 canonicalization must lowercase scheme/host and drop the trailing slash, \
             or the resource identifier sent to the AS won't match what the PRM/token audience expects"
        );
        assert_eq!(
            canonical_resource_uri("https://mcp.example.com/").unwrap(),
            "https://mcp.example.com",
            "a bare-root URL must canonicalize to no path at all, not an empty-string path"
        );
        assert_eq!(
            canonical_resource_uri("https://mcp.example.com:8443/x").unwrap(),
            "https://mcp.example.com:8443/x",
            "a non-default port must be preserved verbatim"
        );
    }

    #[test]
    fn canonical_uri_rejects_a_fragment_and_a_missing_scheme() {
        // RFC 8707 forbids a fragment; a bare host is not an absolute URI.
        assert!(
            canonical_resource_uri("https://mcp.example.com#frag").is_err(),
            "a fragment must be rejected, not silently stripped — RFC 8707 forbids fragments on the resource parameter"
        );
        assert!(
            canonical_resource_uri("mcp.example.com").is_err(),
            "a schemeless host must be rejected rather than accepted as if it had a scheme"
        );
    }

    #[test]
    fn prm_authorization_servers_are_returned_in_document_order() {
        let doc = serde_json::json!({
            "resource": "https://mcp.example.com",
            "authorization_servers": ["https://as-one.example", "https://as-two.example"]
        });
        assert_eq!(
            authorization_servers_from_prm(&doc).unwrap(),
            vec![
                "https://as-one.example".to_string(),
                "https://as-two.example".to_string()
            ],
            "server selection must try issuers in document order — reordering here would silently \
             change which authorization server a user gets routed to"
        );
    }

    #[test]
    fn a_prm_document_without_authorization_servers_is_an_error() {
        // The MCP spec requires at least one; an empty or absent list means
        // the server cannot be authorized and the user needs to be told so,
        // not handed a silent no-op.
        let empty = serde_json::json!({ "resource": "https://mcp.example.com", "authorization_servers": [] });
        assert!(
            authorization_servers_from_prm(&empty).is_err(),
            "an empty authorization_servers array names no AS to authorize against — this must be an \
             actionable error, not Ok(vec![]) that a caller could mistake for 'no auth needed'"
        );
        assert!(
            authorization_servers_from_prm(&serde_json::json!({})).is_err(),
            "a PRM document missing the field entirely must fail the same way as an empty array, \
             not be treated as a different (silently-ignored) case"
        );
    }

    fn metadata() -> OauthServerMetadata {
        serde_json::from_value(serde_json::json!({
            "issuer": "https://as.example",
            "authorization_endpoint": "https://as.example/authorize",
            "token_endpoint": "https://as.example/token"
        }))
        .unwrap()
    }

    #[test]
    fn the_authorize_url_carries_pkce_and_the_resource_parameter() {
        let start = build_authorize_url(
            &metadata(),
            "client-1",
            "http://127.0.0.1:8976/mcp-oauth/rovo/callback",
            "https://mcp.example.com",
            &["read".to_string()],
        )
        .unwrap();

        let url = url::Url::parse(&start.url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            q.get("response_type").map(String::as_str),
            Some("code"),
            "an authorization-code grant must be requested explicitly"
        );
        assert_eq!(
            q.get("code_challenge_method").map(String::as_str),
            Some("S256"),
            "the challenge method must be S256, not the deprecated plain method"
        );
        assert_eq!(
            q.get("client_id").map(String::as_str),
            Some("client-1"),
            "the registered client id must be forwarded verbatim"
        );
        assert_eq!(
            q.get("resource").map(String::as_str),
            Some("https://mcp.example.com"),
            "RFC 8707 requires `resource` on the authorization request — without it the token this \
             flow eventually mints is not bound to this MCP server, and nothing here would tell you"
        );
        assert!(
            !start.verifier.is_empty() && !start.state.is_empty(),
            "both the PKCE verifier and the anti-CSRF state must be non-empty or the callback has \
             nothing to validate the response against"
        );
        assert_eq!(
            q.get("state").map(String::as_str),
            Some(start.state.as_str()),
            "the URL's `state` query parameter must equal the value returned in the struct — if a \
             future edit stopped appending it to the URL, the OAuth callback would have nothing to \
             match against and CSRF protection would be silently gone, while this struct field would \
             still read as populated"
        );
        assert_ne!(
            q.get("code_challenge").map(String::as_str),
            Some(start.verifier.as_str()),
            "the challenge sent to the AS must be the S256 hash of the verifier, never the raw \
             verifier itself — sending the verifier would defeat PKCE entirely"
        );
        assert_eq!(
            q.get("code_challenge").map(String::as_str),
            Some(crate::plugins::oauth::pkce_challenge_s256(&start.verifier)).as_deref(),
            "the URL's `code_challenge` must be exactly the S256 hash of the returned verifier — a \
             truncated hash, the wrong algorithm, or a double-encoding would still pass the weaker \
             not-equal-to-the-verifier check above, but not this one"
        );
    }

    #[test]
    fn the_token_request_also_carries_the_resource_parameter() {
        // This is the half implementers forget: the spec requires `resource`
        // on BOTH requests, and omitting it here yields a token that is not
        // audience-bound even though the authorize call looked correct — and
        // nothing at runtime would tell you, because the token endpoint still
        // returns 200. This test is deliberately separate from the authorize
        // one above so a regression that drops `resource` from only one of
        // the two call sites still fails a test.
        let form = token_request_form(
            "the-code",
            "the-verifier",
            "client-1",
            "http://127.0.0.1:8976/mcp-oauth/rovo/callback",
            "https://mcp.example.com",
        );
        let map: std::collections::HashMap<_, _> = form.into_iter().collect();
        assert_eq!(
            map.get("grant_type").map(String::as_str),
            Some("authorization_code"),
            "the code exchange must use the authorization_code grant"
        );
        assert_eq!(
            map.get("code").map(String::as_str),
            Some("the-code"),
            "the authorization code returned by the callback must be forwarded verbatim"
        );
        assert_eq!(
            map.get("code_verifier").map(String::as_str),
            Some("the-verifier"),
            "the PKCE verifier must accompany the code or the AS cannot validate the challenge \
             it was given at the authorize step"
        );
        assert_eq!(
            map.get("resource").map(String::as_str),
            Some("https://mcp.example.com"),
            "RFC 8707 requires `resource` on the TOKEN request too, not just the authorize request — \
             this is the parameter half of implementers drop, and dropping it here mints an \
             unbound token that still looks like success"
        );
    }

    /// Bind a loopback server that answers `GET /.well-known/oauth-authorization-server`
    /// with `body` when `Some`, or 404s every request when `None` (an empty
    /// `axum::Router` has no route and falls through to its default 404
    /// handler). Same `axum::Router` + `tokio::net::TcpListener` + `axum::serve`
    /// pattern `mcp_http.rs`'s `spawn_json_server` and
    /// `wasm_provider_conformance.rs`'s `MockUpstream` already use — no new
    /// dependency needed.
    async fn spawn_as_metadata_server(body: Option<Value>) -> String {
        use axum::routing::get;
        use axum::Router;

        let app = match body {
            Some(body) => Router::new().route(
                "/.well-known/oauth-authorization-server",
                get(move || {
                    let body = body.clone();
                    async move { axum::Json(body) }
                }),
            ),
            None => Router::new(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn as_metadata_body(issuer: &str) -> Value {
        serde_json::json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
        })
    }

    /// Bind a loopback server for the protected-resource-metadata tests below.
    /// `status` is returned verbatim; `body`, when `Some`, is served as the
    /// JSON response body for a successful status.
    async fn spawn_prm_server(status: axum::http::StatusCode, body: Option<Value>) -> String {
        use axum::routing::get;
        use axum::Router;

        let app = Router::new().route(
            "/.well-known/oauth-protected-resource",
            get(move || async move { (status, axum::Json(body.unwrap_or(Value::Null))) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}/.well-known/oauth-protected-resource")
    }

    #[tokio::test]
    async fn protected_resource_metadata_names_the_status_on_a_non_2xx_response() {
        let url = spawn_prm_server(axum::http::StatusCode::NOT_FOUND, None).await;
        let http = reqwest::Client::new();

        let err = protected_resource_metadata(&http, &url)
            .await
            .expect_err("a non-2xx status must be an error, not an empty/default result");

        assert!(
            err.to_string().contains("404"),
            "the error must name the HTTP status the server actually returned, so the user gets an \
             actionable message instead of a generic failure: {err}"
        );
    }

    #[tokio::test]
    async fn protected_resource_metadata_rejects_a_2xx_body_with_no_authorization_servers() {
        let url = spawn_prm_server(
            axum::http::StatusCode::OK,
            Some(serde_json::json!({ "resource": "https://mcp.example.com", "authorization_servers": [] })),
        )
        .await;
        let http = reqwest::Client::new();

        let err = protected_resource_metadata(&http, &url).await.expect_err(
            "the MCP spec requires at least one authorization server; a 2xx body naming none must \
             be an actionable error, not a silent empty Vec the caller could mistake for success",
        );
        assert!(
            err.to_string().contains("names no authorization server"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn select_authorization_server_tries_candidates_in_document_order() {
        // The FIRST candidate's metadata fetch fails (no route on that
        // server); the SECOND succeeds. A test where the first candidate
        // also succeeds would prove nothing about ordering — it could pass
        // even if the function picked candidates at random.
        let failing = spawn_as_metadata_server(None).await;
        let working =
            spawn_as_metadata_server(Some(as_metadata_body("https://good-as.example"))).await;
        let http = reqwest::Client::new();

        let (issuer, metadata) =
            select_authorization_server(&http, &[failing.clone(), working.clone()])
                .await
                .expect("the second candidate is usable, so selection must succeed overall");

        assert_eq!(
            issuer, working,
            "document order means the first candidate whose RFC 8414 metadata fetches and parses \
             wins — here that's the second URL, since the first 404s"
        );
        assert_eq!(
            metadata.token_endpoint, "https://good-as.example/token",
            "the metadata returned must be the SECOND candidate's, not some other document — \
             pinning this rules out a bug that returns the right issuer string but the wrong metadata"
        );
    }

    #[tokio::test]
    async fn select_authorization_server_returns_the_first_candidates_metadata_when_multiple_succeed(
    ) {
        // Unlike the fallback test above, BOTH candidates are individually
        // valid here. If selection preferred the last successful candidate,
        // or otherwise ignored document order, this would pick the second
        // server's metadata instead of the first's — the fallback-only test
        // above cannot detect that, since it only ever has one viable answer.
        let first =
            spawn_as_metadata_server(Some(as_metadata_body("https://as-first.example"))).await;
        let second =
            spawn_as_metadata_server(Some(as_metadata_body("https://as-second.example"))).await;
        let http = reqwest::Client::new();

        let (issuer, metadata) =
            select_authorization_server(&http, &[first.clone(), second.clone()])
                .await
                .expect("both candidates are individually valid");

        assert_eq!(
            issuer, first,
            "the FIRST candidate must win when both succeed — document order decides, not any \
             other rule (e.g. preferring the last one tried)"
        );
        assert_eq!(metadata.token_endpoint, "https://as-first.example/token");
    }

    #[tokio::test]
    async fn select_authorization_server_names_every_issuer_tried_when_all_fail() {
        let bad_one = spawn_as_metadata_server(None).await;
        let bad_two = spawn_as_metadata_server(None).await;
        let http = reqwest::Client::new();

        let err = select_authorization_server(&http, &[bad_one.clone(), bad_two.clone()])
            .await
            .expect_err("every candidate failing must surface as one error, not a silent None");

        let message = err.to_string();
        assert!(
            message.contains(&bad_one),
            "the error must name the FIRST issuer tried, not just the last: {message}"
        );
        assert!(
            message.contains(&bad_two),
            "the error must name the SECOND issuer tried too: {message}"
        );
    }

    // -----------------------------------------------------------------
    // Task 7: begin_mcp_connect / complete_mcp_connect
    // -----------------------------------------------------------------

    use crate::domain::{McpServerSpec, McpTransport};
    use std::sync::{Arc, Mutex};

    fn mcp_spec(url: &str) -> McpServerSpec {
        McpServerSpec {
            name: "rovo".to_string(),
            transport: McpTransport::Http {
                url: url.to_string(),
                headers: vec![],
            },
        }
    }

    /// Bind a bare MCP resource server that unconditionally 401s a POST with
    /// no `WWW-Authenticate` header at all — the case where there is
    /// nothing whatsoever to discover from.
    async fn spawn_mcp_401_without_www_authenticate() -> String {
        use axum::routing::post;
        use axum::Router;

        async fn handle() -> axum::http::StatusCode {
            axum::http::StatusCode::UNAUTHORIZED
        }

        let app = Router::new().route("/", post(handle));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    /// Bind an MCP resource server that 401s a POST with a `WWW-Authenticate`
    /// header naming its own protected-resource metadata endpoint, which in
    /// turn serves `prm_body` verbatim. Shared by every discovery-path test
    /// below — only the PRM body differs test to test (a real AS, or an
    /// authorization_servers-less document).
    async fn spawn_mcp_401_with_prm(prm_body: Value) -> String {
        use axum::extract::State;
        use axum::response::IntoResponse;
        use axum::routing::{get, post};
        use axum::Router;

        #[derive(Clone)]
        struct McpState {
            mcp_url: Arc<Mutex<String>>,
            prm_body: Value,
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
                .into_response()
        }

        async fn handle_prm(State(state): State<McpState>) -> axum::Json<Value> {
            axum::Json(state.prm_body.clone())
        }

        // The WWW-Authenticate value needs the server's own address, which
        // is only known after binding — the shared `mcp_url` slot is filled
        // in right after bind, before the first request can possibly land.
        let mcp_url_slot = Arc::new(Mutex::new(String::new()));
        let state = McpState {
            mcp_url: mcp_url_slot.clone(),
            prm_body,
        };
        let app = Router::new()
            .route("/", post(handle_probe))
            .route("/.well-known/oauth-protected-resource", get(handle_prm))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mcp_url = format!("http://{addr}");
        *mcp_url_slot.lock().unwrap() = mcp_url.clone();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        mcp_url
    }

    /// A full, self-contained OAuth round: the remote MCP server that 401s
    /// and points at its own protected-resource metadata, and a SEPARATE
    /// authorization server serving RFC 8414 metadata, RFC 7591
    /// registration, and the token endpoint. Two servers, not one — a real
    /// deployment never colocates an MCP resource server with the
    /// authorization server that issues its tokens, and testing that shape
    /// here would hide any code that quietly assumed they were the same
    /// origin.
    struct OauthFixture {
        mcp_url: String,
        as_url: String,
        /// Every request body the token endpoint received, as raw
        /// `application/x-www-form-urlencoded` text — captured so a test can
        /// assert on what the client actually put ON THE WIRE (e.g. that
        /// `resource=` is really there), not merely on what
        /// `token_request_form` returns in isolation.
        token_request_bodies: Arc<Mutex<Vec<String>>>,
        /// Count of POSTs the registration (DCR) endpoint received.
        registration_hits: Arc<Mutex<usize>>,
    }

    async fn spawn_oauth_fixture() -> OauthFixture {
        use axum::extract::{Json, State};
        use axum::routing::{get, post};
        use axum::Router;

        #[derive(Clone)]
        struct AsState {
            as_url: String,
            registration_hits: Arc<Mutex<usize>>,
            token_request_bodies: Arc<Mutex<Vec<String>>>,
        }

        async fn handle_as_metadata(State(state): State<AsState>) -> axum::Json<Value> {
            let as_url = &state.as_url;
            axum::Json(json!({
                "issuer": as_url,
                "authorization_endpoint": format!("{as_url}/authorize"),
                "token_endpoint": format!("{as_url}/token"),
                "registration_endpoint": format!("{as_url}/register"),
            }))
        }

        async fn handle_register(
            State(state): State<AsState>,
            Json(_req): Json<Value>,
        ) -> axum::Json<Value> {
            *state.registration_hits.lock().unwrap() += 1;
            axum::Json(json!({"client_id": "the-registered-client"}))
        }

        async fn handle_token(
            State(state): State<AsState>,
            body: axum::body::Bytes,
        ) -> axum::Json<Value> {
            let text = String::from_utf8_lossy(&body).into_owned();
            state.token_request_bodies.lock().unwrap().push(text);
            axum::Json(json!({
                "access_token": "issued-token",
                "token_type": "Bearer",
                "expires_in": 3600,
                "refresh_token": "issued-refresh",
                "scope": "read write"
            }))
        }

        let as_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let as_addr = as_listener.local_addr().unwrap();
        let as_url = format!("http://{as_addr}");
        let registration_hits = Arc::new(Mutex::new(0usize));
        let token_request_bodies = Arc::new(Mutex::new(Vec::new()));
        let as_state = AsState {
            as_url: as_url.clone(),
            registration_hits: registration_hits.clone(),
            token_request_bodies: token_request_bodies.clone(),
        };
        let as_app = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(handle_as_metadata),
            )
            .route("/register", post(handle_register))
            .route("/token", post(handle_token))
            .with_state(as_state);
        tokio::spawn(async move {
            axum::serve(as_listener, as_app).await.unwrap();
        });

        let mcp_url = spawn_mcp_401_with_prm(json!({
            "resource": "placeholder",
            "authorization_servers": [as_url.clone()],
        }))
        .await;

        OauthFixture {
            mcp_url,
            as_url,
            token_request_bodies,
            registration_hits,
        }
    }

    #[tokio::test]
    async fn a_401_drives_discovery_registration_and_a_stored_token_bound_to_the_resource() {
        let fixture = spawn_oauth_fixture().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        let http = reqwest::Client::new();
        let spec = mcp_spec(&fixture.mcp_url);

        let start = begin_mcp_connect(&store, &http, &spec)
            .await
            .expect("discovery must succeed");
        assert!(
            start
                .url
                .starts_with(&format!("{}/authorize", fixture.as_url)),
            "the authorize URL must point at the discovered AS: {}",
            start.url
        );

        let client_id = store
            .get_mcp_oauth_client(&fixture.as_url)
            .await
            .unwrap()
            .expect("begin_mcp_connect must have registered and persisted a client id");
        assert_eq!(
            client_id, "the-registered-client",
            "the persisted client id must be the one the DCR endpoint actually returned"
        );

        complete_mcp_connect(
            &store,
            &http,
            "rovo",
            &fixture.mcp_url,
            &format!("{}/token", fixture.as_url),
            &client_id,
            "the-code",
            &start.verifier,
        )
        .await
        .expect("the token exchange must succeed");

        let stored =
            store.get_mcp_oauth_token("rovo").await.unwrap().expect(
                "a token must be stored under the server's name after a successful exchange",
            );
        assert_eq!(stored.access_token, "issued-token");

        // PROPERTY: the RFC 8707 `resource` parameter must be on the actual
        // outgoing token-request body, not merely returned by
        // token_request_form() in isolation — this is the assertion that
        // would fail if complete_mcp_connect ever stopped threading
        // `resource` through to the real HTTP call.
        let bodies = fixture.token_request_bodies.lock().unwrap().clone();
        assert_eq!(
            bodies.len(),
            1,
            "expected exactly one token request: {bodies:?}"
        );
        assert!(
            bodies[0].contains("resource="),
            "the token exchange sent to the wire must carry the resource parameter: {}",
            bodies[0]
        );
        let decoded: std::collections::HashMap<_, _> =
            url::form_urlencoded::parse(bodies[0].as_bytes())
                .into_owned()
                .collect();
        assert_eq!(
            decoded.get("resource").map(String::as_str),
            Some(fixture.mcp_url.as_str()),
            "the resource value on the wire must be the MCP server's own canonical URI: {}",
            bodies[0]
        );
    }

    #[tokio::test]
    async fn a_second_connect_for_the_same_issuer_reuses_the_stored_client_id() {
        // PROPERTY: a stored client id for an issuer must be reused rather
        // than re-registered. Proven by counting hits on the registration
        // endpoint itself — a regression that re-registers on every connect
        // would drive this count to 2, not merely leave the stored value
        // looking unchanged (a same-string assertion alone could not catch
        // a DCR endpoint that happens to return the same client_id twice).
        let fixture = spawn_oauth_fixture().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        let http = reqwest::Client::new();
        let spec = mcp_spec(&fixture.mcp_url);

        begin_mcp_connect(&store, &http, &spec)
            .await
            .expect("first connect must succeed");
        assert_eq!(
            *fixture.registration_hits.lock().unwrap(),
            1,
            "the first connect for a new issuer must register exactly once"
        );

        begin_mcp_connect(&store, &http, &spec)
            .await
            .expect("second connect for the same issuer must also succeed");
        assert_eq!(
            *fixture.registration_hits.lock().unwrap(),
            1,
            "a second connect for the SAME issuer must reuse the stored client id — the \
             registration endpoint must not see a second hit"
        );
    }

    #[tokio::test]
    async fn a_401_without_www_authenticate_is_an_actionable_error_not_a_silent_noop() {
        // PROPERTY: a 401 that names no WWW-Authenticate header must fail
        // loudly, not resolve to some default/empty flow the caller could
        // mistake for "nothing to do here."
        let url = spawn_mcp_401_without_www_authenticate().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        let http = reqwest::Client::new();
        let spec = mcp_spec(&url);

        let err = begin_mcp_connect(&store, &http, &spec)
            .await
            .expect_err("a 401 with no WWW-Authenticate header gives nothing to discover from");

        let message = err.to_string();
        // Deliberately a specific phrase, not just "contains WWW-Authenticate":
        // the discovery-failed error a few lines further into
        // `begin_mcp_connect` also mentions "WWW-Authenticate" in passing, so
        // a weaker substring check here would still pass even if the
        // missing-header branch were bypassed and a different, unrelated
        // failure fired downstream instead.
        assert!(
            message.contains("without a WWW-Authenticate header"),
            "the error must specifically name a missing WWW-Authenticate header so a user has \
             something actionable to report, not merely fail for some other reason: {message}"
        );
    }

    #[tokio::test]
    async fn a_prm_naming_no_authorization_server_is_an_actionable_error_through_the_full_flow() {
        // PROPERTY: the same guarantee as
        // `a_prm_document_without_authorization_servers_is_an_error` above,
        // but exercised through begin_mcp_connect end to end rather than
        // calling authorization_servers_from_prm directly — this is what
        // actually catches a regression in how the two are wired together,
        // not just in the parsing function itself.
        let url = spawn_mcp_401_with_prm(json!({
            "resource": "placeholder",
            "authorization_servers": []
        }))
        .await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        let http = reqwest::Client::new();
        let spec = mcp_spec(&url);

        let err = begin_mcp_connect(&store, &http, &spec).await.expect_err(
            "a PRM document naming no authorization server must fail, not silently proceed",
        );

        assert!(
            err.to_string().contains("no authorization server"),
            "got: {err}"
        );
    }

    #[test]
    fn mcp_redirect_uri_is_scoped_per_server_on_the_shared_callback_port() {
        assert_eq!(
            mcp_redirect_uri("rovo"),
            "http://127.0.0.1:8976/mcp-oauth/rovo/callback"
        );
        assert_ne!(
            mcp_redirect_uri("rovo"),
            mcp_redirect_uri("other-server"),
            "two different servers must not collide on the same redirect_uri"
        );
    }
}
