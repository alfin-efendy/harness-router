//! Shared request/response shapes for the RPC command families under
//! `api/`. Populated by each command family as it moves its DTOs and
//! private helpers out of the src-tauri `commands.rs` module (see the Move
//! Recipe) — bindings-stable, so every serde/specta attribute here must stay
//! byte-identical to the source it was moved from.

use crate::domain::SessionGitOptions;
use crate::harness::native::commands::{ProjectCommandInput, ProjectCommandRead};
use crate::llm_router::model_effort::{
    EffectiveEffortSource, SelectableModelInfo, StoredEffortStatus,
};
use crate::llm_router::quota::ProviderQuotaCapability;
use crate::llm_router::secrets::KeychainStatus;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChatContextArg {
    pub branch: Option<String>,
    pub voice_transcript: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
}

/// Per-start git controls from the composer (behavior matrix, workstream B).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GitOptions {
    pub use_worktree: bool,
    pub create_branch: bool,
    pub branch_name: Option<String>,
    pub base_branch: Option<String>,
}

impl From<GitOptions> for SessionGitOptions {
    fn from(g: GitOptions) -> Self {
        let clean = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        SessionGitOptions {
            use_worktree: g.use_worktree,
            create_branch: g.create_branch,
            branch_name: clean(g.branch_name),
            base_branch: clean(g.base_branch),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequestOptions {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub context: Option<ChatContextArg>,
    #[serde(default)]
    pub attachments: Vec<String>,
    /// None => engine default (worktree ON, new engine-named branch from HEAD).
    pub git: Option<GitOptions>,
    /// Initial permission mode for a legacy/composer session.
    pub perm_mode: Option<crate::domain::PermMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentMention {
    pub agent_id: String,
    pub label_snapshot: String,
    pub start_utf16: u32,
    pub end_utf16: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TurnInput {
    pub text: String,
    #[serde(default)]
    pub mentions: Vec<AgentMention>,
    pub context: Option<ChatContextArg>,
    #[serde(default)]
    pub attachments: Vec<String>,
    pub git: Option<GitOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessageInfo {
    pub id: String,
    pub text: String,
}

pub(crate) fn chat_agent_prompt(prompt: &str, context: Option<&ChatContextArg>) -> String {
    let Some(context) = context else {
        return prompt.to_string();
    };
    let mut lines = Vec::new();
    if let Some(branch) = context
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        lines.push(format!("- Branch: {branch}"));
    }
    if let Some(voice) = context
        .voice_transcript
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        lines.push(format!("- Voice transcript: {voice}"));
    }
    for reference in context
        .references
        .iter()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        lines.push(format!("- Referenced file: {reference}"));
    }
    if lines.is_empty() {
        prompt.to_string()
    } else if prompt.trim().is_empty() {
        format!("[Chat context]\n{}", lines.join("\n"))
    } else {
        format!("{prompt}\n\n[Chat context]\n{}", lines.join("\n"))
    }
}

pub(crate) fn content_type_for_path(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "txt" | "md" | "rs" | "ts" | "tsx" | "js" | "jsx" | "json" | "toml" | "yaml" | "yml" => {
            Some("text/plain".to_string())
        }
        "png" => Some("image/png".to_string()),
        "jpg" | "jpeg" => Some("image/jpeg".to_string()),
        "gif" => Some("image/gif".to_string()),
        "pdf" => Some("application/pdf".to_string()),
        "zip" => Some("application/zip".to_string()),
        "webp" => Some("image/webp".to_string()),
        "mp4" => Some("video/mp4".to_string()),
        "webm" => Some("video/webm".to_string()),
        "mov" => Some("video/quicktime".to_string()),
        "mkv" => Some("video/x-matroska".to_string()),
        "mp3" => Some("audio/mpeg".to_string()),
        "wav" => Some("audio/wav".to_string()),
        "ogg" => Some("audio/ogg".to_string()),
        "m4a" => Some("audio/mp4".to_string()),
        "flac" => Some("audio/flac".to_string()),
        _ => None,
    }
}

/// Keep only the final path segment and strip characters unsafe in a file name.
pub(crate) fn sanitize_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("file");
    let cleaned: String = base
        .chars()
        .filter(|c| !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed.to_string()
    }
}

// --- scheduler_api (moved verbatim from apps/cockpit/src-tauri/src/scheduler_cmd.rs) ---

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunInfo {
    pub id: String,
    pub status: String,
    pub started_at_ms: i64,
    pub duration_ms: Option<i64>,
    pub add_lines: Option<i64>,
    pub del_lines: Option<i64>,
    pub note: Option<String>,
    pub error: Option<String>,
    pub session_pk: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub id: String,
    pub name: String,
    pub cron: String,
    pub mode: String,
    pub natural: String,
    pub project_id: String,
    pub project_name: String,
    pub branch: String,
    pub gateway: String,
    pub enabled: bool,
    pub prompt: String,
    pub notify_success: bool,
    pub notify_fail: bool,
    pub next_run_ms: Option<i64>,
    pub history: Vec<RunInfo>,
    /// Model id this job's session starts with, overriding the project/agent
    /// default. `None` when the job uses ordinary model resolution. Not yet
    /// editable from the scheduler panel — set programmatically today (e.g.
    /// by a future `app_jobs` tool); surfaced here so a later job editor can
    /// read and round-trip it without another DTO change.
    #[serde(default)]
    pub model_override: Option<String>,
    /// The plugin that installed this job, if any — mirrors
    /// `crate::scheduler::JobRow.plugin_id`. `None` for a user-created job.
    /// Task 12 addition: lets the scheduler screen distinguish plugin-owned
    /// rows from user-created ones.
    #[serde(default)]
    pub plugin_id: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobInput {
    pub name: String,
    pub mode: String,
    pub natural: String,
    pub cron: String,
    pub project_id: String,
    pub branch: String,
    pub gateway: String,
    pub prompt: String,
    pub notify_success: bool,
    pub notify_fail: bool,
    /// See `JobInfo::model_override`.
    #[serde(default)]
    pub model_override: Option<String>,
}

// --- automation_api (Hook persistence contract; RPC wiring follows in Task 5) ---

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAgentRunActionInput {
    pub project_id: String,
    pub branch: String,
    pub gateway_id: String,
    pub prompt: String,
    pub agent_id: Option<String>,
    pub model_override: Option<String>,
    pub subtask: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationWebhookHeaderInput {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationWebhookOutboundActionInput {
    pub url: String,
    pub method: String,
    #[serde(default)]
    pub headers: Vec<AutomationWebhookHeaderInput>,
    pub payload_template: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", content = "config", deny_unknown_fields)]
pub enum AutomationActionInput {
    #[serde(rename = "agent.run")]
    AgentRun(AutomationAgentRunActionInput),
    #[serde(rename = "webhook.outbound")]
    WebhookOutbound(AutomationWebhookOutboundActionInput),
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationHookInput {
    pub name: String,
    pub trigger_kind: crate::automation::TriggerKind,
    pub action: AutomationActionInput,
    #[serde(default = "automation_enabled_by_default")]
    pub enabled: bool,
}

const fn automation_enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutomationHookInfo {
    pub id: String,
    pub name: String,
    pub trigger_kind: crate::automation::TriggerKind,
    pub action_kind: crate::automation::ActionKind,
    pub enabled: bool,
    pub inbound_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// The plugin that installed this hook, if any — mirrors
    /// `crate::automation::HookRow.plugin_id`. `None` for a user-created
    /// hook. Task 12 addition: lets the Automations screen distinguish
    /// plugin-owned rows from user-created ones.
    pub plugin_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutomationHookRunInfo {
    pub id: String,
    pub hook_id: String,
    pub status: String,
    pub session_pk: Option<String>,
    pub error: Option<String>,
    pub attempt_count: i64,
    pub last_http_status: Option<i64>,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub attempts: Vec<AutomationHookAttemptInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutomationHookAttemptInfo {
    pub run_id: String,
    pub ordinal: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub http_status: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutomationWebhookHeaderInfo {
    pub name: String,
    pub configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutomationWebhookOutboundActionInfo {
    pub url: String,
    pub method: String,
    pub headers: Vec<AutomationWebhookHeaderInfo>,
    pub payload_template: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", content = "config")]
pub enum AutomationActionInfo {
    #[serde(rename = "agent.run")]
    AgentRun(AutomationAgentRunActionInput),
    #[serde(rename = "webhook.outbound")]
    WebhookOutbound(AutomationWebhookOutboundActionInfo),
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutomationHookDetail {
    pub hook: AutomationHookInfo,
    pub action: AutomationActionInfo,
    pub runs: Vec<AutomationHookRunInfo>,
}

impl From<AutomationAgentRunActionInput> for crate::automation::AgentRunAction {
    fn from(value: AutomationAgentRunActionInput) -> Self {
        Self {
            project_id: value.project_id,
            branch: value.branch,
            gateway_id: value.gateway_id,
            prompt: value.prompt,
            agent_id: value.agent_id,
            model_override: value.model_override,
            subtask: value.subtask,
        }
    }
}

impl From<AutomationWebhookHeaderInput> for crate::automation::WebhookHeader {
    fn from(value: AutomationWebhookHeaderInput) -> Self {
        Self {
            name: value.name,
            value: value.value,
        }
    }
}

impl From<AutomationWebhookOutboundActionInput> for crate::automation::WebhookOutboundAction {
    fn from(value: AutomationWebhookOutboundActionInput) -> Self {
        Self {
            url: value.url,
            method: value.method,
            headers: value.headers.into_iter().map(Into::into).collect(),
            payload_template: value.payload_template,
        }
    }
}

impl From<AutomationActionInput> for crate::automation::HookActionInput {
    fn from(value: AutomationActionInput) -> Self {
        match value {
            AutomationActionInput::AgentRun(config) => Self::AgentRun(config.into()),
            AutomationActionInput::WebhookOutbound(config) => Self::WebhookOutbound(config.into()),
        }
    }
}

impl From<AutomationHookInput> for crate::automation::HookInput {
    fn from(value: AutomationHookInput) -> Self {
        Self {
            name: value.name,
            trigger_kind: value.trigger_kind,
            action: value.action.into(),
            enabled: value.enabled,
        }
    }
}

impl From<crate::automation::WebhookHeader> for AutomationWebhookHeaderInfo {
    fn from(value: crate::automation::WebhookHeader) -> Self {
        Self {
            name: value.name,
            configured: true,
        }
    }
}

impl From<crate::automation::HookActionInput> for AutomationActionInfo {
    fn from(value: crate::automation::HookActionInput) -> Self {
        match value {
            crate::automation::HookActionInput::AgentRun(config) => {
                Self::AgentRun(AutomationAgentRunActionInput {
                    project_id: config.project_id,
                    branch: config.branch,
                    gateway_id: config.gateway_id,
                    prompt: config.prompt,
                    agent_id: config.agent_id,
                    model_override: config.model_override,
                    subtask: config.subtask,
                })
            }
            crate::automation::HookActionInput::WebhookOutbound(config) => {
                Self::WebhookOutbound(AutomationWebhookOutboundActionInfo {
                    url: config.url,
                    method: config.method,
                    headers: config.headers.into_iter().map(Into::into).collect(),
                    payload_template: config.payload_template,
                })
            }
        }
    }
}

impl From<crate::automation::HookAttemptRow> for AutomationHookAttemptInfo {
    fn from(value: crate::automation::HookAttemptRow) -> Self {
        Self {
            run_id: value.run_id,
            ordinal: value.ordinal,
            started_at: value.started_at,
            finished_at: value.finished_at,
            http_status: value.http_status,
            error: value.error,
        }
    }
}

impl From<crate::automation::HookRunRow> for AutomationHookRunInfo {
    fn from(value: crate::automation::HookRunRow) -> Self {
        Self {
            id: value.id,
            hook_id: value.hook_id,
            status: value.status,
            session_pk: value.session_pk,
            error: value.error,
            attempt_count: value.attempt_count,
            last_http_status: value.last_http_status,
            queued_at: value.queued_at,
            started_at: value.started_at,
            finished_at: value.finished_at,
            attempts: value
                .attempts
                .into_iter()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(Into::into)
                .collect(),
        }
    }
}

impl From<crate::automation::HookDetail> for AutomationHookDetail {
    fn from(value: crate::automation::HookDetail) -> Self {
        let action = value.hook.action.clone().into();
        Self {
            hook: value.hook.into(),
            action,
            runs: value.runs.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<crate::automation::HookRow> for AutomationHookInfo {
    fn from(value: crate::automation::HookRow) -> Self {
        Self {
            id: value.id,
            name: value.name,
            trigger_kind: value.trigger_kind,
            action_kind: value.action_kind,
            enabled: value.enabled,
            inbound_path: value.inbound_path,
            created_at: value.created_at,
            updated_at: value.updated_at,
            plugin_id: value.plugin_id,
        }
    }
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GatewayResourceInfo {
    pub label: String,
    pub sub: String,
    pub pct: u32,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GatewayInfo {
    pub id: String,
    pub name: String,
    pub badge: String,
    /// local | wsl | ssh
    pub kind: String,
    pub detail: String,
    pub meta_line: String,
    /// connected | offline
    pub status: String,
    pub latency: Option<String>,
    pub daemon_version: String,
    pub uptime: Option<String>,
    pub last_seen_ms: Option<i64>,
    pub resources: Vec<GatewayResourceInfo>,
    pub fingerprint: Option<String>,
    pub fs_mode: String,
    pub paths: Vec<String>,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GatewayEventInfo {
    pub at: i64,
    pub level: String,
    pub text: String,
}

// --- apps_api (moved verbatim from apps/cockpit/src-tauri/src/apps_cmd.rs) ---

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    pub desc: String,
    pub perm: String,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentAccessInfo {
    pub agent_id: String,
    pub allowed: bool,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub initial: String,
    pub color: String,
    pub desc: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub scope: String,
    pub scope_gateways: Vec<String>,
    pub status: String,
    pub status_detail: Option<String>,
    pub version: Option<String>,
    pub publisher: Option<String>,
    pub auth_kind: String,
    pub auth_detail: Option<String>,
    /// Whether THIS host owns the server's credential, and therefore whether
    /// offering an OAuth "Connect" for it tells the truth. `false` for every
    /// stdio row, and for an http row whose spec already carries an
    /// `Authorization` header (a manifest `${setting:…}` API token, or an
    /// owning plugin's own OAuth bearer): for those, `harness::native`'s
    /// `connect_mcp_tools` uses the manifest credential VERBATIM and never
    /// reads, uses or refreshes a token connected here.
    ///
    /// Derived by applying `harness::native::mcp_http_credential` — the same
    /// predicate the session path branches on — to the spec
    /// `mcp::servers_for_session` would attach, never re-derived from
    /// `transport`/`auth_kind`. Cockpit MUST gate the whole OAuth row on this
    /// rather than on `transport == "http"`: that comparison merely correlates
    /// with credential ownership, and where it diverged
    /// (`plugins/atlassian-rovo`, which authenticates with
    /// `Authorization: Basic ${setting:…}`) the card claimed "Not connected",
    /// walked the user through a real Atlassian consent screen, flipped to
    /// "OAuth connected" — and the session kept sending the Basic header.
    /// Mirrors `PluginAuthInfo.oauth_connect_available`, which models the same
    /// thing for a plugin.
    pub oauth_connect_available: bool,
    /// A `mcp_oauth_tokens` row exists for this server's id — independent of
    /// `auth_kind`/`auth_detail` (those describe the manifest/env-derived
    /// credential, never an interactively-connected OAuth token). Only ever
    /// true for a `transport: "http"` row.
    pub oauth_token_stored: bool,
    /// The stored token's `reconnect_required` flag (Task 8: set when a
    /// refreshed request still 401s). `false` whenever `oauth_token_stored`
    /// is `false` — there is nothing to reconnect.
    pub oauth_reconnect_required: bool,
    /// Why this server's last OAuth connect attempt failed, if one did —
    /// cleared when a connect starts and when one succeeds. Mirrors
    /// `PluginAuthInfo.oauth_connect_error`, which models the same thing for
    /// a plugin.
    ///
    /// This exists because the token exchange runs in a BACKGROUND task that
    /// no user-initiated RPC is awaiting: Cockpit's loopback listener
    /// captures the browser redirect and calls `complete_mcp_connect` from a
    /// spawned task whose only error path was an `eprintln!`. Cockpit's card
    /// polls `list_apps` until a five-minute deadline and then says the
    /// sign-in link expired — so a connect refused in the first second
    /// (a token endpoint the binding gate would not accept, an authorization
    /// server returning 400) was indistinguishable from a user who wandered
    /// off, and the real reason reached nobody. Persisting it here is what
    /// lets the poll stop early and say what actually happened.
    pub oauth_connect_error: Option<String>,
    pub tools: Vec<ToolInfo>,
    pub agent_access: Vec<AgentAccessInfo>,
    /// The plugin that owns this server, when it was synced from a plugin's
    /// `[[mcp]]` declaration rather than added by the user. Cockpit uses it
    /// to badge the row and to warn before removing a plugin-managed app —
    /// deleting one only makes it reappear on the plugin's next sync.
    pub plugin_id: Option<String>,
}

/// `begin_mcp_connect` RPC result — the daemon has already discovered the
/// remote server's authorization server, registered (or reused) a client id,
/// and built the authorize URL. Cockpit opens `authorize_url` in the browser
/// and holds `state`/`verifier` locally until its loopback callback captures
/// the redirect (see `mcp_oauth::mcp_redirect_uri` and the Task 9 plan
/// correction on why the callback listener lives in Cockpit's own process,
/// not the daemon's).
///
/// `issuer_token_endpoint` and `client_id` are the token endpoint and client
/// id of the authorization server this flow actually selected — carried
/// forward from `harness::native::mcp_oauth::McpAuthorizeStart` so the caller
/// can hand them straight back to `complete_mcp_connect` instead of
/// rediscovering them (which could resolve a different authorization server
/// than the one that issued the code).
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectStart {
    pub authorize_url: String,
    pub state: String,
    pub verifier: String,
    pub issuer_token_endpoint: String,
    pub client_id: String,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AddAppInput {
    pub id: Option<String>,
    pub name: String,
    pub description: String,
    pub kind: Option<String>,
    /// stdio | http
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    /// KEY=VALUE pairs.
    pub env: Vec<String>,
    pub url: Option<String>,
    pub version: Option<String>,
    pub publisher: Option<String>,
    pub color: Option<String>,
}

// --- native_api (moved verbatim from apps/cockpit/src-tauri/src/native_cmd.rs) ---

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub mode: String,
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CommandOriginInfo {
    Builtin,
    Global,
    Project,
    Plugin,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SlashKindInfo {
    Command,
    Skill,
}

/// One "/" autocomplete entry: a slash command or a user-invocable skill.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SlashEntryInfo {
    pub name: String,
    pub description: String,
    pub kind: SlashKindInfo,
    pub origin: CommandOriginInfo,
    pub home: bool,
    pub session: bool,
    pub requires_project: bool,
    pub effective: bool,
    pub shadows_global: bool,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subtask: bool,
}

/// Editable fields for a global slash command. The command name is
/// supplied separately for updates so a save cannot rename a file by accident.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandFileMutationDto {
    pub description: String,
    pub template: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub subtask: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandFileInputDto {
    pub name: String,
    #[serde(flatten)]
    pub command: CommandFileMutationDto,
}

impl CommandFileMutationDto {
    pub fn with_name(self, name: &str) -> ProjectCommandInput {
        ProjectCommandInput {
            name: name.to_string(),
            description: self.description,
            template: self.template,
            agent: self.agent,
            model: self.model,
            subtask: self.subtask,
        }
    }
}

impl From<CommandFileInputDto> for ProjectCommandInput {
    fn from(value: CommandFileInputDto) -> Self {
        value.command.with_name(&value.name)
    }
}

/// A command file and the revision that must accompany update or delete.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CommandFileInfo {
    pub name: String,
    pub description: String,
    pub template: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subtask: bool,
    pub revision: String,
}

impl From<ProjectCommandRead> for CommandFileInfo {
    fn from(value: ProjectCommandRead) -> Self {
        Self {
            name: value.name,
            description: value.description,
            template: value.template,
            agent: value.agent,
            model: value.model,
            subtask: value.subtask,
            revision: value.revision,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub content: String,
    pub status: String,
}

// --- agent_api (moved verbatim from apps/cockpit/src-tauri/src/agent_cmd.rs) ---

// --- endpoint_api (moved verbatim from apps/cockpit/src-tauri/src/endpoint_cmd.rs) ---
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EndpointStatusInfo {
    pub running: bool,
    pub port: u16,
    pub base_url: String,
    pub autostart: bool,
    pub keychain_status: KeychainStatus,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EndpointKeyInfo {
    pub id: String,
    pub name: String,
    pub key: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UsagePoint {
    pub day: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UsageSeries {
    pub days: Vec<UsagePoint>,
    pub today_requests: i64,
    pub today_input_tokens: i64,
    pub today_output_tokens: i64,
}

// --- connections_api (moved verbatim from apps/cockpit/src-tauri/src/connections_cmd.rs) ---

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub id: String,
    pub provider: String,
    pub provider_name: String,
    pub color: String,
    pub initial: String,
    pub auth_type: String,
    pub label: String,
    pub priority: i32,
    pub enabled: bool,
    pub quota_capability: Option<ProviderQuotaCapability>,
    pub models: Vec<String>,
    /// OAuth connections only: true once refresh has failed terminally and
    /// the user needs to reconnect via the browser/paste flow again.
    pub needs_relogin: bool,
    /// True for the built-in free-tier connections (`mimo-free`/`opencode-free`):
    /// always present (re-seeded at startup), hidden from account management,
    /// not deletable (spec A2).
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionRuntimeInfo {
    pub session_pk: String,
    pub model: Option<String>,
    pub stored_effort: Option<String>,
    pub effective_effort: Option<String>,
    pub effective_effort_label: Option<String>,
    pub effective_source: EffectiveEffortSource,
    pub stored_effort_status: StoredEffortStatus,
    pub model_info: Option<SelectableModelInfo>,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    /// Legacy pass/fail, kept for existing call sites (connection-level
    /// test, toasts). Always derived: `status == "valid"`.
    pub ok: bool,
    /// Tri-state probe verdict: "valid" | "invalid" | "unknown".
    pub status: String,
    pub message: String,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RefreshModelsResult {
    pub connection_id: String,
    pub label: String,
    pub ok: bool,
    pub message: String,
}

/// One persisted probe verdict row for the provider Models card.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatusInfo {
    pub model: String,
    pub status: String,
    pub message: String,
    pub tested_at: i64,
}

/// One persisted probe verdict row across ALL families — hydrates the
/// app-wide model-status store consumed by every model picker.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatusEntry {
    pub family: String,
    pub model: String,
    pub status: String,
    pub message: String,
    pub tested_at: i64,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ManualStartInfo {
    pub authorize_url: String,
    pub verifier: String,
    pub state: String,
    pub redirect_uri: String,
}

/// Device-code flow info shown to the user while they complete the browser
/// step (Kiro): the short code to enter, the URL to visit, and the poll
/// cadence the frontend's `await_kiro_device_flow` call will honor.
// `Deserialize` (not just `Serialize`) is required: the engine serializes
// this as an RPC result, and Cockpit's `EngineClient` deserializes it back
// client-side to read `verification_uri_complete` before opening the
// system browser. A plain `//` comment (not `///`) so it isn't captured
// into the generated TS binding's doc comment.
#[derive(Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFlowInfo {
    pub flow_id: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: i64,
    pub interval: i64,
}

// --- plugins_api (moved verbatim from apps/cockpit/src-tauri/src/plugins_cmd.rs) ---

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub categories: Vec<String>,
    /// The exclusive capability slot this plugin's manifest claims (e.g.
    /// `"memory"`), mirroring `ryuzi_plugin_sdk::PluginManifest::slot`.
    /// `None` when the manifest declares no slot.
    pub slot: Option<String>,
    /// Whether this plugin currently WON its `slot` claim
    /// (first-registration-wins — see `crate::plugins::PluginHost::
    /// slot_owner`). Always `false` when `slot` is `None`. A plugin whose
    /// claim lost still has `slot` set (its own manifest is unaffected) but
    /// `owns_slot: false`; see `plugin_doctor`'s `"slot-conflict"` finding
    /// for the observable signal naming both the winner and the loser.
    pub owns_slot: bool,
    pub verified: bool,
    pub experimental: bool,
    pub enabled: bool,
    /// Same semantics as `PluginAuthInfo.configured` (oauth: token stored &&
    /// !reconnect_required; else a persisted `auth.setting` row or `auth.env`
    /// set). `false` when the manifest declares no `[auth]` block. On the
    /// LIST payload (not just `plugin_detail`) because the Browse grid's
    /// Install/Open split needs it — note this adds per-plugin store lookups
    /// to list assembly.
    pub configured: bool,
    /// `builtin` | `catalog` | `skill-pack`.
    pub source: String,
    /// Any of `provider` | `runtime` | `gateway` | `connector`.
    pub capabilities: Vec<String>,
    /// `integration` | `provider` | `gateway` | `skill-pack`. There is no
    /// `runtime` kind: the native agent is built-in engine behavior, not an
    /// installable/listed plugin, so it never appears in this payload.
    pub kind: String,
    /// Kind-specific "already set up" flag: integration = configured ||
    /// enabled; provider = ≥1 connection in the provider's family; gateway =
    /// all manifest settings present; skill-pack = installed on disk.
    pub installed: bool,
    /// Provider family head id (providers only) — the Models `providerDetail`
    /// navigation target. `None` for other kinds.
    pub family: Option<String>,
    /// Mirrors `crate::store::PluginInstallRecord.pinned` — `false` when the
    /// plugin has no `plugin_installs` ledger row (never installed via the
    /// tracked git-clone path, e.g. builtins/catalog integrations with no
    /// skill-pack install).
    pub pinned: bool,
    /// The ledger row's git origin (`PluginInstallRecord.source_spec`).
    /// Distinct from `source` (the stable builtin/catalog/skill-pack enum
    /// label) — the Provenance card in Cockpit renders it only when present.
    pub source_spec: Option<String>,
    pub resolved_commit: Option<String>,
    pub installed_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub trust_tier: Option<String>,
    /// This id ships as a first-party WASM component bundle
    /// (`plugins::component_catalog::is_component_bundle`). True regardless of
    /// which registration won the id — a provider bundle is represented by its
    /// builtin row but is still component-backed — so Cockpit can offer
    /// release management (install / active version / rollback) for it.
    pub component_backed: bool,
    /// The remote catalog feed's `version` for this id, when a cached
    /// `plugin_catalog_cache` row matches. `None` when the id was never seen
    /// in a fetched feed.
    pub catalog_version: Option<String>,
    /// Set when the remote catalog's signed feed blocked (revoked) this id —
    /// mirrors `RemoteCatalogRow.blocked_reason`. `None` when not blocked.
    pub blocked_reason: Option<String>,
    /// Single-source daemon-computed health/setup status (spec §6):
    /// `ok | disabled | needs-setup | attach-failed | update-available |
    /// blocked | not-installed`. Derived by `derive_plugin_status` from this
    /// row's own `installed`/`enabled`/`configured`/`blocked_reason` plus the
    /// last recorded attach outcome and whether a newer catalog version is
    /// available. (`unchecked` is MCP-app-only and mapped frontend-side, never
    /// emitted here.)
    pub status: String,
    /// Secret-free human-readable detail for `status` — currently populated
    /// for `attach-failed` (the recorded attach reason) and `needs-setup`
    /// (a generic "authentication not configured" message). `None` for every
    /// other status.
    pub status_detail: Option<String>,
    /// Coarse auth requirement for this row: `none` | `token` | `oauth` —
    /// collapses the SDK's 4-way `AuthKind` (`api-key`/`token` both become
    /// `"token"`) since only "is a credential required at all" matters for
    /// `derive_plugin_status`'s needs-setup gate.
    pub auth_kind: String,
    /// Declared tool count for component-backed rows (the embedded bundle
    /// manifest's `tools.len()`, Task 1) — `None` for non-component rows and
    /// for component-backed ids with no embedded manifest here (e.g. a
    /// provider bundle represented by its builtin row).
    pub tool_count: Option<u32>,
    /// Installed skill count for `skill-pack` rows (`InstalledSkillInfo.
    /// skill_count`) — `None` for every other kind, and for a synthesized
    /// curated pack not yet installed.
    pub skill_count: Option<u32>,
    /// Which v2 surfaces this plugin's manifest actually provides. Members
    /// are exactly `"provider" | "tools" | "mcp" | "skills" | "commands" |
    /// "hooks" | "jobs"`, always emitted in that stable order. Derived
    /// straight off the manifest (`provider.is_some()`, `!tools.is_empty()`,
    /// `!mcp.is_empty()`, `!hooks.is_empty()`, `!jobs.is_empty()`) except for
    /// `skills`/`commands`, which check whether an INSTALLED plugin's
    /// `skills/`/`commands/` directories actually contain content (a
    /// `Builtin` plugin — no directory of its own — never reports either).
    /// `gateway` is deliberately never a member: it is an internal-only
    /// surface, first-party-gated structurally (see
    /// `crate::plugins::runtime`'s `HostPolicy::allow_gateway`), never a
    /// public capability a plugin author or Cockpit's UI should see listed.
    pub surfaces: Vec<String>,
    /// How this plugin arrived on disk — `"catalog"` (signed feed,
    /// trusted-by-construction) | `"local-path"` | `"git"` (Task 11's
    /// unsigned tiered-trust installs), mirroring
    /// `crate::plugins::host::InstallProvenance`. `None` for a `Builtin`
    /// plugin (first-party native or embedded catalog — no install
    /// provenance to report).
    pub provenance: Option<String>,
    /// `true` when this plugin's unsigned `[[mcp]]`/`[component]` surfaces
    /// were explicitly trust-accepted (`plugin.<id>.trusted == "true"`) OR
    /// the plugin's provenance is trusted by construction (`Catalog` or
    /// `Builtin`) — see `crate::plugins::host::component_surfaces_trusted`,
    /// the single gate every consuming surface checks. Cockpit uses this to
    /// show whether an unsigned plugin's riskier surfaces are actually live.
    pub trusted: bool,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuthInfo {
    /// `none` | `api-key` | `token` | `oauth`.
    pub kind: String,
    pub setting: Option<String>,
    pub env: Option<String>,
    pub help_url: Option<String>,
    /// A persisted (non-empty) row exists for `setting`, OR `env` is set in
    /// the process environment. Never reveals the value itself.
    pub configured: bool,
    pub oauth_connect_available: bool,
    pub oauth_connect_error: Option<String>,
    pub oauth_token_stored: bool,
    pub oauth_reconnect_required: bool,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginOauthBeginResult {
    pub state_token: String,
    pub authorize_url: String,
    pub redirect_uri: String,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallBeginResult {
    /// `none` | `api-key` | `token` | `oauth`.
    pub auth_kind: String,
    /// `auth.env` is declared AND set in the environment.
    pub env_var_present: bool,
    pub env_var_name: Option<String>,
    /// Endpoints + client id resolved; the browser flow started.
    pub oauth_available: bool,
    /// OAuth brokered outside Cockpit (kind=oauth, no `auth.resource`, no
    /// manifest `authorize_url` — google-workspace).
    pub oauth_external: bool,
    /// oauth, endpoints may be known, but no client id and DCR not
    /// applicable / failed.
    pub needs_client_id: bool,
    /// This call performed a successful registration.
    pub dcr_succeeded: bool,
    /// `auto` (callback server bound) | `manual` (bind failed → paste).
    pub callback_mode: String,
    pub oauth_begin: Option<PluginOauthBeginResult>,
    /// Discovery/DCR failure detail (shown on the manual client id form).
    pub dcr_error: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginFieldInfo {
    pub key: String,
    pub label: String,
    pub help: String,
    pub secret: bool,
    pub required: bool,
    /// A persisted (non-empty) row exists for `key`. Never the value itself.
    pub value_set: bool,
    /// `string` | `int` | `bool` — the value shape Cockpit renders (see
    /// `ryuzi_plugin_sdk::FieldKind`). A plain camelCase-friendly `String`
    /// mirror rather than the SDK enum itself, matching this module's
    /// existing convention (`auth_kind_label`/`mcp_transport_label`) of
    /// never crossing specta's `Type` boundary with an SDK type directly.
    pub kind: String,
    /// Non-empty makes this field an enum/choice — the value must be one of
    /// these members (see `ryuzi_plugin_sdk::SettingField::options`).
    pub options: Vec<String>,
    /// Pre-filled/effective value to show when `value_set` is `false`. Safe
    /// to return even for a `secret` field: it comes from the manifest, not
    /// a persisted credential.
    pub default: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginMcpInfo {
    pub name: String,
    /// `stdio` | `http`.
    pub transport: String,
    /// The raw manifest string (command for stdio, url for http) — no
    /// `${auth}` substitution, matching `ryuzi plugins info`'s output.
    pub command_or_url: String,
}

/// One `[[hooks]]` entry a v2 plugin manifest declares, synced into
/// `automation_hooks` (`crate::plugins::automation_sync::sync_plugin_automations`)
/// and re-read back here as a first-class `HookRow`. Task 12 addition to
/// `PluginDetail` — lets the plugin detail view show a plugin's own
/// automations without a separate round trip through the Automations screen.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginHookInfo {
    /// The stored row's stable id (`automation_hooks.id`, a generated UUID —
    /// distinct from `name`). Task 14 addition: the Automations tab's enable
    /// switch calls `toggle_automation_hook`, which is keyed by this `id`,
    /// not `name` — without it Cockpit had no way to toggle a plugin's own
    /// hook row from its detail page.
    pub id: String,
    /// The stored row name: `"<plugin-id>/<name>"`.
    pub name: String,
    /// Canonical dotted trigger (`crate::automation::TriggerKind::as_str`).
    pub trigger: String,
    /// The Claude Code alias spelling for `trigger`, when one exists (e.g.
    /// `"Stop"` for `"session.end"`) — see `crate::automation::claude_alias_for`.
    pub trigger_alias: Option<String>,
    /// `"agent.run"` | `"webhook.outbound"`.
    pub action: String,
    pub enabled: bool,
    /// `true` for an `agent.run` hook with an empty `project_id` — the sync
    /// convention every plugin-installed `agent.run` hook starts with (no
    /// plugin can guess the user's project). `PluginHost`'s enable guard
    /// refuses enabling a hook while this is `true`; Cockpit uses it to open
    /// a target editor instead of a plain enable switch.
    pub needs_target: bool,
}

/// One `[[jobs]]` entry a v2 plugin manifest declares, synced into `jobs`
/// (same module as [`PluginHookInfo`]) and re-read back as a `JobRow`.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginJobInfo {
    /// The stored row id: `"<plugin-id>/<name>"`.
    pub id: String,
    pub name: String,
    /// Natural-language schedule text when the row's `mode == "natural"`,
    /// else the resolved cron expression — whichever is the human-readable
    /// form for this row.
    pub schedule: String,
    pub enabled: bool,
    /// `true` for a job with an empty `project_id` — same "no plugin can
    /// guess the user's project" convention as [`PluginHookInfo::needs_target`].
    pub needs_target: bool,
}

#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginDetail {
    pub info: PluginInfo,
    pub auth: Option<PluginAuthInfo>,
    pub settings: Vec<PluginFieldInfo>,
    pub mcp: Vec<PluginMcpInfo>,
    pub models: Vec<String>,
    pub homepage: Option<String>,
    pub publisher: String,
    /// Command names (`.md` file stems) found under a plugin's `commands/`
    /// directory — an `Installed` plugin's own directory, or (F7) a
    /// `Builtin` plugin's directory when it's ALSO backed by an active,
    /// installed component bundle (the component-catalog placeholders:
    /// github/atlassian/bitbucket/discord/mimo/opencode). Empty for a
    /// `Builtin` plugin with no matching installed bundle, or any plugin
    /// with no commands surface.
    pub commands: Vec<String>,
    /// Skill directory names found under a plugin's `skills/` directory
    /// (each carries a `SKILL.md`) — same resolution as `commands` above,
    /// including the F7 `Builtin`-with-an-active-bundle case.
    pub skills: Vec<String>,
    pub hooks: Vec<PluginHookInfo>,
    pub jobs: Vec<PluginJobInfo>,
}

// --- Skill/plugin distribution DTOs (trust prompt, update, doctor) ---
//
// The DTOs below are thin camelCase mirrors of
// `crate::skills_install`'s `TrustPrompt`/`UpdateOutcome`/`BeginInstall` and
// `crate::plugins::doctor::DoctorFinding` — those core types derive
// `Serialize`/`Deserialize` but not specta's `Type`, so they cannot cross the
// Tauri IPC boundary directly (same rationale as `PluginInfo` mirroring
// `ryuzi_plugin_sdk::PluginManifest`). None of these add or drop any field
// relative to the core type, and `TrustPrompt` is already secret-free by
// construction (repo path, skill names, hook-script paths, byte count — no
// credential material).

/// Mirror of `crate::skills_install::TrustPrompt`. `total_bytes` stays a
/// `u64` (not narrowed to `u32`) to avoid silently truncating a large pack's
/// byte count — `export_bindings`'s `BigIntExportBehavior::Number` already
/// renders any bigint-sized field as a plain TS `number`, so there's no
/// bindings-shape cost to keeping the wider type.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TrustPromptDto {
    pub token: String,
    pub source_spec: String,
    pub owner_repo: String,
    pub resolved_commit: Option<String>,
    pub skills: Vec<String>,
    pub hook_scripts: Vec<String>,
    pub total_bytes: u64,
    /// Mirrors `TrustPrompt::curated`: true when the source is one of the
    /// curated skill packs — surfaced so the wizard can distinguish "this
    /// prompt exists because the source is arbitrary/unvetted" from a
    /// curated source that still stops here for some other reason.
    pub curated: bool,
}

impl From<crate::skills_install::TrustPrompt> for TrustPromptDto {
    fn from(p: crate::skills_install::TrustPrompt) -> Self {
        TrustPromptDto {
            token: p.token,
            source_spec: p.source_spec,
            owner_repo: p.owner_repo,
            resolved_commit: p.resolved_commit,
            skills: p.skills,
            hook_scripts: p.hook_scripts,
            total_bytes: p.total_bytes,
            curated: p.curated,
        }
    }
}

/// Mirror of `crate::skills_install::BeginInstall`, flattened into a single
/// `{completed, trust?, plugin?}` shape the wizard can branch on without a
/// tagged-union match in TS. `trust` is set for `NeedsConfirmation`, `plugin`
/// for `Completed` — exactly one is ever `Some`.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallBegin {
    pub completed: bool,
    pub trust: Option<TrustPromptDto>,
    pub plugin: Option<crate::skills_install::InstalledSkillPack>,
}

impl From<crate::skills_install::BeginInstall> for SkillInstallBegin {
    fn from(result: crate::skills_install::BeginInstall) -> Self {
        match result {
            crate::skills_install::BeginInstall::Completed(pack) => SkillInstallBegin {
                completed: true,
                trust: None,
                plugin: Some(pack),
            },
            crate::skills_install::BeginInstall::NeedsConfirmation(prompt) => SkillInstallBegin {
                completed: false,
                trust: Some(TrustPromptDto::from(prompt)),
                plugin: None,
            },
        }
    }
}

/// Mirror of `crate::skills_install::UpdateOutcome`. Keeps the same
/// `#[serde(tag = "kind", content = "detail")]` shape so the discriminated
/// union round-trips identically to the core enum.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase", tag = "kind", content = "detail")]
pub enum UpdateOutcomeDto {
    Updated,
    AlreadyCurrent,
    SkippedPinned,
    LocalEdits,
    Failed(String),
    NeedsReack(TrustPromptDto),
}

impl From<crate::skills_install::UpdateOutcome> for UpdateOutcomeDto {
    fn from(outcome: crate::skills_install::UpdateOutcome) -> Self {
        use crate::skills_install::UpdateOutcome;
        match outcome {
            UpdateOutcome::Updated => UpdateOutcomeDto::Updated,
            UpdateOutcome::AlreadyCurrent => UpdateOutcomeDto::AlreadyCurrent,
            UpdateOutcome::SkippedPinned => UpdateOutcomeDto::SkippedPinned,
            UpdateOutcome::LocalEdits => UpdateOutcomeDto::LocalEdits,
            UpdateOutcome::Failed(message) => UpdateOutcomeDto::Failed(message),
            UpdateOutcome::NeedsReack(prompt) => {
                UpdateOutcomeDto::NeedsReack(TrustPromptDto::from(prompt))
            }
        }
    }
}

/// One pack's outcome from `update_all_plugins` —
/// `crate::skills_install::update_all_packs` returns
/// `Vec<(String, UpdateOutcome)>`; specta can't name a bare tuple usefully in
/// the generated TS, so this wraps it in a named struct.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutcomeEntry {
    pub id: String,
    pub outcome: UpdateOutcomeDto,
}

// --- Task 11/12: install a plugin from a local folder or a git URL ---
//
// Thin camelCase mirrors of `crate::plugins::install_sources`'s
// `PluginTrustPrompt` (+ its nested summary shapes) — same rationale as
// `TrustPromptDto` above: the core types stay snake-case-agnostic
// (`Serialize`/`Deserialize` only, no specta `Type`) since Task 11's RPC
// layer passes them wire-side as-is; Task 12 owns the Cockpit-facing
// camelCase translation.

/// Mirror of `crate::plugins::install_sources::McpServerSummary`.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginSourceMcpServerInfo {
    pub name: String,
    /// `stdio` | `http`.
    pub transport: String,
    /// stdio: `"<command> <args...>"`; http: the URL.
    pub detail: String,
}

impl From<crate::plugins::install_sources::McpServerSummary> for PluginSourceMcpServerInfo {
    fn from(value: crate::plugins::install_sources::McpServerSummary) -> Self {
        Self {
            name: value.name,
            transport: value.transport,
            detail: value.detail,
        }
    }
}

/// Mirror of `crate::plugins::install_sources::ComponentToolSummary`.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginSourceComponentToolInfo {
    pub name: String,
    pub writes: bool,
}

impl From<crate::plugins::install_sources::ComponentToolSummary> for PluginSourceComponentToolInfo {
    fn from(value: crate::plugins::install_sources::ComponentToolSummary) -> Self {
        Self {
            name: value.name,
            writes: value.writes,
        }
    }
}

/// Mirror of `crate::plugins::install_sources::ComponentTrustSummary`.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginSourceComponentTrustInfo {
    pub network_hosts: Vec<String>,
    pub tools: Vec<PluginSourceComponentToolInfo>,
}

impl From<crate::plugins::install_sources::ComponentTrustSummary>
    for PluginSourceComponentTrustInfo
{
    fn from(value: crate::plugins::install_sources::ComponentTrustSummary) -> Self {
        Self {
            network_hosts: value.network_hosts,
            tools: value.tools.into_iter().map(Into::into).collect(),
        }
    }
}

/// Mirror of `crate::plugins::install_sources::PluginSurfacesSummary`. Widened
/// from `usize` to `u32` crossing the Tauri IPC boundary, matching this
/// module's existing convention for every other DTO count field.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginSourceSurfacesInfo {
    pub commands: u32,
    pub skills: u32,
    pub hooks: u32,
    pub jobs: u32,
}

impl From<crate::plugins::install_sources::PluginSurfacesSummary> for PluginSourceSurfacesInfo {
    fn from(value: crate::plugins::install_sources::PluginSurfacesSummary) -> Self {
        Self {
            commands: value.commands as u32,
            skills: value.skills as u32,
            hooks: value.hooks as u32,
            jobs: value.jobs as u32,
        }
    }
}

/// Mirror of `crate::plugins::install_sources::PluginTrustPrompt` — what
/// `begin_plugin_source_install` returns, shown to the user before
/// `confirm_plugin_source_install` touches the live install dir.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginSourceInstallBegin {
    pub token: String,
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub surfaces: PluginSourceSurfacesInfo,
    pub mcp_servers: Vec<PluginSourceMcpServerInfo>,
    pub component: Option<PluginSourceComponentTrustInfo>,
    /// `true` iff `mcp_servers` is non-empty or `component` is `Some` — the
    /// wizard should only show the explicit-trust checkbox in that case.
    pub trust_required: bool,
}

impl From<crate::plugins::install_sources::PluginTrustPrompt> for PluginSourceInstallBegin {
    fn from(value: crate::plugins::install_sources::PluginTrustPrompt) -> Self {
        Self {
            token: value.token,
            id: value.id,
            name: value.name,
            publisher: value.publisher,
            surfaces: value.surfaces.into(),
            mcp_servers: value.mcp_servers.into_iter().map(Into::into).collect(),
            component: value.component.map(Into::into),
            trust_required: value.trust_required,
        }
    }
}

/// Mirror of `crate::plugins::doctor::DoctorFinding`. Already secret-free at
/// the source (see that module's doc comment) — this DTO adds no new fields,
/// just the specta `Type` the core struct doesn't derive.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DoctorFinding {
    pub plugin_id: String,
    /// `warn` | `error`.
    pub severity: String,
    /// `reconnect-required` | `missing-binary` | `attach-failed` | `blocked` |
    /// `slot-conflict` | `signature-invalid` | `hash-mismatch` |
    /// `abi-incompatible` | `revoked` | `policy-violation` |
    /// `oauth-profile-unhealthy` | `gateway-restart-exhausted`.
    pub kind: String,
    pub message: String,
    pub suggested_action: String,
}

/// `refresh_catalog`/`catalog_status` rpc result — a thin snapshot of the
/// `catalog_feed_state` row plus counts from the cached
/// `plugin_catalog_cache` table (`crate::store::RemoteCatalogRow`). `sequence`
/// stays a `u64` for the same reason `TrustPromptDto.total_bytes` does: no
/// bindings-shape cost, since `export_bindings`'s `BigIntExportBehavior::Number`
/// already renders it as a plain TS `number`.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CatalogStatus {
    pub sequence: u64,
    pub last_fetch_at: Option<i64>,
    pub outcome: Option<String>,
    pub entries: u32,
    pub blocked: u32,
}

/// One row of a component plugin's release ledger (Task 11a). Mirror of
/// `crate::store::ComponentPluginReleaseRecord` with the specta `Type` the
/// core struct doesn't derive, PLUS a Task 12 addition: `first_party` (see
/// its own doc). Carries no secret (source URL, hash, key id, timestamps,
/// lifecycle flags only).
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ComponentReleaseInfo {
    pub plugin_id: String,
    pub version: String,
    pub source_url: String,
    pub sha256: String,
    pub signing_key_id: String,
    pub installed_at: i64,
    pub active: bool,
    pub revoked: bool,
    pub revocation_reason: Option<String>,
    /// Task 12 addition: `signing_key_id == first_party_key::FIRST_PARTY_KEY_ID`.
    /// Computed server-side (rather than left for Cockpit to compare a magic
    /// string) so the UI's "publisher verification" badge and the backing
    /// trust check can never drift.
    pub first_party: bool,
}

impl From<crate::store::ComponentPluginReleaseRecord> for ComponentReleaseInfo {
    fn from(r: crate::store::ComponentPluginReleaseRecord) -> Self {
        let first_party = r.signing_key_id == crate::plugins::first_party_key::FIRST_PARTY_KEY_ID;
        ComponentReleaseInfo {
            plugin_id: r.plugin_id,
            version: r.version,
            source_url: r.source_url,
            sha256: r.sha256,
            signing_key_id: r.signing_key_id,
            installed_at: r.installed_at,
            active: r.active,
            revoked: r.revoked,
            revocation_reason: r.revocation_reason,
            first_party,
        }
    }
}

/// One OAuth profile a component bundle's manifest declares — id + scopes
/// only (no client id/secret/endpoint: Task 12 renders declared metadata,
/// never a live credential; the full connect flow is Task 13). Mirror of
/// `ryuzi_plugin_sdk::OAuthProfile` trimmed to what Cockpit displays.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ComponentOauthProfileInfo {
    pub id: String,
    pub scopes: Vec<String>,
    /// The profile's token endpoint (`token_url`), which the device-flow poll
    /// exchanges the device code against. `None` when the manifest omits it.
    pub token_url: Option<String>,
    /// The RFC 8628 device-authorization endpoint. Present iff this profile
    /// supports the device grant — Cockpit's Connect button only shows when it
    /// is set.
    pub device_authorization_url: Option<String>,
    /// A token row exists for `(plugin_id, profile_id)` — i.e. the profile is
    /// connected. Enriched from the store in `plugin_release_detail`; the pure
    /// manifest [`From`] conversion leaves it `false`.
    pub connected: bool,
    /// The profile's authorize endpoint (`authorize_url`), which `begin_pkce`
    /// builds the browser-facing authorize URL against. `None` when the
    /// manifest omits it (e.g. a device-flow-only profile).
    pub authorize_url: Option<String>,
    /// A client id resolves for this profile (manifest baked-in, a stored
    /// per-install override, or a settings value) — Connect is gated on this.
    /// The pure [`From`] sets it from the manifest's baked `client-id` only;
    /// `plugin_release_detail` ORs in a stored override.
    pub client_id_configured: bool,
}

/// A tool a component bundle's manifest declares — name, description, and
/// whether it modifies state (writes). Mirror of `ryuzi_plugin_sdk::DeclaredTool`.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ComponentToolInfo {
    pub name: String,
    pub description: String,
    pub writes: bool,
}

/// Task 12 cross-layer addition: the currently ACTIVE version's bundle
/// manifest metadata a component plugin's permission-confirmation summary
/// needs to render (publisher, description, lifecycle, network allowlist
/// "domains", declared OAuth profiles) — none of this was in Task 11a's
/// `ComponentReleaseDetail`, which only carries per-release ledger rows
/// (version/hash/signing key/timestamps), not manifest content.
///
/// Sourced from the already-verified on-disk bundle
/// (`plugins::bundle::load_active_bundles`, the same read
/// `profile_capability_context` already performs) rather than a new network
/// fetch — safe because this data has already passed `verify_bundle`. `None`
/// when nothing is currently active (including: never installed, or
/// uninstalled). First-party component bundles additionally surface their
/// EMBEDDED manifest pre-install via
/// `ComponentReleaseDetail::declared_manifest` (PR-1) — compiled into the
/// binary, so it needs no fetch and no signature check; only bundles with no
/// embedded manifest (e.g. third-party) still render Cockpit's generic
/// "unknown until fetched" acknowledgement on first install.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ComponentManifestInfo {
    pub publisher: String,
    pub description: String,
    /// `singleton` | `per-session` | `per-call` (mirrors
    /// `ryuzi_plugin_sdk::PluginLifecycle`'s kebab-case wire form).
    pub lifecycle: String,
    /// The outbound network allowlist ("domains") — bare or `*.`-wildcard
    /// hostnames the component may reach.
    pub domains: Vec<String>,
    pub oauth_profiles: Vec<ComponentOauthProfileInfo>,
    /// The tools this component declares it exposes to agents.
    pub tools: Vec<ComponentToolInfo>,
}

fn lifecycle_label(l: ryuzi_plugin_sdk::PluginLifecycle) -> &'static str {
    use ryuzi_plugin_sdk::PluginLifecycle::*;
    match l {
        Singleton => "singleton",
        PerSession => "per-session",
        PerCall => "per-call",
    }
}

impl From<ryuzi_plugin_sdk::PluginManifest> for ComponentManifestInfo {
    fn from(m: ryuzi_plugin_sdk::PluginManifest) -> Self {
        let lifecycle = m
            .component
            .as_ref()
            .map(|c| lifecycle_label(c.lifecycle))
            .unwrap_or("singleton")
            .to_string();
        ComponentManifestInfo {
            publisher: m.publisher,
            description: m.description,
            lifecycle,
            domains: m.permissions.network.into_iter().map(|n| n.0).collect(),
            oauth_profiles: m
                .oauth
                .into_iter()
                .map(|p| ComponentOauthProfileInfo {
                    id: p.id,
                    scopes: p.scopes,
                    token_url: p.token_url,
                    device_authorization_url: p.device_authorization_url,
                    connected: false,
                    authorize_url: p.authorize_url,
                    client_id_configured: p.client_id.is_some(),
                })
                .collect(),
            tools: m
                .tools
                .into_iter()
                .map(|t| ComponentToolInfo {
                    name: t.name,
                    description: t.description,
                    writes: t.writes,
                })
                .collect(),
        }
    }
}

/// `plugin_release_detail` RPC result: every recorded release for a component
/// plugin (oldest first, as the store returns them) plus the currently active
/// version, if any.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ComponentReleaseDetail {
    pub plugin_id: String,
    pub releases: Vec<ComponentReleaseInfo>,
    pub active_version: Option<String>,
    /// Task 12 addition — see [`ComponentManifestInfo`]'s doc.
    pub active_manifest: Option<ComponentManifestInfo>,
    /// PR-1 (pre-install metadata): the manifest `id`'s EMBEDDED first-party
    /// bundle declares (`component_catalog::declared_manifest`) —
    /// available before any release is fetched, because it is compiled into
    /// the binary. `None` for non-component ids. UI reads
    /// `activeManifest ?? declaredManifest` so the verified on-disk manifest
    /// stays authoritative once a release is active.
    pub declared_manifest: Option<ComponentManifestInfo>,
}

/// `component_bootstrap_status` RPC result (Task 11a): whether the first-party
/// component bootstrap has a pending retryable failure Cockpit should surface,
/// and the human-readable message if so.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ComponentBootstrapStatus {
    /// True when the last bootstrap attempt landed nothing and bootstrap has
    /// not since completed — Cockpit shows a "retry" banner.
    pub pending: bool,
    /// The recorded retry message, present iff `pending`.
    pub message: Option<String>,
}

/// One entry `plugin_tools` lists for a plugin: an agent-facing tool, an
/// installed skill, or a provider's model — `kind` discriminates which.
/// `writes` is only meaningful (`Some`) for `kind == "tool"`, mirroring
/// [`ComponentToolInfo::writes`]; skills and models never modify state
/// through this listing, so they carry `None`.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolEntry {
    pub name: String,
    pub description: String,
    /// `"tool"` | `"skill"` | `"model"`.
    pub kind: String,
    pub writes: Option<bool>,
}

/// `plugin_tools` RPC result: everything a plugin currently offers — live
/// extension tools, a WASM component's declared tools, a skill pack's
/// skills, or a provider's model list (see `plugins_api::plugin_tools`'s doc
/// for the resolution order between those sources; exactly one applies per
/// plugin id).
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolsResult {
    pub plugin_id: String,
    /// True when `entries` came from a live enumeration of a currently
    /// running extension; false when they are declared/manifest/model data.
    pub live: bool,
    pub entries: Vec<PluginToolEntry>,
}

/// `plugin_profile_begin_pkce` RPC result. Mirror of
/// `crate::plugins::capabilities::oauth::PkceStart`. `verifier` is returned to
/// the caller (Cockpit) so it can complete the token exchange; it must never
/// be persisted to durable telemetry (see that type's doc).
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginProfilePkceStart {
    pub authorize_url: String,
    pub state: String,
    pub verifier: String,
}

impl From<crate::plugins::capabilities::oauth::PkceStart> for PluginProfilePkceStart {
    fn from(p: crate::plugins::capabilities::oauth::PkceStart) -> Self {
        PluginProfilePkceStart {
            authorize_url: p.authorize_url,
            state: p.state,
            verifier: p.verifier,
        }
    }
}

/// `plugin_profile_begin_device_flow` RPC result. Mirror of
/// `crate::plugins::capabilities::oauth::DeviceFlowStart`. `user_code` is shown
/// to the user once and must never be written to durable telemetry.
#[derive(Serialize, Deserialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PluginProfileDeviceFlowStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub interval_secs: u64,
    pub expires_at: i64,
}

impl From<crate::plugins::capabilities::oauth::DeviceFlowStart> for PluginProfileDeviceFlowStart {
    fn from(d: crate::plugins::capabilities::oauth::DeviceFlowStart) -> Self {
        PluginProfileDeviceFlowStart {
            device_code: d.device_code,
            user_code: d.user_code,
            verification_uri: d.verification_uri,
            verification_uri_complete: d.verification_uri_complete,
            interval_secs: d.interval_secs,
            expires_at: d.expires_at,
        }
    }
}

impl From<crate::plugins::doctor::DoctorFinding> for DoctorFinding {
    fn from(f: crate::plugins::doctor::DoctorFinding) -> Self {
        DoctorFinding {
            plugin_id: f.plugin_id,
            severity: f.severity,
            kind: f.kind,
            message: f.message,
            suggested_action: f.suggested_action,
        }
    }
}

// --- agent_api (Plan 3: agent management RPC family for the Cockpit Agents panel) ---

/// An agent's model assignment: either a concrete provider model (with an
/// optional effort override) or a symbolic router route (`free`, ...).
/// Routes never carry an effort — `deny_unknown_fields` makes a
/// `{"kind":"route", ..., "effort": ...}` payload a decode error rather
/// than a silently dropped field.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum AgentModelInfo {
    Concrete {
        name: String,
        effort: Option<String>,
    },
    Route {
        route: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRuleInfo {
    pub id: String,
    pub tool: String,
    pub decision: String,
    pub command_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentValidationInfo {
    pub field: String,
    pub message: String,
}

/// One startup-recovery note surfaced to the UI (for example a quarantined
/// agent file that failed to parse and was set aside).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecoveryInfo {
    pub code: String,
    pub message: String,
}

/// An agent's personality selection: a preset name (matching
/// [`crate::agents::personality::PersonalityPreset`]'s `snake_case` variants,
/// e.g. `"technical"` or `"custom"`) plus optional custom text that only
/// applies when `preset` is `"custom"`.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentPersonalityInfo {
    pub preset: String,
    pub custom: Option<String>,
}

/// One explicit per-tool native permission entry — a tool id and its
/// decision (`"allow"` | `"ask"` | `"off"`). Only tools with an explicit
/// entry in the profile's decision map are represented; a tool absent from
/// this list defaults to `"ask"` in the UI against the configuration
/// catalog, mirroring [`crate::agents::types::AgentPermissions::native_decision`].
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NativeToolDecisionInfo {
    pub tool: String,
    pub decision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummaryInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub avatar_color: String,
    /// Bundled or downloaded pet slug shown alongside the avatar color;
    /// `None` when no pet is configured.
    pub avatar_pet: Option<String>,
    pub model: AgentModelInfo,
    /// True for the built-in, non-editable rows (currently only the
    /// synthetic Fresh Agent row) — `false` for every registry-backed agent.
    pub builtin: bool,
    pub skill_count: u32,
    pub tool_count: u32,
    pub knowledge_count: u32,
    pub executable: bool,
    pub validation: Vec<AgentValidationInfo>,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentDetailInfo {
    pub summary: AgentSummaryInfo,
    pub permission_rules: Vec<PermissionRuleInfo>,
    pub skills: Vec<String>,
    pub native_tools: Vec<NativeToolDecisionInfo>,
    pub plugin_tools: Vec<String>,
    pub apps: Vec<String>,
    pub model_info: Option<SelectableModelInfo>,
    pub personality: AgentPersonalityInfo,
}

/// Everything a create/update mutation may set on an agent. Server-derived
/// fields (`id`, counts, `executable`, `validation`, `is_default`) are
/// deliberately absent so the client can't submit them.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentMutationInfo {
    pub name: String,
    pub description: String,
    pub avatar_color: String,
    /// Bundled or downloaded pet slug; free-form (no catalog check against
    /// the petdex manifest happens on write). `None`/blank clears it.
    pub avatar_pet: Option<String>,
    pub model: AgentModelInfo,
    pub personality: AgentPersonalityInfo,
    pub permission_rules: Vec<PermissionRuleInfo>,
    pub skills: Vec<String>,
    pub native_tools: Vec<NativeToolDecisionInfo>,
    pub plugin_tools: Vec<String>,
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentRegistryInfo {
    pub agents: Vec<AgentSummaryInfo>,
    pub default_agent_id: String,
    pub recovery: Vec<AgentRecoveryInfo>,
    pub subagent_model: AgentModelInfo,
}

/// One tool's lifetime usage counter for a single agent — see
/// [`crate::store::AgentToolUsageRow`].
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolUsageInfo {
    pub tool: String,
    pub count: i64,
    pub last_used: i64,
}

/// Per-agent stats surfaced on the agent detail view: sessions led, priced
/// cost/tokens over the trailing 7 days, run reliability over the trailing
/// 30 days, and the full per-tool usage breakdown (`top_tools`, count DESC).
/// An unknown or synthetic agent id (including the Fresh Agent's `"fresh"`)
/// simply has no matching rows anywhere, so every field zeroes out rather
/// than erroring.
// `specta`'s own camelCase inflector (the `inflector` crate) splits words at
// digit→letter boundaries, unlike serde's `rename_all = "camelCase"` (which
// only capitalizes the first character of each `_`-delimited segment) — left
// alone it would emit `costUsd7D`/`tokens7D`/etc. in bindings.ts, one letter
// off from the real wire key serde actually produces. Pin every digit-suffixed
// field to its true serde-camelCase key so the generated TS type matches the
// runtime JSON exactly.
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatsInfo {
    pub session_count: i64,
    pub last_active: Option<i64>,
    #[specta(rename = "costUsd7d")]
    pub cost_usd_7d: f64,
    #[specta(rename = "tokens7d")]
    pub tokens_7d: i64,
    #[specta(rename = "runsTotal30d")]
    pub runs_total_30d: i64,
    #[specta(rename = "runsFailed30d")]
    pub runs_failed_30d: i64,
    pub top_tools: Vec<AgentToolUsageInfo>,
}

/// Lightweight per-agent stats for roster/list views (`get_agent_stats_batch`)
/// — see [`AgentStatsInfo`] for the full detail-view shape.
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatsLite {
    pub session_count: i64,
    pub last_active: Option<i64>,
    #[specta(rename = "costUsd7d")]
    pub cost_usd_7d: f64,
}

/// One selectable option in the agent configuration catalog (a skill,
/// native tool, plugin tool, or app) — see
/// [`crate::agents::catalog::CatalogEntry`].
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntryInfo {
    pub id: String,
    pub label: String,
    pub description: String,
    pub available: bool,
    pub command_scoped: bool,
    /// Skill entries only: the owning installed skill pack's display name,
    /// when this skill was installed as part of a multi-skill pack (a
    /// plugin-bundled skill pack). `None` for every non-skill entry, and for
    /// a standalone (not pack-installed) skill — the frontend groups `None`
    /// entries under a synthetic "Standalone" heading.
    pub pack: Option<String>,
    /// Plugin-tool entries only: the plugin's coarse kind — see
    /// [`crate::agents::catalog::CatalogEntry::kind`]. `None` for skills,
    /// native tools, and apps.
    pub kind: Option<String>,
}

impl From<crate::agents::catalog::CatalogEntry> for CatalogEntryInfo {
    fn from(entry: crate::agents::catalog::CatalogEntry) -> Self {
        CatalogEntryInfo {
            id: entry.id,
            label: entry.label,
            description: entry.description,
            available: entry.available,
            command_scoped: entry.command_scoped,
            pack: entry.pack,
            kind: entry.kind,
        }
    }
}

/// The full set of configuration options (skills, native tools, plugin
/// tools, apps) offered when building or editing an agent profile — see
/// [`crate::agents::catalog::AgentConfigurationCatalog`].
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigurationCatalogInfo {
    pub skills: Vec<CatalogEntryInfo>,
    pub native_tools: Vec<CatalogEntryInfo>,
    pub plugin_tools: Vec<CatalogEntryInfo>,
    pub apps: Vec<CatalogEntryInfo>,
}

impl From<crate::agents::catalog::AgentConfigurationCatalog> for AgentConfigurationCatalogInfo {
    fn from(catalog: crate::agents::catalog::AgentConfigurationCatalog) -> Self {
        AgentConfigurationCatalogInfo {
            skills: catalog.skills.into_iter().map(Into::into).collect(),
            native_tools: catalog.native_tools.into_iter().map(Into::into).collect(),
            plugin_tools: catalog.plugin_tools.into_iter().map(Into::into).collect(),
            apps: catalog.apps.into_iter().map(Into::into).collect(),
        }
    }
}

/// One knowledge concept as stored in the agent's OKF tree. `timestamp` is
/// RFC3339. `scope` is `None` for non-memory concepts and one of `global`,
/// `user`, or `project` for memory; `project_id` is non-null only for
/// project memory.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeConceptInfo {
    pub id: String,
    pub relative_path: String,
    pub concept_type: String,
    pub title: String,
    pub description: String,
    pub body: String,
    pub scope: Option<String>,
    pub project_id: Option<String>,
    pub tags: Vec<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeConceptMutationInfo {
    pub title: String,
    pub description: String,
    pub body: String,
    pub scope: String,
    pub project_id: Option<String>,
    pub tags: Vec<String>,
}

/// A knowledge file that failed OKF parsing: surfaced with its raw markdown
/// so the UI can offer repair instead of silently dropping it.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct InvalidKnowledgeConceptInfo {
    pub relative_path: String,
    pub error: String,
    pub raw_markdown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JourneyMilestoneInfo {
    pub concept_id: String,
    pub title: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillUsageInfo {
    pub skill_id: String,
    pub uses: u64,
    pub successes: u64,
    pub concept_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LearningReviewInfo {
    pub concept_id: String,
    pub title: String,
    pub description: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CuratorStateInfo {
    pub concept: Option<KnowledgeConceptInfo>,
    pub last_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CuratorHistorySnapshotInfo {
    pub snapshot_id: String,
    pub concept: KnowledgeConceptInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentLearningInfo {
    pub concepts: Vec<KnowledgeConceptInfo>,
    pub invalid: Vec<InvalidKnowledgeConceptInfo>,
    pub journey: Vec<JourneyMilestoneInfo>,
    pub skill_usage: Vec<AgentSkillUsageInfo>,
    pub reviews: Vec<LearningReviewInfo>,
    pub curator: CuratorStateInfo,
    pub curator_history: Vec<CuratorHistorySnapshotInfo>,
}

/// One row of a session's artifact listing (`artifacts_api::list_session_artifacts`):
/// either an artifact the session originated (`reference_id` and its sibling
/// reference fields are `None`) or one shared into the session via a
/// reference (all three are `Some`).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactInfo {
    pub id: String,
    pub source_session_pk: String,
    pub reference_id: Option<String>,
    pub shared_from_session_pk: Option<String>,
    pub parent_reference_id: Option<String>,
    pub status: String,
    pub name: String,
    pub content_type: Option<String>,
    pub size_bytes: u64,
    pub creator: String,
    pub created_at: i64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactFileInfo {
    pub name: String,
    pub content_type: Option<String>,
    pub data_base64: String,
}

/// One pet in the petdex manifest (`pets_api::list_pet_manifest`). Doubles
/// as the manifest JSON's own per-pet wire shape — `Deserialize`d straight
/// out of `https://petdex.dev/api/manifest`'s `pets` array, which also
/// carries `petJsonUrl`/`zipUrl` fields this DTO deliberately omits (serde
/// ignores unknown keys by default, so those are silently dropped rather
/// than surfaced to the frontend).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PetManifestEntryInfo {
    pub slug: String,
    pub display_name: String,
    pub kind: String,
    #[serde(default)]
    pub submitted_by: Option<String>,
    pub spritesheet_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_mention_and_turn_input_use_camel_case_fields() {
        let turn: TurnInput = serde_json::from_value(serde_json::json!({
            "text": "ask Ada",
            "mentions": [{
                "agentId": "ada",
                "labelSnapshot": "Ada",
                "startUtf16": 4,
                "endUtf16": 7
            }],
            "attachments": []
        }))
        .unwrap();

        assert_eq!(
            serde_json::to_value(&turn.mentions[0]).unwrap(),
            serde_json::json!({
                "agentId": "ada",
                "labelSnapshot": "Ada",
                "startUtf16": 4,
                "endUtf16": 7
            })
        );
    }

    #[test]
    fn chat_agent_prompt_appends_context_without_changing_display_text() {
        let out = chat_agent_prompt(
            "/review auth",
            Some(&ChatContextArg {
                branch: Some("feature/auth".into()),
                voice_transcript: Some("review the auth changes".into()),
                references: vec![],
            }),
        );
        assert!(out.starts_with("/review auth\n\n[Chat context]"));
        assert!(out.contains("- Branch: feature/auth"));
        assert!(out.contains("- Voice transcript: review the auth changes"));
    }

    #[test]
    fn chat_agent_prompt_appends_referenced_files_from_context_mentions() {
        let out = chat_agent_prompt(
            "explain this",
            Some(&ChatContextArg {
                references: vec!["src/main.rs".into(), "crates/core/src/lib.rs".into()],
                ..Default::default()
            }),
        );
        assert!(out.contains("- Referenced file: src/main.rs"));
        assert!(out.contains("- Referenced file: crates/core/src/lib.rs"));
    }

    #[test]
    fn git_options_convert_to_session_git_options_trimming_blanks() {
        let core: SessionGitOptions = GitOptions {
            use_worktree: true,
            create_branch: false,
            branch_name: Some("   ".into()),
            base_branch: Some(" develop ".into()),
        }
        .into();
        assert!(core.use_worktree);
        assert!(!core.create_branch);
        assert_eq!(core.branch_name, None, "blank names collapse to None");
        assert_eq!(core.base_branch.as_deref(), Some("develop"));
    }

    #[test]
    fn sanitize_file_name_strips_directories_and_unsafe_chars() {
        assert_eq!(sanitize_file_name("shot.png"), "shot.png");
        // rsplit keeps only the last path segment — traversal collapses away.
        assert_eq!(sanitize_file_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_file_name("a/b/c.png"), "c.png");
        assert_eq!(sanitize_file_name("we|ird?.png"), "weird.png");
        assert_eq!(sanitize_file_name("   "), "file");
    }
}

#[cfg(test)]
mod agent_management_dto_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_model_union_uses_discriminated_camel_case_shape() {
        assert_eq!(
            serde_json::to_value(AgentModelInfo::Concrete {
                name: "anthropic/claude-opus-4-8".into(),
                effort: Some("high".into()),
            })
            .unwrap(),
            json!({"kind":"concrete","name":"anthropic/claude-opus-4-8","effort":"high"})
        );
        assert_eq!(
            serde_json::to_value(AgentModelInfo::Route {
                route: "free".into()
            })
            .unwrap(),
            json!({"kind":"route","route":"free"})
        );
    }

    #[test]
    fn mutation_input_rejects_route_effort_by_construction() {
        let parsed = serde_json::from_value::<AgentMutationInfo>(json!({
            "name":"Reviewer",
            "description":"Reviews changes",
            "avatarColor":"violet",
            "model":{"kind":"route","route":"free","effort":"high"},
            "personality": {"preset": "helpful", "custom": null},
            "permissionRules":[],
            "skills":[],
            "nativeTools":[{"tool":"read","decision":"allow"}],
            "pluginTools":[],
            "apps":[]
        }));
        assert!(parsed.is_err());
    }
}
