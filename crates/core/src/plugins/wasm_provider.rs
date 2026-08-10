//! Bridge a WASM component's `ryuzi:provider/provider` export (an in-process
//! model provider) into the LLM router.
//!
//! # Shape
//! The WIT `provider` interface is deliberately tiny: `list-models` (for
//! discoverability) and `complete` (a single call returning ALL completion
//! chunks as a `list<completion-chunk>`). This module exposes it behind a
//! generic [`WasmProviderRuntime`] trait so the router can dispatch to any
//! installed provider bundle without knowing a plugin id, and a concrete
//! [`WasmProviderTransport`] over the Task-9 callable-component-handle runtime
//! ([`CompiledComponent`]) — each provider owns its own epoch-isolated engine,
//! so a trapping/looping `complete` is caught by the host fuel/epoch budget and
//! surfaces as an `Err`, never a daemon crash.
//!
//! # The router seam (generic, no plugin id)
//! `list-models` results are registered as leaked `&'static ProviderDescriptor`s
//! via `crate::llm_router::registry::register_custom_descriptor` (the same seam
//! user custom providers use) so `route_model` resolves a provider bundle like a
//! built-in. The concrete transports are held in a process-wide registry keyed
//! by provider id ([`register_wasm_provider`]/[`wasm_provider`]); the router's
//! `anthropic_messages_stream` diverts a routed connection to
//! `wasm_provider_stream` iff [`wasm_provider`]`(&target.conn.provider)` finds a
//! transport — a DATA-driven predicate, so the choke point stays generic. Both
//! the descriptor registration and the transport bootstrap are wired by Task 11;
//! this module builds the reusable seam and its behaviour.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use async_trait::async_trait;

use crate::plugins::capabilities::wit_bindings::exports::ryuzi::provider0_1_0::provider as wit;
use crate::plugins::capabilities::wit_bindings::exports::ryuzi::provider0_2_0::provider as wit_v2;
use crate::plugins::capabilities::PluginCapabilityContext;
use crate::plugins::runtime::{CompiledComponent, ComponentInstance, PluginRuntimeError};
use crate::settings::SettingsStore;
use crate::store::Store;
use crate::telemetry::Telemetry;

/// One model a WASM provider advertises (the host-side mirror of the WIT
/// `model-info`). Registered as a `ProviderDescriptor` model by Task 11.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmModelInfo {
    pub id: String,
    pub display_name: String,
    pub context_window: u32,
}

/// A completion request handed to a WASM provider (host-side mirror of the WIT
/// `completion-request`). The router flattens an Anthropic-Messages body into
/// the flat `prompt` this carries.
#[derive(Debug, Clone, PartialEq)]
pub struct WasmCompletionRequest {
    pub model: String,
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// Token usage a completion chunk may report (host-side mirror of the WIT
/// `token-usage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WasmTokenUsage {
    pub input: u32,
    pub output: u32,
}

/// Who authored a message (host-side mirror of the WIT 0.2.0 `role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmRole {
    System,
    User,
    Assistant,
}

/// A tool the agent bound for this turn (mirror of WIT `tool-def`).
/// `input_schema` is a serialized JSON Schema object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: String,
}

/// One model-emitted tool call (mirror of WIT `tool-call`). `arguments` is a
/// serialized JSON object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// The outcome of a previous tool call, replayed on the next turn (mirror of
/// WIT `tool-result`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmToolResult {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
}

/// One piece of a message (mirror of WIT `content-block`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasmContentBlock {
    Text(String),
    ToolUse(WasmToolCall),
    ToolResult(WasmToolResult),
}

/// One turn of the transcript (mirror of WIT `message`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmMessage {
    pub role: WasmRole,
    pub content: Vec<WasmContentBlock>,
}

/// How hard the model should be pushed to call a tool (mirror of WIT
/// `tool-choice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmToolChoice {
    Auto,
    None,
    Required,
}

/// Why the model stopped (mirror of WIT `stop-reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmStopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other,
}

/// What a component can actually do (mirror of WIT `provider-capabilities`).
/// `tools: false` is the correct answer for a 0.1.0-only component and for a
/// 0.2.0 component whose upstream rejects a tools array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WasmProviderCapabilities {
    pub tools: bool,
    pub parallel_tool_calls: bool,
}

/// A structured completion request (mirror of the WIT 0.2.0
/// `completion-request`). Unlike [`WasmCompletionRequest`], this preserves
/// roles, tool calls and tool results instead of flattening them into one
/// string.
#[derive(Debug, Clone, PartialEq)]
pub struct WasmCompletionRequestV2 {
    pub model: String,
    pub messages: Vec<WasmMessage>,
    pub tools: Vec<WasmToolDef>,
    pub tool_choice: WasmToolChoice,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// One completion chunk from a WASM provider (host-side mirror of the WIT
/// `completion-chunk`). `complete` returns these as an ORDERED list; the router
/// presents them as an ordered stream, preserving this order.
#[derive(Debug, Clone, PartialEq)]
pub struct WasmCompletionChunk {
    pub text: String,
    /// Tool calls the model emitted in this chunk. Always empty on the 0.1.0
    /// path, which has no tool channel.
    pub tool_calls: Vec<WasmToolCall>,
    pub finished: bool,
    /// Why the model stopped. `None` on the 0.1.0 path (its ABI cannot say),
    /// where a finished chunk is treated as `end-turn`.
    pub stop_reason: Option<WasmStopReason>,
    pub usage: Option<WasmTokenUsage>,
}

/// The generic provider seam the LLM router dispatches to. Object-safe so a
/// transport can be held as `Arc<dyn WasmProviderRuntime>` in the process-wide
/// registry and looked up by provider id, with no plugin id in the router.
#[async_trait]
pub trait WasmProviderRuntime: Send + Sync {
    /// The provider id this transport is registered under — matches the
    /// `ProviderDescriptor.id`/`ConnectionRow.provider` a route resolves to.
    fn provider_id(&self) -> &str;

    /// The plugin (bundle) id that OWNS this transport — distinct from
    /// `provider_id()`, which is the router-facing alias a bundle declares
    /// (`resolved_provider_ids`, e.g. mimo's bundle registers under
    /// `"mimo-free"`). Callers that need to drop every transport a plugin
    /// owns — uninstall/disable/hot-reload — must key off THIS, never
    /// `provider_id()`, or an aliased bundle's transport survives.
    fn plugin_id(&self) -> &str;

    /// Enumerate the provider's models. A guest `provider-error`, or any
    /// host-side trap/timeout/instantiation failure, becomes an `Err(String)` —
    /// never a panic.
    async fn list_models(&self) -> Result<Vec<WasmModelInfo>, String>;

    /// Run a completion, returning every chunk in order. A guest
    /// `provider-error`, or any host-side trap/timeout, becomes an `Err(String)`
    /// the router converts into a route-scoped failure — never a panic or a hung
    /// daemon.
    async fn complete(
        &self,
        request: WasmCompletionRequest,
    ) -> Result<Vec<WasmCompletionChunk>, String>;

    /// What this transport can actually do. Infallible, side-effect free, and
    /// consulted by the router on EVERY routing decision — so it must never
    /// instantiate the component or touch the network. A 0.1.0-only component
    /// reports `tools: false`.
    fn capabilities(&self) -> WasmProviderCapabilities;

    /// Whether this transport speaks the structured 0.2.0 ABI. Which ABI to
    /// call is decided by THIS, never by `capabilities().tools` — a
    /// 0.2.0-only component may honestly report `tools: false` (mimo does,
    /// because its live probe found no evidence the upstream accepts a tools
    /// array), and calling the 0.1.0 `complete` on a component that never
    /// exports `ryuzi:provider/provider@0.1.0` fails outright instead of
    /// returning a toolless completion. The router must call `complete_v2`
    /// with an empty tools list in that case, never fall back to `complete`.
    fn speaks_structured_abi(&self) -> bool;

    /// Run a structured completion. Called whenever [`Self::speaks_structured_abi`]
    /// is true, with an empty `tools` list and `tool_choice: None` when
    /// [`Self::capabilities`] reports `tools: false`. Same failure contract as
    /// [`WasmProviderRuntime::complete`].
    async fn complete_v2(
        &self,
        request: WasmCompletionRequestV2,
    ) -> Result<Vec<WasmCompletionChunk>, String>;
}

/// A generic provider backed by one enabled component bundle, compiled once and
/// re-instantiated per call (so concurrent completions never share mutable Wasm
/// state), mirroring [`crate::plugins::wasm_connector::WasmActivation`].
pub struct WasmProviderTransport {
    compiled: Arc<CompiledComponent>,
    ctx: Arc<PluginCapabilityContext>,
    provider_id: String,
    /// The guest's own `capabilities()` answer, resolved ONCE by
    /// [`WasmProviderTransport::resolve_capabilities`] at discovery time and
    /// read (never awaited) by the synchronous
    /// [`WasmProviderRuntime::capabilities`] the router consults on every
    /// routing decision.
    capabilities: OnceLock<WasmProviderCapabilities>,
}

impl WasmProviderTransport {
    /// Build a transport for one enabled provider bundle AND resolve its
    /// capabilities before returning it. `compiled` is the validated
    /// component; `ctx` carries the shared settings/store/telemetry backends;
    /// `provider_id` is the id the router resolves connections to.
    ///
    /// This is the ONLY way to obtain a `WasmProviderTransport` outside this
    /// module: the synchronous [`Self::new`] and [`Self::resolve_capabilities`]
    /// are both private, so no caller can construct a transport, skip
    /// resolution, and hand it to [`register_wasm_provider`] — the
    /// resolve-before-register ordering this whole capability-negotiation
    /// design depends on is a type-level invariant, not a convention a caller
    /// (or a future refactor) could get backwards. See
    /// `discovered_v2_bundle_registers_a_tool_capable_transport` for what is
    /// (and, after this change, is NOT) actually tested about that ordering.
    pub(crate) async fn new_resolved(
        compiled: Arc<CompiledComponent>,
        ctx: Arc<PluginCapabilityContext>,
        provider_id: String,
    ) -> Self {
        let transport = Self::new(compiled, ctx, provider_id);
        transport.resolve_capabilities().await;
        transport
    }

