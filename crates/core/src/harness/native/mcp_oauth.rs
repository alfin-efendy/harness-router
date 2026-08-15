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

/// The ceiling on ANY discovery or token response body this module reads.
///
/// The reads below are awaited inside an RPC handler and inside session start,
/// under a 120-second deadline, from URLs a remote server names — so an
/// unbounded read is a remote memory-exhaustion lever independent of the
/// timeout. RFC 8414/9728 documents and token responses are a few kilobytes;
/// 256 KiB is orders of magnitude of headroom.
const MAX_METADATA_BODY_BYTES: usize = 256 * 1024;

/// Read a JSON response body, aborting as soon as it exceeds
/// [`MAX_METADATA_BODY_BYTES`]. Streamed rather than buffered-then-measured:
/// measuring after the fact would already have allocated whatever arrived.
async fn read_json_capped(response: reqwest::Response, what: &str) -> anyhow::Result<Value> {
    use futures::StreamExt;

    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if buf.len() + chunk.len() > MAX_METADATA_BODY_BYTES {
            anyhow::bail!("{what} exceeded the {MAX_METADATA_BODY_BYTES}-byte limit");
        }
        buf.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&buf).map_err(|e| anyhow::anyhow!("{what} is not valid JSON: {e}"))
}

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

/// Validate the `resource_metadata` URL a remote MCP server named in its 401
/// BEFORE any request is made to it.
///
/// The URL arrives in a `WWW-Authenticate` header written by the remote server,
/// so it is attacker-controlled input in the case that matters: a compromised
/// MCP server naming a plaintext URL, or an internal host only this machine can
/// reach, would otherwise steer the whole discovery chain and turn the daemon
/// into a probe inside the user's network. Three rules, all from RFC 9728 §3:
///
/// 1. `https://` — subject to the same test-build carve-out
///    [`super::mcp_http::plaintext_allowed`] applies to the transport itself,
///    because every MCP fixture in this crate can only bind plaintext loopback.
/// 2. Same origin (scheme, host AND port) as the configured MCP server URL. A
///    server may only describe its own protected-resource metadata.
/// 3. The path carries the `/.well-known/oauth-protected-resource` segment,
///    which RFC 9728 mandates — matched with `contains` so both the
///    path-inserted form (`/.well-known/oauth-protected-resource/mcp`) and the
///    path-suffixed form (`/mcp/.well-known/oauth-protected-resource`) pass.
pub fn validate_metadata_url(server_url: &str, metadata_url: &str) -> anyhow::Result<()> {
    let server = url::Url::parse(server_url)
        .map_err(|e| anyhow::anyhow!("invalid MCP server URL {server_url}: {e}"))?;
    let metadata = url::Url::parse(metadata_url).map_err(|e| {
        anyhow::anyhow!("the MCP server named an unparseable metadata URL {metadata_url}: {e}")
    })?;
    if metadata.scheme() != "https" && !super::mcp_http::plaintext_allowed() {
        anyhow::bail!(
            "the MCP server named a non-https protected-resource metadata URL ({metadata_url}) — \
             refusing to fetch it"
        );
    }
    // The `Origin` VALUES, not their serializations: a non-`http(s)` scheme
    // produces an opaque origin, which is never equal to any other origin —
    // whereas every opaque origin serializes to the same string `"null"`, so
    // comparing strings would be a bypass.
    if metadata.origin() != server.origin() {
        anyhow::bail!(
            "the MCP server named a protected-resource metadata URL on a DIFFERENT origin \
             ({}, expected {}) — refusing to fetch it",
            metadata.origin().ascii_serialization(),
            server.origin().ascii_serialization()
        );
    }
    if !metadata
        .path()
        .contains("/.well-known/oauth-protected-resource")
    {
        anyhow::bail!(
            "the MCP server named a metadata URL that is not an RFC 9728 well-known path \
             ({metadata_url}) — refusing to fetch it"
        );
    }
    Ok(())
}

