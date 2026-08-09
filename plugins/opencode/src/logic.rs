//! Pure, host-free OpenCode Zen wire logic — ported from `ryuzi-core`'s
//! `llm_router` `opencode-free` descriptor + request-header assembly. Every
//! function is deterministic over its inputs, so the module is fully covered by
//! native `cargo test`; the wasm `guest` glue supplies HTTP and maps these
//! plain types to WIT.
//!
//! OpenCode Zen is an OpenAI-compatible `/chat/completions` endpoint, so its
//! request/response mapping is byte-for-byte what `ryuzi_openai_format`
//! already implements and tests — [`build_chat_body`] and
//! [`parse_chat_response`] are thin delegations to [`CONFIG`] rather than a
//! second implementation of that mapping. What stays genuinely local:
//! [`BEARER`]/[`X_OPENCODE_CLIENT`]/[`request_headers`] (the static free-tier
//! credential and client tag — this component owns its own egress, unlike the
//! host-injected-credential components built on
//! `ryuzi_openai_format::provider_component!`), [`parse_models`] (OpenCode's
//! `/models` carries a per-entry `context_length` the generic
//! `OpenAiFormat::parse_models` does not read), and [`classify_chat_error`]
//! (see its doc comment for why it is kept distinct from the shared
//! `classify_error`).

use ryuzi_openai_format::{OpenAiFormat, DEFAULT_CONTEXT_WINDOW};
use serde_json::Value;

pub use ryuzi_openai_format::{
    BlockIn, ChatRequest, ChunkOut, MessageIn, ProviderFail, RoleIn, StopOut, ToolCallOut,
    ToolChoiceIn, ToolIn, UsageOut,
};

/// OpenCode Zen free-tier base (from the `opencode-free` descriptor).
pub const BASE_URL: &str = "https://opencode.ai/zen/v1";

/// Chat endpoint (OpenAI-compatible `/chat/completions`).
pub const CHAT_URL: &str = "https://opencode.ai/zen/v1/chat/completions";

/// Model-discovery endpoint (`has_models_endpoint: true`).
pub const MODELS_URL: &str = "https://opencode.ai/zen/v1/models";

/// The static free-tier bearer (`llm_router::client`'s `opencode-free` header).
pub const BEARER: &str = "public";

/// The client tag OpenCode's free tier expects.
pub const X_OPENCODE_CLIENT: &str = "desktop";

/// One model the provider advertises (host-free mirror of WIT `model-info`).
///
/// Deliberately NOT the shared `ryuzi_openai_format::ModelOut` — the two
/// happen to have identical fields today, but this component's own
/// [`parse_models`] populates `context_window` from OpenCode's per-entry
/// `context_length`, a source the shared `OpenAiFormat::parse_models` does
/// not read, so the type stays local to keep that fact visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOut {
    pub id: String,
    pub display_name: String,
    pub context_window: u32,
}

/// This provider's OpenAI-format wire configuration — everything
/// [`build_chat_body`] and [`parse_chat_response`] delegate to.
///
/// `context_windows` is empty and `default_context_window` is unused by this
/// component in practice: [`parse_models`] never calls into `CONFIG` for a
/// window (it reads OpenCode's own per-entry `context_length` instead), and
/// `build_chat_body`/`parse_chat_response` never consult a model's window.
/// The field is populated with the shared crate's own conservative default
/// purely so `CONFIG` stays a complete, honest `OpenAiFormat` value.
const CONFIG: OpenAiFormat = OpenAiFormat {
    provider_label: "OpenCode",
    default_base_url: BASE_URL,
    models_path: "/models",
    chat_path: "/chat/completions",
    max_tokens_field: "max_tokens",
    context_windows: &[],
    default_context_window: DEFAULT_CONTEXT_WINDOW,
};