    /// Build a transport WITHOUT resolving its capabilities. Private: the
    /// only callers are [`Self::new_resolved`] and, in tests, the
    /// deliberately-unresolved construction path
    /// (`build_test_transport_inner` with `resolve = false`), which exists to
    /// pin [`WasmProviderRuntime::capabilities`]'s fail-closed default. No
    /// other caller may construct an unresolved transport.
    fn new(
        compiled: Arc<CompiledComponent>,
        ctx: Arc<PluginCapabilityContext>,
        provider_id: String,
    ) -> Self {
        WasmProviderTransport {
            compiled,
            ctx,
            provider_id,
            capabilities: OnceLock::new(),
        }
    }

    /// Read the guest's own `capabilities()` once and cache it. Called ONLY
    /// by [`Self::new_resolved`] as part of construction — never call this
    /// directly; a component that traps or times out here is treated as
    /// toolless rather than failing discovery.
    async fn resolve_capabilities(&self) {
        if !self.exports_provider_v2() {
            let _ = self.capabilities.set(WasmProviderCapabilities::default());
            return;
        }
        let resolved = self
            .call_guest_capabilities()
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(
                    plugin = %self.ctx.plugin_id,
                    "wasm provider: capabilities() failed, treating as toolless: {error}"
                );
                WasmProviderCapabilities::default()
            });
        let _ = self.capabilities.set(resolved);
    }

    async fn call_guest_capabilities(&self) -> Result<WasmProviderCapabilities, String> {
        let mut instance = self.instantiate().await.map_err(|e| e.to_string())?;
        let declared = instance
            .call(|inst, store| {
                let pre = inst.instance_pre(&*store);
                let guest = wit_v2::GuestIndices::new(&pre)?.load(&mut *store, &inst)?;
                guest.call_capabilities(&mut *store)
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok(WasmProviderCapabilities {
            tools: declared.tools,
            parallel_tool_calls: declared.parallel_tool_calls,
        })
    }

    /// Whether this component actually exports `ryuzi:provider/provider` — the
    /// caller (Task 11 bootstrap) skips a non-provider bundle before ever
    /// instantiating it (mirrors the connector/hooks IMP-2 gating).
    pub fn exports_provider(&self) -> bool {
        self.compiled.exports_provider()
    }

    /// Whether this component exports the structured 0.2.0 provider interface.
    pub fn exports_provider_v2(&self) -> bool {
        self.compiled.exports_provider_v2()
    }

    async fn instantiate(&self) -> Result<ComponentInstance, PluginRuntimeError> {
        self.compiled.instantiate(self.ctx.clone()).await
    }
}

#[async_trait]
impl WasmProviderRuntime for WasmProviderTransport {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    fn plugin_id(&self) -> &str {
        &self.ctx.plugin_id
    }

    async fn list_models(&self) -> Result<Vec<WasmModelInfo>, String> {
        // Prefer the structured 0.2.0 export when the component has it — a
        // pure-v2 component (no 0.1.0 export at all) must still advertise its
        // models, exactly like `discover_provider_components`'s OR-gate
        // already requires for discovery itself. Fall back to the 0.1.0
        // export, and only error when the component exports neither.
        if self.exports_provider_v2() {
            let mut instance = self.instantiate().await.map_err(|e| e.to_string())?;
            let result = instance
                .call(|inst, store| {
                    let pre = inst.instance_pre(&*store);
                    let guest = wit_v2::GuestIndices::new(&pre)?.load(&mut *store, &inst)?;
                    guest.call_list_models(&mut *store)
                })
                .await
                .map_err(|e| e.to_string())?;
            return match result {
                Ok(models) => Ok(models.into_iter().map(model_from_wit_v2).collect()),
                Err(provider_error) => Err(describe_provider_error_v2(&provider_error)),
            };
        }
        if !self.exports_provider() {
            return Err("component does not export ryuzi:provider/provider".to_string());
        }
        let mut instance = self.instantiate().await.map_err(|e| e.to_string())?;
        let result = instance
            .call(|inst, store| {
                let pre = inst.instance_pre(&*store);
                let guest = wit::GuestIndices::new(&pre)?.load(&mut *store, &inst)?;
                guest.call_list_models(&mut *store)
            })
            .await
            .map_err(|e| e.to_string())?;
        match result {
            Ok(models) => Ok(models.into_iter().map(model_from_wit).collect()),
            Err(provider_error) => Err(describe_provider_error(&provider_error)),
        }
    }

    async fn complete(
        &self,
        request: WasmCompletionRequest,
    ) -> Result<Vec<WasmCompletionChunk>, String> {
        if !self.exports_provider() {
            return Err("component does not export ryuzi:provider/provider".to_string());
        }
        let wit_request = request_to_wit(request);
        let mut instance = self.instantiate().await.map_err(|e| e.to_string())?;
        let result = instance
            .call(move |inst, store| {
                let pre = inst.instance_pre(&*store);
                let guest = wit::GuestIndices::new(&pre)?.load(&mut *store, &inst)?;
                guest.call_complete(&mut *store, &wit_request)
            })
            .await
            .map_err(|e| e.to_string())?;
        match result {
            Ok(chunks) => Ok(chunks.into_iter().map(chunk_from_wit).collect()),
            Err(provider_error) => Err(describe_provider_error(&provider_error)),
        }
    }

    fn capabilities(&self) -> WasmProviderCapabilities {
        // Fail closed: an unresolved transport is toolless, never
        // optimistically tool-capable. Every transport reachable from
        // production went through `WasmProviderTransport::new_resolved` (the
        // only way to build one outside this module), which resolves before
        // returning — so this default is unreachable in production and only
        // guards the deliberately-unresolved transport a test builds via the
        // private synchronous `new`.
        self.capabilities.get().copied().unwrap_or_default()
    }

    fn speaks_structured_abi(&self) -> bool {
        self.exports_provider_v2()
    }

    async fn complete_v2(
        &self,
        request: WasmCompletionRequestV2,
    ) -> Result<Vec<WasmCompletionChunk>, String> {
        if !self.exports_provider_v2() {
            return Err("component does not export ryuzi:provider/provider@0.2.0".to_string());
        }
        let wit_request = request_to_wit_v2(request);
        let mut instance = self.instantiate().await.map_err(|e| e.to_string())?;
        let result = instance
            .call(move |inst, store| {
                let pre = inst.instance_pre(&*store);
                let guest = wit_v2::GuestIndices::new(&pre)?.load(&mut *store, &inst)?;
                guest.call_complete(&mut *store, &wit_request)
            })
            .await
            .map_err(|e| e.to_string())?;
        match result {
            Ok(chunks) => Ok(chunks.into_iter().map(chunk_from_wit_v2).collect()),
            Err(provider_error) => Err(describe_provider_error_v2(&provider_error)),
        }
    }
}

fn model_from_wit(model: wit::ModelInfo) -> WasmModelInfo {
    WasmModelInfo {
        id: model.id,
        display_name: model.display_name,
        context_window: model.context_window,
    }
}

fn model_from_wit_v2(model: wit_v2::ModelInfo) -> WasmModelInfo {
    WasmModelInfo {
        id: model.id,
        display_name: model.display_name,
        context_window: model.context_window,
    }
}

fn chunk_from_wit(chunk: wit::CompletionChunk) -> WasmCompletionChunk {
    WasmCompletionChunk {
        text: chunk.text,
        tool_calls: Vec::new(),
        finished: chunk.finished,
        stop_reason: None,
        usage: chunk.usage.map(|usage| WasmTokenUsage {
            input: usage.input,
            output: usage.output,
        }),
    }
}

fn request_to_wit(request: WasmCompletionRequest) -> wit::CompletionRequest {
    wit::CompletionRequest {
        model: request.model,
        prompt: request.prompt,
        max_tokens: request.max_tokens,
        temperature: request.temperature,
    }
}

/// A human-readable, secret-free rendering of a WIT `provider-error`.
fn describe_provider_error(error: &wit::ProviderError) -> String {
    match error {
        wit::ProviderError::InvalidRequest(message) => {
            format!("invalid provider request: {message}")
        }
        wit::ProviderError::ModelNotFound => "provider model not found".to_string(),
        wit::ProviderError::RateLimited => "provider rate limited".to_string(),
        wit::ProviderError::Unavailable => "provider unavailable".to_string(),
        wit::ProviderError::Failed(message) => format!("provider failed: {message}"),
    }
}

fn role_to_wit_v2(role: WasmRole) -> wit_v2::Role {
    match role {
        WasmRole::System => wit_v2::Role::System,
        WasmRole::User => wit_v2::Role::User,
        WasmRole::Assistant => wit_v2::Role::Assistant,
    }
}

fn tool_call_to_wit_v2(call: WasmToolCall) -> wit_v2::ToolCall {
    wit_v2::ToolCall {
        id: call.id,
        name: call.name,
        arguments: call.arguments,
    }
}

fn tool_call_from_wit_v2(call: wit_v2::ToolCall) -> WasmToolCall {
    WasmToolCall {
        id: call.id,
        name: call.name,
        arguments: call.arguments,
    }
}

fn content_block_to_wit_v2(block: WasmContentBlock) -> wit_v2::ContentBlock {
    match block {
        WasmContentBlock::Text(text) => wit_v2::ContentBlock::Text(text),
        WasmContentBlock::ToolUse(call) => wit_v2::ContentBlock::ToolUse(tool_call_to_wit_v2(call)),
        WasmContentBlock::ToolResult(result) => {
            wit_v2::ContentBlock::ToolResult(wit_v2::ToolResult {
                tool_call_id: result.tool_call_id,
                content: result.content,
                is_error: result.is_error,
            })
        }
    }
}

fn message_to_wit_v2(message: WasmMessage) -> wit_v2::Message {
    wit_v2::Message {
        role: role_to_wit_v2(message.role),
        content: message
            .content
            .into_iter()
            .map(content_block_to_wit_v2)
            .collect(),
    }
}

fn tool_def_to_wit_v2(tool: WasmToolDef) -> wit_v2::ToolDef {
    wit_v2::ToolDef {
        name: tool.name,
        description: tool.description,
        input_schema: tool.input_schema,
    }
}

fn tool_choice_to_wit_v2(choice: WasmToolChoice) -> wit_v2::ToolChoice {
    match choice {
        WasmToolChoice::Auto => wit_v2::ToolChoice::Auto,
        WasmToolChoice::None => wit_v2::ToolChoice::None,
        WasmToolChoice::Required => wit_v2::ToolChoice::Required,
    }
}

fn stop_reason_from_wit_v2(reason: wit_v2::StopReason) -> WasmStopReason {
    match reason {
        wit_v2::StopReason::EndTurn => WasmStopReason::EndTurn,
        wit_v2::StopReason::ToolUse => WasmStopReason::ToolUse,
        wit_v2::StopReason::MaxTokens => WasmStopReason::MaxTokens,
        wit_v2::StopReason::Other => WasmStopReason::Other,
    }
}

fn request_to_wit_v2(request: WasmCompletionRequestV2) -> wit_v2::CompletionRequest {
    wit_v2::CompletionRequest {
        model: request.model,
        messages: request
            .messages
            .into_iter()
            .map(message_to_wit_v2)
            .collect(),
        tools: request.tools.into_iter().map(tool_def_to_wit_v2).collect(),
        tool_choice: tool_choice_to_wit_v2(request.tool_choice),
        max_tokens: request.max_tokens,
        temperature: request.temperature,
    }
}

fn chunk_from_wit_v2(chunk: wit_v2::CompletionChunk) -> WasmCompletionChunk {
    WasmCompletionChunk {
        text: chunk.text,
        tool_calls: chunk
            .tool_calls
            .into_iter()
            .map(tool_call_from_wit_v2)
            .collect(),
        finished: chunk.finished,
        stop_reason: chunk.stop_reason.map(stop_reason_from_wit_v2),
        usage: chunk.usage.map(|usage| WasmTokenUsage {
            input: usage.input,
            output: usage.output,
        }),
    }
}

/// A human-readable, secret-free rendering of a WIT 0.2.0 `provider-error`.
fn describe_provider_error_v2(error: &wit_v2::ProviderError) -> String {
    match error {
        wit_v2::ProviderError::InvalidRequest(message) => {
            format!("invalid provider request: {message}")
        }
        wit_v2::ProviderError::ModelNotFound => "provider model not found".to_string(),
        wit_v2::ProviderError::RateLimited => "provider rate limited".to_string(),
        wit_v2::ProviderError::Unavailable => "provider unavailable".to_string(),
        wit_v2::ProviderError::Failed(message) => format!("provider failed: {message}"),
    }
}

/// Process-wide registry of live WASM provider transports, keyed by provider id.
/// The router's `anthropic_messages_stream` looks a routed connection up here by
/// `target.conn.provider` — a data-driven predicate, so the divert stays generic
/// (no plugin id string). Mirrors the leaked custom-descriptor cache in
/// `llm_router::registry`.
static WASM_PROVIDERS: OnceLock<RwLock<HashMap<String, Arc<dyn WasmProviderRuntime>>>> =
    OnceLock::new();

fn provider_registry() -> &'static RwLock<HashMap<String, Arc<dyn WasmProviderRuntime>>> {
    WASM_PROVIDERS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register (or replace) a live provider transport under its `provider_id`.
pub fn register_wasm_provider(transport: Arc<dyn WasmProviderRuntime>) {
    let id = transport.provider_id().to_string();
    provider_registry()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id, transport);
}

