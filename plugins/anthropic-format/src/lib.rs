//! Shared, host-free Anthropic **Messages** wire logic for Ryuzi's first-party
//! Anthropic provider components.
//!
//! Two provider descriptors in `crates/core/src/llm_router/registry.rs` declare
//! `format: ApiFormat::Anthropic` — `anthropic` (an `x-api-key` API-key
//! provider) and `anthropic-oauth` (a host-managed OAuth provider). They speak
//! the IDENTICAL `/messages` + `/models` wire shape and differ only in which
//! egress capability the guest calls (`ryuzi:provider-auth` vs `ryuzi:oauth`).
//! This crate owns everything they have in COMMON — base-URL override
//! resolution, request-body shaping from the flat provider ABI, `/models` and
//! message-response parsing, and upstream-status -> `provider-error`
//! classification — so that logic is written, reviewed and tested ONCE instead
//! of once per component.
//!
//! What stays per-provider is the data that provider's `ProviderDescriptor`
//! already carries (see [`AnthropicFormat`], a config transcription of the
//! descriptor), the egress capability its guest calls, and — for the OAuth
//! variant — the Claude-subscription auth markers (`anthropic-beta` flag and the
//! Claude-Code system prompt), which live in that component, not here.
//!
//! # Why this is not built on `ryuzi-openai-format`
//! That crate is the OpenAI-chat format: `messages[].content` as a string,
//! `choices[0].message.content` back, `usage.prompt_tokens`/`completion_tokens`,
//! an `error.code` vocabulary. Anthropic differs in every one of those — a
//! required `max_tokens`, `content[]` blocks out, `usage.input_tokens`/
//! `output_tokens`, `error.type` — so sharing would mean a format flag on every
//! function rather than shared behaviour. Anthropic is its own shape, so it gets
//! its own shared crate, extracted exactly the way `ryuzi-openai-format` was.
//!
//! # Nothing here touches a credential
//! The Anthropic provider components authenticate host-side: the API-key variant
//! through `ryuzi:provider-auth` (the host injects `x-api-key`), the OAuth
//! variant through `ryuzi:oauth` (the host injects the bearer). No function in
//! this crate sees, stores, or renders one — and [`error_tag`] exists
//! specifically to keep upstream error PROSE (which can echo a submitted key)
//! out of the guest-visible error string.
//!
//! # The 0.2.0 interface: transcript and tools in, tool calls out
//! Every component built on this crate exports `ryuzi:provider/provider@0.2.0`
//! and reports `capabilities().tools == true` — the Anthropic Messages API
//! supports tool use natively on every model these components serve.
//! [`AnthropicFormat::build_messages_body`] takes the full structured
//! transcript (messages, tools, tool-choice) rather than a single flat prompt,
//! and [`AnthropicFormat::parse_message_response`] reads tool-use blocks and
//! the stop reason back out. Because the host already speaks Anthropic
//! Messages, this mapping is close to an IDENTITY transform rather than a
//! translation — see the doc comments on those two functions for exactly
//! where it is not. The one remaining accepted limitation is no true token
//! streaming: the single buffered upstream response is returned as one
//! terminal chunk, never deltas. The OAuth variant's leading system prompt
//! (its Claude-subscription auth marker) travels as
//! [`MessagesRequest::leading_system`], injected by the component.

use serde_json::{json, Map, Value};

/// Key in a component's (host-scoped) `ryuzi:storage` slice holding an OPTIONAL
/// base-URL override — the same product-level affordance every provider
/// component exposes (pointing at a compatible gateway, and letting the provider
/// conformance harness aim the component at a loopback mock). A blank/whitespace
/// value is treated as "unset". The manifest network allowlist still governs
/// whatever the override resolves to, so an override can never widen where the
/// user's credential may travel.
pub const BASE_URL_STORAGE_KEY: &str = "base-url";

/// The Anthropic API version this format pins, sent as the `anthropic-version`
/// request header on every call.
///
/// Anthropic REQUIRES this header and treats it as the contract version for
/// request and response shapes, so it must be a value chosen deliberately rather
/// than tracked implicitly. `2023-06-01` is the current stable Messages version
/// and is the SAME value the native router path already pins
/// (`llm_router::client::oauth_upstream_request`,
/// `llm_router::models::models_request`), so the components and the native path
/// cannot interpret the same account differently. It is a protocol version, not
/// a credential: the guest sets it, the host forwards it.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// `max_tokens` sent when the flat provider ABI's `max-tokens` is absent.
///
/// Unlike OpenAI-chat, Anthropic REQUIRES `max_tokens` on every `/messages`
/// request — omitting it is a hard 400, so "leave it out when the caller did not
/// ask" is not an option here and a default must be picked. This matches the
/// default the engine's own OpenAI->Anthropic request translation already
/// injects for exactly this reason (`llm_router::translate::
/// openai_to_anthropic_request`), so a caller that specifies nothing gets the
/// same cap through the component that it gets through the native path.
///
/// It is a cap, not a target: a shorter completion still stops on its own. The
/// cost of it being low is a truncated long answer; the cost of it being high is
/// nothing until a model actually generates that much. 4096 is the conservative
/// middle the rest of this codebase already settled on.
pub const DEFAULT_MAX_TOKENS: u32 = 4_096;

/// Context window advertised for a model no static table covers.
///
/// Anthropic's `/models` response carries no context length (`id`,
/// `display_name`, `created_at`, `type`), so a window is either a static hint or
/// a guess. This is the conservative hint, and it deliberately mirrors the value
/// the router itself already falls back to
/// (`llm_router::model_meta::FALLBACK.context_window`) rather than introducing a
/// second, differently-wrong default.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 128_000;

/// Longest an `error.type` tag may be before it stops looking like a
/// machine-readable code and starts looking like prose that could carry
/// upstream-echoed request material. See [`error_tag`].
const MAX_ERROR_TAG_LEN: usize = 64;

/// The Anthropic `error.type` that means "that model does not exist". Anthropic
/// returns it for any unknown resource, but on these components' only two
/// endpoints (`/models` and `/messages`) the resource in question is the model.
const MODEL_NOT_FOUND_TYPE: &str = "not_found_error";

