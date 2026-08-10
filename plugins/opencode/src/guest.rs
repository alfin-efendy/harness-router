//! wasm32-only guest glue: wires [`crate::logic`] to the `ryuzi:http` host
//! import and exports `ryuzi:provider/provider@0.2.0`. No storage, no
//! bootstrap — the bearer is a constant, so both `list-models` (a `/models`
//! GET) and `complete` (a `/chat/completions` POST) are single stateless
//! requests.
//!
//! This component can NOT use `ryuzi_openai_format::provider_component!`: that
//! macro's egress goes through the host-mediated `ryuzi:provider-auth`
//! capability, which injects a HOST-managed credential. OpenCode's world
//! imports `ryuzi:http` directly and sends its own static `Bearer public`
//! credential (see [`crate::logic::request_headers`]), so this glue is
//! hand-rolled — but it follows the exact mapper shapes
//! `ryuzi_openai_format::__openai_provider_guest_core!` uses, so a reader who
//! knows that macro recognizes every function here.

use crate::logic::{self, ChunkOut, ProviderFail};

wit_bindgen::generate!({
    path: "wit",
    world: "opencode",
    generate_all,
});

use exports::ryuzi::provider0_2_0::provider::{
    CompletionChunk, CompletionRequest, ContentBlock, Guest, ModelInfo, ProviderCapabilities,
    ProviderError, Role, StopReason, TokenUsage, ToolCall, ToolChoice,
};

struct OpenCode;

impl Guest for OpenCode {
    /// OpenCode Zen speaks native function calling: a live probe of
    /// `big-pickle` on 2026-08-09 returned `finish_reason: "tool_calls"` with
    /// a well-formed `tool_calls` array, so `tools` is `true` on real evidence,
    /// not an assumption. `parallel_tool_calls` is not claimed — support is
    /// unproven and the host only treats the flag as a hint.
    fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            tools: true,
            parallel_tool_calls: false,
        }
    }

    fn list_models() -> Result<Vec<ModelInfo>, ProviderError> {
        let response = http_send("GET", logic::MODELS_URL, None).map_err(map_fail)?;
        if !(200..300).contains(&response.status) {
            return Err(map_fail(logic::classify_chat_error(
                response.status,
                &response.body,
            )));
        }
        logic::parse_models(&response.body)
            .map(|models| models.into_iter().map(map_model).collect())
            .map_err(map_fail)
    }

    fn complete(request: CompletionRequest) -> Result<Vec<CompletionChunk>, ProviderError> {
        if request.model.is_empty() {
            return Err(ProviderError::InvalidRequest(
                "a completion request must name a model".to_string(),
            ));
        }
        let messages: Vec<logic::MessageIn> = request.messages.iter().map(map_message_in).collect();
        let tools: Vec<logic::ToolIn> = request
            .tools
            .iter()
            .map(|tool| logic::ToolIn {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect();
        let body = logic::build_chat_body(logic::ChatRequest {
            model: &request.model,
            messages: &messages,
            tools: &tools,
            tool_choice: map_tool_choice(request.tool_choice),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            // OpenCode has no anti-abuse gate marker (that is MiMo's, not
            // built on this glue), so no leading system message is injected.
            leading_system: None,
        });
        let response = http_send("POST", logic::CHAT_URL, Some(body)).map_err(map_fail)?;
        let chunks = if (200..300).contains(&response.status) {
            logic::parse_chat_response(&response.body)
        } else {
            Err(logic::classify_chat_error(response.status, &response.body))
        };
        chunks
            .map(|c| c.into_iter().map(map_chunk).collect())
            .map_err(map_fail)
    }
}

/// One request through the host HTTP capability, carrying the shared OpenCode
/// headers (static bearer + client tag).
fn http_send(
    method: &str,
    url: &str,
    body: Option<Vec<u8>>,
) -> Result<ryuzi::http::http::HttpResponse, ProviderFail> {
    let request = ryuzi::http::http::HttpRequest {
        method: method.to_string(),
        url: url.to_string(),
        headers: logic::request_headers()
            .into_iter()
            .map(|(name, value)| ryuzi::http::http::Header { name, value })
            .collect(),
        body,
    };
    ryuzi::http::http::request(&request)
        .map_err(|error| ProviderFail::Failed(describe_http_error(error)))
}

fn describe_http_error(error: ryuzi::http::http::HttpError) -> String {
    use ryuzi::http::http::HttpError as E;
    match error {
        E::InvalidRequest(message) => format!("invalid HTTP request: {message}"),
        E::Rejected => "HTTP request rejected by the host allowlist".to_string(),
        E::Unavailable => "HTTP capability unavailable".to_string(),
        E::Failed(message) => format!("HTTP request failed: {message}"),
    }
}

fn map_model(model: logic::ModelOut) -> ModelInfo {
    ModelInfo {
        id: model.id,
        display_name: model.display_name,
        context_window: model.context_window,
    }
}

fn map_message_in(message: &exports::ryuzi::provider0_2_0::provider::Message) -> logic::MessageIn {
    logic::MessageIn {
        role: match message.role {
            Role::System => logic::RoleIn::System,
            Role::User => logic::RoleIn::User,
            Role::Assistant => logic::RoleIn::Assistant,
        },
        content: message
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text(text) => logic::BlockIn::Text(text.clone()),
                ContentBlock::ToolUse(call) => logic::BlockIn::ToolUse {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                },
                ContentBlock::ToolResult(result) => logic::BlockIn::ToolResult {
                    tool_call_id: result.tool_call_id.clone(),
                    content: result.content.clone(),
                    is_error: result.is_error,
                },
            })
            .collect(),
    }
}

fn map_tool_choice(choice: ToolChoice) -> logic::ToolChoiceIn {
    match choice {
        ToolChoice::Auto => logic::ToolChoiceIn::Auto,
        ToolChoice::None => logic::ToolChoiceIn::None,
        ToolChoice::Required => logic::ToolChoiceIn::Required,
    }
}

fn map_chunk(chunk: ChunkOut) -> CompletionChunk {
    CompletionChunk {
        text: chunk.text,
        tool_calls: chunk
            .tool_calls
            .into_iter()
            .map(|call| ToolCall {
                id: call.id,
                name: call.name,
                arguments: call.arguments,
            })
            .collect(),
        finished: chunk.finished,
        stop_reason: chunk.stop_reason.map(|reason| match reason {
            logic::StopOut::EndTurn => StopReason::EndTurn,
            logic::StopOut::ToolUse => StopReason::ToolUse,
            logic::StopOut::MaxTokens => StopReason::MaxTokens,
            logic::StopOut::Other => StopReason::Other,
        }),
        usage: chunk.usage.map(|u| TokenUsage {
            input: u.input,
            output: u.output,
        }),
    }
}

fn map_fail(fail: ProviderFail) -> ProviderError {
    match fail {
        ProviderFail::InvalidRequest(message) => ProviderError::InvalidRequest(message),
        ProviderFail::ModelNotFound => ProviderError::ModelNotFound,
        ProviderFail::RateLimited => ProviderError::RateLimited,
        ProviderFail::Unavailable => ProviderError::Unavailable,
        ProviderFail::Failed(message) => ProviderError::Failed(message),
    }
}

export!(OpenCode);