/// The shared request headers for both `/models` and `/chat/completions`: the
/// static bearer plus the OpenCode client tag. The host forwards this
/// `authorization` header only because this is a VERIFIED first-party bundle.
pub fn request_headers() -> Vec<(String, String)> {
    vec![
        ("authorization".to_string(), format!("Bearer {BEARER}")),
        (
            "x-opencode-client".to_string(),
            X_OPENCODE_CLIENT.to_string(),
        ),
        ("accept".to_string(), "application/json".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
    ]
}

/// Build the OpenAI-format chat request body for a full transcript. A thin
/// delegation to [`CONFIG`] — OpenCode Zen's `/chat/completions` shape is
/// exactly `ryuzi_openai_format::OpenAiFormat::build_chat_body`'s, so the
/// mapping is never reimplemented here. OpenCode has no gate/system marker
/// of its own, so callers pass `leading_system: None`.
pub fn build_chat_body(request: ChatRequest<'_>) -> Vec<u8> {
    CONFIG.build_chat_body(request)
}

/// Parse an OpenAI-style `/models` response (`{"data":[{"id":...}]}`) into the
/// advertised model list. A per-entry `context_length`/`context_window` is used
/// when present, else [`DEFAULT_CONTEXT_WINDOW`]. Entries without a string `id`
/// are skipped.
pub fn parse_models(body: &[u8]) -> Result<Vec<ModelOut>, ProviderFail> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|e| ProviderFail::Failed(format!("OpenCode /models response is not JSON: {e}")))?;
    let data = value.get("data").and_then(Value::as_array).ok_or_else(|| {
        ProviderFail::Failed("OpenCode /models response has no data array".to_string())
    })?;
    let models = data
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_str)?.to_string();
            let context_window = entry
                .get("context_length")
                .or_else(|| entry.get("context_window"))
                .and_then(Value::as_u64)
                .map(|w| w as u32)
                .unwrap_or(DEFAULT_CONTEXT_WINDOW);
            Some(ModelOut {
                display_name: id.clone(),
                id,
                context_window,
            })
        })
        .collect();
    Ok(models)
}

/// Convert a buffered (non-stream) OpenAI chat completion into ordered
/// completion chunks. A thin delegation to [`CONFIG`] — same reasoning as
/// [`build_chat_body`]: the response shape (including tool calls and stop
/// reason) is exactly what `ryuzi_openai_format::OpenAiFormat::parse_chat_response`
/// already parses and tests.
pub fn parse_chat_response(body: &[u8]) -> Result<Vec<ChunkOut>, ProviderFail> {
    CONFIG.parse_chat_response(body)
}