/// Everything that differs between two Anthropic-Messages providers.
///
/// Every field is DATA the provider's `ProviderDescriptor`
/// (`crates/core/src/llm_router/registry.rs`) already states, so the config is a
/// transcription of the descriptor rather than an independent guess. The struct
/// exists (rather than bare constants) so a second Anthropic-shaped provider —
/// the OAuth variant, a gateway — is a new config value, not a fork of this
/// module.
pub struct AnthropicFormat {
    /// Human-readable provider name used in guest-visible error strings.
    /// Never a credential.
    pub provider_label: &'static str,
    /// The descriptor's `base_url`. Used unless the component's storage slice
    /// carries an override at [`BASE_URL_STORAGE_KEY`].
    pub default_base_url: &'static str,
    /// Model-discovery path appended to the resolved base. Only meaningful for
    /// a descriptor with `has_models_endpoint: true`.
    pub models_path: &'static str,
    /// Message-generation path appended to the resolved base.
    pub messages_path: &'static str,
    /// `max_tokens` when the ABI carries none — see [`DEFAULT_MAX_TOKENS`].
    pub default_max_tokens: u32,
    /// Static context-window hints by model-id PREFIX, scanned IN ORDER so the
    /// most specific prefix must be listed first. Empty for a provider with no
    /// published per-family values worth pinning.
    pub context_windows: &'static [(&'static str, u32)],
    /// Window for a model [`Self::context_windows`] does not cover.
    pub default_context_window: u32,
}

/// One model the provider advertises (host-free mirror of the WIT `model-info`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOut {
    pub id: String,
    pub display_name: String,
    pub context_window: u32,
}

/// Token usage a chunk may report (host-free mirror of WIT `token-usage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageOut {
    pub input: u32,
    pub output: u32,
}

/// Who authored a message (host-free mirror of the WIT 0.2.0 `role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleIn {
    System,
    User,
    Assistant,
}

/// One piece of a message (host-free mirror of WIT `content-block`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockIn {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        /// A serialized JSON object — Anthropic's wire `input` is the object
        /// itself, so [`AnthropicFormat::build_messages_body`] parses this
        /// back before emitting it.
        arguments: String,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
}

/// One turn of the transcript (host-free mirror of WIT `message`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageIn {
    pub role: RoleIn,
    pub content: Vec<BlockIn>,
}

/// A tool the agent bound for this turn (host-free mirror of WIT `tool-def`).
/// `input_schema` is a serialized JSON Schema object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIn {
    pub name: String,
    pub description: String,
    pub input_schema: String,
}

/// How hard to push the model to call a tool (mirror of WIT `tool-choice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolChoiceIn {
    Auto,
    None,
    Required,
}

/// Why the model stopped (mirror of WIT `stop-reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOut {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other,
}

/// One model-emitted tool call (mirror of WIT `tool-call`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallOut {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Everything one `/messages` call needs.
///
/// A struct rather than a parameter list for the same reason as the
/// OpenAI-format sibling: it grew past the point where positional arguments
/// read safely, and `leading_system` is easy to pass in the wrong slot as a
/// bare `Option<&str>`.
pub struct MessagesRequest<'a> {
    pub model: &'a str,
    pub messages: &'a [MessageIn],
    pub tools: &'a [ToolIn],
    pub tool_choice: ToolChoiceIn,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    /// Emitted as the FIRST block of the request's top-level `system` array
    /// when set, ahead of any `RoleIn::System` messages the transcript itself
    /// carries. Exists for the OAuth variant's Claude-subscription auth
    /// marker (see `plugins/anthropic-oauth`); the API-key variant passes
    /// `None`.
    pub leading_system: Option<&'a str>,
}

/// One completion chunk (host-free mirror of WIT `completion-chunk`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkOut {
    pub text: String,
    /// Tool calls the model emitted. Empty for a plain text turn.
    pub tool_calls: Vec<ToolCallOut>,
    pub finished: bool,
    pub stop_reason: Option<StopOut>,
    pub usage: Option<UsageOut>,
}

/// A provider failure (host-free mirror of WIT `provider-error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderFail {
    InvalidRequest(String),
    ModelNotFound,
    RateLimited,
    Unavailable,
    Failed(String),
}

impl AnthropicFormat {
    /// The upstream base for this call: a non-blank stored override, else
    /// [`Self::default_base_url`]. Any trailing `/` is trimmed so path joins
    /// never produce a doubled separator.
    pub fn resolve_base_url(&self, stored: Option<&str>) -> String {
        let base = stored
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(self.default_base_url);
        base.trim_end_matches('/').to_string()
    }

    /// `<base><models_path>` — the model-discovery endpoint.
    pub fn models_url(&self, base: &str) -> String {
        format!("{base}{}", self.models_path)
    }

    /// `<base><messages_path>` — the message-generation endpoint.
    pub fn messages_url(&self, base: &str) -> String {
        format!("{base}{}", self.messages_path)
    }

    /// The static context window for `model_id` — the first matching prefix in
    /// [`Self::context_windows`], else [`Self::default_context_window`].
    pub fn context_window_for(&self, model_id: &str) -> u32 {
        self.context_windows
            .iter()
            .find(|(prefix, _)| model_id.starts_with(prefix))
            .map(|(_, window)| *window)
            .unwrap_or(self.default_context_window)
    }

