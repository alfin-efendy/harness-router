//! Pure, host-free MiMo wire-protocol logic — ported from `ryuzi-core`'s
//! `llm_router::mimo`. Every function here is deterministic over its inputs
//! (no network, no clock, no storage), so the whole module is covered by
//! native `cargo test`. The wasm `guest` glue supplies the live effects
//! (HTTP, storage, the current time) and maps these plain types to WIT.
//!
//! MiMo's free chat endpoint is OpenAI-compatible (`/openai/chat`), so its
//! request/response mapping is byte-for-byte what `ryuzi_openai_format`
//! already implements and tests — [`build_chat_body`] and
//! [`parse_chat_response`] are thin delegations to [`CONFIG`] rather than a
//! second implementation of that mapping. What stays genuinely local: the
//! bootstrap/JWT dance and its expiry handling, the device-id/session-affinity
//! derivation, the anti-abuse gate headers ([`CHROME_UA`]/[`X_MIMO_SOURCE`]),
//! and [`classify_chat_error`] (MiMo's transient-block protocol has no
//! counterpart in the shared crate's generic `classify_error`).
//!
//! # The gate marker (`SYSTEM_MARKER`)
//! The free chat endpoint 403s any request whose FIRST message is not this
//! exact MiMoCode signature string. `ryuzi_openai_format::ChatRequest` carries
//! a `leading_system` field specifically for this: the `guest` glue always
//! passes `Some(SYSTEM_MARKER)`, which the shared builder prepends as
//! `messages[0]` ahead of the real transcript. See
//! `tests::the_gate_marker_stays_messages_zero_ahead_of_the_transcript`, which
//! pins this ordering — a regression here would silently 403 every request in
//! production.

use ryuzi_openai_format::{OpenAiFormat, DEFAULT_CONTEXT_WINDOW};
use serde_json::{Map, Value};

pub use ryuzi_openai_format::{
    BlockIn, ChatRequest, ChunkOut, MessageIn, ProviderFail, RoleIn, StopOut, ToolCallOut,
    ToolChoiceIn, ToolIn, UsageOut,
};

use base64::Engine as _;
use sha2::{Digest, Sha256};

/// Bootstrap endpoint that mints the free-tier JWT (keyed by a device
/// fingerprint). Same host as the chat endpoint — both covered by the
/// manifest's `api.xiaomimimo.com` allowlist entry.
pub const BOOTSTRAP_URL: &str = "https://api.xiaomimimo.com/api/free-ai/bootstrap";

/// The free chat endpoint (OpenAI-compatible).
pub const CHAT_URL: &str = "https://api.xiaomimimo.com/api/free-ai/openai/chat";

/// Anti-abuse gate marker: the free chat endpoint requires a system message
/// containing this exact MiMoCode signature substring (verbatim from
/// `llm_router::mimo::SYSTEM_MARKER`).
pub const SYSTEM_MARKER: &str =
    "You are MiMoCode, an interactive CLI tool that helps users with software engineering tasks.";

/// The gate rejects non-browser user agents with 403 "Illegal access"
/// (verbatim from `llm_router::mimo::CHROME_UA`).
pub const CHROME_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Source tag the MiMoCode free CLI sends.
pub const X_MIMO_SOURCE: &str = "mimocode-cli-free";

/// The only (always-valid) MiMo free model id.
pub const MODEL_ID: &str = "mimo-auto";

/// Lifetime assumed for a JWT whose `exp` claim can't be parsed (matches
/// `llm_router::mimo::JWT_FALLBACK_TTL_MS`).
pub const JWT_FALLBACK_TTL_MS: i64 = 3_000 * 1_000;

/// A token this close to expiry is re-minted proactively (matches
/// `llm_router::mimo::JWT_EXPIRY_BUFFER_MS`).
pub const JWT_EXPIRY_BUFFER_MS: i64 = 300 * 1_000;

/// One model the provider advertises (host-free mirror of the WIT `model-info`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOut {
    pub id: String,
    pub display_name: String,
    pub context_window: u32,
}

