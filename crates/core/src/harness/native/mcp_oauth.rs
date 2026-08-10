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
    if parsed.scheme().is_empty() || parsed.host_str().is_none() {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_ne!(
            q.get("code_challenge").map(String::as_str),
            Some(start.verifier.as_str()),
            "the challenge sent to the AS must be the S256 hash of the verifier, never the raw \
             verifier itself — sending the verifier would defeat PKCE entirely"
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
}