    /// Build the NON-STREAMING `/messages` body for a full transcript.
    ///
    /// # This is nearly an identity mapping
    /// The host already speaks Anthropic Messages: `content-block` is
    /// text/tool-use/tool-result (Anthropic's exact three shapes), `message` is
    /// `{role, content}` (Anthropic's exact shape), and `tool-def.input-schema`
    /// is meant to travel as the schema itself (Anthropic's `input_schema`,
    /// unlike OpenAI's nested `function.parameters`). So a `User`/`Assistant`
    /// message maps ONE-TO-ONE onto one Anthropic message — no splitting, no
    /// synthesized turns, no reshaping beyond re-rendering field names — and a
    /// bound tool maps onto one Anthropic tool definition the same way. The two
    /// places this is genuinely NOT identity:
    /// - Anthropic has no per-turn `system` role: a `RoleIn::System` message
    ///   cannot appear in `messages` at all, so its `Text` blocks are pulled out
    ///   into the top-level `system` array instead (see below). A `ToolUse`/
    ///   `ToolResult` block inside a system message is a caller bug — a system
    ///   turn only carries text — and is silently dropped rather than failing
    ///   the whole request.
    /// - `arguments`/`input_schema` are serialized JSON STRINGS in the WIT ABI
    ///   (WIT has no JSON value type) but Anthropic wants the parsed OBJECT
    ///   under `input`/`input_schema`; this function parses them back. A string
    ///   that fails to parse is a host bug, not a reason to fail the turn, so it
    ///   degrades to an empty/open object rather than erroring.
    /// - `ToolChoiceIn::Required` (force SOME tool call) renders as Anthropic's
    ///   `{"type":"any"}`, not a `"required"` tag — Anthropic's tool_choice
    ///   vocabulary is `auto`/`any`/`tool`/`none`.
    ///
    /// `system` is an array of leading text blocks: [`MessagesRequest::leading_system`]
    /// (if set) FIRST, then each `RoleIn::System` message's text blocks in
    /// transcript order. The array is omitted entirely when both are absent —
    /// the API-key path with no system messages carries no `system` key at all,
    /// byte-for-byte as before this task.
    ///
    /// `stream` is false because the host capability is a buffered
    /// request/response: the component asks for the whole message and returns it
    /// as one terminal chunk.
    ///
    /// `tools`/`tool_choice` are omitted entirely when no tools are bound — the
    /// same "no tools means no tools key" rule the OpenAI-format crate applies,
    /// so a pure-chat turn's body is unchanged from before tool support existed.
    ///
    /// `max_tokens` is ALWAYS present, falling back to
    /// [`Self::default_max_tokens`] — Anthropic rejects a request without it.
    ///
    /// `temperature` is OMITTED when it is not finite (NaN/±inf): JSON has no
    /// representation for those values, so there is nothing to send. The request
    /// still goes out and the upstream applies its own default — failing an
    /// entire completion over an unrepresentable optional tuning knob would be
    /// the worse trade.
    pub fn build_messages_body(&self, request: MessagesRequest<'_>) -> Vec<u8> {
        let mut system_blocks: Vec<Value> = Vec::new();
        if let Some(leading) = request.leading_system {
            system_blocks.push(json!({"type": "text", "text": leading}));
        }

        let mut messages: Vec<Value> = Vec::new();
        for message in request.messages {
            if matches!(message.role, RoleIn::System) {
                // No per-turn system role on the wire: pull this message's
                // text out into the top-level `system` array instead of
                // `messages`. Non-text blocks in a system message do not fit
                // Anthropic's `system` shape and are dropped.
                for block in &message.content {
                    if let BlockIn::Text(text) = block {
                        system_blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                continue;
            }
            if message.content.is_empty() {
                continue;
            }
            let blocks: Vec<Value> = message
                .content
                .iter()
                .map(|block| match block {
                    BlockIn::Text(text) => json!({"type": "text", "text": text}),
                    BlockIn::ToolUse {
                        id,
                        name,
                        arguments,
                    } => json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": serde_json::from_str::<Value>(arguments)
                            .unwrap_or_else(|_| json!({})),
                    }),
                    BlockIn::ToolResult {
                        tool_call_id,
                        content,
                        is_error,
                    } => json!({
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": content,
                        "is_error": is_error,
                    }),
                })
                .collect();
            messages.push(json!({
                "role": role_name(message.role),
                "content": blocks,
            }));
        }

        let mut obj = Map::new();
        obj.insert(
            "model".to_string(),
            Value::String(request.model.to_string()),
        );
        obj.insert(
            "max_tokens".to_string(),
            Value::from(request.max_tokens.unwrap_or(self.default_max_tokens)),
        );
        obj.insert("messages".to_string(), Value::Array(messages));
        obj.insert("stream".to_string(), Value::Bool(false));
        if !system_blocks.is_empty() {
            obj.insert("system".to_string(), Value::Array(system_blocks));
        }
        if !request.tools.is_empty() {
            obj.insert(
                "tools".to_string(),
                Value::Array(
                    request
                        .tools
                        .iter()
                        .map(|tool| {
                            json!({
                                "name": tool.name,
                                "description": tool.description,
                                // Anthropic takes the schema DIRECTLY as
                                // `input_schema` — no `function.parameters`
                                // nesting the way OpenAI wants it. A schema
                                // that does not parse degrades to the
                                // permissive open object rather than failing
                                // the turn.
                                "input_schema": serde_json::from_str::<Value>(&tool.input_schema)
                                    .unwrap_or_else(|_| json!({"type": "object"})),
                            })
                        })
                        .collect(),
                ),
            );
            obj.insert(
                "tool_choice".to_string(),
                match request.tool_choice {
                    ToolChoiceIn::Auto => json!({"type": "auto"}),
                    ToolChoiceIn::None => json!({"type": "none"}),
                    // WIT's "force some tool call" is Anthropic's `any`, not a
                    // `"required"` tag.
                    ToolChoiceIn::Required => json!({"type": "any"}),
                },
            );
        }
        if let Some(temp) = request.temperature {
            if let Some(number) = serde_json::Number::from_f64(temp as f64) {
                obj.insert("temperature".to_string(), Value::Number(number));
            }
        }
        serde_json::to_vec(&Value::Object(obj)).expect("messages body always serializes")
    }

    /// Parse an Anthropic `/models` response
    /// (`{"data":[{"type":"model","id":...,"display_name":...}]}`) into the
    /// advertised model list, preserving the served order.
    ///
    /// Unlike the OpenAI-format listing, Anthropic DOES carry a human display
    /// name, so it is used when present and the id stands in when it is not.
    /// The response still carries no context length, so the window comes from
    /// [`Self::context_window_for`]. Entries without a string `id` are skipped
    /// rather than failing the whole listing.
    pub fn parse_models(&self, body: &[u8]) -> Result<Vec<ModelOut>, ProviderFail> {
        let label = self.provider_label;
        let value: Value = serde_json::from_slice(body).map_err(|e| {
            ProviderFail::Failed(format!("{label} /models response is not JSON: {e}"))
        })?;
        let data = value.get("data").and_then(Value::as_array).ok_or_else(|| {
            ProviderFail::Failed(format!("{label} /models response has no data array"))
        })?;
        Ok(data
            .iter()
            .filter_map(|entry| {
                let id = entry.get("id").and_then(Value::as_str)?.to_string();
                let display_name = entry
                    .get("display_name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .unwrap_or(&id)
                    .to_string();
                Some(ModelOut {
                    display_name,
                    context_window: self.context_window_for(&id),
                    id,
                })
            })
            .collect())
    }

    /// Convert a buffered (non-stream) `/messages` response into ordered
    /// completion chunks: one terminal chunk carrying every `text` block's
    /// prose (newline-joined), every `tool_use` block as a tool call, the
    /// mapped stop reason, and the response's token usage when present.
    ///
    /// Anthropic returns `content` as an array of typed blocks. `thinking`/
    /// `redacted_thinking` (and any future block type this crate does not
    /// recognize) are skipped rather than rendered, so a reasoning model's
    /// private thinking is never surfaced as the completion.
    ///
    /// A response with NO text block is not an error — a tool-only turn
    /// legitimately carries zero `text` blocks (just `tool_use`), and
    /// rejecting it would fail every tool call the same way the pre-tools
    /// OpenAI-format parser used to. Only a response with no `content` array
    /// at all is malformed.
    pub fn parse_message_response(&self, body: &[u8]) -> Result<Vec<ChunkOut>, ProviderFail> {
        let label = self.provider_label;
        let value: Value = serde_json::from_slice(body).map_err(|e| {
            ProviderFail::Failed(format!("{label} message response is not JSON: {e}"))
        })?;
        let blocks = value
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProviderFail::Failed(format!("{label} message response carried no content array"))
            })?;