/// This provider's OpenAI-format wire configuration — everything
/// [`build_chat_body`] and [`parse_chat_response`] delegate to.
///
/// `models_path`/`chat_path`/`default_base_url` are unused in practice:
/// [`CHAT_URL`] is a fixed constant (no base-URL override, no `/models`
/// fetch — see [`models`]), so nothing here ever calls
/// `CONFIG.chat_url()`/`CONFIG.models_url()`. `context_windows` is empty and
/// `default_context_window` mirrors [`models`]'s single static entry — kept
/// populated purely so `CONFIG` stays a complete, honest `OpenAiFormat` value.
const CONFIG: OpenAiFormat = OpenAiFormat {
    provider_label: "MiMo",
    default_base_url: "https://api.xiaomimimo.com/api/free-ai",
    models_path: "/openai/models",
    chat_path: "/openai/chat",
    max_tokens_field: "max_tokens",
    context_windows: &[],
    default_context_window: DEFAULT_CONTEXT_WINDOW,
};

/// The static model list. MiMo's free host has no `/models` endpoint (see
/// `llm_router::registry`'s `has_models_endpoint: false`), so `list-models`
/// returns this seed without any network call.
pub fn models() -> Vec<ModelOut> {
    vec![ModelOut {
        id: MODEL_ID.to_string(),
        display_name: "MiMo (free)".to_string(),
        context_window: 128_000,
    }]
}

/// Lowercase-hex SHA-256 of an opaque per-install seed — the device
/// fingerprint the bootstrap gate keys its rate limits on. The server only
/// needs a stable, unique-per-install hex id; the guest generates the seed once
/// and persists the result in storage.
pub fn fingerprint_from_seed(seed: &[u8]) -> String {
    hex_lower(&Sha256::digest(seed))
}

/// Per-install session-affinity id: `ses_` + 24 lowercase hex chars derived
/// from an opaque seed (mirrors `llm_router::mimo::session_affinity`'s shape).
pub fn session_affinity_from_seed(seed: &[u8]) -> String {
    let digest = hex_lower(&Sha256::digest(seed));
    format!("ses_{}", &digest[..24])
}

/// The bootstrap request body: `{"client": <fingerprint>}`.
pub fn bootstrap_body(fingerprint: &str) -> Vec<u8> {
    let mut obj = Map::new();
    obj.insert("client".to_string(), Value::String(fingerprint.to_string()));
    serde_json::to_vec(&Value::Object(obj)).expect("bootstrap body always serializes")
}

/// Extract the non-empty `jwt` string from a bootstrap response body.
pub fn parse_bootstrap_jwt(body: &[u8]) -> Result<String, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|e| format!("bootstrap response is not JSON: {e}"))?;
    value
        .get("jwt")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "bootstrap response carried no JWT".to_string())
}

/// Expiry of a JWT in ms, from its `exp` claim (seconds); a token whose middle
/// segment doesn't parse gets the fallback TTL from `now_ms`.
pub fn jwt_exp_ms(jwt: &str, now_ms: i64) -> i64 {
    let claim = jwt.split('.').nth(1).and_then(|payload| {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .ok()?;
        serde_json::from_slice::<Value>(&bytes)
            .ok()?
            .get("exp")?
            .as_i64()
    });
    match claim {
        Some(exp_s) => exp_s * 1_000,
        None => now_ms + JWT_FALLBACK_TTL_MS,
    }
}

/// Whether a cached JWT with expiry `exp_ms` is still usable at `now_ms` (i.e.
/// not within the proactive re-mint buffer).
pub fn jwt_is_fresh(exp_ms: i64, now_ms: i64) -> bool {
    now_ms < exp_ms - JWT_EXPIRY_BUFFER_MS
}

/// Build the OpenAI-format chat request body for a full transcript. A thin
/// delegation to [`CONFIG`] — MiMo's `/openai/chat` shape is exactly
/// `ryuzi_openai_format::OpenAiFormat::build_chat_body`'s, so the mapping is
/// never reimplemented here. The `guest` glue is responsible for always
/// passing `leading_system: Some(SYSTEM_MARKER)` — the gate 403s any request
/// whose first message is not that marker, and this function does not enforce
/// it; see the module docs and
/// `tests::the_gate_marker_stays_messages_zero_ahead_of_the_transcript`.
pub fn build_chat_body(request: ChatRequest<'_>) -> Vec<u8> {
    CONFIG.build_chat_body(request)
}