/// Look up a live provider transport by provider id — the router's generic
/// divert predicate. `None` means this connection is not backed by an installed
/// WASM provider bundle (so the generic HTTP path handles it).
pub fn wasm_provider(provider_id: &str) -> Option<Arc<dyn WasmProviderRuntime>> {
    provider_registry()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(provider_id)
        .cloned()
}

/// Drop ONE transport by its router provider id. Production uninstall/disable
/// paths must use [`unregister_wasm_providers_for_plugin`] instead — a bundle's
/// declared `provider-ids` may alias its plugin id (mimo → "mimo-free"), so
/// removing by a single id string misses aliased registrations.
pub fn unregister_wasm_provider(provider_id: &str) {
    provider_registry()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(provider_id);
}

/// Drop EVERY live transport registered by `plugin_id`'s bundle, regardless of
/// which router provider ids it declared (`resolved_provider_ids` may alias —
/// mimo's bundle registers under "mimo-free"). Keyed off the transport's own
/// capability context, so it works even after the bundle is gone from disk.
pub fn unregister_wasm_providers_for_plugin(plugin_id: &str) {
    provider_registry()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|_, transport| transport.plugin_id() != plugin_id);
}

/// Discover every active WASM component bundle under `root`, keep only the
/// ENABLED ones that export `ryuzi:provider/provider`, compile each once, and
/// register a live [`WasmProviderTransport`] into the process-wide
/// [`register_wasm_provider`] registry under EACH router provider id the bundle
/// declares (`PluginManifest::resolved_provider_ids`, which falls back to
/// the bundle id when none are declared) — the provider analogue of
/// [`crate::plugins::wasm_gateway::discover_gateway_components`]. Returns the
/// provider ids registered (for logging / test cleanup); the daemon consumes no
/// value from it, since routing looks transports up out of the shared registry.
///
/// Every failure mode is warn-and-skip (missing root, discovery error,
/// unavailable runtime, per-bundle compile failure, enablement-lookup error),
/// so a broken provider plugin never blocks daemon startup, and a clean install
/// (nothing enabled AND configured that exports a provider) registers
/// nothing — an enabled-but-unconfigured bundle (missing a required secret
/// setting) is skipped the same as a disabled one, not attached and left to
/// fail `start()`. `root` is a
/// parameter (rather than always
/// [`crate::plugins::bundle::installed_bundle_root`]) purely so tests can point
/// discovery at a hermetic install root; production passes the real per-user
/// root.
pub(crate) async fn discover_provider_components(
    store: Arc<Store>,
    settings: &SettingsStore,
    telemetry: Arc<dyn Telemetry>,
    root: &std::path::Path,
) -> Vec<String> {
    use crate::plugins::runtime::{ComponentRuntime, HostPolicy};

    if !root.exists() {
        return Vec::new();
    }
    let bundles = match crate::plugins::bundle::load_active_bundles(root, &store).await {
        Ok(bundles) => bundles,
        Err(error) => {
            tracing::warn!("wasm provider: discovering component bundles failed: {error}");
            return Vec::new();
        }
    };
    if bundles.is_empty() {
        return Vec::new();
    }
    let runtime = match ComponentRuntime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::warn!("wasm provider: component runtime unavailable: {error}");
            return Vec::new();
        }
    };
    let mut registered = Vec::new();
    for bundle in bundles {
        let id = bundle.manifest.id.clone();
        // A declarative bundle (no `[component]`) has no wasm to compile —
        // its `component_path` is a directory placeholder, so compiling it
        // always fails. Skip it rather than letting the attempt fail and
        // log every pass.
        if bundle.manifest.component.is_none() {
            continue;
        }
        match crate::plugins::host::component_plugin_enabled(settings, &id).await {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                tracing::warn!(plugin = %id, "wasm provider: enablement check failed: {error}");
                continue;
            }
        }
        // Enabled is not the same as configured (see `lifecycle.rs`'s
        // session-tools attach for the full rationale) — an enabled bundle
        // whose derived auth still needs a setting is needs-setup, not
        // broken; attaching it would restart-loop `start()`'s `InvalidConfig`
        // forever.
        match crate::plugins::host::component_required_settings_configured(settings, &id).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!(
                    plugin = %id,
                    "wasm provider: skipping {id}: required settings not configured (needs-setup)"
                );
                continue;
            }
            Err(error) => {
                tracing::warn!(plugin = %id, "wasm provider: configured-settings check failed: {error}");
                continue;
            }
        }
        // Task 11 tiered trust: belt-and-braces over the Task 4 signing gate
        // (`HostPolicy::for_installed_bundle`'s `allow_self_auth` derivation)
        // — an unsigned (local-folder/git-URL) component provider must not
        // register a live transport at all until the user has explicitly
        // accepted the trust prompt.
        let provenance = crate::plugins::install_sources::read_install_provenance(&bundle.root);
        if !crate::plugins::host::component_surfaces_trusted_for(settings, &id, &provenance).await {
            tracing::info!(
                plugin = %id,
                "wasm provider: skipping {id}: unsigned component requires explicit trust acceptance"
            );
            continue;
        }
        // Single source of truth for the installed-bundle capability policy
        // (incl. the first-party-only `allow_self_auth` gate that keeps mimo's
        // bootstrap JWT header) — see `HostPolicy::for_installed_bundle`.
        let policy = HostPolicy::for_installed_bundle(&bundle);
        let compiled = match runtime.compile(&bundle, policy) {
            Ok(compiled) => Arc::new(compiled),
            Err(error) => {
                tracing::warn!(plugin = %id, "wasm provider: component compile failed: {error}");
                continue;
            }
        };
        // Only provider bundles are registered; a gateway/connector/hooks-only
        // bundle is skipped before any instantiation (IMP-2). A bundle
        // implementing EITHER the 0.1.0 or the 0.2.0 provider interface (or
        // both) counts — a component that exports only the structured 0.2.0
        // interface is exactly what Task 4's capability negotiation exists to
        // support, so gating on the 0.1.0 export alone would make a pure-v2,
        // tool-capable-only component permanently undiscoverable (proven by
        // `discovered_v2_bundle_registers_a_tool_capable_transport`, which
        // fails without this OR).
        if !compiled.exports_provider() && !compiled.exports_provider_v2() {
            continue;
        }
        let ctx = Arc::new(PluginCapabilityContext {
            plugin_id: id.clone(),
            version: bundle.manifest.version.clone(),
            settings: settings.clone(),
            store: store.clone(),
            telemetry: telemetry.clone(),
            network_allowlist: bundle
                .manifest
                .permissions
                .network
                .iter()
                .map(|entry| entry.0.clone())
                .collect(),
            oauth_profile_ids: bundle
                .manifest
                .oauth
                .iter()
                .map(|profile| profile.id.clone())
                .collect(),
            provider_ids: bundle.manifest.resolved_provider_ids(),
        });
        // One transport per DECLARED router provider id (mimo -> `mimo-free`),
        // all sharing the single compiled component + capability context. The
        // bundle-id -> router-id mapping is data-driven from the manifest, so
        // there is NO plugin-id host branch here.
        for provider_id in bundle.manifest.resolved_provider_ids() {
            // `new_resolved` resolves the guest's capability declaration as
            // part of construction, so there is no separate step here that
            // could be reordered against `register_wasm_provider` —
            // `capabilities()` is synchronous and must never observe an
            // unresolved transport.
            let transport = WasmProviderTransport::new_resolved(
                compiled.clone(),
                ctx.clone(),
                provider_id.clone(),
            )
            .await;
            register_wasm_provider(Arc::new(transport));
            registered.push(provider_id);
        }
    }
    registered
}