        let mut text_parts: Vec<&str> = Vec::new();
        let mut tool_calls: Vec<ToolCallOut> = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        text_parts.push(text);
                    }
                }
                Some("tool_use") => {
                    // A malformed tool_use entry (missing id/name) is skipped
                    // rather than failing the whole response — mirrors how
                    // `ryuzi_openai_format::parse_chat_response` treats a
                    // malformed `tool_calls` entry.
                    if let (Some(id), Some(name)) = (
                        block.get("id").and_then(Value::as_str),
                        block.get("name").and_then(Value::as_str),
                    ) {
                        // Anthropic's `input` is a JSON OBJECT; the WIT
                        // `tool-call.arguments` is a serialized JSON string,
                        // so it is re-serialized here.
                        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                        tool_calls.push(ToolCallOut {
                            id: id.to_string(),
                            name: name.to_string(),
                            arguments: serde_json::to_string(&input)
                                .unwrap_or_else(|_| "{}".to_string()),
                        });
                    }
                }
                // `thinking`, `redacted_thinking`, and anything else this
                // crate does not recognize carry no completion content.
                _ => {}
            }
        }

        Ok(vec![ChunkOut {
            text: text_parts.join("\n"),
            tool_calls,
            finished: true,
            stop_reason: Some(match value.get("stop_reason").and_then(Value::as_str) {
                Some("tool_use") => StopOut::ToolUse,
                Some("max_tokens") => StopOut::MaxTokens,
                Some("end_turn") | Some("stop_sequence") => StopOut::EndTurn,
                _ => StopOut::Other,
            }),
            usage: parse_usage(&value),
        }])
    }

    /// Map a non-2xx upstream response onto a [`ProviderFail`].
    ///
    /// - `429` -> rate-limited
    /// - `5xx` -> unavailable (transient/environmental, never a "bad model" verdict)
    /// - a `not_found_error` type -> model-not-found
    /// - any other `4xx` (and anything else non-2xx) -> invalid-request
    ///
    /// The rendered message carries only the provider label, the status and the
    /// short [`error_tag`] — never the upstream `message`, which can echo the
    /// submitted credential.
    pub fn classify_error(&self, status: u16, body: &[u8]) -> ProviderFail {
        let label = self.provider_label;
        let tag = error_tag(body);
        if status == 429 {
            return ProviderFail::RateLimited;
        }
        if status >= 500 {
            return ProviderFail::Unavailable;
        }
        if tag.as_deref() == Some(MODEL_NOT_FOUND_TYPE) {
            return ProviderFail::ModelNotFound;
        }
        ProviderFail::InvalidRequest(match tag {
            Some(tag) => format!("{label} rejected the request: HTTP {status} ({tag})"),
            None => format!("{label} rejected the request: HTTP {status}"),
        })
    }
}

/// Anthropic's per-message `role` — only ever `"user"` or `"assistant"` on the
/// wire (a `RoleIn::System` message never reaches this function; it is
/// extracted into the top-level `system` array before this is called).
fn role_name(role: RoleIn) -> &'static str {
    match role {
        RoleIn::System => "system",
        RoleIn::User => "user",
        RoleIn::Assistant => "assistant",
    }
}

/// Whether an upstream status is a success (and so parsed rather than classified
/// as an error).
pub fn status_is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// The short, machine-readable `error.type` from an Anthropic error body
/// (`{"type":"error","error":{"type":"...","message":"..."}}`), if it really
/// looks like a code.
///
/// Deliberately NOT `error.message`: Anthropic's authentication failures quote
/// the submitted key back in that prose, and this value crosses into a
/// guest-visible `provider-error`. A tag that is blank, over
/// [`MAX_ERROR_TAG_LEN`], or contains whitespace is prose rather than a code and
/// is dropped.
///
/// This filter is this crate's one non-obvious security-relevant behaviour: it
/// is what makes [`AnthropicFormat::classify_error`] safe to surface. It is the
/// same rule `ryuzi_openai_format::error_tag` applies, restated for THIS wire
/// shape rather than shared, because the field it reads (`error.type`, no
/// `error.code`) and the code vocabulary it recognizes are Anthropic's.
pub fn error_tag(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let tag = value.get("error")?.get("type")?.as_str()?;
    (!tag.is_empty() && tag.len() <= MAX_ERROR_TAG_LEN && !tag.chars().any(char::is_whitespace))
        .then(|| tag.to_string())
}

/// Anthropic reports usage as `input_tokens`/`output_tokens` (the OpenAI shape
/// says `prompt_tokens`/`completion_tokens`).
fn parse_usage(value: &Value) -> Option<UsageOut> {
    let usage = value.get("usage")?;
    let input = usage.get("input_tokens").and_then(Value::as_u64)?;
    let output = usage.get("output_tokens").and_then(Value::as_u64)?;
    Some(UsageOut {
        input: saturating_u32(input),
        output: saturating_u32(output),
    })
}