/// Map a non-2xx chat response to a [`ProviderFail`]. Kept LOCAL rather than
/// delegated to `ryuzi_openai_format::OpenAiFormat::classify_error`: that
/// shared classifier distinguishes 5xx (`Unavailable`) and an
/// `error.code == "model_not_found"` body (`ModelNotFound`), while this one
/// maps every non-429 status — 5xx included — to a generic `Failed` carrying
/// the status, and never emits `ModelNotFound` for OpenCode. OpenCode's free
/// tier has no bespoke transient-block protocol and no documented
/// `model_not_found` error code, so a transient hiccup is not persisted as a
/// bad model. Changing this classification is out of scope for this task —
/// see the C4 report for the discrepancy.
pub fn classify_chat_error(status: u16, _body: &[u8]) -> ProviderFail {
    if status == 429 {
        ProviderFail::RateLimited
    } else {
        ProviderFail::Failed(format!("OpenCode chat failed: HTTP {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn request_headers_carry_the_static_bearer_and_client_tag() {
        let headers = request_headers();
        let get = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("authorization"), Some("Bearer public"));
        assert_eq!(get("x-opencode-client"), Some("desktop"));
        assert_eq!(get("content-type"), Some("application/json"));
    }

    fn chat_request<'a>(messages: &'a [MessageIn]) -> ChatRequest<'a> {
        ChatRequest {
            model: "some-model",
            messages,
            tools: &[],
            tool_choice: ToolChoiceIn::Auto,
            max_tokens: None,
            temperature: None,
            leading_system: None,
        }
    }

    #[test]
    fn chat_body_sends_a_single_user_message_and_the_legacy_max_tokens_field() {
        let messages = vec![MessageIn {
            role: RoleIn::User,
            content: vec![BlockIn::Text("ping".into())],
        }];
        let mut request = chat_request(&messages);
        request.max_tokens = Some(32);
        request.temperature = Some(0.5);
        let body: Value = serde_json::from_slice(&build_chat_body(request)).unwrap();
        assert_eq!(body["model"], "some-model");
        assert_eq!(body["stream"], false);
        assert_eq!(body["max_tokens"], 32);
        assert!(
            body.get("max_completion_tokens").is_none(),
            "OpenCode uses the legacy max_tokens field, not max_completion_tokens"
        );
        let out = body["messages"].as_array().unwrap();
        assert_eq!(out.len(), 1, "no system/gate message for OpenCode");
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "ping");
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
    fn parse_models_reads_ids_and_context_windows() {
        let body = br#"{"data":[
            {"id":"claude-3-5-sonnet","context_length":200000},
            {"id":"grok-code","context_window":256000},
            {"id":"no-window"}
        ]}"#;
        let models = parse_models(body).unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "claude-3-5-sonnet");
        assert_eq!(models[0].display_name, "claude-3-5-sonnet");
        assert_eq!(models[0].context_window, 200000);
        assert_eq!(models[1].context_window, 256000);
        assert_eq!(
            models[2].context_window, DEFAULT_CONTEXT_WINDOW,
            "a model without a reported window gets the default"
        );
    }

    #[test]
    fn parse_models_skips_entries_without_a_string_id_and_rejects_bad_shapes() {
        let body = br#"{"data":[{"id":"ok"},{"noid":1},{"id":123}]}"#;
        let models = parse_models(body).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "ok");

        assert!(matches!(
            parse_models(br#"{"nope":1}"#),
            Err(ProviderFail::Failed(_))
        ));
        assert!(matches!(
            parse_models(b"not json"),
            Err(ProviderFail::Failed(_))
        ));
    }

    #[test]
    fn parse_chat_response_yields_one_finished_chunk_with_usage() {
        let body = br#"{
            "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Hi there"}}],
            "usage":{"prompt_tokens":5,"completion_tokens":2}
        }"#;
        let chunks = parse_chat_response(body).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hi there");
        assert!(chunks[0].finished);
        assert_eq!(chunks[0].stop_reason, Some(StopOut::EndTurn));
        assert_eq!(
            chunks[0].usage,
            Some(UsageOut {
                input: 5,
                output: 2
            })
        );
    }

    #[test]
    fn parse_chat_response_without_usage_still_succeeds() {
        let chunks = parse_chat_response(br#"{"choices":[{"message":{"content":"ok"}}]}"#).unwrap();
        assert_eq!(chunks[0].text, "ok");
        assert_eq!(chunks[0].usage, None);
    }

    #[test]
    fn parse_chat_response_rejects_a_body_without_content() {
        assert!(matches!(
            parse_chat_response(br#"{"choices":[]}"#),
            Err(ProviderFail::Failed(_))
        ));
    }

    #[test]
    fn classify_chat_error_maps_429_to_rate_limited_else_failed() {
        assert_eq!(classify_chat_error(429, b""), ProviderFail::RateLimited);
        match classify_chat_error(503, b"down") {
            ProviderFail::Failed(msg) => assert!(msg.contains("503")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn the_chat_body_comes_from_the_shared_openai_format_mapping() {
        use ryuzi_openai_format::{BlockIn, ChatRequest, MessageIn, RoleIn, ToolChoiceIn, ToolIn};

        let messages = vec![
            MessageIn {
                role: RoleIn::System,
                content: vec![BlockIn::Text("sys".into())],
            },
            MessageIn {
                role: RoleIn::User,
                content: vec![BlockIn::Text("weather?".into())],
            },
        ];
        let tools = vec![ToolIn {
            name: "get_weather".into(),
            description: "Get weather".into(),
            input_schema: r#"{"type":"object"}"#.into(),
        }];
        let body: Value = serde_json::from_slice(&build_chat_body(ChatRequest {
            model: "big-pickle",
            messages: &messages,
            tools: &tools,
            tool_choice: ToolChoiceIn::Auto,
            max_tokens: Some(256),
            temperature: None,
            leading_system: None,
        }))
        .unwrap();

        assert_eq!(body["model"], "big-pickle");
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert_eq!(body["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(body["max_tokens"], 256);
    }

    #[test]
    fn a_tool_call_response_parses_through_the_shared_mapping() {
        let chunks = parse_chat_response(
            br#"{"choices":[{"finish_reason":"tool_calls","message":{"content":"",
                "tool_calls":[{"id":"c1","type":"function",
                    "function":{"name":"jira_search","arguments":"{}"}}]}}]}"#,
        )
        .unwrap();

        assert_eq!(chunks[0].tool_calls[0].name, "jira_search");
        assert_eq!(
            chunks[0].stop_reason,
            Some(ryuzi_openai_format::StopOut::ToolUse)
        );
    }

    #[test]
    fn models_parsing_still_reads_the_per_entry_context_length() {
        // OpenCode's /models DOES carry context_length, unlike the generic
        // OpenAI-format listing — this stays component-local on purpose.
        let models =
            parse_models(br#"{"data":[{"id":"claude-3-5-sonnet","context_length":200000}]}"#)
                .unwrap();
        assert_eq!(models[0].context_window, 200000);
    }
}