/// The prebuilt provider fixture artifact (caller must build fixtures first).
/// Module-level (not inside `mod tests`) so the router-level test in
/// `llm_router::client` can reuse it.
#[cfg(test)]
pub(crate) fn provider_fixture_artifact() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/component-provider/target/wasm32-wasip2/release")
        .join("ryuzi_component_provider_fixture.wasm")
}

/// The prebuilt 0.2.0 provider fixture artifact (caller must build fixtures
/// first). Module-level for the same reason the v1 accessor is.
#[cfg(test)]
pub(crate) fn provider_v2_fixture_artifact() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/component-provider-v2/target/wasm32-wasip2/release")
        .join("ryuzi_component_provider_v2_fixture.wasm")
}

/// The prebuilt 0.2.0-only, `tools: false` provider fixture artifact (caller
/// must build fixtures first). This is the real-compiled-component analog of
/// mimo's shape — exports ONLY `ryuzi:provider/provider@0.2.0` and honestly
/// reports no proven tool support — the exact combination the Critical
/// review finding is about (see the fixture's own doc comment). Module-level
/// for the same reason the other fixture accessors are.
#[cfg(test)]
pub(crate) fn provider_v2_toolless_fixture_artifact() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/component-provider-v2-toolless/target/wasm32-wasip2/release")
        .join("ryuzi_component_provider_v2_toolless_fixture.wasm")
}

/// The prebuilt gateway fixture artifact (caller must build fixtures first via
/// [`crate::plugins::build_fixture_components_once`]) — a real compiled
/// component exporting `ryuzi:gateway/gateway`, not `ryuzi:provider/provider`.
/// Module-level (not inside `mod tests`) so `api::plugins_api`'s test for
/// [`crate::api::plugins_api`]'s `installed_bundle_is_gateway` positive path
/// can reuse it instead of compiling its own throwaway gateway component.
#[cfg(test)]
pub(crate) fn gateway_fixture_artifact() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/component-gateway/target/wasm32-wasip2/release")
        .join("ryuzi_component_gateway_fixture.wasm")
}

/// Lay a verified, active bundle onto `root` in the exact on-disk layout
/// [`crate::plugins::bundle::load_active_bundles`] requires (versioned dir +
/// `current` pointer + `ryuzi-plugin.toml` + `release.json` + the component,
/// hashes all agreeing) and seed the matching active release row into
/// `store`. Signed under the first-party key so
/// `HostPolicy::for_installed_bundle` grants `allow_self_auth`, exactly like
/// the real mimo/opencode bundles.
///
/// Module-level (not inside `mod tests`), same reason as
/// [`gateway_fixture_artifact`]: `api::plugins_api`'s hermetic positive-path
/// test for `installed_bundle_is_gateway` reuses this exact staging logic
/// rather than duplicating it.
#[cfg(test)]
pub(crate) async fn install_bundle_on_disk(
    root: &std::path::Path,
    store: &Store,
    plugin_id: &str,
    component_artifact: &std::path::Path,
    provider_ids: &[&str],
) {
    install_bundle_on_disk_signed(
        root,
        store,
        plugin_id,
        component_artifact,
        provider_ids,
        crate::plugins::first_party_key::FIRST_PARTY_KEY_ID,
    )
    .await
}

/// [`install_bundle_on_disk`] with a caller-chosen `signing_key_id` — used by
/// the Task 11 tiered-trust tests to stage an UNSIGNED (non-first-party)
/// component bundle, the shape `install_sources::confirm_plugin_install`
/// actually produces for a local-folder/git-URL install
/// (`install_sources::UNSIGNED_SIGNING_KEY_ID`).
#[cfg(test)]
pub(crate) async fn install_bundle_on_disk_signed(
    root: &std::path::Path,
    store: &Store,
    plugin_id: &str,
    component_artifact: &std::path::Path,
    provider_ids: &[&str],
    signing_key_id: &str,
) {
    use crate::store::ComponentPluginReleaseRecord;
    use sha2::{Digest, Sha256};

    let version = "0.1.0";
    let component_name = "plugin.wasm";
    let version_dir = root.join(plugin_id).join(version);
    std::fs::create_dir_all(&version_dir).unwrap();
    let bytes = std::fs::read(component_artifact).unwrap();
    std::fs::write(version_dir.join(component_name), &bytes).unwrap();
    let sha = format!("{:x}", Sha256::digest(&bytes));

    let provider_block = if provider_ids.is_empty() {
        // An empty `[provider]` block still exists (so `resolved_provider_ids()`
        // falls back to the manifest id) but declares no explicit ids.
        "\n[provider]\n".to_string()
    } else {
        let quoted = provider_ids
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!("\n[provider]\nids = [{quoted}]\n")
    };
    let manifest = format!(
        "contract = 2\n\
         id = \"{plugin_id}\"\n\
         name = \"{plugin_id}\"\n\
         version = \"{version}\"\n\
         \n\
         [component]\n\
         file = \"{component_name}\"\n\
         wit-api = \"^0.1.0\"\n\
         lifecycle = \"per-call\"\n\
         {provider_block}"
    );
    std::fs::write(version_dir.join("ryuzi-plugin.toml"), manifest).unwrap();

    let release = serde_json::json!({
        "id": plugin_id,
        "version": version,
        "wit-api": "0.1.0",
        "component_url": "https://example.invalid/x.wasm",
        "component_sha256": sha,
    });
    std::fs::write(
        version_dir.join("release.json"),
        serde_json::to_vec(&release).unwrap(),
    )
    .unwrap();
    std::fs::write(root.join(plugin_id).join("current"), version).unwrap();

    let record = ComponentPluginReleaseRecord {
        plugin_id: plugin_id.to_string(),
        version: version.to_string(),
        source_url: "https://example.invalid/x.wasm".to_string(),
        sha256: sha,
        signing_key_id: signing_key_id.to_string(),
        installed_at: 0,
        active: false,
        revoked: false,
        revocation_reason: None,
    };
    store.upsert_component_release(&record).await.unwrap();
    store
        .set_active_component_release(plugin_id, version)
        .await
        .unwrap();
}

/// Like [`install_bundle_on_disk`], but lays a DECLARATIVE, component-less
/// bundle — no `[component]` block and no wasm file at all — the exact
/// shape `atlassian-rovo` ships (a remote-MCP-over-HTTP manifest, see
/// `bundle.rs`'s `manifest_toml_without_component`). `record.sha256`/
/// `source_url` stay empty to match `release.json`'s omitted
/// `component_sha256`/`component_url`, exactly what `load_active_bundles`'s
/// metadata-agreement check requires for a component-less bundle. Signed
/// under the first-party key so discovery reaches the same trust gate a real
/// component-less install would.
///
/// Module-level (not inside `mod tests`) so [`crate::control::lifecycle`]'s
/// `build_component_mcp_servers` tests can stage the identical shape without
/// duplicating this staging logic.
#[cfg(test)]
pub(crate) async fn install_component_less_bundle_on_disk(
    root: &std::path::Path,
    store: &Store,
    plugin_id: &str,
) {
    use crate::store::ComponentPluginReleaseRecord;

    let version = "0.1.0";
    let version_dir = root.join(plugin_id).join(version);
    std::fs::create_dir_all(&version_dir).unwrap();

    let manifest = format!(
        "contract = 2\n\
         id = \"{plugin_id}\"\n\
         name = \"{plugin_id}\"\n\
         version = \"{version}\"\n"
    );
    std::fs::write(version_dir.join("ryuzi-plugin.toml"), manifest).unwrap();

    let release = serde_json::json!({
        "id": plugin_id,
        "version": version,
    });
    std::fs::write(
        version_dir.join("release.json"),
        serde_json::to_vec(&release).unwrap(),
    )
    .unwrap();
    std::fs::write(root.join(plugin_id).join("current"), version).unwrap();

    let record = ComponentPluginReleaseRecord {
        plugin_id: plugin_id.to_string(),
        version: version.to_string(),
        source_url: String::new(),
        sha256: String::new(),
        signing_key_id: crate::plugins::first_party_key::FIRST_PARTY_KEY_ID.to_string(),
        installed_at: 0,
        active: false,
        revoked: false,
        revocation_reason: None,
    };
    store.upsert_component_release(&record).await.unwrap();
    store
        .set_active_component_release(plugin_id, version)
        .await
        .unwrap();
}

