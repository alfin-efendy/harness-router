//! wasm32-only guest glue for the `anthropic-oauth` provider component.
//!
//! Kept deliberately thin — no wire DECISIONS live here, only effect
//! orchestration and WIT type mapping. Every request is built by
//! [`crate::logic`] and sent through `oauth.authorized-request("anthropic-oauth",
//! ..)`, which injects the host-managed Claude-subscription bearer and strips
//! any `Authorization` the component set (it sets none); the component never
//! sees a token. Responses and non-2xx statuses are handed straight back to the
//! shared `ryuzi_anthropic_format` parsing/classification.
//!
//! # No `Authorization` is ever set here
//! There is no `ryuzi:http` import and no `ryuzi:provider-auth` import to set a
//! credential on: the ONLY egress is `ryuzi:oauth`. The headers this glue sets
//! are content negotiation, the `anthropic-version` protocol version, the
//! `anthropic-beta` OAuth flag, and the static Claude-Code client-identity
//! headers — none of them a credential. The Claude-subscription system marker
//! the endpoint also requires travels in the request BODY (see
//! [`crate::logic::CLAUDE_CODE_SYSTEM_PROMPT`]), because the host owns headers.

use crate::logic::{
    self, BlockIn, ChunkOut, MessageIn, ModelOut, OAuthFail, RoleIn, StopOut, ToolCallOut,
    ToolChoiceIn, ToolIn, CONFIG,
};
use ryuzi_anthropic_format::ProviderFail;

wit_bindgen::generate!({
    path: "wit",
    world: "anthropic-oauth",
    generate_all,
});

use exports::ryuzi::provider0_2_0::provider::{
    CompletionChunk, CompletionRequest, ContentBlock, Guest, ModelInfo, ProviderCapabilities,
    ProviderError, Role, StopReason, TokenUsage, ToolCall, ToolChoice,
};
use ryuzi::oauth::oauth::{self, AuthorizedResponse, Header, OauthError, OauthRequest};
use ryuzi::storage::storage;

struct ProviderComponent;

impl Guest for ProviderComponent {
    /// The Anthropic Messages API supports native tool use on every model this
    /// component serves (a Claude subscription reaches the same `/messages`
    /// endpoint as the x-api-key bundle). Parallel tool calls are NOT claimed:
    /// the host only treats the flag as a hint.
    fn capabilities() -> ProviderCapabilities {
        ProviderCapabilities {
            tools: true,
            parallel_tool_calls: false,
        }
    }

    fn list_models() -> Result<Vec<ModelInfo>, ProviderError> {
        let url = logic::models_url(&base_url());
        let response = authorized_request("GET", &url, None).map_err(map_fail)?;
        let models = if logic::status_is_success(response.status) {
            CONFIG.parse_models(&response.body)
        } else {
            Err(CONFIG.classify_error(response.status, &response.body))
        };
        models
            .map(|list| list.into_iter().map(map_model).collect())
            .map_err(map_fail)
    }

    fn complete(request: CompletionRequest) -> Result<Vec<CompletionChunk>, ProviderError> {
        if request.model.is_empty() {
            return Err(ProviderError::InvalidRequest(
                "a completion request must name a model".to_string(),
            ));
        }
        let messages: Vec<MessageIn> = request.messages.iter().map(map_message_in).collect();
        let tools: Vec<ToolIn> = request.tools.iter().map(map_tool_def).collect();
        let url = logic::messages_url(&base_url());
        let body = logic::build_completion_body(
            &request.model,
            &messages,
            &tools,
            map_tool_choice(request.tool_choice),
            request.max_tokens,
            request.temperature,
        );
        let response = authorized_request("POST", &url, Some(body)).map_err(map_fail)?;
        let chunks = if logic::status_is_success(response.status) {
            CONFIG.parse_message_response(&response.body)
        } else {
            Err(CONFIG.classify_error(response.status, &response.body))
        };
        chunks
            .map(|list| list.into_iter().map(map_chunk).collect())
            .map_err(map_fail)
    }
}

/// Map a generated WIT `message` onto the host-free [`MessageIn`] the shared
/// crate's `build_messages_body` consumes. Field-for-field, since the WIT
/// `content-block` variant already carries Anthropic's exact three block
/// shapes. Kept structurally identical to the `anthropic` bundle's own mapper.
fn map_message_in(message: &exports::ryuzi::provider0_2_0::provider::Message) -> MessageIn {
    MessageIn {
        role: match message.role {
            Role::System => RoleIn::System,
            Role::User => RoleIn::User,
            Role::Assistant => RoleIn::Assistant,
        },
        content: message
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text(text) => BlockIn::Text(text.clone()),
                ContentBlock::ToolUse(call) => BlockIn::ToolUse {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                },
                ContentBlock::ToolResult(result) => BlockIn::ToolResult {
                    tool_call_id: result.tool_call_id.clone(),
                    content: result.content.clone(),
                    is_error: result.is_error,
                },
            })
            .collect(),
    }
}