/// Validate an `authorization_servers` entry before RFC 8414 discovery fetches
/// it. The entry comes from a document the remote MCP server controls, so the
/// same SSRF reasoning as [`validate_metadata_url`] applies — but an
/// authorization server is legitimately a DIFFERENT origin from the MCP server
/// (a real deployment never colocates them), so origin cannot be the rule here.
/// What RFC 8414 §2 does require of an issuer identifier is: an `https` URL with
/// a host, and no query or fragment component.
pub fn validate_issuer_url(issuer: &str) -> anyhow::Result<()> {
    let parsed = url::Url::parse(issuer)
        .map_err(|e| anyhow::anyhow!("unparseable authorization server URL {issuer}: {e}"))?;
    if parsed.scheme() != "https" && !super::mcp_http::plaintext_allowed() {
        anyhow::bail!("authorization server {issuer} is not https");
    }
    if parsed.host_str().is_none() {
        anyhow::bail!("authorization server {issuer} has no host");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        anyhow::bail!(
            "authorization server {issuer} has a query or fragment — RFC 8414 issuer identifiers \
             have neither"
        );
    }
    Ok(())
}

/// Parse a PRM document and verify it actually describes `server_url`, then
/// return its `authorization_servers` in document order.
///
/// The `resource` claim is not decoration: RFC 9728 §3.3 requires the client to
/// check it, and it is the only thing in the document tying it to the server
/// being connected. Without the check, a server could serve (or proxy) a
/// document describing some OTHER resource entirely and route the user's
/// authorization at it. Both sides go through [`canonical_resource_uri`] first
/// so a trailing slash or an upper-case host is not a spurious mismatch.
pub fn protected_resource_from_prm(doc: &Value, server_url: &str) -> anyhow::Result<Vec<String>> {
    let claimed = doc
        .get("resource")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "the MCP server's protected-resource metadata carries no `resource` claim, so \
                 nothing ties it to this server"
            )
        })?;
    let claimed_canonical = canonical_resource_uri(claimed).map_err(|e| {
        anyhow::anyhow!(
            "the protected-resource metadata's `resource` claim is not a valid URI: {e}"
        )
    })?;
    let expected = canonical_resource_uri(server_url)?;
    if claimed_canonical != expected {
        anyhow::bail!(
            "the protected-resource metadata describes {claimed_canonical}, not {expected} — \
             refusing to authorize against it"
        );
    }
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
///
/// The explicit per-request `timeout` is not redundant with the client's own:
/// `http` is supplied by the CALLER, and a caller that hands in a bare
/// `reqwest::Client::new()` has no deadline of any kind — this function is
/// awaited from `begin_mcp_connect` (inside an RPC handler) and from
/// `mcp_http`'s refresh-on-401 path (inside `start_harness_session`), both of
/// which wedge outright if it never returns. Same reasoning at every other
/// request in this module.
///
/// `metadata_url` is validated against `server_url` — the URL the user actually
/// configured — BEFORE the request is made, so a metadata URL a remote server
/// named on some other origin is never fetched at all. See
/// [`validate_metadata_url`].
pub async fn protected_resource_metadata(
    http: &reqwest::Client,
    server_url: &str,
    metadata_url: &str,
) -> anyhow::Result<Vec<String>> {
    // BEFORE the request, not after: the point is that this URL is never
    // fetched at all unless it belongs to the server being connected.
    validate_metadata_url(server_url, metadata_url)?;
    let response = http
        .get(metadata_url)
        .timeout(super::mcp_http::request_timeout())
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!(
            "protected-resource metadata fetch failed: HTTP {}",
            response.status()
        );
    }
    protected_resource_from_prm(
        &read_json_capped(response, "protected-resource metadata").await?,
        server_url,
    )
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
        // The list comes from a document the remote MCP server controls, so an
        // entry is validated before it is fetched — same reasoning as
        // `validate_metadata_url`, minus the origin rule, which cannot apply
        // here (a real authorization server is a different origin by design).
        // A rejected entry is RECORDED and SKIPPED, not raised: the documented
        // document-order fallback must survive one hostile or broken entry.
        if let Err(e) = validate_issuer_url(issuer) {
            tried.push(format!("{issuer}: {e}"));
            continue;
        }
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