/// The extra capability grants + storage seeding
/// [`build_test_transport_with_grants`] layers onto the baseline `deny_all`
/// policy every test transport starts from. Kept as one struct (rather than
/// growing the function's parameter list) so a future grant — e.g. a second
/// per-provider slice needing `ryuzi:oauth` — is one new field, not a new
/// call-site shape.
///
/// [`build_test_transport`] is the zero-grants case (`Self::default()`); the
/// provider-conformance harness
/// (`crate::plugins::wasm_provider_conformance`) drives both the http+storage
/// case (the synthetic fixture) and the http+storage+provider-auth case (the
/// real `openai` component) — all of them call THIS builder so the ~80 lines
/// of bundle/context/policy boilerplate exist exactly once.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestTransportGrants {
    /// Non-empty iff the http capability should be granted: allowlists these
    /// bare hosts (matched on host, not port) in both the manifest's
    /// `permissions.network` declaration (what authorizes the http import in
    /// the component linker) and `PluginCapabilityContext.network_allowlist`
    /// (what the host's `AllowedHttpClient` actually enforces at request
    /// time).
    pub network_allowlist: Vec<String>,
    /// Grants `ryuzi:storage`.
    pub allow_storage: bool,
    /// `(key, value)` pairs seeded into this provider's own storage slice
    /// before the transport is handed back — the generic endpoint-override
    /// channel a provider component reads through `ryuzi:storage` (e.g. the
    /// conformance harness's mock upstream base URL).
    pub storage_seed: Vec<(String, Vec<u8>)>,
    /// Router provider ids the bundle DECLARES (`provider-ids`). Non-empty
    /// alongside a non-empty `network_allowlist` grants
    /// `ryuzi:provider-auth` — the exact gate
    /// [`crate::plugins::runtime::HostPolicy::for_installed_bundle`] applies in
    /// production, mirrored here rather than re-derived, so a test transport
    /// can never be more permissive than a real install.
    pub provider_ids: Vec<String>,
    /// `(provider_id, api_key)` user credentials seeded through the SAME
    /// storage the real router uses (`llm_router::connections`, encrypted at
    /// rest by `llm_router::secrets`), so `ryuzi:provider-auth` resolves a real
    /// key instead of reporting `not-configured`.
    pub provider_credentials: Vec<(String, String)>,
    /// OAuth profile ids the bundle DECLARES (`[[oauth]]`). Non-empty grants
    /// `ryuzi:oauth` — the exact gate
    /// [`crate::plugins::runtime::HostPolicy::for_installed_bundle`] applies in
    /// production (`allow_oauth = !manifest.oauth.is_empty()`), mirrored here so
    /// a test transport can never be more permissive than a real install. Each
    /// id is also declared as an `[[oauth]]` profile on the test bundle
    /// manifest (what the compiled component reads to authorize the profile).
    pub oauth_profile_ids: Vec<String>,
    /// `(profile_id, access_token)` OAuth tokens seeded through the SAME store
    /// the real host reads (`Store::upsert_plugin_oauth_profile_token`, keyed by
    /// this bundle's plugin id), so `ryuzi:oauth`'s `authorized-request`
    /// resolves a real bearer to inject instead of reporting `denied`. The
    /// component never sees this value — the host injects it and returns only the
    /// upstream response, exactly as `capabilities::oauth`'s own tests seed it.
    pub oauth_tokens: Vec<(String, String)>,
}

/// Build a [`WasmProviderTransport`] over a component at `component_path`,
/// keyed by `provider_id`, under a `timeout`, with `grants` layered onto the
/// baseline `HostPolicy::deny_all()`. Returns the store tempfile so it isn't
/// dropped before the transport is used. Shared with the router-level test in
/// `llm_router::client` and the provider-conformance harness, so it lives at
/// module level rather than inside `mod tests`.
///
/// This deliberately builds the policy from `HostPolicy::deny_all()` directly
/// rather than the production `HostPolicy::for_installed_bundle` derivation —
/// every test transport's `release_record.signing_key_id` below is an inert
/// placeholder; `allow_self_auth` stays `false` (from `deny_all()`) no matter
/// what that field says, which is exactly what a strict-Authorization-
/// stripping check depends on.
///
/// Goes through [`WasmProviderTransport::new_resolved`] (via
/// `build_test_transport_inner`'s `resolve = true`), so the returned
/// transport has already resolved its capabilities — every OTHER test in
/// this module relies on that (mirroring production discovery); only
/// [`build_unresolved_test_transport`] opts out.
#[cfg(test)]
pub(crate) async fn build_test_transport_with_grants(
    component_path: std::path::PathBuf,
    provider_id: &str,
    timeout: std::time::Duration,
    grants: TestTransportGrants,
) -> (Arc<WasmProviderTransport>, tempfile::NamedTempFile) {
    build_test_transport_inner(component_path, provider_id, timeout, grants, true).await
}