/// Narrow a JSON-wide `u64` token count to the WIT `token-usage`'s `u32`,
/// SATURATING rather than wrapping. A wrapping cast would turn an absurd (or
/// hostile) upstream count into a small plausible one and silently under-report
/// spend to the router; clamping at least stays monotonic and obviously extreme.
fn saturating_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `anthropic` descriptor's config, transcribed — an EMPTY
    /// context-window table (Anthropic's `/models` carries no context length and
    /// the descriptor pins no per-model windows). The `anthropic-oauth`
    /// descriptor shares these exact wire values, differing only in egress.
    const ANTHROPIC: AnthropicFormat = AnthropicFormat {
        provider_label: "Anthropic",
        default_base_url: "https://api.anthropic.com/v1",
        models_path: "/models",
        messages_path: "/messages",
        default_max_tokens: DEFAULT_MAX_TOKENS,
        context_windows: &[],
        default_context_window: DEFAULT_CONTEXT_WINDOW,
    };

    /// A deliberately DIFFERENT config in every dimension the struct exposes —
    /// label, base, both paths, token default, non-default window. Its purpose
    /// is anti-tautology: assertions run against both configs, so a function that
    /// ignored `self` and hardcoded Anthropic's values would fail here even
    /// though it passed against [`ANTHROPIC`].
    const OTHER: AnthropicFormat = AnthropicFormat {
        provider_label: "Contoso",
        default_base_url: "https://api.contoso.test/anthropic/v2",
        models_path: "/model-list",
        messages_path: "/chat",
        default_max_tokens: 77,
        context_windows: &[("claude-opus", 200_000)],
        default_context_window: 32_768,
    };

    #[test]
    fn format_produces_the_expected_anthropic_endpoints() {
        assert_eq!(
            ANTHROPIC.resolve_base_url(None),
            "https://api.anthropic.com/v1"
        );
        assert_eq!(
            ANTHROPIC.messages_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/messages",
            "ApiFormat::Anthropic generates at /messages, not /chat/completions",
        );
        assert_eq!(
            ANTHROPIC.models_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(ANTHROPIC.provider_label, "Anthropic");
        assert_eq!(ANTHROPIC_VERSION, "2023-06-01");
    }

    #[test]
    fn base_url_defaults_to_the_configured_api_and_honours_a_non_empty_override() {
        assert_eq!(
            ANTHROPIC.resolve_base_url(Some("")),
            "https://api.anthropic.com/v1"
        );
        assert_eq!(
            ANTHROPIC.resolve_base_url(Some("   ")),
            "https://api.anthropic.com/v1"
        );
        assert_eq!(
            OTHER.resolve_base_url(None),
            "https://api.contoso.test/anthropic/v2",
            "the default must come from the config, not a hardcoded vendor",
        );
        assert_eq!(
            ANTHROPIC.resolve_base_url(Some("http://127.0.0.1:8080")),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            ANTHROPIC.resolve_base_url(Some("https://gateway.test/v1/")),
            "https://gateway.test/v1",
            "a trailing slash is trimmed so path joins never double up",
        );
        assert_eq!(
            OTHER.messages_url(&OTHER.resolve_base_url(None)),
            "https://api.contoso.test/anthropic/v2/chat"
        );
        assert_eq!(
            OTHER.models_url(&OTHER.resolve_base_url(None)),
            "https://api.contoso.test/anthropic/v2/model-list"
        );
    }

    /// Build a [`MessagesRequest`] with every field at its "nothing bound" default
    /// except `model`/`messages`/`tools`, exactly like the OpenAI-format
    /// sibling's own `req()` test helper.
    fn req<'a>(messages: &'a [MessageIn], tools: &'a [ToolIn]) -> MessagesRequest<'a> {
        MessagesRequest {
            model: "m",
            messages,
            tools,
            tool_choice: ToolChoiceIn::Auto,
            max_tokens: None,
            temperature: None,
            leading_system: None,
        }
    }

    #[test]
    fn messages_body_carries_the_full_transcript_as_content_block_arrays() {
        // The host already speaks Anthropic Messages, so a User/Assistant
        // message maps ONE-TO-ONE onto one Anthropic message with a `content`
        // BLOCK ARRAY — never collapsed to a bare string the way the old flat
        // ABI's single turn was.
        let messages = vec![
            MessageIn {
                role: RoleIn::User,
                content: vec![BlockIn::Text("hi".into())],
            },
            MessageIn {
                role: RoleIn::Assistant,
                content: vec![BlockIn::Text("hello".into())],
            },
        ];
        let body: Value =
            serde_json::from_slice(&ANTHROPIC.build_messages_body(req(&messages, &[]))).unwrap();
        let out = body["messages"].as_array().unwrap();
        assert_eq!(
            out.len(),
            2,
            "no expansion, no collapsing — one in, one out"
        );
        assert_eq!(out[0]["role"], "user");
        assert_eq!(
            out[0]["content"],
            serde_json::json!([{"type": "text", "text": "hi"}])
        );
        assert_eq!(out[1]["role"], "assistant");
        assert_eq!(
            out[1]["content"],
            serde_json::json!([{"type": "text", "text": "hello"}])
        );
        assert!(body.get("temperature").is_none());
        assert!(
            body.get("system").is_none(),
            "no leading marker and no System-role message means no system key at all",
        );
    }

    #[test]
    fn a_system_role_message_is_pulled_out_of_messages_into_the_top_level_system_array() {
        // Anthropic has NO per-turn system role, so unlike every other role a
        // `RoleIn::System` message cannot map onto a `messages` entry at all —
        // this is the one place the mapping is a real reshape, not identity.
        let messages = vec![
            MessageIn {
                role: RoleIn::System,
                content: vec![BlockIn::Text("sys".into())],
            },
            MessageIn {
                role: RoleIn::User,
                content: vec![BlockIn::Text("hi".into())],
            },
        ];
        let body: Value =
            serde_json::from_slice(&ANTHROPIC.build_messages_body(req(&messages, &[]))).unwrap();
        assert_eq!(
            body["system"],
            serde_json::json!([{"type": "text", "text": "sys"}]),
        );
        let out = body["messages"].as_array().unwrap();
        assert_eq!(out.len(), 1, "the system message never lands in messages");
        assert_eq!(out[0]["role"], "user");
    }

    #[test]
    fn a_leading_system_marker_precedes_any_transcript_system_messages() {
        // The OAuth variant's Claude-subscription auth marker
        // (`leading_system`) must come FIRST, ahead of whatever System-role
        // messages the transcript itself carries.
        let messages = vec![
            MessageIn {
                role: RoleIn::System,
                content: vec![BlockIn::Text("transcript sys".into())],
            },
            MessageIn {
                role: RoleIn::User,
                content: vec![BlockIn::Text("hi".into())],
            },
        ];
        let mut request = req(&messages, &[]);
        request.leading_system = Some("MARKER");
        let body: Value = serde_json::from_slice(&ANTHROPIC.build_messages_body(request)).unwrap();
        assert_eq!(
            body["system"],
            serde_json::json!([
                {"type": "text", "text": "MARKER"},
                {"type": "text", "text": "transcript sys"},
            ]),
        );
    }

    #[test]
    fn tools_are_forwarded_with_input_schema_directly_not_nested_under_function() {
        // Anthropic takes the schema AS the tool's `input_schema` — no
        // `function.parameters` nesting the way OpenAI wants it.
        let messages = vec![MessageIn {
            role: RoleIn::User,
            content: vec![BlockIn::Text("w?".into())],
        }];
        let tools = vec![ToolIn {
            name: "get_weather".into(),
            description: "Get weather".into(),
            input_schema: r#"{"type":"object","properties":{"city":{"type":"string"}}}"#.into(),
        }];
        let body: Value =
            serde_json::from_slice(&ANTHROPIC.build_messages_body(req(&messages, &tools))).unwrap();
        assert_eq!(body["tools"][0]["name"], "get_weather");
        assert_eq!(body["tools"][0]["description"], "Get weather");
        assert_eq!(
            body["tools"][0]["input_schema"]["properties"]["city"]["type"],
            "string"
        );
        assert!(
            body["tools"][0].get("function").is_none(),
            "no function wrapper — Anthropic's tool shape has none",
        );
        assert_eq!(body["tool_choice"], serde_json::json!({"type": "auto"}));
    }

    #[test]
    fn no_tools_means_no_tools_or_tool_choice_key_at_all() {
        let messages = vec![MessageIn {
            role: RoleIn::User,
            content: vec![BlockIn::Text("hi".into())],
        }];
        let body: Value =
            serde_json::from_slice(&ANTHROPIC.build_messages_body(req(&messages, &[]))).unwrap();
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn tool_choice_required_renders_as_anthropics_any_not_a_required_tag() {
        let messages = vec![MessageIn {
            role: RoleIn::User,
            content: vec![BlockIn::Text("hi".into())],
        }];
        let tools = vec![ToolIn {
            name: "t".into(),
            description: String::new(),
            input_schema: "{}".into(),
        }];
        let mut request = req(&messages, &tools);
        request.tool_choice = ToolChoiceIn::Required;
        let body: Value = serde_json::from_slice(&ANTHROPIC.build_messages_body(request)).unwrap();
        assert_eq!(
            body["tool_choice"],
            serde_json::json!({"type": "any"}),
            "WIT's Required (force some tool) is Anthropic's `any`, not `required`",
        );

        let mut none_request = req(&messages, &tools);
        none_request.tool_choice = ToolChoiceIn::None;
        let none_body: Value =
            serde_json::from_slice(&ANTHROPIC.build_messages_body(none_request)).unwrap();
        assert_eq!(
            none_body["tool_choice"],
            serde_json::json!({"type": "none"})
        );
    }

    #[test]
    fn an_unparseable_tool_schema_degrades_to_an_open_object_instead_of_failing() {
        let messages = vec![MessageIn {
            role: RoleIn::User,
            content: vec![BlockIn::Text("hi".into())],
        }];
        let tools = vec![ToolIn {
            name: "broken".into(),
            description: String::new(),
            input_schema: "not json".into(),
        }];
        let body: Value =
            serde_json::from_slice(&ANTHROPIC.build_messages_body(req(&messages, &tools))).unwrap();
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    }

    #[test]
    fn an_assistant_tool_use_and_its_result_stay_in_their_own_messages_content_array() {
        // Unlike the OpenAI-format mapping (which SPLITS a tool result into a
        // separate `role: "tool"` message), Anthropic's tool_use/tool_result
        // blocks travel inside the SAME per-turn content array the transcript
        // already puts them in — no expansion needed, because the wire and the
        // ABI agree on where a tool block lives.
        let messages = vec![
            MessageIn {
                role: RoleIn::Assistant,
                content: vec![BlockIn::ToolUse {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    arguments: r#"{"city":"Jakarta"}"#.into(),
                }],
            },
            MessageIn {
                role: RoleIn::User,
                content: vec![BlockIn::ToolResult {
                    tool_call_id: "call_1".into(),
                    content: "31C".into(),
                    is_error: false,
                }],
            },
        ];
        let body: Value =
            serde_json::from_slice(&ANTHROPIC.build_messages_body(req(&messages, &[]))).unwrap();
        let out = body["messages"].as_array().unwrap();
        assert_eq!(
            out.len(),
            2,
            "one input message, one output message — no split"
        );
        assert_eq!(out[0]["role"], "assistant");
        assert_eq!(out[0]["content"][0]["type"], "tool_use");
        assert_eq!(out[0]["content"][0]["id"], "call_1");
        assert_eq!(out[0]["content"][0]["name"], "get_weather");
        assert_eq!(
            out[0]["content"][0]["input"],
            serde_json::json!({"city": "Jakarta"}),
            "arguments is a serialized string on the WIT ABI but Anthropic wants the object",
        );
        assert_eq!(out[1]["role"], "user");
        assert_eq!(out[1]["content"][0]["type"], "tool_result");
        assert_eq!(out[1]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(out[1]["content"][0]["content"], "31C");
        assert_eq!(out[1]["content"][0]["is_error"], false);
    }

    #[test]
    fn messages_body_always_carries_max_tokens_defaulting_when_the_abi_omits_it() {
        // Anthropic REJECTS a request without max_tokens, so unlike the
        // OpenAI-format components this field can never be omitted.
        let messages = vec![MessageIn {
            role: RoleIn::User,
            content: vec![BlockIn::Text("hi".into())],
        }];
        let defaulted: Value =
            serde_json::from_slice(&ANTHROPIC.build_messages_body(req(&messages, &[]))).unwrap();
        assert_eq!(defaulted["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(DEFAULT_MAX_TOKENS, 4_096);

        let mut requested = req(&messages, &[]);
        requested.max_tokens = Some(64);
        requested.temperature = Some(0.2);
        let requested_body: Value =
            serde_json::from_slice(&ANTHROPIC.build_messages_body(requested)).unwrap();
        assert_eq!(
            requested_body["max_tokens"], 64,
            "a caller-supplied cap must win over the default",
        );
        // The WIT temperature is an f32, so the JSON number is its widened
        // value — compare within f32 precision rather than bit-exactly.
        assert!((requested_body["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);

        // ...and the default is the config's, not a module-level constant baked
        // into the builder.
        let other: Value =
            serde_json::from_slice(&OTHER.build_messages_body(req(&messages, &[]))).unwrap();
        assert_eq!(other["max_tokens"], 77);
    }

    #[test]
    fn messages_body_drops_a_non_finite_temperature_rather_than_failing() {
        let messages = vec![MessageIn {
            role: RoleIn::User,
            content: vec![BlockIn::Text("hi".into())],
        }];
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut request = req(&messages, &[]);
            request.temperature = Some(bad);
            let body: Value =
                serde_json::from_slice(&ANTHROPIC.build_messages_body(request)).unwrap();
            assert!(
                body.get("temperature").is_none(),
                "a non-finite temperature ({bad}) must be omitted, not serialized",
            );
            assert_eq!(
                body["messages"][0]["content"][0]["text"], "hi",
                "the request still goes"
            );
            assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        }
    }

    #[test]
    fn parse_models_maps_data_entries_preferring_anthropics_display_name() {
        let body = br#"{"data":[
            {"type":"model","id":"claude-opus-4-5","display_name":"Claude Opus 4.5","created_at":"2025-01-01T00:00:00Z"},
            {"type":"model","id":"claude-haiku-4-5"},
            {"type":"model","display_name":"nameless"},
            {"type":"model","id":"claude-sonnet-4-5","display_name":""}
        ],"has_more":false}"#;
        let models = ANTHROPIC.parse_models(body).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["claude-opus-4-5", "claude-haiku-4-5", "claude-sonnet-4-5"],
            "entries without a string id are skipped, order is preserved",
        );
        assert_eq!(
            models[0].display_name, "Claude Opus 4.5",
            "Anthropic's /models DOES carry a display name — use it",
        );
        assert_eq!(
            models[1].display_name, "claude-haiku-4-5",
            "the id stands in when no display name is served",
        );
        assert_eq!(
            models[2].display_name, "claude-sonnet-4-5",
            "an EMPTY display name is not a name",
        );
        for model in &models {
            assert_eq!(model.context_window, DEFAULT_CONTEXT_WINDOW);
        }
    }

    #[test]
    fn parse_models_uses_the_configs_own_window_table() {
        // Same body, different config: the window must come from the config's
        // table/default, never from a value baked into the parser.
        let body = br#"{"data":[{"id":"claude-opus-4-5"},{"id":"claude-haiku-4-5"}]}"#;
        let models = OTHER.parse_models(body).unwrap();
        assert_eq!(
            models.iter().map(|m| m.context_window).collect::<Vec<_>>(),
            vec![200_000, 32_768],
            "a prefix hit takes the table value, a miss the config's default",
        );
    }

    #[test]
    fn parse_models_rejects_a_body_without_a_data_array() {
        assert!(matches!(
            ANTHROPIC.parse_models(b"not json"),
            Err(ProviderFail::Failed(_))
        ));
        assert!(matches!(
            ANTHROPIC.parse_models(br#"{"has_more":false}"#),
            Err(ProviderFail::Failed(_))
        ));
        match OTHER.parse_models(b"{}") {
            Err(ProviderFail::Failed(message)) => assert!(message.contains("Contoso"), "{message}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn parse_message_response_yields_one_terminal_chunk_with_anthropic_usage() {
        let body = br#"{
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [{"type":"text","text":"Hello, world!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 7, "output_tokens": 3}
        }"#;
        let chunks = ANTHROPIC.parse_message_response(body).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hello, world!");
        assert!(chunks[0].tool_calls.is_empty());
        assert!(chunks[0].finished);
        assert_eq!(chunks[0].stop_reason, Some(StopOut::EndTurn));
        assert_eq!(
            chunks[0].usage,
            Some(UsageOut {
                input: 7,
                output: 3
            }),
            "usage is input_tokens/output_tokens, NOT prompt_/completion_tokens",
        );
    }

    #[test]
    fn parse_message_response_reads_tool_use_blocks_and_the_tool_use_stop_reason() {
        let body = br#"{
            "content": [
                {"type":"text","text":"Let me check."},
                {"type":"tool_use","id":"toolu_01","name":"get_weather","input":{"city":"Jakarta"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }"#;
        let chunks = ANTHROPIC.parse_message_response(body).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].finished);
        assert_eq!(chunks[0].stop_reason, Some(StopOut::ToolUse));
        assert_eq!(chunks[0].text, "Let me check.");
        assert_eq!(chunks[0].tool_calls.len(), 1);
        assert_eq!(chunks[0].tool_calls[0].id, "toolu_01");
        assert_eq!(chunks[0].tool_calls[0].name, "get_weather");
        // Anthropic's `input` is a JSON OBJECT; the WIT ABI wants a serialized
        // string, so it must be re-serialized rather than passed through raw.
        let parsed: Value = serde_json::from_str(&chunks[0].tool_calls[0].arguments).unwrap();
        assert_eq!(parsed, serde_json::json!({"city": "Jakarta"}));
    }

    #[test]
    fn a_tool_only_response_with_no_text_block_is_not_an_error() {
        // The PREVIOUS parser required a text block and would have failed every
        // single tool call — the same defect Task C1 fixed for OpenAI-format.
        let chunks = ANTHROPIC
            .parse_message_response(
                br#"{"content":[{"type":"tool_use","id":"t1","name":"n","input":{}}],
                     "stop_reason":"tool_use"}"#,
            )
            .unwrap();
        assert_eq!(chunks[0].text, "");
        assert_eq!(chunks[0].tool_calls.len(), 1);
        assert_eq!(chunks[0].stop_reason, Some(StopOut::ToolUse));
    }

    #[test]
    fn a_length_finish_maps_to_max_tokens_and_an_unknown_one_to_other() {
        let stop = |reason: &str| {
            ANTHROPIC
                .parse_message_response(
                    format!(
                        r#"{{"content":[{{"type":"text","text":"x"}}],"stop_reason":"{reason}"}}"#
                    )
                    .as_bytes(),
                )
                .unwrap()[0]
                .stop_reason
        };
        assert_eq!(stop("max_tokens"), Some(StopOut::MaxTokens));
        assert_eq!(stop("stop_sequence"), Some(StopOut::EndTurn));
        assert_eq!(stop("pause_turn"), Some(StopOut::Other));
        assert_eq!(stop("refusal"), Some(StopOut::Other));
    }

    #[test]
    fn parse_message_response_skips_non_text_blocks_and_never_surfaces_thinking() {
        let body = br#"{
            "content": [
                {"type":"thinking","thinking":"secret chain of thought"},
                {"type":"text","text":"the answer"}
            ],
            "usage": {"input_tokens": 1, "output_tokens": 2}
        }"#;
        let chunks = ANTHROPIC.parse_message_response(body).unwrap();
        assert_eq!(chunks[0].text, "the answer");
        assert!(
            !chunks[0].text.contains("secret chain of thought"),
            "a thinking block must never be rendered as the completion",
        );
    }

    #[test]
    fn parse_message_response_ignores_an_openai_shaped_body() {
        // Guards against the two formats being crossed: an OpenAI chat completion
        // has `choices`, no `content[]`, and must NOT parse here.
        let body = br#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],
                        "usage":{"prompt_tokens":1,"completion_tokens":2}}"#;
        assert!(matches!(
            ANTHROPIC.parse_message_response(body),
            Err(ProviderFail::Failed(_))
        ));
    }

    #[test]
    fn parse_message_response_saturates_a_usage_count_that_exceeds_u32() {
        // The WIT `token-usage` fields are u32 but JSON numbers are u64-wide. An
        // absurd/hostile count must SATURATE, never wrap: a wrapping cast would
        // turn 5_000_000_000 into 705_032_704 and silently under-report spend.
        let body = br#"{
            "content":[{"type":"text","text":"hi"}],
            "usage":{"input_tokens":5000000000,"output_tokens":4294967296}
        }"#;
        let chunks = ANTHROPIC.parse_message_response(body).unwrap();
        assert_eq!(
            chunks[0].usage,
            Some(UsageOut {
                input: u32::MAX,
                output: u32::MAX
            })
        );
    }

    #[test]
    fn parse_message_response_without_usage_still_succeeds() {
        let chunks = ANTHROPIC
            .parse_message_response(br#"{"content":[{"type":"text","text":"hi"}]}"#)
            .unwrap();
        assert_eq!(chunks[0].text, "hi");
        assert!(chunks[0].finished);
        assert_eq!(chunks[0].usage, None);
    }

    #[test]
    fn parse_message_response_rejects_only_a_body_with_no_content_array_at_all() {
        // An EMPTY content array is a well-formed (if unusual) success — no
        // text, no tool calls. A malformed tool_use entry (missing id/name) is
        // silently skipped, not a hard failure. Only a missing `content` key or
        // invalid JSON is a parse error.
        let empty = ANTHROPIC
            .parse_message_response(br#"{"content":[]}"#)
            .unwrap();
        assert_eq!(empty[0].text, "");
        assert!(empty[0].tool_calls.is_empty());

        let malformed_tool_use = ANTHROPIC
            .parse_message_response(br#"{"content":[{"type":"tool_use"}]}"#)
            .unwrap();
        assert!(
            malformed_tool_use[0].tool_calls.is_empty(),
            "a tool_use block with no id/name is skipped, not surfaced as a bogus call",
        );

        assert!(matches!(
            ANTHROPIC.parse_message_response(br#"{"id":"msg_01"}"#),
            Err(ProviderFail::Failed(_))
        ));
        assert!(matches!(
            ANTHROPIC.parse_message_response(b"not json"),
            Err(ProviderFail::Failed(_))
        ));
    }

    #[test]
    fn classify_error_maps_429_to_rate_limited_and_5xx_to_unavailable() {
        assert_eq!(
            ANTHROPIC.classify_error(429, b""),
            ProviderFail::RateLimited
        );
        assert_eq!(
            ANTHROPIC.classify_error(
                429,
                br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#
            ),
            ProviderFail::RateLimited
        );
        for status in [500u16, 502, 503, 529] {
            assert_eq!(
                ANTHROPIC.classify_error(
                    status,
                    br#"{"type":"error","error":{"type":"overloaded_error"}}"#
                ),
                ProviderFail::Unavailable,
                "status {status}",
            );
        }
    }

    #[test]
    fn classify_error_maps_anthropics_not_found_type_to_model_not_found() {
        let body =
            br#"{"type":"error","error":{"type":"not_found_error","message":"model: nope"}}"#;
        assert_eq!(
            ANTHROPIC.classify_error(404, body),
            ProviderFail::ModelNotFound
        );
        // A 404 with some other type stays a plain invalid-request: the router
        // must not persist a bogus "bad model" verdict.
        assert!(matches!(
            ANTHROPIC.classify_error(404, br#"{"type":"error","error":{"type":"unknown_route"}}"#),
            ProviderFail::InvalidRequest(_)
        ));
    }

    #[test]
    fn classify_error_maps_other_4xx_to_invalid_request_naming_the_provider() {
        match ANTHROPIC.classify_error(
            400,
            br#"{"type":"error","error":{"type":"invalid_request_error"}}"#,
        ) {
            ProviderFail::InvalidRequest(message) => {
                assert!(
                    message.contains("400"),
                    "the status must be reported: {message}"
                );
                assert!(message.contains("invalid_request_error"));
                assert!(message.contains("Anthropic"));
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
        // The same status through a different config names THAT provider.
        match OTHER.classify_error(403, b"") {
            ProviderFail::InvalidRequest(message) => {
                assert!(message.contains("Contoso"), "{message}");
                assert!(!message.contains("Anthropic"), "{message}");
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn a_classified_error_never_echoes_the_upstream_message_or_a_credential() {
        // Anthropic's 401 body puts prose in `error.message`, and that prose can
        // quote the submitted key. Nothing from it may reach a guest-visible
        // error string.
        let body = br#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key: sk-ant-api03-LIVEKEY"}}"#;
        let rendered = format!("{:?}", ANTHROPIC.classify_error(401, body));
        assert!(
            !rendered.contains("sk-ant-api03-LIVEKEY"),
            "leaked a credential: {rendered}",
        );
        assert!(
            !rendered.contains("invalid x-api-key"),
            "the upstream message must not be echoed verbatim: {rendered}",
        );
        assert!(
            rendered.contains("authentication_error"),
            "the short type is safe to surface",
        );
    }

    #[test]
    fn error_type_is_extracted_only_from_a_short_safe_field() {
        assert_eq!(
            error_tag(br#"{"type":"error","error":{"type":"overloaded_error"}}"#).as_deref(),
            Some("overloaded_error")
        );
        assert_eq!(
            error_tag(br#"{"type":"error","error":{"message":"boom"}}"#),
            None,
            "the prose message is never a tag",
        );
        assert_eq!(error_tag(b"not json"), None);
        assert_eq!(error_tag(br#"{"error":{}}"#), None);
        assert_eq!(
            error_tag(br#"{"error":{"type":"a b c"}}"#),
            None,
            "a whitespace-bearing tag is prose, not a machine code",
        );
        let long = "x".repeat(MAX_ERROR_TAG_LEN + 1);
        assert_eq!(
            error_tag(format!(r#"{{"error":{{"type":"{long}"}}}}"#).as_bytes()),
            None
        );
        let at_limit = "y".repeat(MAX_ERROR_TAG_LEN);
        assert_eq!(
            error_tag(format!(r#"{{"error":{{"type":"{at_limit}"}}}}"#).as_bytes()).as_deref(),
            Some(at_limit.as_str()),
            "a tag exactly at the limit is still accepted",
        );
    }

    #[test]
    fn success_statuses_are_not_classified_as_errors() {
        assert!(status_is_success(200));
        assert!(status_is_success(299));
        assert!(!status_is_success(300));
        assert!(!status_is_success(199));
    }
}