fn map_tool_def(tool: &exports::ryuzi::provider0_2_0::provider::ToolDef) -> ToolIn {
    ToolIn {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: tool.input_schema.clone(),
    }
}

fn map_tool_choice(choice: ToolChoice) -> ToolChoiceIn {
    match choice {
        ToolChoice::Auto => ToolChoiceIn::Auto,
        ToolChoice::None => ToolChoiceIn::None,
        ToolChoice::Required => ToolChoiceIn::Required,
    }
}

/// Send one request through the host-managed OAuth egress for the
/// `anthropic-oauth` profile. The host injects the bearer and strips any
/// component-set `Authorization` (never present here); the component never sees
/// the token. The headers set are content negotiation, the `anthropic-version`
/// protocol version, the `anthropic-beta` OAuth flag, and the static Claude-Code
/// client-identity headers the native path also sends — none a credential.
fn authorized_request(
    method: &str,
    url: &str,
    body: Option<Vec<u8>>,
) -> Result<AuthorizedResponse, ProviderFail> {
    let mut headers = vec![
        header("accept", "application/json"),
        // Required on every Anthropic request; a protocol version, not a
        // credential (see `ryuzi_anthropic_format::ANTHROPIC_VERSION`).
        header("anthropic-version", logic::ANTHROPIC_VERSION),
        // The OAuth beta flag (carries `oauth-2025-04-20`) the subscription
        // endpoint requires — ported from the native path.
        header("anthropic-beta", logic::ANTHROPIC_OAUTH_BETA),
        // Static Claude-Code client-identity headers the native
        // `oauth_upstream_request` also sends. Token-independent; help the
        // request look like the official client.
        header("anthropic-dangerous-direct-browser-access", "true"),
        header("user-agent", "claude-cli/2.1.92 (external, sdk-cli)"),
        header("x-app", "cli"),
    ];
    if body.is_some() {
        headers.push(header("content-type", "application/json"));
    }
    oauth::authorized_request(
        logic::OAUTH_PROFILE,
        &OauthRequest {
            method: method.to_string(),
            url: url.to_string(),
            headers,
            body,
        },
    )
    .map_err(|error| logic::map_oauth_error(map_oauth_error(error)))
}

fn header(name: &str, value: &str) -> Header {
    Header {
        name: name.to_string(),
        value: value.to_string(),
    }
}

/// The upstream base: the override in this component's storage slice when one is
/// set, else the config's default. `ryuzi:storage` is a WORLD IMPORT, so it is
/// always linked; the `Err(_) => None` arm covers "no value stored yet" or a
/// failed read, either of which degrades to the default — the override is an
/// optional affordance, never a correctness dependency.
fn base_url() -> String {
    let stored = match storage::get(logic::BASE_URL_STORAGE_KEY) {
        Ok(value) => String::from_utf8(value.value).ok(),
        Err(_) => None,
    };
    logic::resolve_base_url(stored.as_deref())
}

/// Convert the generated WIT `oauth-error` into the host-free [`OAuthFail`] the
/// natively-tested [`logic::map_oauth_error`] consumes. No variant carries a
/// token: the host's `oauth-error` contract keeps it out.
fn map_oauth_error(error: OauthError) -> OAuthFail {
    match error {
        OauthError::InvalidRequest(message) => OAuthFail::InvalidRequest(message),
        OauthError::Denied => OAuthFail::Denied,
        OauthError::Expired => OAuthFail::Expired,
        OauthError::Failed(message) => OAuthFail::Failed(message),
    }
}

fn map_model(model: ModelOut) -> ModelInfo {
    ModelInfo {
        id: model.id,
        display_name: model.display_name,
        context_window: model.context_window,
    }
}

fn map_chunk(chunk: ChunkOut) -> CompletionChunk {
    CompletionChunk {
        text: chunk.text,
        tool_calls: chunk
            .tool_calls
            .into_iter()
            .map(|call: ToolCallOut| ToolCall {
                id: call.id,
                name: call.name,
                arguments: call.arguments,
            })
            .collect(),
        finished: chunk.finished,
        stop_reason: chunk.stop_reason.map(|reason| match reason {
            StopOut::EndTurn => StopReason::EndTurn,
            StopOut::ToolUse => StopReason::ToolUse,
            StopOut::MaxTokens => StopReason::MaxTokens,
            StopOut::Other => StopReason::Other,
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

export!(ProviderComponent);