/// The gate headers for a chat request, including the bootstrap bearer. The
/// host forwards this `authorization` header only because this is a VERIFIED
/// first-party bundle (see `capabilities::http` self-auth); it strips
/// `host`/`content-length` regardless.
pub fn chat_headers(jwt: &str, session_affinity: &str) -> Vec<(String, String)> {
    vec![
        ("user-agent".to_string(), CHROME_UA.to_string()),
        ("x-mimo-source".to_string(), X_MIMO_SOURCE.to_string()),
        (
            "x-session-affinity".to_string(),
            session_affinity.to_string(),
        ),
        ("accept".to_string(), "application/json".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        ("authorization".to_string(), format!("Bearer {jwt}")),
    ]
}

/// Classify a MiMo non-2xx error body. The free tier signals a transient abuse
/// throttle as `{"error":{"code":"441","type":"risk_control"}}` and refuses
/// ungated requests with `illegal_access` — both transient/environmental, never
/// a permanent "bad model" verdict. Returns a user-facing message for a known
/// transient block (verbatim behaviour from `llm_router::mimo`).
pub fn transient_block_message(model: &str, body: &str) -> Option<String> {
    let b = body.to_ascii_lowercase();
    if b.contains("risk_control") || b.contains("\"441\"") {
        Some(format!(
            "Model {model} is temporarily rate-limited by MiMo (risk control) — try again in a few minutes."
        ))
    } else if b.contains("illegal_access") {
        Some(format!(
            "Model {model} was blocked by MiMo's access gate — try again shortly."
        ))
    } else {
        None
    }
}

/// Map a non-2xx chat response to a [`ProviderFail`]: a transient block becomes
/// a soft rate-limit/unavailable (never `model-not-found`); anything else is a
/// generic failure carrying the status. Kept LOCAL rather than delegated to
/// `ryuzi_openai_format::OpenAiFormat::classify_error`: MiMo's transient-block
/// protocol (`risk_control`/`illegal_access`) has no counterpart in the shared
/// classifier, and MiMo must never emit `ModelNotFound` for the always-valid
/// `mimo-auto` even on a permanent-looking 4xx.
///
/// # Task C5 Step 1 probe result (2026-08-09)
/// Live-probed via the throwaway `plugins/mimo/tests/probe_tools_gate.rs`
/// (bootstrap succeeded, HTTP 200) — BOTH a toolless baseline request and a
/// one-tool request against `CHAT_URL` came back identically:
/// `HTTP 400 {"error":{"code":"400","message":"Unsupported model mimo-auto"}}`.
/// Because the toolless baseline fails the exact same way, this is not a
/// tools rejection: the entire `mimo-auto` free channel is gone. Upstream's
/// own `XiaomiMiMo/MiMo-Code` (`packages/opencode/src/util/free-api-sunset.ts`,
/// commit as of 2026-08-07) sets `FREE_API_SUNSET_AT =
/// 2026-07-26T10:00:00.000Z`, already past at probe time. There is therefore
/// no live evidence that the gate accepts a `tools` array, so
/// `capabilities()` in `guest.rs` reports `tools: false` rather than assuming
/// it from the thirteen already-migrated OpenAI-format siblings.
pub fn classify_chat_error(status: u16, body: &[u8]) -> ProviderFail {
    let text = String::from_utf8_lossy(body);
    if transient_block_message(MODEL_ID, &text).is_some() {
        if text.to_ascii_lowercase().contains("illegal_access") {
            return ProviderFail::Unavailable;
        }
        return ProviderFail::RateLimited;
    }
    ProviderFail::Failed(format!("MiMo chat failed: HTTP {status}"))
}

/// Convert a buffered (non-stream) OpenAI chat completion into ordered
/// completion chunks. A thin delegation to [`CONFIG`] — same reasoning as
/// [`build_chat_body`]: the response shape is exactly what
/// `ryuzi_openai_format::OpenAiFormat::parse_chat_response` already parses and
/// tests.
pub fn parse_chat_response(body: &[u8]) -> Result<Vec<ChunkOut>, ProviderFail> {
    CONFIG.parse_chat_response(body)
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A JWT whose middle segment carries a controlled `exp` (seconds).
    fn tjwt(exp_ms: i64) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&json!({ "exp": exp_ms / 1000 })).unwrap());
        format!("h.{payload}.sig")
    }

    #[test]
    fn models_returns_the_single_static_mimo_model() {
        let models = models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "mimo-auto");
        assert!(models[0].context_window > 0);
    }

    #[test]
    fn fingerprint_is_a_stable_64_char_lowercase_hex() {
        let a = fingerprint_from_seed(b"seed-one");
        assert_eq!(a.len(), 64);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(a, fingerprint_from_seed(b"seed-one"), "same seed is stable");
        assert_ne!(
            a,
            fingerprint_from_seed(b"seed-two"),
            "distinct seeds differ"
        );
    }

    #[test]
    fn session_affinity_is_ses_prefixed_with_24_hex_chars() {
        let id = session_affinity_from_seed(b"install-x");
        assert!(id.starts_with("ses_"));
        assert_eq!(id.len(), "ses_".len() + 24);
        assert!(id["ses_".len()..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            id,
            session_affinity_from_seed(b"install-x"),
            "stable per seed"
        );
    }

    #[test]
    fn bootstrap_body_carries_the_fingerprint_as_client() {
        let fp = fingerprint_from_seed(b"abc");
        let body: Value = serde_json::from_slice(&bootstrap_body(&fp)).unwrap();
        assert_eq!(body["client"].as_str().unwrap(), fp);
    }

    #[test]
    fn parse_bootstrap_jwt_extracts_a_non_empty_token() {
        let ok = parse_bootstrap_jwt(br#"{"jwt":"abc.def.ghi"}"#).unwrap();
        assert_eq!(ok, "abc.def.ghi");
        assert!(
            parse_bootstrap_jwt(br#"{"jwt":""}"#).is_err(),
            "empty jwt rejected"
        );
        assert!(
            parse_bootstrap_jwt(br#"{"nope":1}"#).is_err(),
            "missing jwt rejected"
        );
        assert!(parse_bootstrap_jwt(b"not json").is_err());
    }

    #[test]
    fn jwt_expiry_comes_from_the_exp_claim_with_a_ttl_fallback() {
        assert_eq!(jwt_exp_ms(&tjwt(1_900_000_000_000), 0), 1_900_000_000_000);
        let now = 1_000_000;
        let fallback = jwt_exp_ms("not-a-jwt", now);
        assert_eq!(fallback, now + JWT_FALLBACK_TTL_MS);
    }

    #[test]
    fn jwt_freshness_respects_the_expiry_buffer() {
        let now = 1_000_000_000;
        // Expiry ten minutes out is fresh...
        assert!(jwt_is_fresh(now + 10 * 60 * 1000, now));
        // ...but ten seconds inside the 5-minute buffer is treated as stale.
        assert!(!jwt_is_fresh(now + JWT_EXPIRY_BUFFER_MS - 10_000, now));
    }

    fn chat_request<'a>(messages: &'a [MessageIn]) -> ChatRequest<'a> {
        ChatRequest {
            model: MODEL_ID,
            messages,
            tools: &[],
            tool_choice: ToolChoiceIn::Auto,
            max_tokens: None,
            temperature: None,
            leading_system: Some(SYSTEM_MARKER),
        }
    }

    #[test]
    fn chat_body_injects_the_marker_system_message_and_the_prompt() {
        let messages = vec![MessageIn {
            role: RoleIn::User,
            content: vec![BlockIn::Text("ping".into())],
        }];
        let mut request = chat_request(&messages);
        request.max_tokens = Some(64);
        request.temperature = Some(0.2);
        let body: Value = serde_json::from_slice(&build_chat_body(request)).unwrap();
        assert_eq!(body["model"], "mimo-auto");
        assert_eq!(body["stream"], false);
        assert_eq!(body["max_tokens"], 64);
        let out = body["messages"].as_array().unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[0]["content"], SYSTEM_MARKER);
        assert_eq!(out[1]["role"], "user");
        assert_eq!(out[1]["content"], "ping");
    }

    #[test]
    fn chat_body_omits_optional_fields_when_absent() {
        let messages = vec![MessageIn {
            role: RoleIn::User,
            content: vec![BlockIn::Text("hi".into())],
        }];
        let body: Value =
            serde_json::from_slice(&build_chat_body(chat_request(&messages))).unwrap();
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn chat_headers_carry_the_gate_headers_and_bearer() {
        let headers = chat_headers("the-jwt", "ses_0123456789abcdef01234567");
        let get = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("user-agent"), Some(CHROME_UA));
        assert_eq!(get("x-mimo-source"), Some("mimocode-cli-free"));
        assert_eq!(
            get("x-session-affinity"),
            Some("ses_0123456789abcdef01234567")
        );
        assert_eq!(get("authorization"), Some("Bearer the-jwt"));
    }

    #[test]
    fn transient_block_flags_risk_control_and_illegal_access_only() {
        let risk = r#"{"error":{"code":"441","message":"...","type":"risk_control"}}"#;
        assert!(transient_block_message("mimo-auto", risk)
            .unwrap()
            .contains("rate-limited"));
        let illegal =
            r#"{"error":{"code":"403","message":"Illegal access","type":"illegal_access"}}"#;
        assert!(transient_block_message("mimo-auto", illegal)
            .unwrap()
            .contains("access gate"));
        // A genuine bad-request body is NOT reclassified as transient.
        let real = r#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#;
        assert!(transient_block_message("mimo-auto", real).is_none());
        assert!(transient_block_message("mimo-auto", "").is_none());
    }

    #[test]
    fn classify_chat_error_maps_transient_blocks_to_soft_errors() {
        let risk = br#"{"error":{"code":"441","type":"risk_control"}}"#;
        assert_eq!(classify_chat_error(400, risk), ProviderFail::RateLimited);
        let illegal = br#"{"error":{"type":"illegal_access"}}"#;
        assert_eq!(classify_chat_error(403, illegal), ProviderFail::Unavailable);
        // A non-transient failure is a generic Failed, never ModelNotFound.
        match classify_chat_error(500, b"boom") {
            ProviderFail::Failed(msg) => assert!(msg.contains("500")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_response_yields_one_finished_chunk_with_usage() {
        let body = br#"{
            "choices": [{"message": {"role": "assistant", "content": "Hello, world!"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3}
        }"#;
        let chunks = parse_chat_response(body).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hello, world!");
        assert!(chunks[0].finished);
        assert_eq!(
            chunks[0].usage,
            Some(UsageOut {
                input: 7,
                output: 3
            })
        );
    }

    #[test]
    fn parse_chat_response_without_usage_still_succeeds() {
        let body = br#"{"choices":[{"message":{"content":"hi"}}]}"#;
        let chunks = parse_chat_response(body).unwrap();
        assert_eq!(chunks[0].text, "hi");
        assert_eq!(chunks[0].usage, None);
    }

    #[test]
    fn parse_chat_response_rejects_a_body_without_content() {
        assert!(matches!(
            parse_chat_response(br#"{"choices":[]}"#),
            Err(ProviderFail::Failed(_))
        ));
        assert!(matches!(
            parse_chat_response(b"not json"),
            Err(ProviderFail::Failed(_))
        ));
    }

    #[test]
    fn the_gate_marker_stays_messages_zero_ahead_of_the_transcript() {
        use ryuzi_openai_format::{BlockIn, ChatRequest, MessageIn, RoleIn, ToolChoiceIn};

        let messages = vec![
            MessageIn {
                role: RoleIn::System,
                content: vec![BlockIn::Text("agent sys".into())],
            },
            MessageIn {
                role: RoleIn::User,
                content: vec![BlockIn::Text("hi".into())],
            },
        ];
        let body: Value = serde_json::from_slice(&build_chat_body(ChatRequest {
            model: MODEL_ID,
            messages: &messages,
            tools: &[],
            tool_choice: ToolChoiceIn::Auto,
            max_tokens: None,
            temperature: None,
            leading_system: Some(SYSTEM_MARKER),
        }))
        .unwrap();

        let out = body["messages"].as_array().unwrap();
        assert_eq!(
            out[0]["content"], SYSTEM_MARKER,
            "the gate rejects any request whose first message is not the marker"
        );
        assert_eq!(out[1]["content"], "agent sys");
        assert_eq!(out[2]["role"], "user");
    }
}