/// A started PKCE authorization: the URL to open, the verifier and state the
/// callback must be matched against, and — critically — the token endpoint
/// and client id of the authorization server this flow actually selected.
///
/// The token endpoint and client id are carried here, rather than left for
/// the completion step to rediscover, precisely so nothing downstream is
/// ever tempted to re-run RFC 9728/8414 discovery to recover them. Between
/// authorize and completion, a second discovery run could resolve a
/// DIFFERENT authorization server than the one that actually issued the
/// code (for instance if a PRM document lists several and the first becomes
/// reachable or unreachable in between) — carrying these two values forward
/// is what makes that impossible.
#[derive(Debug, Clone)]
pub struct McpAuthorizeStart {
    pub url: String,
    pub verifier: String,
    pub state: String,
    /// The `token_endpoint` of the authorization server [`select_authorization_server`]
    /// chose for this flow. Thread this to [`complete_mcp_connect`] verbatim.
    pub issuer_token_endpoint: String,
    /// The client id registered (or reused) for that same authorization
    /// server. Thread this to [`complete_mcp_connect`] verbatim.
    pub client_id: String,
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
        issuer_token_endpoint: metadata.token_endpoint.clone(),
        client_id: client_id.to_string(),
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
///
/// `server_name` is a PATH SEGMENT, so it is percent-encoded rather than
/// interpolated: a server id containing `?` or `#` would otherwise reshape the
/// URL this client registers with the authorization server and then sends as
/// `redirect_uri` — `…/mcp-oauth/a#b/callback` is a fragment, and
/// `…/mcp-oauth/a?b/callback` is a query string, neither of which is the
/// callback route Cockpit actually serves. (Host and port are hardcoded, so
/// loopback-only holds either way; the shape of the path does not.)
pub fn mcp_redirect_uri(server_name: &str) -> String {
    let mut url = url::Url::parse("http://127.0.0.1:8976/mcp-oauth/")
        .expect("the base callback URL is a compile-time constant and always parses");
    url.path_segments_mut()
        .expect("an http:// URL always has a path that can be segmented")
        .pop_if_empty()
        .push(server_name)
        .push("callback");
    url.to_string()
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
        .timeout(super::mcp_http::request_timeout())
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

    let issuers = protected_resource_metadata(http, url, &metadata_url).await?;
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
            crate::plugins::oauth::register_oauth_client(http, registration, &redirect_uri).await?
        }
    };
    // Written on EVERY connect, reused client id included, because the row is
    // what `api::apps_api::require_registered_token_endpoint` consults to
    // decide whether the endpoint `complete_mcp_connect` is handed may be
    // POSTed to — and this is the only place the authorization server's own
    // RFC 8414 metadata is in hand to record it from. Writing it only on the
    // registration branch would leave every client registered before the
    // `token_endpoint` column existed permanently unable to complete a
    // connect; re-upserting here backfills such a row on the next attempt.
    store
        .upsert_mcp_oauth_client(&issuer, &client_id, &metadata.token_endpoint)
        .await?;
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
/// than rediscovered here: the caller already holds both, carried forward
/// verbatim from [`McpAuthorizeStart`] (the matching `begin_mcp_connect`
/// call's return value). Thread those two fields through unchanged — do NOT
/// try to reconstruct them some other way (e.g. re-running RFC 9728/8414
/// discovery "just for the issuer", or re-deriving the issuer from the store
/// some other route). There is no cheap, side-effect-free way to recover the
/// token endpoint without redoing discovery, and redoing discovery here
/// reopens exactly the hazard this function exists to avoid: between
/// authorize and completion, discovery could resolve a DIFFERENT
/// authorization server than the one that actually issued the code.
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
    let response = http
        .post(issuer_token_endpoint)
        .timeout(super::mcp_http::request_timeout())
        .form(&form)
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!(
            "token exchange failed for {server_name}: HTTP {}",
            response.status()
        );
    }
    let body = read_json_capped(response, "the token response").await?;
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
    fn a_metadata_url_on_another_origin_is_refused() {
        // PROPERTY: the URL comes out of a header the REMOTE server wrote. A
        // cross-origin value is the whole attack — it is how a compromised MCP
        // server points discovery at a host of its choosing.
        assert!(
            validate_metadata_url(
                "https://mcp.example.com/mcp",
                "https://elsewhere.example/.well-known/oauth-protected-resource"
            )
            .is_err(),
            "a metadata URL on a different HOST must be refused before it is fetched"
        );
        assert!(
            validate_metadata_url(
                "https://mcp.example.com/mcp",
                "https://mcp.example.com:8443/.well-known/oauth-protected-resource"
            )
            .is_err(),
            "a different PORT is a different origin too — same-host is not the rule"
        );
    }

    #[test]
    fn a_same_origin_well_known_metadata_url_is_accepted_in_both_rfc9728_forms() {
        validate_metadata_url(
            "https://mcp.example.com/mcp",
            "https://mcp.example.com/.well-known/oauth-protected-resource/mcp",
        )
        .expect("the RFC 9728 path-inserted form must be accepted");
        validate_metadata_url(
            "https://mcp.example.com/mcp",
            "https://mcp.example.com/mcp/.well-known/oauth-protected-resource",
        )
        .expect("the path-suffixed form must be accepted too");
        assert!(
            validate_metadata_url(
                "https://mcp.example.com/mcp",
                "https://mcp.example.com/anything-else"
            )
            .is_err(),
            "a same-origin URL that is not an RFC 9728 well-known path must still be refused"
        );
    }

    #[test]
    fn a_plaintext_metadata_url_is_refused_under_the_production_rule() {
        // The carve-out defaults to permissive in test builds because every MCP
        // fixture here binds plaintext loopback; this guard turns the PRODUCTION
        // rule on so it can actually be proven.
        let _https = crate::harness::native::mcp_http::enforce_https_in_this_test();
        assert!(
            validate_metadata_url(
                "http://mcp.example.com/mcp",
                "http://mcp.example.com/.well-known/oauth-protected-resource"
            )
            .is_err(),
            "same-origin is not enough — a plaintext discovery fetch must be refused outright"
        );
    }

    #[test]
    fn an_issuer_url_must_be_an_rfc8414_issuer_identifier() {
        let _https = crate::harness::native::mcp_http::enforce_https_in_this_test();
        validate_issuer_url("https://as.example/tenant-1").expect("a plain https issuer is valid");
        assert!(
            validate_issuer_url("http://as.example").is_err(),
            "a plaintext issuer must be refused"
        );
        assert!(
            validate_issuer_url("https://as.example?tenant=1").is_err(),
            "RFC 8414 issuer identifiers carry no query component"
        );
        assert!(
            validate_issuer_url("https://as.example#frag").is_err(),
            "RFC 8414 issuer identifiers carry no fragment component"
        );
        assert!(
            validate_issuer_url("not-a-url").is_err(),
            "an unparseable issuer must be an error, not silently attempted"
        );
    }

    #[test]
    fn prm_authorization_servers_are_returned_in_document_order() {
        let doc = serde_json::json!({
            "resource": "https://mcp.example.com",
            "authorization_servers": ["https://as-one.example", "https://as-two.example"]
        });
        assert_eq!(
            protected_resource_from_prm(&doc, "https://mcp.example.com").unwrap(),
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
            protected_resource_from_prm(&empty, "https://mcp.example.com").is_err(),
            "an empty authorization_servers array names no AS to authorize against — this must be an \
             actionable error, not Ok(vec![]) that a caller could mistake for 'no auth needed'"
        );
        assert!(
            protected_resource_from_prm(&serde_json::json!({}), "https://mcp.example.com").is_err(),
            "a PRM document missing every field must fail — here on the absent `resource` claim, \
             before the authorization_servers check is even reached"
        );
    }

    #[test]
    fn a_prm_document_describing_a_different_resource_is_refused() {
        // PROPERTY: the `resource` claim is the only thing in the document that
        // ties it to the server being connected. A document describing some
        // other resource must never be used to pick an authorization server for
        // THIS one, however well-formed the rest of it is.
        let doc = serde_json::json!({
            "resource": "https://someone-else.example",
            "authorization_servers": ["https://as-one.example"]
        });
        let err = protected_resource_from_prm(&doc, "https://mcp.example.com")
            .expect_err("a mismatched `resource` claim must be refused, not ignored");
        assert!(
            err.to_string().contains("someone-else.example"),
            "the error must name the resource the document actually claimed, so the failure is \
             diagnosable: {err}"
        );
    }

    #[test]
    fn a_prm_resource_claim_matches_across_trailing_slash_and_case() {
        // Canonicalization on both sides, or every deployment that writes its
        // own URL slightly differently would be refused for no security reason.
        let doc = serde_json::json!({
            "resource": "HTTPS://MCP.Example.COM/mcp/",
            "authorization_servers": ["https://as-one.example"]
        });
        assert!(
            protected_resource_from_prm(&doc, "https://mcp.example.com/mcp").is_ok(),
            "an upper-case host and a trailing slash are the SAME resource — both sides go \
             through canonical_resource_uri first"
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
        assert_eq!(
            start.issuer_token_endpoint, "https://as.example/token",
            "McpAuthorizeStart must carry the SELECTED authorization server's token endpoint \
             forward, so a caller never has to rediscover it (and risk landing on a different AS) \
             at completion time"
        );
        assert_eq!(
            start.client_id, "client-1",
            "McpAuthorizeStart must carry the client id this flow actually used forward too"
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
    /// JSON response body for a successful status. A `"resource"` of
    /// `"placeholder"` is substituted with the real bound URL at serve time —
    /// the bound port is not known when the caller builds the document, and the
    /// client now VERIFIES that claim.
    ///
    /// Returns the server's BASE URL: callers now need both it (as the
    /// configured server URL) and the well-known path built from it.
    async fn spawn_prm_server(status: axum::http::StatusCode, body: Option<Value>) -> String {
        use axum::routing::get;
        use axum::Router;

        let mcp_url_slot = Arc::new(Mutex::new(String::new()));
        let slot_for_route = mcp_url_slot.clone();
        let app = Router::new().route(
            "/.well-known/oauth-protected-resource",
            get(move || {
                let body = body.clone();
                let slot = slot_for_route.clone();
                async move {
                    let mut body = body.unwrap_or(Value::Null);
                    if body.get("resource").and_then(Value::as_str) == Some("placeholder") {
                        body["resource"] = Value::String(slot.lock().unwrap().clone());
                    }
                    (status, axum::Json(body))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        *mcp_url_slot.lock().unwrap() = base.clone();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        base
    }

    #[tokio::test]
    async fn protected_resource_metadata_names_the_status_on_a_non_2xx_response() {
        let base = spawn_prm_server(axum::http::StatusCode::NOT_FOUND, None).await;
        let metadata_url = format!("{base}/.well-known/oauth-protected-resource");
        let http = reqwest::Client::new();

        let err = protected_resource_metadata(&http, &base, &metadata_url)
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
        // `"placeholder"`, not a hard-coded URL: the fixture substitutes the real
        // bound URL, so the `resource` check passes and the empty
        // `authorization_servers` list is what this test actually gets to prove.
        let base = spawn_prm_server(
            axum::http::StatusCode::OK,
            Some(serde_json::json!({ "resource": "placeholder", "authorization_servers": [] })),
        )
        .await;
        let metadata_url = format!("{base}/.well-known/oauth-protected-resource");
        let http = reqwest::Client::new();

        let err = protected_resource_metadata(&http, &base, &metadata_url)
            .await
            .expect_err(
                "the MCP spec requires at least one authorization server; a 2xx body naming none \
                 must be an actionable error, not a silent empty Vec the caller could mistake for \
                 success",
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

    #[tokio::test]
    async fn a_malformed_issuer_is_skipped_and_a_later_valid_one_still_wins() {
        // PROPERTY: a bad entry is skipped, not fatal — document-order fallback
        // is the documented behaviour and one hostile or broken entry must not
        // be able to deny service to a legitimate later one.
        let working =
            spawn_as_metadata_server(Some(as_metadata_body("https://good-as.example"))).await;
        let http = reqwest::Client::new();

        let (issuer, _metadata) =
            select_authorization_server(&http, &["not-a-url".to_string(), working.clone()])
                .await
                .expect("the second candidate is valid, so selection must still succeed");

        assert_eq!(
            issuer, working,
            "the unparseable first entry must be skipped, not returned and not fatal"
        );
    }

    #[tokio::test]
    async fn an_oversized_metadata_body_is_refused_rather_than_buffered() {
        // PROPERTY: the ceiling is on the BODY, independent of the timeout — a
        // server that streams a valid, fast, enormous document must be cut off.
        let base = spawn_prm_server(
            axum::http::StatusCode::OK,
            Some(serde_json::json!({
                "resource": "placeholder",
                "authorization_servers": ["https://as.example"],
                "padding": "x".repeat(MAX_METADATA_BODY_BYTES + 1),
            })),
        )
        .await;
        let metadata_url = format!("{base}/.well-known/oauth-protected-resource");
        let http = reqwest::Client::new();

        let err = protected_resource_metadata(&http, &base, &metadata_url)
            .await
            .expect_err("a body past the ceiling must be an error, not a large allocation");
        assert!(
            err.to_string().contains("exceeded"),
            "the failure must be the size ceiling, not an incidental parse error: {err}"
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
            let mut body = state.prm_body.clone();
            // The bound address is unknown when the caller builds the document,
            // hence the sentinel — and the client now VERIFIES this claim, so it
            // has to be the real URL by the time it is served.
            if body.get("resource").and_then(Value::as_str) == Some("placeholder") {
                let mcp_url = state.mcp_url.lock().unwrap().clone();
                body["resource"] = Value::String(mcp_url);
            }
            axum::Json(body)
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

        // PROPERTY: `McpAuthorizeStart` must carry the same token endpoint and
        // client id a caller would otherwise have to re-derive by hand (from
        // the store, or by redoing discovery) — the whole point of carrying
        // them is that a caller can use `start.issuer_token_endpoint`/
        // `start.client_id` directly rather than reaching for either.
        assert_eq!(
            start.issuer_token_endpoint,
            format!("{}/token", fixture.as_url)
        );
        assert_eq!(start.client_id, client_id);

        complete_mcp_connect(
            &store,
            &http,
            "rovo",
            &fixture.mcp_url,
            &start.issuer_token_endpoint,
            &start.client_id,
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
        // calling protected_resource_from_prm directly — this is what
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

    /// Bind an MCP server that 401s with a `WWW-Authenticate` header pointing at
    /// a DIFFERENT origin's protected-resource metadata, plus that other origin
    /// as a separate server which records whether it was ever hit.
    ///
    /// Two servers, and a hit counter rather than only an error assertion: the
    /// guarantee under test is that the daemon never MAKES the request, not
    /// merely that it reports a failure afterwards.
    async fn spawn_mcp_401_pointing_at_another_origin() -> (String, Arc<Mutex<usize>>) {
        use axum::extract::State;
        use axum::response::IntoResponse;
        use axum::routing::{get, post};
        use axum::Router;

        let other_hits = Arc::new(Mutex::new(0usize));
        let hits_for_route = other_hits.clone();
        let other_app = Router::new().route(
            "/.well-known/oauth-protected-resource",
            get(move || {
                let hits = hits_for_route.clone();
                async move {
                    *hits.lock().unwrap() += 1;
                    axum::Json(json!({
                        "resource": "https://whatever.example",
                        "authorization_servers": ["https://as.example"],
                    }))
                }
            }),
        );
        let other_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other_addr = other_listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(other_listener, other_app).await.unwrap();
        });
        let other_url = format!("http://{other_addr}");

        async fn handle_probe(State(other_url): State<String>) -> axum::response::Response {
            let www_auth = format!(
                "Bearer resource_metadata=\"{other_url}/.well-known/oauth-protected-resource\""
            );
            axum::response::Response::builder()
                .status(axum::http::StatusCode::UNAUTHORIZED)
                .header(axum::http::header::WWW_AUTHENTICATE, www_auth)
                .body(axum::body::Body::empty())
                .unwrap()
                .into_response()
        }

        let app = Router::new()
            .route("/", post(handle_probe))
            .with_state(other_url);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), other_hits)
    }

    #[tokio::test]
    async fn a_metadata_url_on_another_origin_is_never_even_requested() {
        let (mcp_url, other_hits) = spawn_mcp_401_pointing_at_another_origin().await;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        let http = reqwest::Client::new();
        let spec = mcp_spec(&mcp_url);

        let err = begin_mcp_connect(&store, &http, &spec)
            .await
            .expect_err("a cross-origin metadata URL must fail the connect");

        assert!(
            err.to_string().contains("DIFFERENT origin"),
            "the refusal must be the origin check, not some incidental downstream failure: {err}"
        );
        assert_eq!(
            *other_hits.lock().unwrap(),
            0,
            "the named URL must never be REQUESTED — an error raised only after the fetch would \
             still have made the daemon probe a host of the remote server's choosing"
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

    /// PROPERTY: the server id lands in ONE path segment and cannot reshape
    /// the URL. Interpolated with `format!`, an id containing `?` or `#` turns
    /// the rest of the template into a query string or a fragment — so the
    /// `redirect_uri` this client REGISTERS with the authorization server, and
    /// then sends on both the authorize and token requests, stops being the
    /// callback route Cockpit actually serves.
    #[test]
    fn mcp_redirect_uri_percent_encodes_a_server_id_that_would_otherwise_reshape_the_url() {
        let uri = mcp_redirect_uri("weird?q=1#frag");
        let parsed = url::Url::parse(&uri).expect("the redirect_uri must remain a valid URL");

        assert_eq!(
            parsed.query(),
            None,
            "a `?` in the server id must not open a query string — everything after it would \
             otherwise stop being part of the path: {uri}"
        );
        assert_eq!(
            parsed.fragment(),
            None,
            "a `#` in the server id must not open a fragment — a fragment is never even sent to \
             the server, so `/callback` would vanish from the request entirely: {uri}"
        );
        let segments: Vec<&str> = parsed
            .path_segments()
            .expect("a loopback http URL always has path segments")
            .collect();
        assert_eq!(
            segments.len(),
            3,
            "exactly three segments — the scope prefix, the encoded id, and the callback: \
             {segments:?}"
        );
        assert_eq!(segments[0], "mcp-oauth");
        assert_eq!(
            segments[2], "callback",
            "the `/callback` suffix must survive whatever the id contains: {uri}"
        );
        assert!(
            !segments[1].contains('?') && !segments[1].contains('#'),
            "the id must be percent-encoded into its single segment, not left raw: {}",
            segments[1]
        );
        assert_eq!(
            parsed.host_str(),
            Some("127.0.0.1"),
            "loopback-only must still hold — host and port are hardcoded"
        );
        assert_eq!(parsed.port(), Some(8976));
    }

    /// PROPERTY: every request in this module is bounded even when the CALLER
    /// hands in a client that has no deadline of its own.
    ///
    /// That is not hypothetical: `api/apps_api.rs` builds a bare
    /// `reqwest::Client::new()` for both `begin_mcp_connect` and
    /// `complete_mcp_connect`. The client used here is deliberately just as
    /// bare, so the per-REQUEST timeout is the only thing under test — and
    /// the outer `tokio::time::timeout` is what turns an unbounded body read
    /// into a test FAILURE instead of a hung test run.
    #[tokio::test]
    async fn protected_resource_metadata_is_bounded_even_with_a_timeout_less_caller_client() {
        let _timeout = crate::harness::native::mcp_http::override_request_timeout(
            std::time::Duration::from_millis(300),
        );
        // That fixture answers ANY request path, and returns its BASE URL — so
        // it doubles as the configured server URL here, with the RFC 9728
        // well-known path appended for the metadata URL. Passing the base as
        // the metadata URL would now be refused by the well-known-path rule
        // BEFORE the fetch, and this test would pass on the wrong error.
        let base = crate::harness::native::mcp_http::tests::spawn_hanging_body_server().await;
        let metadata_url = format!("{base}/.well-known/oauth-protected-resource");
        let http = reqwest::Client::new();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            protected_resource_metadata(&http, &base, &metadata_url),
        )
        .await
        .expect(
            "the metadata fetch must return on its own — this call is awaited inside the \
             begin_mcp_connect RPC handler and inside mcp_http's refresh-on-401 path, both of \
             which wedge indefinitely if it never does",
        );

        let err = outcome.expect_err("a response whose body never arrives is not a valid document");
        assert!(
            format!("{err:#}")
                .to_ascii_lowercase()
                .contains("timed out"),
            "the failure must be the timeout, not some other incidental error: {err:#}"
        );
    }
}