/// Shared boilerplate behind [`build_test_transport_with_grants`] and
/// [`build_unresolved_test_transport`] — bundle/context/policy construction
/// (~80 lines) is otherwise identical between the two, differing only in
/// whether the returned transport has resolved its capabilities. `resolve =
/// true` goes through [`WasmProviderTransport::new_resolved`] (the
/// production-mirroring path every caller except `build_unresolved_test_transport`
/// needs); `resolve = false` returns the raw, deliberately-unresolved
/// transport via the private synchronous [`WasmProviderTransport::new`].
#[cfg(test)]
async fn build_test_transport_inner(
    component_path: std::path::PathBuf,
    provider_id: &str,
    timeout: std::time::Duration,
    grants: TestTransportGrants,
    resolve: bool,
) -> (Arc<WasmProviderTransport>, tempfile::NamedTempFile) {
    use crate::plugins::bundle::InstalledBundle;
    use crate::plugins::runtime::{ComponentRuntime, HostPolicy};
    use crate::settings::SettingsStore;
    use crate::store::ComponentPluginReleaseRecord;
    use crate::telemetry::NoopTelemetry;
    use ryuzi_plugin_sdk::{
        ComponentSpec, NetworkPermission, OAuthProfile, PluginLifecycle, PluginManifest,
        PluginPermissions, PluginRelease, ProviderSpec,
    };

    let mut policy = HostPolicy::deny_all();
    policy.allow_network = !grants.network_allowlist.is_empty();
    policy.allow_storage = grants.allow_storage;
    // Same conjunction as `HostPolicy::for_installed_bundle`: an explicitly
    // declared provider id AND a declared outbound host.
    policy.allow_provider_auth =
        !grants.provider_ids.is_empty() && !grants.network_allowlist.is_empty();
    // Same gate as `HostPolicy::for_installed_bundle`: a declared `[[oauth]]`
    // profile grants `ryuzi:oauth`.
    policy.allow_oauth = !grants.oauth_profile_ids.is_empty();
    policy.limits.timeout = timeout;

    // Keeps the encrypted-at-rest credential seeding below off the real OS
    // keychain / `secret.key`. Guarded: this mutates the process-global
    // `RYUZI_SECRET_KEY_FILE`, so a transport that seeds NO credential must not
    // reach for it — a zero-grant transport has nothing to encrypt and no
    // business perturbing shared process state for every other test in the
    // binary.
    if !grants.provider_credentials.is_empty() {
        crate::llm_router::secrets::use_test_key_file();
    }
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let store = Arc::new(crate::store::Store::open(tmp.path()).await.unwrap());
    for (key, value) in &grants.storage_seed {
        store
            .put_component_storage(provider_id, key, value)
            .await
            .unwrap();
    }
    for (credential_provider, api_key) in &grants.provider_credentials {
        let now = crate::paths::now_ms();
        crate::llm_router::connections::add_connection(
            &store,
            crate::llm_router::connections::ConnectionRow {
                id: format!("test-conn-{credential_provider}"),
                provider: credential_provider.clone(),
                auth_type: "api_key".to_string(),
                label: credential_provider.clone(),
                priority: 0,
                enabled: true,
                data: crate::llm_router::connections::ConnectionData {
                    api_key: Some(api_key.clone()),
                    ..Default::default()
                },
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
    }
    // Seed OAuth profile tokens exactly the way `capabilities::oauth`'s own
    // tests do, keyed by this bundle's plugin id (== provider_id) and the
    // profile id — so the host resolves a real bearer to inject and the
    // conformance auth-absence check sees the HOST-injected value, not a guest
    // one. A comfortably future expiry keeps `needs_refresh` from reporting the
    // token expired mid-battery.
    for (profile_id, access_token) in &grants.oauth_tokens {
        let now = crate::paths::now_ms();
        store
            .upsert_plugin_oauth_profile_token(
                provider_id,
                profile_id,
                &crate::plugins::oauth::PluginOauthToken {
                    plugin_id: provider_id.to_string(),
                    access_token: access_token.clone(),
                    refresh_token: None,
                    token_type: "Bearer".to_string(),
                    expires_at: Some(now + 3_600_000),
                    scopes: vec![],
                    reconnect_required: false,
                },
            )
            .await
            .unwrap();
    }

    let ctx = Arc::new(PluginCapabilityContext {
        plugin_id: provider_id.to_string(),
        version: "0.1.0".to_string(),
        settings: SettingsStore::new(store.clone()),
        store,
        telemetry: Arc::new(NoopTelemetry),
        network_allowlist: grants.network_allowlist.clone(),
        oauth_profile_ids: grants.oauth_profile_ids.clone(),
        provider_ids: grants.provider_ids.clone(),
    });
    let bundle = InstalledBundle {
        manifest: PluginManifest {
            contract: ryuzi_plugin_sdk::CONTRACT_VERSION,
            id: provider_id.to_string(),
            name: provider_id.to_string(),
            version: "0.1.0".to_string(),
            publisher: String::new(),
            description: String::new(),
            homepage: None,
            icon: None,
            categories: vec![],
            slot: None,
            verified: false,
            experimental: false,
            auth: None,
            settings: vec![],
            component: Some(ComponentSpec {
                file: "plugin.wasm".to_string(),
                wit_api: "^0.1.0".to_string(),
                lifecycle: PluginLifecycle::Singleton,
            }),
            permissions: PluginPermissions {
                network: grants
                    .network_allowlist
                    .iter()
                    .map(|host| NetworkPermission(host.clone()))
                    .collect(),
            },
            // One minimal `[[oauth]]` profile per granted id: the compiled
            // component reads these (`CompiledComponent.oauth_profile_ids`) to
            // authorize the profile the guest passes to `authorized-request`.
            // Only the id matters on the completion path — `authorized-request`
            // resolves the seeded token, not the authorize/token URLs.
            oauth: grants
                .oauth_profile_ids
                .iter()
                .map(|id| OAuthProfile {
                    id: id.clone(),
                    authorize_url: None,
                    token_url: None,
                    device_authorization_url: None,
                    resource: None,
                    scopes: vec![],
                    client_id: None,
                    client_id_setting: None,
                    client_secret_setting: None,
                    dynamic_registration: false,
                    extra_authorize_params: Default::default(),
                })
                .collect(),
            provider: Some(ProviderSpec {
                ids: grants.provider_ids.clone(),
                ..Default::default()
            }),
            tools: vec![],
            mcp: vec![],
            hooks: vec![],
            jobs: vec![],
            gateway: false,
        },
        release: PluginRelease {
            id: provider_id.to_string(),
            version: "0.1.0".to_string(),
            wit_api: "0.1.0".to_string(),
            component_url: "https://example.invalid/x.wasm".to_string(),
            component_sha256: "0".repeat(64),
            size_bytes: None,
            published_at: None,
        },
        // A placeholder, non-first-party signing key id. It plays no part in
        // `allow_self_auth` on this path (see the function doc above) — it
        // only exists to satisfy `InstalledBundle`'s shape.
        release_record: ComponentPluginReleaseRecord {
            plugin_id: provider_id.to_string(),
            version: "0.1.0".to_string(),
            source_url: "https://example.invalid/x.wasm".to_string(),
            sha256: "0".repeat(64),
            signing_key_id: "test".to_string(),
            installed_at: 0,
            active: true,
            revoked: false,
            revocation_reason: None,
        },
        root: component_path.parent().unwrap().to_path_buf(),
        component_path,
    };
    let runtime = ComponentRuntime::new().unwrap();
    let compiled = Arc::new(runtime.compile(&bundle, policy).unwrap());
    // `manifest.version`/`component.wit_api`/`release.wit_api` above are all
    // fixed at `"0.1.0"`/`"^0.1.0"` regardless of which artifact
    // `component_path` actually points at (this helper backs BOTH the v1
    // `provider_fixture_artifact()` and the v2 `provider_v2_fixture_artifact()`
    // callers). That is harmless: export detection
    // (`exports_provider`/`exports_provider_v2`) introspects the COMPILED
    // component's actual WIT exports, never these manifest strings, so a v2
    // fixture staged with v1-looking metadata here still resolves correctly.
    let transport = if resolve {
        // Mirror production discovery: resolve the guest's capability
        // declaration before handing the transport back, so a test never
        // observes the fail-closed default for a real v2 fixture.
        WasmProviderTransport::new_resolved(compiled, ctx, provider_id.to_string()).await
    } else {
        // Deliberately skip resolution — leaving the `capabilities` `OnceLock`
        // unresolved is the entire point of `build_unresolved_test_transport`.
        WasmProviderTransport::new(compiled, ctx, provider_id.to_string())
    };
    (Arc::new(transport), tmp)
}

/// Build a [`WasmProviderTransport`] over the prebuilt provider fixture, keyed
/// by `provider_id`, under a `timeout`, with NO extra capability grants (the
/// baseline case of [`build_test_transport_with_grants`]).
#[cfg(test)]
pub(crate) async fn build_test_transport(
    component_path: std::path::PathBuf,
    provider_id: &str,
    timeout: std::time::Duration,
) -> (Arc<WasmProviderTransport>, tempfile::NamedTempFile) {
    build_test_transport_with_grants(
        component_path,
        provider_id,
        timeout,
        TestTransportGrants::default(),
    )
    .await
}

/// Build a [`WasmProviderTransport`] exactly like the zero-grants case of
/// [`build_test_transport_with_grants`], EXCEPT it deliberately never
/// resolves capabilities — the one test construction path that leaves the
/// transport's `capabilities` `OnceLock` unresolved (via the private
/// synchronous [`WasmProviderTransport::new`], never
/// [`WasmProviderTransport::new_resolved`]), so a test built on it can
/// observe [`WasmProviderRuntime::capabilities`]'s fail-closed
/// `unwrap_or_default()` fallback directly.
///
/// Shares its ~80 lines of bundle/context/policy construction with
/// `build_test_transport_with_grants` via `build_test_transport_inner`
/// (`resolve = false`); every OTHER test in this module goes through the
/// `resolve = true` path (mirroring production discovery), so that behavior
/// is unchanged.
#[cfg(test)]
pub(crate) async fn build_unresolved_test_transport(
    component_path: std::path::PathBuf,
    provider_id: &str,
) -> (Arc<WasmProviderTransport>, tempfile::NamedTempFile) {
    build_test_transport_inner(
        component_path,
        provider_id,
        // Never consulted: `resolve = false` returns before the component is
        // ever instantiated. 30s matches this helper's original bespoke
        // policy (bare `HostPolicy::deny_all()`, whose default
        // `limits.timeout` is also 30s — see `ResourceLimits::default`).
        std::time::Duration::from_secs(30),
        TestTransportGrants::default(),
        false,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::plugins::build_fixture_components_once as build_fixtures;

    fn provider_artifact() -> std::path::PathBuf {
        provider_fixture_artifact()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_v1_fixture_reports_v1_only() {
        build_fixtures();
        let (transport, _tmp) = build_test_transport(
            provider_artifact(),
            "wasm-prov-v1probe",
            Duration::from_secs(10),
        )
        .await;
        assert!(transport.exports_provider());
        assert!(
            !transport.exports_provider_v2(),
            "the 0.1.0 fixture must not be mistaken for a v2 component"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_models_returns_the_static_fixture_model() {
        build_fixtures();
        let (transport, _tmp) = build_test_transport(
            provider_artifact(),
            "wasm-prov-list",
            Duration::from_secs(10),
        )
        .await;
        let models = transport
            .list_models()
            .await
            .expect("list-models must succeed");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "fixture-model");
        assert_eq!(models[0].context_window, 8192);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_models_returns_the_v2_only_fixture_model() {
        // A pure-v2 component (exports ONLY `ryuzi:provider/provider@0.2.0`,
        // no 0.1.0 export at all) must still advertise its models through
        // `list_models`. A regression here — gating `list_models` on the
        // 0.1.0 export alone, as it once did — silently strips EVERY
        // provider of its model list the moment it moves to 0.2.0-only,
        // since `list_models` results are exactly what the router registers
        // as that provider's `ProviderDescriptor` models.
        build_fixtures();
        let (transport, _tmp) = build_test_transport(
            provider_v2_fixture_artifact(),
            "wasm-prov-list-v2",
            Duration::from_secs(10),
        )
        .await;
        assert!(!transport.exports_provider());
        assert!(transport.exports_provider_v2());
        let models = transport.list_models().await.expect(
            "a 0.2.0-only component must still advertise its models via list_models; \
             gating list_models on the 0.1.0 export alone regresses every provider's \
             model list the moment it moves to 0.2.0-only",
        );
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "fixture-model");
        assert_eq!(models[0].context_window, 8192);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn complete_returns_two_chunks_in_order() {
        build_fixtures();
        let (transport, _tmp) =
            build_test_transport(provider_artifact(), "wasm-prov-ok", Duration::from_secs(10))
                .await;
        let chunks = transport
            .complete(WasmCompletionRequest {
                model: "fixture-model".to_string(),
                prompt: "hello".to_string(),
                max_tokens: Some(64),
                temperature: Some(0.2),
            })
            .await
            .expect("complete must succeed");
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["Hello, ", "world!"],
            "chunk order must be preserved"
        );
        assert!(!chunks[0].finished);
        assert!(chunks[1].finished);
        assert_eq!(
            chunks[1].usage,
            Some(WasmTokenUsage {
                input: 7,
                output: 3
            })
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn complete_surfaces_a_provider_error_without_crashing() {
        build_fixtures();
        let (transport, _tmp) = build_test_transport(
            provider_artifact(),
            "wasm-prov-reject",
            Duration::from_secs(10),
        )
        .await;
        let error = transport
            .complete(WasmCompletionRequest {
                model: "fixture-model".to_string(),
                prompt: "please reject".to_string(),
                max_tokens: None,
                temperature: None,
            })
            .await
            .expect_err("a provider-error must surface as Err");
        assert!(
            error.contains("intentional provider failure"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn complete_isolates_a_nonterminating_provider_via_timeout() {
        build_fixtures();
        let (transport, _tmp) = build_test_transport(
            provider_artifact(),
            "wasm-prov-boom",
            Duration::from_millis(200),
        )
        .await;
        let started = std::time::Instant::now();
        let error = transport
            .complete(WasmCompletionRequest {
                model: "fixture-model".to_string(),
                prompt: "make it boom".to_string(),
                max_tokens: None,
                temperature: None,
            })
            .await
            .expect_err("a looping completion must be caught, not hang the host");
        let elapsed = started.elapsed();
        assert!(
            error.contains("timeout") || error.contains("budget"),
            "expected a timeout error, got: {error}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "timeout must fire promptly: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn register_and_lookup_round_trips_by_provider_id() {
        build_fixtures();
        let (transport, _tmp) = build_test_transport(
            provider_artifact(),
            "wasm-prov-registry",
            Duration::from_secs(10),
        )
        .await;
        assert!(wasm_provider("wasm-prov-registry").is_none());
        register_wasm_provider(transport);
        assert!(wasm_provider("wasm-prov-registry").is_some());
        unregister_wasm_provider("wasm-prov-registry");
        assert!(wasm_provider("wasm-prov-registry").is_none());
    }

    // -----------------------------------------------------------------
    // discover_provider_components: production discovery + registration
    // -----------------------------------------------------------------

    use crate::telemetry::NoopTelemetry;

    /// A fresh temp store + a `SettingsStore` over it + a throwaway on-disk
    /// install root, all sharing one lifetime tempfile so nothing is dropped
    /// mid-test.
    async fn discovery_env() -> (
        Arc<Store>,
        SettingsStore,
        tempfile::TempDir,
        tempfile::NamedTempFile,
    ) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(tmp.path()).await.unwrap());
        let settings = SettingsStore::new(store.clone());
        let root = tempfile::tempdir().unwrap();
        (store, settings, root, tmp)
    }

    /// Flip a component plugin's enablement on. Writes the raw
    /// `plugin.<id>.enabled` row `component_plugin_enabled` reads (the schema
    /// `SettingsStore::set` path rejects a key for a plugin not registered in a
    /// `PluginHost`, which these hermetically installed on-disk bundles are not).
    async fn enable(store: &Store, plugin_id: &str) {
        store
            .set_setting_raw(&format!("plugin.{plugin_id}.enabled"), "true")
            .await
            .unwrap();
    }

    // ---------- Task 11: tiered trust gate ----------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsigned_component_without_trust_is_skipped() {
        build_fixtures();
        let (store, settings, root, _tmp) = discovery_env().await;
        install_bundle_on_disk_signed(
            root.path(),
            &store,
            "disc-prov-unsigned",
            &provider_artifact(),
            &["disc-prov-unsigned-served"],
            crate::plugins::install_sources::UNSIGNED_SIGNING_KEY_ID,
        )
        .await;
        // Unsigned provenance, but no trust acceptance stamped/settings —
        // stays untrusted.
        crate::plugins::install_sources::write_install_stamp(
            &root.path().join("disc-prov-unsigned").join("0.1.0"),
            &crate::plugins::host::InstallProvenance::LocalPath,
            0,
        )
        .unwrap();
        enable(&store, "disc-prov-unsigned").await;

        let registered = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;

        assert!(
            registered.is_empty(),
            "an unsigned, untrusted component must not register any transport"
        );
        assert!(wasm_provider("disc-prov-unsigned-served").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsigned_component_with_accepted_trust_registers() {
        build_fixtures();
        let (store, settings, root, _tmp) = discovery_env().await;
        install_bundle_on_disk_signed(
            root.path(),
            &store,
            "disc-prov-trusted",
            &provider_artifact(),
            &["disc-prov-trusted-served"],
            crate::plugins::install_sources::UNSIGNED_SIGNING_KEY_ID,
        )
        .await;
        crate::plugins::install_sources::write_install_stamp(
            &root.path().join("disc-prov-trusted").join("0.1.0"),
            &crate::plugins::host::InstallProvenance::LocalPath,
            0,
        )
        .unwrap();
        enable(&store, "disc-prov-trusted").await;
        store
            .set_setting_raw("plugin.disc-prov-trusted.trusted", "true")
            .await
            .unwrap();

        let registered = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;

        assert_eq!(registered, vec!["disc-prov-trusted-served".to_string()]);
        for id in registered {
            unregister_wasm_provider(&id);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enabled_provider_bundle_registers_under_its_declared_id() {
        build_fixtures();
        let (store, settings, root, _tmp) = discovery_env().await;
        install_bundle_on_disk(
            root.path(),
            &store,
            "disc-prov-enabled",
            &provider_artifact(),
            &["disc-prov-enabled-served"],
        )
        .await;
        enable(&store, "disc-prov-enabled").await;

        let registered = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;

        assert_eq!(registered, vec!["disc-prov-enabled-served".to_string()]);
        let transport = wasm_provider("disc-prov-enabled-served")
            .expect("an enabled provider bundle must register a live transport");
        assert_eq!(transport.provider_id(), "disc-prov-enabled-served");

        for id in registered {
            unregister_wasm_provider(&id);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn declared_provider_ids_are_honored_and_empty_falls_back_to_manifest_id() {
        build_fixtures();
        let (store, settings, root, _tmp) = discovery_env().await;
        // A bundle whose router id (`disc-map-free`) differs from its bundle id
        // (`disc-map`) — the mimo/opencode shape.
        install_bundle_on_disk(
            root.path(),
            &store,
            "disc-map",
            &provider_artifact(),
            &["disc-map-free"],
        )
        .await;
        enable(&store, "disc-map").await;
        // A bundle that declares NO provider-ids: it must fall back to its id.
        install_bundle_on_disk(
            root.path(),
            &store,
            "disc-fallback",
            &provider_artifact(),
            &[],
        )
        .await;
        enable(&store, "disc-fallback").await;

        let registered = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;

        // Registered under the DECLARED router id, never the bundle id.
        assert!(
            wasm_provider("disc-map-free").is_some(),
            "must register under the declared router id",
        );
        assert!(
            wasm_provider("disc-map").is_none(),
            "must NOT register under the bundle id when provider-ids is declared",
        );
        // The no-declaration bundle falls back to its manifest id.
        assert!(
            wasm_provider("disc-fallback").is_some(),
            "an empty provider-ids must fall back to the manifest id",
        );

        for id in registered {
            unregister_wasm_provider(&id);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disabled_provider_bundle_registers_nothing() {
        build_fixtures();
        let (store, settings, root, _tmp) = discovery_env().await;
        // Installed + active but explicitly disabled (PR-2 fix: active bundles
        // default to enabled, so we must explicitly disable to test the disabled path).
        install_bundle_on_disk(
            root.path(),
            &store,
            "disc-disabled",
            &provider_artifact(),
            &["disc-disabled-served"],
        )
        .await;

        // Explicitly disable the bundle.
        store
            .set_setting_raw("plugin.disc-disabled.enabled", "false")
            .await
            .unwrap();

        let registered = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;

        assert!(
            registered.is_empty(),
            "a disabled bundle must register nothing: {registered:?}",
        );
        assert!(wasm_provider("disc-disabled-served").is_none());
        assert!(wasm_provider("disc-disabled").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn non_provider_bundle_registers_nothing() {
        build_fixtures();
        let (store, settings, root, _tmp) = discovery_env().await;
        // A gateway fixture: enabled + compiles, but exports gateway, not
        // provider — the `exports_provider()` gate must skip it.
        install_bundle_on_disk(
            root.path(),
            &store,
            "disc-gateway",
            &gateway_fixture_artifact(),
            &[],
        )
        .await;
        enable(&store, "disc-gateway").await;

        let registered = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;

        assert!(
            registered.is_empty(),
            "a non-provider (gateway) bundle must register nothing: {registered:?}",
        );
        assert!(wasm_provider("disc-gateway").is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn component_less_bundle_registers_nothing() {
        let (store, settings, root, _tmp) = discovery_env().await;
        // A declarative-only bundle (no `[component]` at all — the
        // atlassian-rovo shape): enabled, configured, and trusted, so the
        // ONLY reason discovery could skip it is the missing component
        // itself. Its `component_path` is a directory placeholder (see
        // `bundle.rs`'s `load_active_bundles`), so a naive
        // `runtime.compile` attempt would always fail — discovery must skip
        // it before ever reaching `compile`.
        install_component_less_bundle_on_disk(root.path(), &store, "disc-declarative").await;
        enable(&store, "disc-declarative").await;

        let registered = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;

        assert!(
            registered.is_empty(),
            "a component-less bundle must register nothing: {registered:?}",
        );
        assert!(wasm_provider("disc-declarative").is_none());
    }

    // -----------------------------------------------------------------
    // unregister_wasm_providers_for_plugin: fixes the aliased-id bug where
    // callers unregistered by PLUGIN id even though discovery registers
    // transports under the bundle's DECLARED router provider id(s), which may
    // differ (mimo's bundle registers under "mimo-free", not "mimo").
    // -----------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unregister_for_plugin_removes_aliased_provider_ids() {
        build_fixtures();
        let (store, settings, root, _tmp) = discovery_env().await;
        // "acme" the PLUGIN id declares "acme-free" as its router provider id
        // — the exact mimo shape (bundle id != served provider id).
        install_bundle_on_disk(
            root.path(),
            &store,
            "acme",
            &provider_artifact(),
            &["acme-free"],
        )
        .await;
        enable(&store, "acme").await;

        let registered = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;
        assert_eq!(registered, vec!["acme-free".to_string()]);
        assert!(
            wasm_provider("acme-free").is_some(),
            "discovery must register the transport under the declared router id"
        );

        // Pin the ORIGINAL bug: unregistering by the bare plugin id ("acme")
        // does NOT touch a transport registered under an aliased id
        // ("acme-free") — this is exactly what left mimo's transport alive
        // across disable/uninstall before this fix.
        unregister_wasm_provider("acme");
        assert!(
            wasm_provider("acme-free").is_some(),
            "unregister_wasm_provider keyed by plugin id must NOT remove an \
             aliased router-id transport (this pins the original bug)"
        );

        // The fix: unregister by OWNING PLUGIN id drops every transport that
        // plugin's bundle registered, regardless of declared router alias.
        unregister_wasm_providers_for_plugin("acme");
        assert!(
            wasm_provider("acme-free").is_none(),
            "unregister_wasm_providers_for_plugin must drop the aliased transport"
        );
    }

    // -----------------------------------------------------------------
    // Task 4: transport capability negotiation + complete_v2
    // -----------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_v1_only_component_declares_no_tool_support() {
        build_fixtures();
        let (transport, _tmp) = build_test_transport(
            provider_artifact(),
            "wasm-prov-caps-v1",
            Duration::from_secs(10),
        )
        .await;
        assert_eq!(
            transport.capabilities(),
            WasmProviderCapabilities {
                tools: false,
                parallel_tool_calls: false
            },
            "a 0.1.0 component has no tool channel, so it must never claim one"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_v2_component_declares_tool_support_and_echoes_the_transcript() {
        build_fixtures();
        let (transport, _tmp) = build_test_transport(
            provider_v2_fixture_artifact(),
            "wasm-prov-caps-v2",
            Duration::from_secs(10),
        )
        .await;

        assert!(transport.capabilities().tools);

        let chunks = transport
            .complete_v2(WasmCompletionRequestV2 {
                model: "fixture-model".to_string(),
                messages: vec![WasmMessage {
                    role: WasmRole::User,
                    content: vec![WasmContentBlock::Text("call the tool".to_string())],
                }],
                tools: vec![WasmToolDef {
                    name: "echo".to_string(),
                    description: "echo".to_string(),
                    input_schema: "{}".to_string(),
                }],
                tool_choice: WasmToolChoice::Auto,
                max_tokens: None,
                temperature: None,
            })
            .await
            .expect("complete_v2 must succeed");

        let last = chunks.last().expect("at least one chunk");
        assert!(last.finished);
        assert_eq!(last.stop_reason, Some(WasmStopReason::ToolUse));
        assert_eq!(last.tool_calls.len(), 1);
        assert_eq!(last.tool_calls[0].name, "echo");
    }

    /// Pins the fail-closed default itself: a freshly constructed transport
    /// whose `capabilities` `OnceLock` has NEVER been resolved must report
    /// toolless, not optimistically tool-capable. Every other test in this
    /// module goes through `build_test_transport`/`build_test_transport_with_grants`,
    /// which resolve immediately after construction — so without this test,
    /// the `unwrap_or_default()` fallback in `WasmProviderRuntime::capabilities`
    /// is dead code as far as the suite is concerned. If that fallback were
    /// ever changed to default to tool-capable (or dropped in favor of an
    /// `.unwrap()`), this is the only test that would catch it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capabilities_reports_toolless_before_resolve_capabilities_runs() {
        build_fixtures();
        let (transport, _tmp) =
            build_unresolved_test_transport(provider_v2_fixture_artifact(), "wasm-prov-unresolved")
                .await;
        assert_eq!(
            transport.capabilities(),
            WasmProviderCapabilities {
                tools: false,
                parallel_tool_calls: false
            },
            "an unresolved transport must fail CLOSED (report toolless) even though the \
             underlying component is the tool-capable v2 fixture — capabilities() must never \
             read as tool-capable before resolve_capabilities() has actually run, or the \
             router could route tool calls into a component that never declared support"
        );
    }

    /// Critical review fix: a REAL compiled component shaped exactly like
    /// mimo — exports ONLY `ryuzi:provider/provider@0.2.0`, no 0.1.0 export
    /// at all, and honestly reports `tools: false` — must still answer a
    /// `complete_v2` call successfully, with no tool calls (never a fabricated
    /// one). Before the router fix, this exact combination was invisible to
    /// every test in the branch and fell into the buggy `else` branch of
    /// `wasm_provider_stream`, which calls `complete` — an interface this
    /// component doesn't export at all, so the guard at the top of `complete`
    /// below would reject it outright. This test proves the CORRECT path
    /// (`complete_v2`, which this component does export) actually works.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_v2_only_toolless_component_completes_successfully_via_complete_v2() {
        build_fixtures();
        let (transport, _tmp) = build_test_transport(
            provider_v2_toolless_fixture_artifact(),
            "wasm-prov-v2-toolless",
            Duration::from_secs(10),
        )
        .await;
        assert!(
            transport.exports_provider_v2(),
            "the toolless v2 fixture must still export the structured 0.2.0 interface"
        );
        assert!(
            !transport.exports_provider(),
            "the toolless v2 fixture must not export the 0.1.0 interface — mirroring mimo, \
             which has no 0.1.0 fallback at all"
        );
        assert!(
            !transport.capabilities().tools,
            "the fixture must honestly report tools: false, mirroring mimo's live-probed \
             capability declaration"
        );
        assert!(
            transport.speaks_structured_abi(),
            "speaks_structured_abi must read the export, not capabilities().tools — this is \
             the exact predicate that decides which ABI the router calls"
        );

        // Driven with a tools-bearing request, exactly like a real turn the
        // router would (after the fix) still send through complete_v2 with an
        // empty tools list — proving the completion succeeds and carries no
        // tool calls, never that it silently fails or fabricates a call.
        let chunks = transport
            .complete_v2(WasmCompletionRequestV2 {
                model: "fixture-model".to_string(),
                messages: vec![WasmMessage {
                    role: WasmRole::User,
                    content: vec![WasmContentBlock::Text("hello".to_string())],
                }],
                tools: Vec::new(),
                tool_choice: WasmToolChoice::None,
                max_tokens: None,
                temperature: None,
            })
            .await
            .expect("complete_v2 must succeed against a real 0.2.0-only, tools:false component");

        assert!(
            chunks.iter().all(|chunk| chunk.tool_calls.is_empty()),
            "a toolless component must never surface a tool call: {chunks:?}"
        );
        let last = chunks.last().expect("at least one chunk");
        assert!(last.finished);
        assert_eq!(last.stop_reason, Some(WasmStopReason::EndTurn));
    }

    /// The mirror image of the test above: calling the 0.1.0 `complete`
    /// against this same component — the path the router's pre-fix
    /// `capabilities().tools`-keyed dispatch would have taken — fails,
    /// because a 0.2.0-only component genuinely does not export that
    /// interface. This is the failure `wasm_provider_stream`'s Critical bug
    /// produced for every `mimo` turn; pinning it here documents exactly
    /// what going through the wrong branch costs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_v2_only_toolless_component_rejects_the_flat_0_1_0_complete() {
        build_fixtures();
        let (transport, _tmp) = build_test_transport(
            provider_v2_toolless_fixture_artifact(),
            "wasm-prov-v2-toolless-reject",
            Duration::from_secs(10),
        )
        .await;
        let error = transport
            .complete(WasmCompletionRequest {
                model: "fixture-model".to_string(),
                prompt: "hello".to_string(),
                max_tokens: None,
                temperature: None,
            })
            .await
            .expect_err(
                "a 0.2.0-only component must reject the 0.1.0 complete — this is exactly the \
                 outright failure the Critical routing bug caused for every mimo turn",
            );
        assert!(
            error.contains("does not export ryuzi:provider/provider"),
            "unexpected error: {error}"
        );
    }

    /// Drives the REAL `discover_provider_components` discovery path (not the
    /// test helpers' own construction) for a bundle that exports ONLY the
    /// 0.2.0 provider interface: installs the v2 fixture on disk, runs
    /// discovery, then reads the resulting transport back out of the
    /// process-wide registry and asserts it reports `tools: true`. This pins
    /// two things: (1) the OR-gate in `discover_provider_components` — a
    /// pure-v2 bundle with no 0.1.0 export at all is still discovered and
    /// registered, not silently skipped; (2) that the transport discovery
    /// registers has ACTUALLY resolved its capabilities from the real
    /// component, rather than sitting at the fail-closed default.
    ///
    /// What this test does NOT pin: resolve-before-register ORDERING.
    /// `discover_provider_components` awaits `WasmProviderTransport::new_resolved`
    /// to completion before this function is ever called, so the read below
    /// would observe the exact same result regardless of whether resolution
    /// happened "before" or "after" registration inside that already-awaited
    /// call — a semantically equivalent reorder (e.g. constructing with the
    /// bare `new`, calling `register_wasm_provider`, then awaiting
    /// `resolve_capabilities()`) would pass this test unchanged if such a
    /// reorder could even be written. It can't: `new_resolved` is the ONLY
    /// way to obtain a `WasmProviderTransport` outside this module, and it
    /// resolves capabilities as an inseparable part of construction — see
    /// its doc comment. The ordering guarantee is now a type-level invariant
    /// enforced by that API shape, not something this (or any) test can
    /// verify by inspecting timing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovered_v2_bundle_registers_a_tool_capable_transport() {
        build_fixtures();
        let (store, settings, root, _tmp) = discovery_env().await;
        install_bundle_on_disk(
            root.path(),
            &store,
            "disc-prov-v2",
            &provider_v2_fixture_artifact(),
            &["disc-prov-v2-served"],
        )
        .await;
        enable(&store, "disc-prov-v2").await;

        let registered = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;

        assert_eq!(registered, vec!["disc-prov-v2-served".to_string()]);
        let transport = wasm_provider("disc-prov-v2-served")
            .expect("a discovered v2 provider bundle must register a live transport");
        assert!(
            transport.capabilities().tools,
            "discover_provider_components must register a transport whose capabilities were \
             actually resolved from the real v2-only component (via \
             WasmProviderTransport::new_resolved), not one left at the capabilities() \
             fail-closed default — otherwise the router would silently drop this component's \
             tools even though the OR-gate in discover_provider_components did discover and \
             register it"
        );

        unregister_wasm_provider("disc-prov-v2-served");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unregister_for_plugin_then_rediscovery_with_bundle_disabled_leaves_registry_empty() {
        build_fixtures();
        let (store, settings, root, _tmp) = discovery_env().await;
        install_bundle_on_disk(
            root.path(),
            &store,
            "acme2",
            &provider_artifact(),
            &["acme2-free"],
        )
        .await;
        enable(&store, "acme2").await;

        let registered = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;
        assert_eq!(registered, vec!["acme2-free".to_string()]);
        assert!(wasm_provider("acme2-free").is_some());

        // Simulate the fail-closed hot-reload sequence Fix 3 wires around
        // install/rollback: drop every transport this plugin owns BEFORE
        // re-running discovery. With the bundle now disabled (the
        // uninstall/disable path this pins), re-discovery must not resurrect
        // the transport — the registry stays empty for this plugin.
        unregister_wasm_providers_for_plugin("acme2");
        store
            .set_setting_raw("plugin.acme2.enabled", "false")
            .await
            .unwrap();
        let registered_again = super::discover_provider_components(
            store.clone(),
            &settings,
            Arc::new(NoopTelemetry),
            root.path(),
        )
        .await;
        assert!(
            registered_again.is_empty(),
            "a disabled bundle must not re-register after unregister: {registered_again:?}"
        );
        assert!(wasm_provider("acme2-free").is_none());
    }
}
