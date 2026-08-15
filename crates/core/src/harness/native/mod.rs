//! Native agent runtime.
//!
//! The native runtime runs the agentic loop in-process: it calls LLMs through
//! [`crate::llm_router::client`], executes its own built-in tools
//! ([`tools`]), enforces permissions ([`permission`]), and persists a
//! provider-turn ledger ([`ledger`]). It is the engine's only session
//! harness, held as the single factory slot in [`crate::plugins::Registries`].
//!
//! See `docs/design/2026-07-05-native-agent-runtime-design.md`.

pub mod agents;
pub mod arguments;
pub mod background;
pub mod capabilities;
pub mod commands;
pub mod context;
pub mod context_manager;
pub mod cost;
pub mod delegation;
pub mod file_reference;
pub mod format;
pub mod hooks;
pub mod iteration_budget;
pub mod ledger;
pub mod llm;
pub mod lsp;
pub mod mcp_client;
pub mod mcp_http;
pub mod mcp_oauth;
pub mod memory;
pub mod permission;
pub mod runner;
pub mod skills;
pub mod slash_catalog;
pub mod snapshot;
pub mod steer;
pub mod summary_budget;
pub mod tool_contract;
pub mod tool_plan;
pub mod tools;

use crate::harness::{Harness, HarnessFactory, HarnessSession, SessionCtx, TurnPrompt};
use crate::plugins::{CorePlugin, PluginSource};
use async_trait::async_trait;
use ryuzi_plugin_sdk::PluginManifest;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// The native runtime harness id — the sole in-process agent runtime.
pub const NATIVE_ID: &str = "native";

#[derive(Debug)]
struct AdaptedPrimary {
    agent: agents::Agent,
    allowed_skills: Option<Vec<String>>,
}

/// Which native tools this profile advertises, plus its plugin/app bindings
/// resolved against the LIVE registry (`available`).
///
/// Native tools: every non-namespaced id in `available` whose decision is
/// `enabled()` (`Allow` or the default-absent `Ask`) — only an explicit `Off`
/// excludes it. There is no "everything empty" shortcut to `ToolFilter::All`:
/// an empty decision map already means every native tool is `Ask` (enabled),
/// so an agent with no plugin/app bindings still resolves to exactly the
/// native registry, not a blanket allow that would also sweep in `mcp__`
/// tools it never configured. There is one namespace for every plugin- or
/// server-provided tool (Task 6 merged the old `wasm__` component path into
/// `mcp__`), so only that one prefix is special-cased below.
fn tool_filter_for_profile(
    profile: &crate::agents::types::AgentProfile,
    available: &[String],
) -> agents::ToolFilter {
    let namespaced = |name: &str| name.starts_with("mcp__");
    let mut allowed: Vec<String> = available
        .iter()
        .filter(|name| !namespaced(name))
        .filter(|name| profile.permissions.native_decision(name).enabled())
        .cloned()
        .collect();
    for plugin in &profile.tools.plugins {
        if available.iter().any(|name| name == plugin) {
            allowed.push(plugin.clone());
            continue;
        }
        match plugin.split_once('.') {
            // Namespaced `<plugin>.<tool>`: exact single-tool grant (unchanged).
            Some((provider, tool)) => allowed.extend(
                available
                    .iter()
                    .filter(|name| *name == &format!("mcp__{provider}__{tool}"))
                    .cloned(),
            ),
            // Bare plugin id: every tool the plugin contributes, mirroring the
            // app arm below. The old code fed ("<id>", "") through the exact
            // arm and matched `mcp__<id>__` — i.e. nothing.
            None => allowed.extend(
                available
                    .iter()
                    .filter(|name| name.starts_with(&format!("mcp__{plugin}__")))
                    .cloned(),
            ),
        }
    }
    for app in &profile.tools.apps {
        allowed.extend(
            available
                .iter()
                .filter(|name| name.starts_with(&format!("mcp__{app}__")))
                .cloned(),
        );
    }
    allowed.sort();
    allowed.dedup();
    agents::ToolFilter::Only(allowed)
}

fn adapt_primary_profile(
    profile: &crate::agents::types::AgentProfile,
) -> anyhow::Result<AdaptedPrimary> {
    Ok(AdaptedPrimary {
        agent: agents::Agent {
            name: profile.id.clone(),
            description: profile.description.clone(),
            mode: agents::AgentMode::Primary,
            prompt: None,
            identity_prompt: Some(profile.personality.prompt()?.to_owned()),
            // Fail-closed placeholder: a registry-blind config cannot express
            // the profile's plugin/app bindings, so every consumer that
            // reaches a model rebuilds this against the live registry via
            // `tool_filter_for_profile` (`primary_turn_config_with_tools` at
            // session/dispatch build time, `refresh_primary_turn` at prompt
            // time). If a new consumer forgets, every tool is blocked —
            // loudly visible — instead of silently overgranting.
            tools: agents::ToolFilter::Only(Vec::new()),
            permission_rules: profile.permissions.rules.clone(),
            can_delegate: false,
            builtin: false,
        },
        // PR-2 fix E shim: a profile written before per-skill binding may
        // still store a pack id here — expand it to that pack's member
        // skill names (the exact strings the runtime matches skills by), so
        // a legacy binding isn't silently inert. See
        // `crate::skills_install::expand_skill_bindings`.
        allowed_skills: (!profile.skills.is_empty())
            .then(|| crate::skills_install::expand_skill_bindings(&profile.skills)),
    })
}

pub(crate) fn primary_turn_config(
    agent: Arc<crate::agents::types::AgentSnapshot>,
    run_id: String,
    root_run_id: String,
    perm_mode: crate::domain::PermMode,
) -> anyhow::Result<crate::harness::PrimaryTurnConfig> {
    let adapted = adapt_primary_profile(&agent.profile)?;
    let (model, effort) = match &agent.profile.model {
        crate::agents::types::AgentModel::Concrete { name, effort } => {
            (Some(name.clone()), effort.clone())
        }
        crate::agents::types::AgentModel::Route { route } => (Some(route.clone()), None),
    };
    Ok(crate::harness::PrimaryTurnConfig {
        perm_mode,
        agent,
        run_id,
        root_run_id,
        model,
        effort,
        agent_tools: adapted.agent,
        allowed_skills: adapted.allowed_skills,
    })
}

pub(crate) fn primary_turn_config_with_tools(
    agent: Arc<crate::agents::types::AgentSnapshot>,
    run_id: String,
    root_run_id: String,
    perm_mode: crate::domain::PermMode,
    available_tools: &[String],
) -> anyhow::Result<crate::harness::PrimaryTurnConfig> {
    let mut config = primary_turn_config(agent.clone(), run_id, root_run_id, perm_mode)?;
    config.agent_tools.tools = tool_filter_for_profile(&agent.profile, available_tools);
    Ok(config)
}

/// The native agent runtime as a [`Harness`]. Each session runs the agentic
/// loop in-process via [`runner::run_turn`].
pub struct NativeHarness {
    /// Factory for the LLM stream. Overridable in tests to script conversations.
    llm_factory: Arc<dyn llm::LlmStreamFactory>,
}

impl NativeHarness {
    pub fn new() -> Self {
        NativeHarness {
            llm_factory: Arc::new(llm::RouterLlmStreamFactory),
        }
    }

    /// Construct with a custom LLM stream factory (used by tests).
    pub fn with_llm_factory(llm_factory: Arc<dyn llm::LlmStreamFactory>) -> Self {
        NativeHarness { llm_factory }
    }
}

impl Default for NativeHarness {
    fn default() -> Self {
        Self::new()
    }
}

/// Who owns the credential a remote (HTTP) MCP server authenticates with.
///
/// This is the single predicate the whole product is allowed to branch on for
/// that question — [`open_http_mcp`] (and therefore every agent session) and
/// `api::apps_api::assemble`'s `AppInfo.oauth_connect_available` (and
/// therefore the Connection card's OAuth affordance) both read it, so the card
/// cannot claim one thing while the session does another.
///
/// It exists because `transport == "http"` was standing in for it, and merely
/// CORRELATES with it: `atlassian-rovo` is an http server that authenticates
/// with a manifest `Authorization: Basic ${setting:…}` header, so the card
/// offered a full OAuth consent flow whose resulting token
/// [`open_http_mcp`] then never used — a successful-looking sign-in with
/// literally zero effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpHttpCredential {
    /// The spec already carries an `Authorization` header — a `${setting:…}`
    /// API token or an injected plugin OAuth bearer that
    /// `plugins::mcp_sync` resolved, or headers a user typed by hand. It wins
    /// VERBATIM: no stored MCP OAuth token is read, used, refreshed or
    /// reconnect-marked for this server, because the credential is not this
    /// host's to manage.
    Manifest,
    /// No manifest `Authorization`, so the credential slot belongs to this
    /// host: a token minted by the MCP OAuth connect flow is what
    /// authenticates the session — or nothing at all, when none is stored
    /// (an unauthenticated/public server, or one waiting to be connected).
    /// This, and only this, is when offering "Connect" tells the truth.
    HostManaged,
}

impl McpHttpCredential {
    /// Whether this host owns the credential — the exact question the UI's
    /// OAuth-connect affordance has to be gated on.
    pub(crate) fn host_managed(self) -> bool {
        matches!(self, McpHttpCredential::HostManaged)
    }
}

/// Classify an HTTP MCP spec's resolved headers. Case-insensitive, per RFC
/// 9110 §5.1 — a manifest that writes `authorization:` in lower case supplies
/// a credential exactly as much as one that writes `Authorization`.
///
/// Only `Authorization` counts: the question this answers is not "does the
/// manifest supply SOME credential" but the narrower, decidable one "would a
/// stored MCP OAuth token be used for this server" — and a manifest header of
/// any other name (`X-Api-Key`, a tenant id) is sent ALONGSIDE such a token
/// rather than instead of it, so it leaves the host in charge of the
/// `Authorization` slot.
pub(crate) fn mcp_http_credential(headers: &[(String, String)]) -> McpHttpCredential {
    if headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
    {
        McpHttpCredential::Manifest
    } else {
        McpHttpCredential::HostManaged
    }
}

/// [`mcp_http_credential`] for a whole spec — `None` for a stdio transport,
/// which has no credential of this kind at all (and therefore never an OAuth
/// connect affordance).
pub(crate) fn mcp_http_credential_of(
    spec: &crate::domain::McpServerSpec,
) -> Option<McpHttpCredential> {
    match &spec.transport {
        crate::domain::McpTransport::Http { headers, .. } => Some(mcp_http_credential(headers)),
        crate::domain::McpTransport::Stdio { .. } => None,
    }
}

/// Open a remote MCP connection (initialize → tools/list) applying the auth
/// precedence rule [`McpHttpCredential`] describes.
///
/// The ONE implementation of that rule: session start goes through it via
/// [`connect_mcp_tools`], and so does `api::apps_api::probe_and_persist` —
/// the Probe button, `add_app`, and the re-probe after an OAuth connect.
/// That matters because the probe used to POST a bare `initialize` with NO
/// credential at all: every auth-gated remote server showed a red "HTTP
/// initialize failed — check the URL" for a perfectly good URL, and since the
/// probe is also what writes `mcp_tools` rows, no remote server's per-tool
/// permissions could be configured at all. A probe that authenticates
/// differently from the session it predicts is worse than no probe.
pub(crate) async fn open_http_mcp(
    store: &Arc<crate::store::Store>,
    spec: &crate::domain::McpServerSpec,
) -> anyhow::Result<mcp_http::McpHttpConnection> {
    let Some(credential) = mcp_http_credential_of(spec) else {
        anyhow::bail!("{} is not a remote (http) MCP server", spec.name);
    };
    match credential {
        // bearer=None leaves the manifest header untouched, and this
        // connection never wires a Store, so a later 401 is never treated as
        // ours to refresh or reconnect-mark.
        McpHttpCredential::Manifest => mcp_http::connect_http(spec, None).await,
        McpHttpCredential::HostManaged => {
            // reconnect_required tokens are filtered out HERE, not merely
            // ignored downstream — this is the only place that decides
            // whether a stored token is used at all.
            let stored = store
                .get_mcp_oauth_token(&spec.name)
                .await
                .ok()
                .flatten()
                .filter(|t| !t.reconnect_required);
            match stored {
                Some(token) => {
                    mcp_http::connect_http_with_store(
                        spec,
                        Some(&token.access_token),
                        Arc::clone(store),
                    )
                    .await
                }
                None => mcp_http::connect_http(spec, None).await,
            }
        }
    }
}

/// Connect the session's enabled MCP servers — stdio (`mcp_client`) and
/// remote Streamable HTTP (`mcp_http`) alike — and build native tool
/// wrappers for their tools. Servers connect CONCURRENTLY (`join_all` — each
/// handshake is independent), so total startup latency is the slowest
/// server, not the sum. Failures are logged and skipped; `join_all`
/// preserves input order, so tool order stays deterministic.
///
/// `principals` is the `SessionCtx.mcp_principals` binding map
/// (`McpServerSpec.name` → owning plugin); a server absent from it (a
/// DB-configured, non-plugin server) resolves every one of its tools to
/// `principal = None`.
///
/// `store` resolves auth for an HTTP server per the plan's precedence rule
/// (Task 8), which lives in [`open_http_mcp`] / [`McpHttpCredential`] rather
/// than inline here: a manifest-supplied `Authorization` header always wins
/// when present, so a declaratively-configured plugin never drags the user
/// into an OAuth flow it never asked for; only when the spec carries none is
/// a token this host minted itself used, and only when it is not
/// `reconnect_required`.
async fn connect_mcp_tools(
    store: &Arc<crate::store::Store>,
    mcp_servers: &[crate::domain::McpServerSpec],
    principals: &std::collections::HashMap<String, crate::domain::Principal>,
) -> Vec<Arc<dyn tools::Tool>> {
    let connections = futures::future::join_all(mcp_servers.iter().map(|spec| async move {
        let opened: anyhow::Result<(
            String,
            Vec<mcp_client::McpToolDef>,
            Arc<dyn mcp_client::McpCaller>,
        )> = match &spec.transport {
            crate::domain::McpTransport::Stdio { .. } => {
                mcp_client::McpConnection::connect_stdio(spec)
                    .await
                    .map(|conn| {
                        let conn = Arc::new(conn);
                        (
                            conn.server_name.clone(),
                            conn.tools.clone(),
                            conn as Arc<dyn mcp_client::McpCaller>,
                        )
                    })
            }
            // The auth-precedence rule lives in `open_http_mcp` — shared
            // verbatim with the Apps probe, so the two can never drift.
            crate::domain::McpTransport::Http { .. } => {
                open_http_mcp(store, spec).await.map(|conn| {
                    let conn = Arc::new(conn);
                    (
                        conn.server_name.clone(),
                        conn.tools.clone(),
                        conn as Arc<dyn mcp_client::McpCaller>,
                    )
                })
            }
        };
        match opened {
            Ok(parts) => Some(parts),
            Err(e) => {
                tracing::warn!("native: MCP server `{}` unavailable: {e}", spec.name);
                None
            }
        }
    }))
    .await;
    let mut extra: Vec<Arc<dyn tools::Tool>> = Vec::new();
    for (server_name, tool_defs, caller) in connections.into_iter().flatten() {
        let principal = principals.get(&server_name).cloned();
        for t in &tool_defs {
            extra.push(Arc::new(tools::mcp::McpTool::new(
                &server_name,
                &t.name,
                &t.description,
                t.input_schema.clone(),
                caller.clone(),
                principal.clone(),
            )));
        }
    }
    extra
}

/// Wrap every enabled WASM component's already-discovered connector tools
/// (Task 6) as `mcp__<component>__<tool>` native `Tool`s — called at the same
/// session-start point as `connect_mcp_tools` and folded into the SAME
/// registry, via the SAME `McpTool` wrapper an external stdio MCP server's
/// tools go through. An empty `component_mcp` (no enabled component bundle
/// installed — the common case, and every bare test `SessionCtx`) is a true
/// zero-cost no-op — no allocation, no extra tools.
fn connect_component_mcp_tools(
    component_mcp: &[Arc<crate::plugins::mcp_component::ComponentMcpServer>],
) -> Vec<Arc<dyn tools::Tool>> {
    let mut extra: Vec<Arc<dyn tools::Tool>> = Vec::new();
    for server in component_mcp {
        for def in &server.tools {
            extra.push(Arc::new(tools::mcp::McpTool::new(
                &server.server_id,
                &def.name,
                &def.description,
                def.input_schema.clone(),
                server.clone() as Arc<dyn mcp_client::McpCaller>,
                Some(server.principal.clone()),
            )));
        }
    }
    extra
}

async fn resolve_native_model(
    store: &crate::store::Store,
    configured: Option<String>,
) -> Option<String> {
    if let Some(model) = configured.filter(|m| !m.trim().is_empty()) {
        if crate::llm_router::client::route_model_for_anthropic_messages(store, &model)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            return Some(model);
        }
    }
    crate::llm_router::client::default_anthropic_messages_model(store).await
}

async fn resolve_native_tools_version(
    store: &crate::store::Store,
    session_pk: &str,
    run_id: &str,
) -> anyhow::Result<capabilities::NativeToolsVersion> {
    if tool_plan::load_plan(store, run_id).await?.is_some() {
        return Ok(capabilities::NativeToolsVersion::V2);
    }
    let requested = match store.get_setting("native_tools.version").await? {
        Some(value) => capabilities::NativeToolsVersion::parse(value.trim())?,
        None => capabilities::NativeToolsVersion::V1,
    };
    let stored = store
        .get_or_insert_native_tool_session_version(session_pk, requested.as_str())
        .await?;
    Ok(capabilities::NativeToolsVersion::parse(&stored.version)?)
}

#[async_trait]
impl Harness for NativeHarness {
    async fn start_session(&self, ctx: SessionCtx) -> anyhow::Result<Box<dyn HarnessSession>> {
        let llm = self.llm_factory.create(ctx.store.clone());
        let native_tools_version =
            resolve_native_tools_version(&ctx.store, &ctx.session_pk, &ctx.run_id).await?;
        // Native speaks Anthropic Messages internally; resolve configured
        // routes/models through that capability and fall back to a compatible
        // route/model when a stale project pins a target no connection
        // actually serves anymore.
        let model = resolve_native_model(&ctx.store, ctx.model.clone()).await;
        let meta =
            crate::llm_router::model_meta::resolve(&ctx.store, model.as_deref().unwrap_or(""))
                .await;
        crate::llm_router::model_meta::spawn_refresh();
        // Discover agents + slash commands from the worktree (and global config).
        let agents = Arc::new(agents::AgentRegistry::load(&ctx.work_dir));
        let commands = Arc::new(commands::CommandRegistry::load_with_plugins(
            &ctx.work_dir,
            &ctx.plugin_command_roots,
        ));
        // The durable snapshot owns this session's native persona. Legacy
        // worktree agents remain available only for slash-command/subagent
        // selection; they must never replace a durable primary by name.
        // Plugin hooks: observational — a `session.start` hook is notified but
        // cannot block startup (only `tool.before` gates).
        let _ = hooks::fire_hook(
            &ctx.store,
            &ctx.work_dir,
            hooks::HookEvent::SessionStart,
            &json!({
                "session": ctx.session_pk.clone(),
                "project": ctx.project_id.clone(),
                "model": model.clone(),
                "work_dir": ctx.work_dir.display().to_string(),
            }),
            None,
        )
        .await;
        crate::automation::dispatch_lifecycle_observation(
            ctx.automation_events.clone(),
            crate::automation::TriggerKind::SessionStart,
            ctx.session_pk.clone(),
            json!({
                "model": model.clone(),
                "workDir": ctx.work_dir.display().to_string(),
            }),
        );
        // Connect MCP servers and expose their tools; the wrapping Arcs keep the
        // connections alive for the session's lifetime.
        let mut extra_tools =
            connect_mcp_tools(&ctx.store, &ctx.mcp_servers, &ctx.mcp_principals).await;
        // Task 6: fold in every enabled WASM component's connector tools
        // alongside the external MCP ones — both are `McpTool`s in the SAME
        // registry now, dispatched through the identical `deps.tools.get(name)`
        // path with no special-casing by source, and governed by the same
        // `mcp__*` permission path.
        extra_tools.extend(connect_component_mcp_tools(&ctx.component_mcp));
        let tools = Arc::new(tools::ToolRegistry::with_extra(extra_tools));
        // The registry is complete only after MCP and WASM-tool attachment.
        // Resolve this immutable profile against that final namespace so a
        // constrained explicit target cannot fall back to `ToolFilter::All`.
        let primary_turn = primary_turn_config_with_tools(
            ctx.primary_agent.clone(),
            ctx.run_id.clone(),
            ctx.root_run_id.clone(),
            ctx.perm_mode,
            &tools.names(),
        )?;
        let agent = primary_turn.agent_tools.clone();
        let model_name = model.as_deref().unwrap_or("");
        let mut effort_policy =
            crate::llm_router::model_effort::build_utility_effort_policy(&ctx.store, model_name)
                .await?;
        effort_policy.caller_override = match &primary_turn.agent.profile.model {
            crate::agents::types::AgentModel::Concrete { effort, .. } => effort.clone(),
            crate::agents::types::AgentModel::Route { .. } => None,
        };
        // Persistent memory is unconditional: a chat (project-less) session
        // still gets GLOBAL + USER memory, while a project session gets
        // global + user + project scope. `at_default(None)` sets the global
        // and user paths unconditionally and leaves the project path unset —
        // global/user memory work, project-scope ops error cleanly — so
        // previously skipping `MemoryStore` entirely for `project_id: None`
        // needlessly denied chat sessions memory. Tool-policy lookups
        // (below, via `RunnerDeps::project_id`) stay project-scoped and off
        // without a project — chat sessions have no project to scope a
        // `tool_policies` row to.
        let project_id = ctx.project_id.clone();
        let memory_store = Some(Arc::new(memory::MemoryStore::for_agent(
            ctx.agent_knowledge.clone(),
            &ctx.main_agent_id,
            project_id.as_deref(),
        )?));
        // One buffer for the session's whole lifetime: cloned into
        // `RunnerDeps` below so `drive()` can drain what `NativeSession::steer`
        // pushes — both sides share the same `Arc<Mutex<_>>` (Task B3).
        let steer = steer::SteerBuffer::new();
        // Rendered into the system prompt below so `delegate_agent` always
        // advertises the CURRENT executable catalog, excluding this
        // session's own profile (a profile can't delegate to itself).
        let delegation_catalog = ctx
            .delegation
            .delegate_catalog(&ctx.primary_agent.profile.id)
            .await;
        Ok(Box::new(NativeSession {
            session_pk: ctx.session_pk.clone(),
            automation_events: ctx.automation_events.clone(),
            steer: steer.clone(),
            deps: Mutex::new(runner::RunnerDeps {
                session_pk: ctx.session_pk,
                primary_agent: ctx.primary_agent,
                run_id: ctx.run_id.clone(),
                root_run_id: ctx.root_run_id,
                delegation: ctx.delegation,
                isolated_target: ctx.isolated_target,
                main_agent_id: ctx.main_agent_id,
                learning_queue: ctx.learning_queue,
                agent_knowledge: ctx.agent_knowledge,
                kind: ctx.kind,
                work_dir: ctx.work_dir,
                // Isolated explicit targets never inherit the parent session's
                // attachment root, even if a caller constructs SessionCtx
                // directly instead of going through the control plane.
                attachments_dir: (!ctx.isolated_target)
                    .then_some(ctx.attachments_dir)
                    .flatten(),
                artifacts: ctx.artifacts,
                plugin_command_roots: ctx.plugin_command_roots,
                plugin_skill_roots: ctx.plugin_skill_roots,
                model,
                turn_effort_policy: Arc::new(effort_policy),
                meta,
                perm_mode: Arc::new(std::sync::Mutex::new(ctx.perm_mode)),
                project_id,
                perm_overrides: Arc::new(std::sync::Mutex::new(Default::default())),
                store: ctx.store,
                telemetry: ctx.telemetry,
                events: ctx.events,
                approvals: ctx.approvals,
                automation_events: ctx.automation_events,
                llm,
                tools,
                native_tools_version,
                native_tool_runtime_surfaces: capabilities::RuntimeToolSurfaces::direct_only(),
                native_tool_override_mode: None,
                agent,
                agents,
                commands,
                allowed_skills: primary_turn.allowed_skills.clone(),
                memory: memory_store,
                snapshots: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                snapshot_taker: Arc::new(runner::GitSnapshotTaker),
                steer,
                background: ctx.background,
                // Explicit-target and noninteractive-session facades must
                // never cross the harness boundary from their parent.
                app_control: (!ctx.isolated_target).then_some(ctx.app_control).flatten(),
                // Primary sessions advertise lazily (hot core + load_tools);
                // sub-agents strip this back to `None` (eager) via
                // `deps_for_subagent`. Worker sessions are ALSO eager, same
                // as a sub-agent, so their primary session gets `None` here.
                // An isolated-target
                // session (an explicit-mention or retried delegated main
                // child) is likewise eager: it executes against a complete
                // immutable target profile snapshot, so its tool allowlist is
                // exact from the first turn — lazy load_tools staging would
                // silently overgrant beyond the target's configured filter.
                activated_tools: if ctx.isolated_target
                    || matches!(ctx.kind, crate::domain::SessionKind::Worker)
                {
                    None
                } else {
                    Some(std::sync::Arc::new(tokio::sync::Mutex::new(
                        std::collections::BTreeSet::new(),
                    )))
                },
                // Every agent tool call — even one an interactive human turn
                // triggers — is the AGENT deciding to call a tool, not a
                // direct human action, so every autonomous agent session
                // (Project/Chat/Worker) runs as `Agent` origin, engaging the
                // negative-space storage guard; the human acts as `User`
                // through Cockpit/TUI instead. A Worker is an unattended
                // agent — it must be at least as guarded as an attended
                // chat, never less (avoiding the "unattended-with-more-power"
                // inversion). Review never routes through here: its fork
                // builds `RunnerDeps` directly with `BackgroundReview`, so
                // the `Review` arm below is defensively dead.
                write_origin: match ctx.kind {
                    crate::domain::SessionKind::Project
                    | crate::domain::SessionKind::Chat
                    | crate::domain::SessionKind::Worker => crate::domain::WriteOrigin::Agent,
                    // Defensively least-privileged: this arm is dead (the review
                    // fork builds its own `RunnerDeps` with `BackgroundReview` and
                    // never routes through here), but if ever reached it must not
                    // grant the most-privileged `User` origin.
                    crate::domain::SessionKind::Review => {
                        crate::domain::WriteOrigin::BackgroundReview
                    }
                },
                delegation_catalog,
            }),
            live_cancel: Mutex::new(None),
            turn_lock: tokio::sync::Mutex::new(()),
        }))
    }
}

/// A live native session. `send_prompt` runs one full turn to completion.
pub struct NativeSession {
    deps: Mutex<runner::RunnerDeps>,
    session_pk: String,
    automation_events: Option<Arc<dyn crate::automation::AutomationEventSink>>,
    /// The in-flight turn's cancellation token, set for the duration of
    /// `send_prompt` so `cancel`/`end` can trip it.
    live_cancel: Mutex<Option<CancellationToken>>,
    /// Serializes turns: two concurrent `send_prompt`s (double-send, gateway +
    /// UI race) must never interleave their `provider_turns` appends, or the
    /// ledger's user/assistant alternation — and its tool_use/tool_result
    /// pairing — breaks durably.
    turn_lock: tokio::sync::Mutex<()>,
    /// Mid-turn steering buffer (Task B3) — the SAME buffer cloned into
    /// `deps.steer`, so a `steer()` call here is visible to whatever turn is
    /// currently running in `send_prompt`/`drive()`.
    steer: steer::SteerBuffer,
}

#[async_trait]
impl HarnessSession for NativeSession {
    async fn send_prompt(&self, prompt: TurnPrompt) -> anyhow::Result<()> {
        // One turn at a time per session. A queued second prompt simply waits;
        // `cancel()` trips only the CURRENT turn's token (the queued turn gets
        // a fresh one when it starts).
        let _turn = self.turn_lock.lock().await;
        let deps = self.deps.lock().unwrap().clone();
        let cancel = CancellationToken::new();
        *self.live_cancel.lock().unwrap() = Some(cancel.clone());
        let result = runner::run_turn(&deps, prompt, cancel).await;
        *self.live_cancel.lock().unwrap() = None;
        result
    }

    async fn cancel(&self) -> anyhow::Result<()> {
        if let Some(tok) = self.live_cancel.lock().unwrap().as_ref() {
            tok.cancel();
        }
        Ok(())
    }

    async fn end(&self) -> anyhow::Result<()> {
        // Trip any in-flight turn; there is no external process to tear down.
        if let Some(tok) = self.live_cancel.lock().unwrap().as_ref() {
            tok.cancel();
        }
        // Plugin hooks: observational `session.end`. `end()` is called from
        // exactly one place — `ControlPlane::end_session`'s teardown, the
        // sole path that removes the live handle from `running` — so this
        // fires once per real session end, never on a `stop_session`
        // interrupt (which cancels but does not `end()`).
        let deps = self.deps.lock().unwrap().clone();
        let _ = hooks::fire_hook(
            &deps.store,
            &deps.work_dir,
            hooks::HookEvent::SessionEnd,
            &json!({ "session": self.session_pk.clone(), "reason": "ended" }),
            None,
        )
        .await;
        crate::automation::dispatch_lifecycle_observation(
            self.automation_events.clone(),
            crate::automation::TriggerKind::SessionEnd,
            self.session_pk.clone(),
            json!({ "reason": "ended" }),
        );
        Ok(())
    }

    async fn dispatch_retry_child(
        &self,
        child: crate::delegation::RunHandle,
    ) -> anyhow::Result<()> {
        let deps = self.deps.lock().unwrap().clone();
        match child.run.agent_kind {
            crate::domain::AgentRunKind::MainDelegate => {
                runner::dispatch_retry_main_delegate(deps, child)
            }
            crate::domain::AgentRunKind::Subagent => runner::dispatch_retry_subagent(deps, child),
            crate::domain::AgentRunKind::Primary => anyhow::bail!("only child runs can be retried"),
        }
    }

    async fn refresh_primary_turn(&self, primary: crate::harness::PrimaryTurnConfig) {
        // Share `turn_lock` with `send_prompt` so this can only replace the
        // queued turn's configuration. The in-flight turn holds a cloned
        // `RunnerDeps` snapshot until it completes.
        let _turn = self.turn_lock.lock().await;
        let mut deps = self.deps.lock().unwrap();
        // The control plane builds this config registry-blind (it has no tool
        // registry), so its ToolFilter cannot express plugin/app bindings.
        // Rebuild the filter against the live registry — the same resolution
        // `start_session` uses — so bound plugin/app tools survive every
        // follow-up prompt instead of collapsing to All (natives empty) or
        // Only(natives) (natives set).
        let mut agent_tools = primary.agent_tools;
        agent_tools.tools = tool_filter_for_profile(&primary.agent.profile, &deps.tools.names());
        deps.primary_agent = primary.agent;
        deps.run_id = primary.run_id;
        deps.root_run_id = primary.root_run_id;
        deps.model = primary.model;
        deps.perm_mode = Arc::new(std::sync::Mutex::new(primary.perm_mode));
        deps.agent = agent_tools;
        deps.allowed_skills = primary.allowed_skills;
    }

    fn set_perm_mode(&self, mode: crate::domain::PermMode) {
        // Live update: the next turn's tool gate reads this fresh, so a
        // composer/project-settings permission change applies without a restart.
        self.deps.lock().unwrap().set_perm_mode(mode);
    }

    fn agent_session_id(&self) -> Option<String> {
        // The native runtime owns its own history (the provider_turns ledger),
        // so the session_pk is a stable, always-present resume id.
        Some(self.session_pk.clone())
    }

    fn steer(&self, text: String) {
        // Never touches turn_lock/live_cancel: this queues for whatever turn
        // is (or will be) running, it does not interrupt or race it.
        self.steer.push(text);
    }
}

/// Builds [`NativeHarness`] instances for the registry.
pub struct NativeHarnessFactory {
    llm_factory: Arc<dyn llm::LlmStreamFactory>,
}

impl NativeHarnessFactory {
    pub fn new() -> Self {
        NativeHarnessFactory {
            llm_factory: Arc::new(llm::RouterLlmStreamFactory),
        }
    }

    pub fn with_llm_factory(llm_factory: Arc<dyn llm::LlmStreamFactory>) -> Self {
        NativeHarnessFactory { llm_factory }
    }
}

impl Default for NativeHarnessFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl HarnessFactory for NativeHarnessFactory {
    fn create(&self) -> anyhow::Result<Arc<dyn Harness>> {
        Ok(Arc::new(NativeHarness::with_llm_factory(
            self.llm_factory.clone(),
        )))
    }
}

/// The `native` built-in plugin: harness-only, no external binary — the
/// native runtime runs the agentic loop in-process (see the module doc).
pub fn native_plugin() -> CorePlugin {
    native_plugin_with_llm_factory(Arc::new(llm::RouterLlmStreamFactory))
}

/// Construct with a custom LLM stream factory (used by tests, mirroring the
/// old `NativeIntegration::with_llm_factory` seam).
pub fn native_plugin_with_llm_factory(llm_factory: Arc<dyn llm::LlmStreamFactory>) -> CorePlugin {
    CorePlugin {
        manifest: PluginManifest {
            contract: ryuzi_plugin_sdk::CONTRACT_VERSION,
            id: NATIVE_ID.to_string(),
            name: "Ryuzi".to_string(),
            version: "0.0.0".to_string(),
            publisher: "ryuzi".to_string(),
            description: "Ryuzi's built-in agent runtime — runs the loop and tools in-process, using your configured model providers".to_string(),
            homepage: None,
            icon: Some("cpu".to_string()),
            categories: vec!["runtime".to_string()],
            slot: None,
            verified: true,
            experimental: false,
            auth: None,
            settings: vec![],
            component: None,
            permissions: Default::default(),
            oauth: vec![],
            provider: None,
            tools: vec![],
            mcp: vec![],
            hooks: vec![],
            jobs: vec![],
            gateway: false,
        },
        harness: Some(Arc::new(NativeHarnessFactory::with_llm_factory(
            llm_factory,
        ))),
        gateway: None,
        connector: None,
        provider: None,
        source: PluginSource::Builtin,
    }
}

/// `pub(crate)` (the same reason `mcp_http::tests` is): `api::apps_api`'s
/// tests reuse [`tests::spawn_auth_echo_server`] to prove the Apps card's
/// `oauth_connect_available` and this module's session-time auth precedence
/// read the SAME predicate, which needs one fixture observing one wire.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::agents::types::NativeToolDecision;
    use crate::approval::ApprovalHub;
    use crate::domain::PermMode;
    use crate::llm_router::client::AnthropicEvent;
    use crate::store::Store;
    use std::collections::BTreeMap;
    use tokio::sync::broadcast;

    /// Builds a `permissions.native` decision map where every listed tool is
    /// `Allow`. NOTE: tools NOT listed here are `Ask` (still enabled, per
    /// `tool_filter_for_profile`'s absent-entry semantics) — use [`off_map`]
    /// when a fixture needs to positively EXCLUDE every other builtin.
    fn allow_map(tools: &[&str]) -> BTreeMap<String, NativeToolDecision> {
        tools
            .iter()
            .map(|tool| (tool.to_string(), NativeToolDecision::Allow))
            .collect()
    }

    /// Builds a `permissions.native` decision map where every listed tool is
    /// `Off` (excluded from both advertisement and the permission gate). Used
    /// to positively narrow a profile to a small allow-set under the new
    /// absent-means-`Ask`-means-enabled semantics, where clearing the map no
    /// longer means "no natives" — it means "every native is enabled".
    fn off_map(tools: &[String]) -> BTreeMap<String, NativeToolDecision> {
        tools
            .iter()
            .map(|tool| (tool.clone(), NativeToolDecision::Off))
            .collect()
    }

    /// A factory that hands every session the same scripted conversation.
    struct ScriptedFactory {
        turns: Vec<Vec<AnthropicEvent>>,
    }
    impl llm::LlmStreamFactory for ScriptedFactory {
        fn create(&self, _store: Arc<Store>) -> Arc<dyn llm::LlmStream> {
            Arc::new(runner::testutil::ScriptedLlm::new(self.turns.clone()))
        }
    }

    async fn ctx_for(store: Arc<Store>, work_dir: std::path::PathBuf) -> SessionCtx {
        crate::llm_router::connections::add_connection(
            &store,
            conn_for_resolution_tests("test-anthropic", "anthropic", "test/model"),
        )
        .await
        .unwrap();
        crate::agents::bootstrap::ensure_default_routes(&store)
            .await
            .unwrap();
        if store.get_session("sess").await.unwrap().is_none() {
            store
                .insert_session(crate::domain::Session {
                    session_pk: "sess".into(),
                    primary_agent_id: Some("ryuzi".into()),
                    primary_agent_snapshot: Some(crate::domain::AgentIdentitySnapshot {
                        id: "ryuzi".into(),
                        name: "Ryuzi".into(),
                        avatar_color: "blue".into(),
                        avatar_pet: None,
                    }),
                    project_id: None,
                    agent_session_id: None,
                    worktree_path: None,
                    branch: None,
                    title: Some("test".into()),
                    status: crate::domain::SessionStatus::Idle,
                    perm_mode: PermMode::BypassPermissions,
                    started_by: None,
                    created_at: Some(0),
                    last_active: Some(0),
                    resume_attempts: 0,
                    branch_owned: false,
                    kind: crate::domain::SessionKind::Chat,
                    speaker: None,
                    agent: None,
                    parent_session_pk: None,
                    archived_at: None,
                })
                .await
                .unwrap();
        }
        let (events, _rx) = broadcast::channel(64);
        let persistence = crate::agents::bootstrap::AgentPersistence::temporary(store.clone())
            .await
            .unwrap();
        let primary_agent = persistence
            .registry
            .resolved_snapshot("ryuzi")
            .await
            .unwrap();
        let delegation = crate::delegation::DelegationRuntime::new(
            store.clone(),
            persistence.registry.clone(),
            None,
            events.clone(),
        );
        let run = delegation
            .begin_primary("sess", primary_agent.clone(), "test")
            .await
            .unwrap();
        SessionCtx {
            session_pk: "sess".into(),
            primary_agent,
            run_id: run.run.run_id.clone(),
            root_run_id: run.run.run_id,
            delegation,
            main_agent_id: "ryuzi".into(),
            project_id: None,
            kind: crate::domain::SessionKind::Chat,
            agent: None,
            isolated_target: false,
            work_dir: work_dir.clone(),
            artifacts: Arc::new(crate::artifacts::ArtifactService::new(
                store.clone(),
                crate::artifacts::ArtifactStorage::new(work_dir.join("artifacts")),
                crate::artifacts::ArtifactConfig {
                    max_bytes: 26_214_400,
                    session_max_bytes: 262_144_000,
                    read_max_bytes: 50_000,
                },
            )),
            attachments_dir: None,
            perm_mode: PermMode::BypassPermissions,
            model: Some("test/model".into()),
            effort: None,
            resume: None,
            mcp_servers: vec![],
            mcp_principals: std::collections::HashMap::new(),
            plugin_command_roots: vec![],
            plugin_skill_roots: vec![],
            component_mcp: vec![],
            events,
            approvals: Arc::new(ApprovalHub::new()),
            automation_events: None,
            background: super::background::BackgroundRegistry::new(),
            agent_knowledge: persistence.knowledge,
            learning_queue: persistence.learning,
            store,
            telemetry: Arc::new(crate::telemetry::NoopTelemetry),
            app_control: None,
        }
    }

    #[tokio::test]
    async fn v2_session_version_stays_frozen_after_global_toggle_to_v1() {
        let db = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(db.path()).await.unwrap());
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_for(store.clone(), dir.path().to_path_buf()).await;
        store
            .set_setting(
                crate::domain::WriteOrigin::User,
                "native_tools.version",
                "v2",
            )
            .await
            .unwrap();
        assert_eq!(
            resolve_native_tools_version(&store, &ctx.session_pk, &ctx.run_id)
                .await
                .unwrap(),
            capabilities::NativeToolsVersion::V2
        );

        store
            .set_setting(
                crate::domain::WriteOrigin::User,
                "native_tools.version",
                "v1",
            )
            .await
            .unwrap();
        assert_eq!(
            resolve_native_tools_version(&store, &ctx.session_pk, &ctx.run_id)
                .await
                .unwrap(),
            capabilities::NativeToolsVersion::V2
        );
    }

    #[test]
    fn native_plugin_registers_under_native_id() {
        let mut regs = crate::plugins::Registries::new();
        regs.add_plugin(native_plugin());
        assert!(regs.plugins.get(NATIVE_ID).is_some());
        assert!(regs.gateway.get(NATIVE_ID).is_none());
    }

    #[test]
    fn durable_primary_adapter_uses_the_profile_id_and_a_fail_closed_tool_placeholder() {
        let profile = crate::agents::bootstrap::default_ryuzi_profile("ryuzi".into());
        let adapted = adapt_primary_profile(&profile).unwrap();

        assert_eq!(adapted.agent.name, "ryuzi");
        // adapt_primary_profile is registry-blind, so it can no longer derive
        // a real filter from profile.tools.native; it emits the fail-closed
        // placeholder and leaves rebuilding to tool_filter_for_profile.
        assert_eq!(adapted.agent.tools, agents::ToolFilter::Only(Vec::new()));
        assert_eq!(adapted.allowed_skills, None);
    }

    #[test]
    fn durable_primary_adapter_still_filters_skills_without_build_fallback() {
        let mut profile = crate::agents::bootstrap::default_ryuzi_profile("ryuzi".into());
        profile.permissions.native = allow_map(&["read"]);
        profile.skills = vec!["release".into()];
        let adapted = adapt_primary_profile(&profile).unwrap();

        assert_eq!(adapted.agent.name, "ryuzi");
        // Tools are always the fail-closed placeholder regardless of
        // profile.tools.native; only the registry-independent skills mapping
        // is exercised here.
        assert_eq!(adapted.agent.tools, agents::ToolFilter::Only(Vec::new()));
        assert_eq!(adapted.allowed_skills, Some(vec!["release".into()]));
    }

    #[tokio::test]
    async fn isolated_target_cannot_read_parent_attachments_or_use_parent_facade() {
        use runner::testutil::{
            input_json_delta, message_delta, message_stop, tool_use_start, RecordingLlm,
        };

        let work_dir = tempfile::tempdir().unwrap();
        let attachments = tempfile::tempdir().unwrap();
        let attachment = attachments.path().join("parent-only.txt");
        tokio::fs::write(&attachment, "parent-only attachment")
            .await
            .unwrap();
        let attachment_db = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(attachment_db.path()).await.unwrap());
        let mut ctx = ctx_for(store.clone(), work_dir.path().to_path_buf()).await;
        let mut target = (*ctx.primary_agent).clone();
        target.profile.id = "mentioned-target".into();
        target.profile.name = "Mentioned target".into();
        target.profile.permissions.native = allow_map(&["read", "app_projects"]);
        target.profile.tools.plugins.clear();
        target.profile.tools.apps.clear();
        ctx.primary_agent = Arc::new(target);
        ctx.main_agent_id = "mentioned-target".into();
        ctx.isolated_target = true;
        ctx.attachments_dir = Some(attachments.path().to_path_buf());
        ctx.app_control = Some(Arc::new(tools::testutil::FakeAppControl::default()));
        let llm = Arc::new(RecordingLlm::new(vec![
            vec![
                tool_use_start(0, "read-parent-attachment", "read"),
                input_json_delta(0, &serde_json::json!({ "path": attachment }).to_string()),
                message_delta("tool_use"),
                message_stop(),
            ],
            vec![
                tool_use_start(0, "parent-facade", "app_projects"),
                input_json_delta(0, r#"{"action":"list"}"#),
                message_delta("tool_use"),
                message_stop(),
            ],
            vec![
                runner::testutil::text_delta("done"),
                message_delta("end_turn"),
                message_stop(),
            ],
        ]));
        struct OneShotFactory(Arc<RecordingLlm>);
        impl llm::LlmStreamFactory for OneShotFactory {
            fn create(&self, _store: Arc<Store>) -> Arc<dyn llm::LlmStream> {
                self.0.clone()
            }
        }
        let harness = NativeHarness::with_llm_factory(Arc::new(OneShotFactory(llm)));
        let session = harness.start_session(ctx).await.unwrap();
        session
            .send_prompt(TurnPrompt::text("inspect attachment", "inspect attachment"))
            .await
            .unwrap();

        let tool_rows = store.list_messages("sess").await.unwrap();
        let tool_row = tool_rows
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("read-parent-attachment"))
            .expect("the mentioned child records its read attempt");
        assert_eq!(tool_row.status.as_deref(), Some("failed"));
        assert!(
            !tool_row.payload["output"]
                .as_str()
                .is_some_and(|output| output.contains("parent-only attachment")),
            "the mentioned child must not receive its parent's attachment read root"
        );
        let facade_row = tool_rows
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("parent-facade"))
            .expect("the mentioned child records its parent-facade attempt");
        assert_eq!(facade_row.status.as_deref(), Some("failed"));
        assert!(
            facade_row.payload["output"]
                .as_str()
                .is_some_and(|output| output.contains("not available in this context")),
            "the mentioned child must not receive its parent's app facade"
        );
    }

    #[tokio::test]
    async fn isolated_target_uses_complete_profile_tool_allowlist_after_registry_attach() {
        use runner::testutil::{message_delta, message_stop, text_delta, RecordingLlm};

        let work_dir = tempfile::tempdir().unwrap();
        let profile_db = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(profile_db.path()).await.unwrap());
        let mut ctx = ctx_for(store, work_dir.path().to_path_buf()).await;
        let mut target = (*ctx.primary_agent).clone();
        target.profile.id = "mentioned-target".into();
        target.profile.name = "Mentioned target".into();
        // Under the new absent=Ask=enabled semantics, clearing the map no
        // longer means "no natives" — every builtin would default to `Ask`
        // (still enabled). Explicitly `Off` every builtin so this fixture
        // keeps testing its original intent: a target with plugin/app
        // bindings that never resolve against the (empty) registry must not
        // fall back to advertising the native tool registry.
        target.profile.permissions.native = off_map(&tools::ToolRegistry::builtin_ids());
        target.profile.tools.plugins = vec!["github.search".into()];
        target.profile.tools.apps = vec!["slack".into()];
        ctx.primary_agent = Arc::new(target);
        ctx.main_agent_id = "mentioned-target".into();
        ctx.isolated_target = true;
        let llm = Arc::new(RecordingLlm::new(vec![vec![
            text_delta("done"),
            message_delta("end_turn"),
            message_stop(),
        ]]));
        struct OneShotFactory(Arc<RecordingLlm>);
        impl llm::LlmStreamFactory for OneShotFactory {
            fn create(&self, _store: Arc<Store>) -> Arc<dyn llm::LlmStream> {
                self.0.clone()
            }
        }
        let harness = NativeHarness::with_llm_factory(Arc::new(OneShotFactory(llm.clone())));
        let session = harness.start_session(ctx).await.unwrap();
        session
            .send_prompt(TurnPrompt::text("inspect", "inspect"))
            .await
            .unwrap();

        let bodies = llm.bodies.lock().unwrap();
        let advertised = bodies[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(
            advertised.is_empty(),
            "unattached configured tools must not overgrant native tools: {advertised:?}"
        );
    }

    /// A plugin/app-only profile (no natives bound) must keep advertising ZERO
    /// native tools on the SECOND prompt. The control plane refreshes every
    /// follow-up prompt with a registry-blind config (`PrimaryTurn::config()`
    /// → `refresh_primary_turn`); before the fix that clobbered the filter to
    /// `ToolFilter::All` and advertised the whole native registry.
    #[tokio::test]
    async fn refresh_primary_turn_rebuilds_bindings_against_live_registry() {
        use runner::testutil::{message_delta, message_stop, text_delta, RecordingLlm};

        let work_dir = tempfile::tempdir().unwrap();
        let profile_db = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(profile_db.path()).await.unwrap());
        let mut ctx = ctx_for(store, work_dir.path().to_path_buf()).await;
        let mut primary = (*ctx.primary_agent).clone();
        primary.profile.id = "plugin-app-only".into();
        // See the sibling `isolated_target_uses_complete_profile_tool_allowlist_
        // after_registry_attach` fixture comment: clearing the map now means
        // every builtin defaults to `Ask` (enabled), not excluded, so this
        // "plugin/app-only" fixture must `Off` every builtin explicitly.
        primary.profile.permissions.native = off_map(&tools::ToolRegistry::builtin_ids());
        primary.profile.tools.plugins = vec!["github.search".into()];
        primary.profile.tools.apps = vec!["slack".into()];
        ctx.primary_agent = Arc::new(primary);
        ctx.main_agent_id = "plugin-app-only".into();
        ctx.isolated_target = true;
        let refresh_agent = ctx.primary_agent.clone();
        let run_id = ctx.run_id.clone();
        let root_run_id = ctx.root_run_id.clone();

        let turn = vec![
            text_delta("done"),
            message_delta("end_turn"),
            message_stop(),
        ];
        let llm = Arc::new(RecordingLlm::new(vec![turn.clone(), turn]));
        struct TwoTurnFactory(Arc<RecordingLlm>);
        impl llm::LlmStreamFactory for TwoTurnFactory {
            fn create(&self, _store: Arc<Store>) -> Arc<dyn llm::LlmStream> {
                self.0.clone()
            }
        }
        let harness = NativeHarness::with_llm_factory(Arc::new(TwoTurnFactory(llm.clone())));
        let session = harness.start_session(ctx).await.unwrap();
        session
            .send_prompt(TurnPrompt::text("first", "first"))
            .await
            .unwrap();

        // Exactly what `continue_session_with_primary_turn` does per prompt:
        // build a registry-blind config and push it at the live session.
        let registry_blind =
            primary_turn_config(refresh_agent, run_id, root_run_id, PermMode::Default).unwrap();
        session.refresh_primary_turn(registry_blind).await;
        session
            .send_prompt(TurnPrompt::text("second", "second"))
            .await
            .unwrap();

        let bodies = llm.bodies.lock().unwrap();
        let advertised = |i: usize| -> Vec<String> {
            bodies[i]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect()
        };
        assert!(
            advertised(0).is_empty(),
            "start-path guard (already correct today): {:?}",
            advertised(0)
        );
        assert!(
            advertised(1).is_empty(),
            "a prompt-time refresh must not clobber plugin/app bindings into ToolFilter::All: {:?}",
            advertised(1)
        );
    }

    #[test]
    fn profile_tool_filter_resolves_native_plugin_and_app_tools_without_fallback() {
        let mut profile = crate::agents::bootstrap::default_ryuzi_profile("target".into());
        // Absent = `Ask` = enabled now, so narrowing to exactly `read` (plus
        // the plugin/app bindings below) requires explicitly `Off`-ing every
        // other builtin — otherwise `write` (present in `available` and
        // unmapped) would also resolve as enabled.
        profile.permissions.native = off_map(&tools::ToolRegistry::builtin_ids());
        profile
            .permissions
            .native
            .insert("read".into(), NativeToolDecision::Allow);
        profile.tools.plugins = vec!["github.search".into()];
        profile.tools.apps = vec!["slack".into()];
        let available = vec![
            "read".into(),
            "write".into(),
            "mcp__github__search".into(),
            "mcp__slack__send".into(),
        ];

        assert_eq!(
            tool_filter_for_profile(&profile, &available),
            agents::ToolFilter::Only(vec![
                "mcp__github__search".into(),
                "mcp__slack__send".into(),
                "read".into(),
            ])
        );

        // A configuration with every native explicitly `Off` and unbound
        // plugin/app entries must never broaden to every registered tool —
        // same guard as above, restated under the new semantics (previously
        // this relied on an empty/non-matching native list; now it requires
        // an explicit `Off` map since absence alone no longer excludes).
        profile.permissions.native = off_map(&tools::ToolRegistry::builtin_ids());
        profile.tools.plugins = vec!["missing.tool".into()];
        profile.tools.apps = vec!["missing-app".into()];
        assert_eq!(
            tool_filter_for_profile(&profile, &available),
            agents::ToolFilter::Only(Vec::new()),
            "a nonempty configuration must never broaden to every registered tool"
        );
    }

    /// Step 1 of the brief: the new absent-entry semantics — an unmapped
    /// native tool defaults to `Ask` (still enabled/advertised), and only an
    /// explicit `Off` excludes it. `write` here is `Off`, so it drops out
    /// even though it's present in `available`; `read` is absent (never
    /// mentioned in the map) and still resolves as enabled.
    #[test]
    fn filter_includes_unmapped_natives_and_excludes_off() {
        let mut profile = crate::agents::bootstrap::default_ryuzi_profile("t".into());
        profile.permissions.native.clear();
        profile
            .permissions
            .native
            .insert("write".into(), NativeToolDecision::Off);
        profile.tools.plugins = vec!["github.search".into()];
        let available = vec!["read".into(), "write".into(), "mcp__github__search".into()];
        assert_eq!(
            tool_filter_for_profile(&profile, &available),
            agents::ToolFilter::Only(vec!["mcp__github__search".into(), "read".into()])
        );
    }

    // PR-2 fix F: a bare plugin id in tools.plugins used to demand an exact
    // match on `mcp__<id>__` (empty tool segment) and therefore granted
    // NOTHING. It must grant every tool the plugin contributes, mirroring the
    // app arm's prefix match. The namespaced form stays exact.
    #[test]
    fn bare_plugin_id_grants_all_of_that_plugins_tools() {
        let mut profile = crate::agents::bootstrap::default_ryuzi_profile("target".into());
        profile.permissions.native = off_map(&tools::ToolRegistry::builtin_ids());
        profile.tools.plugins = vec!["github".into()];
        let available = vec![
            "mcp__github__create_issue".to_string(),
            "mcp__github__get_repo".to_string(),
            "mcp__discord__send".to_string(),
            "read_file".to_string(),
        ];
        let agents::ToolFilter::Only(allowed) = tool_filter_for_profile(&profile, &available)
        else {
            panic!("expected Only");
        };
        assert!(allowed.contains(&"mcp__github__create_issue".to_string()));
        assert!(allowed.contains(&"mcp__github__get_repo".to_string()));
        assert!(!allowed.contains(&"mcp__discord__send".to_string()));
    }

    #[test]
    fn namespaced_plugin_binding_stays_exact() {
        let mut profile = crate::agents::bootstrap::default_ryuzi_profile("target".into());
        profile.permissions.native = off_map(&tools::ToolRegistry::builtin_ids());
        profile.tools.plugins = vec!["github.create_issue".into()];
        let available = vec![
            "mcp__github__create_issue".to_string(),
            "mcp__github__get_repo".to_string(),
        ];
        let agents::ToolFilter::Only(allowed) = tool_filter_for_profile(&profile, &available)
        else {
            panic!("expected Only");
        };
        assert!(allowed.contains(&"mcp__github__create_issue".to_string()));
        assert!(!allowed.contains(&"mcp__github__get_repo".to_string()));
    }

    #[test]
    fn durable_primary_adapter_accepts_plugin_app_and_permission_capabilities() {
        let mut profile = crate::agents::bootstrap::default_ryuzi_profile("ryuzi".into());
        profile.tools.plugins = vec!["github.search".into()];
        profile.tools.apps = vec!["github".into()];
        profile.permissions.rules = vec![crate::agents::types::PermissionRule {
            id: "deny-bash".into(),
            tool: "bash".into(),
            decision: crate::agents::types::PermissionDecision::Deny,
            command_prefix: None,
        }];

        let adapted = adapt_primary_profile(&profile).unwrap();
        assert_eq!(adapted.agent.permission_rules, profile.permissions.rules);
    }
    #[test]
    fn native_plugin_manifest_has_expected_identity() {
        let plugin = native_plugin();
        assert_eq!(plugin.manifest.contract, 2);
        assert_eq!(plugin.manifest.id, "native");
        assert_eq!(plugin.manifest.name, "Ryuzi");
        assert_eq!(plugin.manifest.publisher, "ryuzi");
        assert!(plugin.manifest.verified);
        assert_eq!(plugin.manifest.categories, vec!["runtime".to_string()]);
        assert_eq!(plugin.manifest.icon.as_deref(), Some("cpu"));
        assert!(plugin.harness.is_some());
        assert!(plugin.gateway.is_none());
        assert!(plugin.connector.is_none());
    }

    /// Feature C1b: `start_session` must fire the `session.start` hook
    /// (observational) once the model/agent are resolved, carrying the
    /// session id, project id, model, and work_dir. This exercises the real
    /// `NativeHarness::start_session` call site, not just `hooks::run`'s
    /// dispatcher contract (covered separately in `hooks.rs`).
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn start_session_fires_the_session_start_hook() {
        use serde_json::Value;
        use std::os::unix::fs::PermissionsExt;
        let _guard = StateDirGuard::new();
        let dir = tempfile::tempdir().unwrap();
        let hook_dir = dir.path().join(".ryuzi/hooks/session.start");
        std::fs::create_dir_all(&hook_dir).unwrap();
        let capture = dir.path().join("captured.json");
        let script = hook_dir.join("capture.sh");
        std::fs::write(&script, format!("#!/bin/sh\ncat > {}\n", capture.display())).unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(tmp.path()).await.unwrap());
        crate::harness::native::hooks::trust_hooks(&store, dir.path())
            .await
            .unwrap();
        let factory = Arc::new(ScriptedFactory { turns: vec![] });
        let plugin = native_plugin_with_llm_factory(factory);
        let harness = plugin.harness.unwrap().create().unwrap();
        let _session = harness
            .start_session(ctx_for(store.clone(), dir.path().to_path_buf()).await)
            .await
            .unwrap();

        let captured: Value =
            serde_json::from_str(&std::fs::read_to_string(&capture).unwrap()).unwrap();
        assert_eq!(captured["session"], "sess");
        assert_eq!(captured["work_dir"], dir.path().display().to_string());
        // `project`/`model` are present regardless of what they resolve to —
        // the shape of the payload is what this test asserts, not the native
        // model-routing outcome for a fresh store with no connections.
        assert!(captured.get("project").is_some());
        assert!(captured.get("model").is_some());
    }

    /// Feature C1c: the session-teardown seam is `NativeSession::end()` —
    /// the only place `HarnessSession::end` is invoked is
    /// `ControlPlane::end_session`'s real teardown (never the
    /// interrupt-only `stop_session` path), so firing `session.end` there
    /// fires exactly once per real session end. Also proves the hook is NOT
    /// fired merely by starting a session.
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn end_fires_the_session_end_hook() {
        use serde_json::Value;
        use std::os::unix::fs::PermissionsExt;
        let _guard = StateDirGuard::new();
        let dir = tempfile::tempdir().unwrap();
        let hook_dir = dir.path().join(".ryuzi/hooks/session.end");
        std::fs::create_dir_all(&hook_dir).unwrap();
        let capture = dir.path().join("captured.json");
        let script = hook_dir.join("capture.sh");
        std::fs::write(&script, format!("#!/bin/sh\ncat > {}\n", capture.display())).unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(tmp.path()).await.unwrap());
        crate::harness::native::hooks::trust_hooks(&store, dir.path())
            .await
            .unwrap();
        let factory = Arc::new(ScriptedFactory { turns: vec![] });
        let plugin = native_plugin_with_llm_factory(factory);
        let harness = plugin.harness.unwrap().create().unwrap();
        let session = harness
            .start_session(ctx_for(store.clone(), dir.path().to_path_buf()).await)
            .await
            .unwrap();

        assert!(!capture.exists(), "session.end must not fire before end()");
        session.end().await.unwrap();

        let captured: Value =
            serde_json::from_str(&std::fs::read_to_string(&capture).unwrap()).unwrap();
        assert_eq!(captured["session"], "sess");
        assert_eq!(captured["reason"], "ended");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn live_session_resolves_project_command_created_updated_and_deleted_after_start() {
        use crate::domain::Project;
        use crate::llm_router::connections::{self, ConnectionData, ConnectionRow};
        use runner::testutil::{
            input_json_delta, message_delta, message_stop, text_delta, tool_use_start, RecordingLlm,
        };

        // Project command CRUD is gone from core (commands are now
        // global-only); write the `.ryuzi/commands` file directly to
        // exercise the registry's still-supported project-file discovery.
        fn write_project_command_file(
            work_dir: &std::path::Path,
            template: &str,
            agent: Option<&str>,
            model: Option<&str>,
            subtask: bool,
        ) {
            let dir = work_dir.join(".ryuzi/commands");
            std::fs::create_dir_all(&dir).unwrap();
            let mut frontmatter = "---\ndescription: Ship a release\n".to_string();
            if let Some(agent) = agent {
                frontmatter.push_str(&format!("agent: {agent}\n"));
            }
            if let Some(model) = model {
                frontmatter.push_str(&format!("model: {model}\n"));
            }
            frontmatter.push_str(&format!("subtask: {subtask}\n---\n{template}"));
            std::fs::write(dir.join("ship.md"), frontmatter).unwrap();
        }

        let _guard = StateDirGuard::new();
        let dir = tempfile::tempdir().unwrap();
        let canonical_workdir = dir.path().join("project");
        let active_worktree = dir.path().join("session-worktree");
        std::fs::create_dir_all(&canonical_workdir).unwrap();
        std::fs::create_dir_all(&active_worktree).unwrap();
        std::fs::create_dir_all(canonical_workdir.join(".ryuzi/agents")).unwrap();
        std::fs::write(
            canonical_workdir.join(".ryuzi/agents/reviewer.md"),
            "---\ndescription: Canonical reviewer\n---\nYou are the canonical reviewer.",
        )
        .unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(tmp.path()).await.unwrap());
        store
            .insert_project(Project {
                project_id: "project-1".into(),
                name: "project".into(),
                workdir: canonical_workdir.display().to_string(),
                source: None,
                model: None,
                effort: None,
                perm_mode: PermMode::BypassPermissions,
                created_at: None,
                is_git: false,
            })
            .await
            .unwrap();
        connections::add_connection(
            &store,
            ConnectionRow {
                id: "canonical-model".into(),
                provider: "anthropic".into(),
                auth_type: "api_key".into(),
                label: "canonical model".into(),
                priority: 0,
                enabled: true,
                data: ConnectionData {
                    api_key: Some("sk-test".into()),
                    models_override: Some(vec!["canonical-model".into()]),
                    ..Default::default()
                },
                created_at: 1,
                updated_at: 1,
            },
        )
        .await
        .unwrap();
        store
            .set_setting(
                crate::domain::WriteOrigin::User,
                "agent.max_provider_turns",
                "1",
            )
            .await
            .unwrap();
        store
            .set_setting(
                crate::domain::WriteOrigin::User,
                "agent.auto_continue_budget",
                "0",
            )
            .await
            .unwrap();
        let llm = Arc::new(RecordingLlm::new(vec![
            vec![
                text_delta("first"),
                message_delta("end_turn"),
                message_stop(),
            ],
            vec![
                text_delta("second"),
                message_delta("end_turn"),
                message_stop(),
            ],
            vec![
                tool_use_start(0, "canonical-ls", "ls"),
                input_json_delta(0, r#"{"path":"."}"#),
                message_delta("tool_use"),
                message_stop(),
            ],
            vec![
                text_delta("canonical complete"),
                message_delta("end_turn"),
                message_stop(),
            ],
            vec![
                text_delta("third"),
                message_delta("end_turn"),
                message_stop(),
            ],
        ]));
        struct OneShotFactory(Arc<RecordingLlm>);
        impl llm::LlmStreamFactory for OneShotFactory {
            fn create(&self, _store: Arc<Store>) -> Arc<dyn llm::LlmStream> {
                self.0.clone()
            }
        }

        let plugin = native_plugin_with_llm_factory(Arc::new(OneShotFactory(llm.clone())));
        let harness = plugin.harness.unwrap().create().unwrap();
        let mut ctx = ctx_for(store.clone(), active_worktree.clone()).await;
        ctx.project_id = Some("project-1".into());
        ctx.kind = crate::domain::SessionKind::Project;
        let session = harness.start_session(ctx).await.unwrap();

        write_project_command_file(
            &canonical_workdir,
            "Ship v1 $ARGUMENTS",
            Some("reviewer"),
            None,
            false,
        );
        assert!(canonical_workdir.join(".ryuzi/commands/ship.md").exists());
        assert!(
            !active_worktree.join(".ryuzi/commands/ship.md").exists(),
            "the active session worktree must not supply this command"
        );
        session
            .send_prompt(TurnPrompt::text("/ship release", "/ship release"))
            .await
            .unwrap();

        write_project_command_file(
            &canonical_workdir,
            "Ship v2 $ARGUMENTS",
            Some("reviewer"),
            None,
            false,
        );
        session
            .send_prompt(TurnPrompt::text("/ship release", "/ship release"))
            .await
            .unwrap();

        write_project_command_file(
            &canonical_workdir,
            "Ship canonical $ARGUMENTS",
            None,
            Some("canonical-model"),
            true,
        );
        session
            .send_prompt(TurnPrompt::text("/ship release", "/ship release"))
            .await
            .unwrap();

        std::fs::remove_file(canonical_workdir.join(".ryuzi/commands/ship.md")).unwrap();
        session
            .send_prompt(TurnPrompt::text("/ship release", "/ship release"))
            .await
            .unwrap();

        let bodies = llm.bodies.lock().unwrap();
        assert!(bodies[0].to_string().contains("Ship v1 release"));
        assert!(bodies[0]
            .get("system")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|system| system.contains("You are the canonical reviewer.")));
        assert!(bodies[1].to_string().contains("Ship v2 release"));
        assert!(bodies[2].to_string().contains("Ship canonical release"));
        assert_eq!(bodies[2]["model"], "canonical-model");
        assert!(
            !bodies[2]["system"]
                .to_string()
                .contains("You are the canonical reviewer."),
            "an agent-less command must retain the session agent"
        );
        assert_eq!(
            bodies.len(),
            5,
            "subtask commands get a second provider turn despite the one-turn parent setting"
        );
        assert!(bodies[4].to_string().contains("/ship release"));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn session_runs_a_turn_and_exposes_stable_resume_id() {
        use runner::testutil::{message_delta, message_stop, text_delta};
        let _guard = StateDirGuard::new();
        let dir = tempfile::tempdir().unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(tmp.path()).await.unwrap());

        let factory = Arc::new(ScriptedFactory {
            turns: vec![vec![
                text_delta("hello from native"),
                message_delta("end_turn"),
                message_stop(),
            ]],
        });
        let plugin = native_plugin_with_llm_factory(factory);
        let harness = plugin.harness.unwrap().create().unwrap();
        let session = harness
            .start_session(ctx_for(store.clone(), dir.path().to_path_buf()).await)
            .await
            .unwrap();

        assert_eq!(session.agent_session_id().as_deref(), Some("sess"));

        session
            .send_prompt(TurnPrompt::text("hi", "hi"))
            .await
            .unwrap();

        let msgs = store.list_messages("sess").await.unwrap();
        assert!(msgs
            .iter()
            .any(|m| m.role == "assistant" && m.payload["text"] == "hello from native"));

        // cancel()/end() are safe no-ops when idle.
        session.cancel().await.unwrap();
        session.end().await.unwrap();
    }

    /// Redirect `dirs::home_dir()`/`dirs::data_dir()` into a tempdir for the
    /// duration of a test so the agent knowledge bundle resolved below cannot
    /// touch the developer's actual config directory. Process-global env, so
    /// every test using this needs `#[serial]` (mirrors
    /// `control::tests::StateDirGuard`).
    struct StateDirGuard {
        _dir: tempfile::TempDir,
    }
    impl StateDirGuard {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            std::env::set_var("XDG_DATA_HOME", dir.path().join("data"));
            std::env::set_var("HOME", dir.path());
            StateDirGuard { _dir: dir }
        }
    }

    /// The actual wiring bug this task fixes: a chat (project-less) session
    /// previously skipped `MemoryStore` construction entirely (`project_id:
    /// None` short-circuited it in `NativeHarness::start_session`), so a
    /// fact saved by one chat session was invisible to the next. Seed the
    /// GLOBAL and USER memory files `at_default(None)` resolves to, start a
    /// session through the real `Harness` trait with `ctx.project_id: None`
    /// (as `ctx_for` now sets), and confirm both seeded entries reach the
    /// first request's system prompt exactly like `memory_snapshot_reaches_
    /// primary_system_but_not_subagents` proves it does for a project
    /// session in `runner.rs`. A chat session has no project, so `user` is
    /// the only per-person scope it ever sees.
    #[tokio::test]
    #[serial_test::serial]
    async fn chat_session_without_a_project_still_gets_global_and_user_memory() {
        use runner::testutil::{message_delta, message_stop, text_delta, RecordingLlm};
        let _guard = StateDirGuard::new();
        let dir = tempfile::tempdir().unwrap();
        let work_dir = dir.path().to_path_buf();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(tmp.path()).await.unwrap());
        let ctx = ctx_for(store.clone(), work_dir).await;
        let mem =
            memory::MemoryStore::for_agent(ctx.agent_knowledge.clone(), "ryuzi", None).unwrap();
        mem.add(
            memory::MemoryScope::Global,
            "the deploy key lives in 1Password",
        )
        .await
        .unwrap();
        mem.add(memory::MemoryScope::User, "prefers terse answers")
            .await
            .unwrap();

        let llm = Arc::new(RecordingLlm::new(vec![vec![
            text_delta("ok"),
            message_delta("end_turn"),
            message_stop(),
        ]]));
        struct OneShotFactory(Arc<RecordingLlm>);
        impl llm::LlmStreamFactory for OneShotFactory {
            fn create(&self, _store: Arc<Store>) -> Arc<dyn llm::LlmStream> {
                self.0.clone()
            }
        }
        let plugin = native_plugin_with_llm_factory(Arc::new(OneShotFactory(llm.clone())));
        let harness = plugin.harness.unwrap().create().unwrap();
        // ctx_for's SessionCtx carries project_id: None — the chat-session shape.
        let session = harness.start_session(ctx).await.unwrap();
        session
            .send_prompt(TurnPrompt::text("hi", "hi"))
            .await
            .unwrap();

        let bodies = llm.bodies.lock().unwrap();
        let system = bodies[0]["system"].as_str().unwrap_or_default();
        assert!(
            system.contains("the deploy key lives in 1Password"),
            "{system}"
        );
        assert!(system.contains("# Persistent memory (global)"), "{system}");
        assert!(system.contains("prefers terse answers"), "{system}");
        assert!(system.contains("# Persistent memory (user)"), "{system}");
        // No project in a chat session, so no project section.
        assert!(
            !system.contains("# Persistent memory (project)"),
            "{system}"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn concurrent_prompts_on_one_session_are_serialized() {
        use runner::testutil::{message_delta, message_stop, text_delta};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let _guard = StateDirGuard::new();

        /// Holds each provider stream open ~100ms and records how many
        /// streams were ever active at once: >1 means two turns interleaved
        /// their provider calls (and therefore their ledger appends).
        struct OverlapLlm {
            active: Arc<AtomicUsize>,
            max_seen: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl llm::LlmStream for OverlapLlm {
            async fn stream(
                &self,
                _request: crate::llm_router::provenance::LlmRequest,
            ) -> anyhow::Result<crate::llm_router::provenance::RoutedStream> {
                let n = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_seen.fetch_max(n, Ordering::SeqCst);
                let (tx, rx) = tokio::sync::mpsc::channel(8);
                let active = self.active.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let _ = tx.send(Ok(text_delta("ok"))).await;
                    let _ = tx.send(Ok(message_delta("end_turn"))).await;
                    // Mark the stream finished BEFORE the terminal event: a
                    // serialized follow-up turn can only start after
                    // message_stop is consumed, so it never counts as overlap.
                    active.fetch_sub(1, Ordering::SeqCst);
                    let _ = tx.send(Ok(message_stop())).await;
                });
                Ok(crate::llm_router::provenance::RoutedStream {
                    selection: runner::testutil::test_route_selection(),
                    events: rx,
                })
            }
        }

        struct SharedFactory(Arc<OverlapLlm>);
        impl llm::LlmStreamFactory for SharedFactory {
            fn create(&self, _store: Arc<Store>) -> Arc<dyn llm::LlmStream> {
                self.0.clone()
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(tmp.path()).await.unwrap());
        let overlap = Arc::new(OverlapLlm {
            active: Arc::new(AtomicUsize::new(0)),
            max_seen: Arc::new(AtomicUsize::new(0)),
        });
        let plugin = native_plugin_with_llm_factory(Arc::new(SharedFactory(overlap.clone())));
        let harness = plugin.harness.unwrap().create().unwrap();
        let session = harness
            .start_session(ctx_for(store.clone(), dir.path().to_path_buf()).await)
            .await
            .unwrap();

        // Two prompts land on the SAME session at the same time (double-send,
        // UI + gateway race, boot-nudge racing a user prompt).
        let (ra, rb) = tokio::join!(
            session.send_prompt(TurnPrompt::text("one", "one")),
            session.send_prompt(TurnPrompt::text("two", "two")),
        );
        ra.unwrap();
        rb.unwrap();

        assert_eq!(
            overlap.max_seen.load(Ordering::SeqCst),
            1,
            "turns must not interleave provider calls"
        );
        // The durable ledger alternates cleanly: two complete turns in a row.
        let turns = store.list_provider_turns("sess").await.unwrap();
        let roles: Vec<&str> = turns.iter().map(|t| t.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "user", "assistant"]);
    }

    #[tokio::test]
    async fn concurrent_turn_keeps_primary_snapshot_despite_project_runtime_changes() {
        use crate::domain::{Project, Session, SessionStatus};
        use crate::llm_router::connections;
        use crate::llm_router::model_effort::TurnEffortPolicy;
        use runner::testutil::{message_delta, message_stop, text_delta};
        use std::sync::Mutex as StdMutex;

        struct SnapshotLlm {
            policies: StdMutex<Vec<Arc<TurnEffortPolicy>>>,
            first_started: tokio::sync::Notify,
            release_first: tokio::sync::Notify,
        }

        #[async_trait]
        impl llm::LlmStream for SnapshotLlm {
            async fn stream(
                &self,
                request: crate::llm_router::provenance::LlmRequest,
            ) -> anyhow::Result<crate::llm_router::provenance::RoutedStream> {
                let effort_policy = request.metadata.effort_policy;
                let index = {
                    let mut policies = self.policies.lock().unwrap();
                    let index = policies.len();
                    policies.push(effort_policy);
                    index
                };
                let (tx, rx) = tokio::sync::mpsc::channel(8);
                if index == 0 {
                    self.first_started.notify_one();
                    self.release_first.notified().await;
                }
                tokio::spawn(async move {
                    let _ = tx.send(Ok(text_delta("ok"))).await;
                    let _ = tx.send(Ok(message_delta("end_turn"))).await;
                    let _ = tx.send(Ok(message_stop())).await;
                });
                Ok(crate::llm_router::provenance::RoutedStream {
                    selection: runner::testutil::test_route_selection(),
                    events: rx,
                })
            }
        }

        struct SnapshotFactory(Arc<SnapshotLlm>);
        impl llm::LlmStreamFactory for SnapshotFactory {
            fn create(&self, _store: Arc<Store>) -> Arc<dyn llm::LlmStream> {
                self.0.clone()
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(tmp.path()).await.unwrap());
        connections::add_connection(
            &store,
            conn_for_resolution_tests("claude", "anthropic", "model-a"),
        )
        .await
        .unwrap();
        let mut conn = connections::get_connection(&store, "claude")
            .await
            .unwrap()
            .unwrap();
        conn.data.models_override = Some(vec!["model-a".into(), "model-b".into()]);
        connections::update_connection(&store, conn).await.unwrap();
        store
            .insert_project(Project {
                project_id: "p".into(),
                name: "p".into(),
                workdir: dir.path().to_string_lossy().into_owned(),
                source: None,
                model: Some("anthropic/model-a".into()),
                effort: Some("low".into()),
                perm_mode: PermMode::BypassPermissions,
                created_at: Some(0),
                is_git: false,
            })
            .await
            .unwrap();
        store
            .insert_session(Session {
                session_pk: "sess".into(),
                primary_agent_id: None,
                primary_agent_snapshot: None,
                project_id: Some("p".into()),
                agent_session_id: None,
                worktree_path: None,
                branch: None,
                title: Some("titled".into()),
                status: SessionStatus::Running,
                perm_mode: PermMode::BypassPermissions,
                started_by: None,
                created_at: Some(0),
                last_active: Some(0),
                resume_attempts: 0,
                branch_owned: true,
                kind: crate::domain::SessionKind::Project,
                speaker: None,
                agent: None,
                parent_session_pk: None,
                archived_at: None,
            })
            .await
            .unwrap();
        let llm = Arc::new(SnapshotLlm {
            policies: StdMutex::new(Vec::new()),
            first_started: tokio::sync::Notify::new(),
            release_first: tokio::sync::Notify::new(),
        });
        let plugin = native_plugin_with_llm_factory(Arc::new(SnapshotFactory(llm.clone())));
        let harness = plugin.harness.unwrap().create().unwrap();
        let mut ctx = ctx_for(store.clone(), dir.path().to_path_buf()).await;
        ctx.project_id = Some("p".into());
        ctx.kind = crate::domain::SessionKind::Project;
        ctx.model = Some("anthropic/model-a".into());
        ctx.effort = Some("low".into());
        let session = harness.start_session(ctx).await.unwrap();

        let first = session.send_prompt(TurnPrompt::text("one", "one"));
        tokio::pin!(first);
        tokio::select! {
            result = &mut first => panic!("first turn ended before release: {result:?}"),
            _ = llm.first_started.notified() => {}
        }
        store
            .update_project_runtime("p", Some("anthropic/model-b".into()), Some("high".into()))
            .await
            .unwrap();
        let second = session.send_prompt(TurnPrompt::text("two", "two"));
        llm.release_first.notify_one();
        let (first_result, second_result) = tokio::join!(first, second);
        first_result.unwrap();
        second_result.unwrap();

        let policies = llm.policies.lock().unwrap();
        assert_eq!(policies.len(), 2);
        assert_eq!(policies[0].requested_model, "anthropic/model-a");
        assert_eq!(
            policies[0].caller_override.as_deref(),
            Some("low"),
            "the first turn keeps the project effort active when it started"
        );
        assert_eq!(policies[1].requested_model, "anthropic/model-a");
        assert_eq!(
            policies[1].caller_override.as_deref(),
            Some("high"),
            "the queued turn may intentionally read the updated project effort while retaining \
             the immutable primary model snapshot"
        );
    }

    fn conn_for_resolution_tests(
        id: &str,
        provider: &str,
        model: &str,
    ) -> crate::llm_router::connections::ConnectionRow {
        use crate::llm_router::connections::{ConnectionData, ConnectionRow};
        let is_oauth = provider.ends_with("oauth");
        ConnectionRow {
            id: id.into(),
            provider: provider.into(),
            auth_type: if is_oauth {
                "oauth".into()
            } else {
                "api_key".into()
            },
            label: id.into(),
            priority: 0,
            enabled: true,
            data: ConnectionData {
                api_key: (!is_oauth).then(|| format!("sk-{id}")),
                access_token: is_oauth.then(|| format!("at-{id}")),
                models_override: Some(vec![model.into()]),
                ..Default::default()
            },
            created_at: 1,
            updated_at: 1,
        }
    }

    #[tokio::test]
    async fn native_model_resolution_serves_a_configured_codex_model_directly() {
        // Codex (openai-oauth) is drivable on the native path now (via
        // `codex_stream`), so a project pinned to it resolves directly
        // instead of falling back to the default route.
        use crate::llm_router::connections;
        use crate::llm_router::routes::{
            self, ModelRouteInfo, ModelRouteStrategy, ModelRouteTarget,
        };

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        connections::add_connection(
            &store,
            conn_for_resolution_tests("chatgpt", "openai-oauth", "gpt-5.2-codex"),
        )
        .await
        .unwrap();
        connections::add_connection(
            &store,
            conn_for_resolution_tests("claude", "anthropic", "claude-sonnet-4-5"),
        )
        .await
        .unwrap();
        routes::save_model_route(
            &store,
            ModelRouteInfo {
                id: "r1".into(),
                name: "fable".into(),
                enabled: true,
                strategy: ModelRouteStrategy::Fallback,
                targets: vec![ModelRouteTarget {
                    provider: "anthropic".into(),
                    model: "claude-sonnet-4-5".into(),
                    effort: None,
                }],
                created_at: 1,
                updated_at: 1,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            resolve_native_model(&store, Some("openai/gpt-5.2-codex".into()))
                .await
                .as_deref(),
            Some("openai/gpt-5.2-codex")
        );
    }

    #[tokio::test]
    async fn native_model_resolution_falls_back_from_an_unresolvable_model() {
        // A configured model that no enabled connection actually serves
        // (stale project config, renamed/removed connection, ...) still
        // falls back to the default native model.
        use crate::llm_router::connections;
        use crate::llm_router::routes::{
            self, ModelRouteInfo, ModelRouteStrategy, ModelRouteTarget,
        };

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        connections::add_connection(
            &store,
            conn_for_resolution_tests("chatgpt", "openai-oauth", "gpt-5.2-codex"),
        )
        .await
        .unwrap();
        connections::add_connection(
            &store,
            conn_for_resolution_tests("claude", "anthropic", "claude-sonnet-4-5"),
        )
        .await
        .unwrap();
        routes::save_model_route(
            &store,
            ModelRouteInfo {
                id: "r1".into(),
                name: "fable".into(),
                enabled: true,
                strategy: ModelRouteStrategy::Fallback,
                targets: vec![ModelRouteTarget {
                    provider: "anthropic".into(),
                    model: "claude-sonnet-4-5".into(),
                    effort: None,
                }],
                created_at: 1,
                updated_at: 1,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            resolve_native_model(&store, Some("openai/gpt-9-does-not-exist".into()))
                .await
                .as_deref(),
            Some("fable")
        );
    }

    /// An HTTP server spec must contribute its tools to the session, with the
    /// same `mcp__<server>__<tool>` naming stdio servers get. Before this
    /// task `connect_mcp_tools` returned early for every HTTP spec — this
    /// would still pass on a build that reintroduced that early return only
    /// if the assertion below is checked against a real dispatch, not a
    /// hand-built tool list, which is why it goes through
    /// `connect_mcp_tools` itself rather than constructing an `McpTool`
    /// directly.
    /// A bare `tempfile::NamedTempFile` + `Store::open` store, matching the
    /// inline-per-test pattern `store.rs` itself uses (there is no shared
    /// `test_store()` helper anywhere in the crate — see the remote-MCP-OAuth
    /// plan's Task 5 scouting note).
    async fn mcp_test_store() -> Arc<Store> {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        Arc::new(Store::open(tmp.path()).await.unwrap())
    }

    #[tokio::test]
    async fn an_http_mcp_server_contributes_its_tools_to_the_session() {
        let (url, _seen, _server) = mcp_http::tests::spawn_json_server().await;
        let spec = crate::domain::McpServerSpec {
            name: "remote".to_string(),
            transport: crate::domain::McpTransport::Http {
                url,
                headers: vec![],
            },
        };
        let store = mcp_test_store().await;

        let tools = connect_mcp_tools(&store, &[spec], &std::collections::HashMap::new()).await;

        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        assert!(
            names.iter().any(|n| n == "mcp__remote__ping"),
            "expected the remote server's tool in the session set, got {names:?}"
        );
    }

    #[tokio::test]
    async fn connect_mcp_tools_skips_unreachable_http_and_stdio_servers() {
        use crate::domain::{McpServerSpec, McpTransport};
        // One HTTP spec pointed at a port nothing listens on, plus two stdio
        // specs whose commands don't exist (spawn fails fast, no real
        // process). The joined connect must complete and yield no tools —
        // failures are logged and skipped, never propagated, for either
        // transport.
        let specs = vec![
            McpServerSpec {
                name: "http-server".into(),
                transport: McpTransport::Http {
                    url: "http://localhost:1/mcp".into(),
                    headers: vec![],
                },
            },
            McpServerSpec {
                name: "ghost-a".into(),
                transport: McpTransport::Stdio {
                    command: "ryuzi-definitely-not-a-real-binary-a".into(),
                    args: vec![],
                    env: vec![],
                },
            },
            McpServerSpec {
                name: "ghost-b".into(),
                transport: McpTransport::Stdio {
                    command: "ryuzi-definitely-not-a-real-binary-b".into(),
                    args: vec![],
                    env: vec![],
                },
            },
        ];
        let store = mcp_test_store().await;
        let tools = connect_mcp_tools(&store, &specs, &std::collections::HashMap::new()).await;
        assert!(
            tools.is_empty(),
            "a server that can't be reached (bad HTTP endpoint or missing stdio binary) must be \
             logged and skipped, not surfaced as a tool or turned into a panic — got {} tool(s)",
            tools.len()
        );
    }

    // -----------------------------------------------------------------
    // Task 8: auth precedence
    // -----------------------------------------------------------------

    fn stored_mcp_token(access_token: &str) -> crate::store::McpOauthToken {
        crate::store::McpOauthToken {
            access_token: access_token.to_string(),
            refresh_token: None,
            token_type: "Bearer".to_string(),
            expires_at: None,
            scopes: vec![],
            reconnect_required: false,
        }
    }

    /// A minimal MCP server that records EVERY `Authorization` header value
    /// on each request (in arrival order, not just the first — a client
    /// that duplicates the header rather than replacing it must be visible,
    /// not hidden by `HeaderMap::get`'s "first wins" behavior) and answers
    /// just enough of the handshake for `connect_http`/`connect_mcp_tools`
    /// to succeed. Used to assert on what actually reached the wire, not on
    /// `connect_mcp_tools`'s return value.
    pub(crate) async fn spawn_auth_echo_server() -> (String, Arc<std::sync::Mutex<Vec<Vec<String>>>>)
    {
        use axum::extract::{Json, State};
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::post;
        use axum::Router;

        type SeenAuth = Arc<std::sync::Mutex<Vec<Vec<String>>>>;

        async fn handle(
            State(seen): State<SeenAuth>,
            headers: HeaderMap,
            Json(msg): Json<serde_json::Value>,
        ) -> (StatusCode, [(&'static str, &'static str); 1], String) {
            seen.lock().unwrap().push(
                headers
                    .get_all("authorization")
                    .iter()
                    .filter_map(|v| v.to_str().ok().map(str::to_string))
                    .collect(),
            );
            let id = msg["id"].clone();
            let result = match msg["method"].as_str().unwrap_or_default() {
                "initialize" => {
                    serde_json::json!({"protocolVersion": "2025-06-18", "capabilities": {}})
                }
                "tools/list" => serde_json::json!({"tools": []}),
                _ => serde_json::Value::Null,
            };
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
            )
        }

        let seen: SeenAuth = Default::default();
        let app = Router::new()
            .route("/", post(handle))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), seen)
    }

    /// PROPERTY: with a manifest `Authorization` header present AND a
    /// stored token available, the manifest value reaches the wire and the
    /// stored token does not — asserted on the header the server actually
    /// received (not on internal state), and asserting exactly ONE
    /// `Authorization` arrives so a regression that sent BOTH would also
    /// fail this, not just a regression that swapped which one won.
    #[tokio::test]
    async fn a_manifest_supplied_authorization_header_wins_over_a_stored_token() {
        crate::llm_router::secrets::use_test_key_file();
        let (url, seen_auth) = spawn_auth_echo_server().await;
        let store = mcp_test_store().await;
        store
            .upsert_mcp_oauth_token("remote", &stored_mcp_token("stored-token"))
            .await
            .unwrap();
        let spec = crate::domain::McpServerSpec {
            name: "remote".to_string(),
            transport: crate::domain::McpTransport::Http {
                url,
                headers: vec![(
                    "Authorization".to_string(),
                    "Bearer manifest-token".to_string(),
                )],
            },
        };

        let _ = connect_mcp_tools(&store, &[spec], &Default::default()).await;

        let requests = seen_auth.lock().unwrap().clone();
        let init = requests
            .first()
            .expect("the initialize request must have reached the server");
        assert_eq!(
            init.len(),
            1,
            "exactly one Authorization header must reach the wire — sending both the manifest \
             token and the stored one would leak the stored credential alongside it: {init:?}"
        );
        assert_eq!(
            init[0], "Bearer manifest-token",
            "the manifest's Authorization header must win over the stored token: {init:?}"
        );
    }

    /// PROPERTY: with no manifest credential, the stored token must reach
    /// the wire.
    #[tokio::test]
    async fn a_stored_token_is_used_when_the_spec_carries_no_credential() {
        crate::llm_router::secrets::use_test_key_file();
        let (url, seen_auth) = spawn_auth_echo_server().await;
        let store = mcp_test_store().await;
        store
            .upsert_mcp_oauth_token("remote", &stored_mcp_token("stored-token"))
            .await
            .unwrap();
        let spec = crate::domain::McpServerSpec {
            name: "remote".to_string(),
            transport: crate::domain::McpTransport::Http {
                url,
                headers: vec![],
            },
        };

        let _ = connect_mcp_tools(&store, &[spec], &Default::default()).await;

        let requests = seen_auth.lock().unwrap().clone();
        let init = requests
            .first()
            .expect("the initialize request must have reached the server");
        assert_eq!(
            init.as_slice(),
            ["Bearer stored-token".to_string()],
            "with no manifest credential, the stored token must reach the wire: {init:?}"
        );
    }

    /// PROPERTY: a token marked `reconnect_required` must NEVER be used —
    /// that is the entire point of the flag. Proven by observing that NO
    /// Authorization header reaches the wire at all (not merely that the
    /// specific stored value is absent), so a regression that instead fell
    /// back to some other credential would still be caught here.
    #[tokio::test]
    async fn a_reconnect_required_token_is_never_used() {
        crate::llm_router::secrets::use_test_key_file();
        let (url, seen_auth) = spawn_auth_echo_server().await;
        let store = mcp_test_store().await;
        let mut token = stored_mcp_token("stale-token");
        token.reconnect_required = true;
        store
            .upsert_mcp_oauth_token("remote", &token)
            .await
            .unwrap();
        let spec = crate::domain::McpServerSpec {
            name: "remote".to_string(),
            transport: crate::domain::McpTransport::Http {
                url,
                headers: vec![],
            },
        };

        let _ = connect_mcp_tools(&store, &[spec], &Default::default()).await;

        let requests = seen_auth.lock().unwrap().clone();
        let init = requests
            .first()
            .expect("the initialize request must have reached the server");
        assert!(
            init.is_empty(),
            "a reconnect_required token must never be used as a bearer — got Authorization: \
             {init:?}"
        );
    }

    // -----------------------------------------------------------------
    // Task 13: a persisted row's header reaches the wire, not just a
    // hand-built spec
    // -----------------------------------------------------------------

    /// PROPERTY: everything above in this "Task 8" section proves the
    /// precedence rule is correct once a spec already carries a header. This
    /// test proves the header actually GETS there for a spec sourced from the
    /// database — via `mcp::upsert_server` + `mcp::set_server_headers` (the
    /// same calls `plugins::mcp_sync::sync_plugin_mcp` makes) and
    /// `mcp::servers_for_session` (not a hand-built `McpServerSpec`, unlike
    /// every test above). Before Task 13, `servers_for_session` hardcoded
    /// `headers: vec![]`, so this scenario would have sent the STORED OAuth
    /// token instead — the exact silent-auth-failure gap Task 11 found.
    #[tokio::test]
    async fn a_servers_for_session_header_reaches_the_wire_over_a_stored_token() {
        crate::llm_router::secrets::use_test_key_file();
        let (url, seen_auth) = spawn_auth_echo_server().await;
        let store = mcp_test_store().await;
        store
            .upsert_mcp_oauth_token("row-remote", &stored_mcp_token("stored-token"))
            .await
            .unwrap();
        crate::mcp::upsert_server(
            &store,
            crate::mcp::McpServerRow {
                id: "row-remote".into(),
                name: "Row Remote".into(),
                kind: "MCP server".into(),
                color: "#8B8B8B".into(),
                description: String::new(),
                transport: "http".into(),
                command: None,
                args: vec![],
                env: vec![],
                url: Some(url),
                scope: "global".into(),
                scope_gateways: vec![],
                version: None,
                publisher: None,
                status: "unchecked".into(),
                status_detail: None,
                auth_kind: "none".into(),
                auth_detail: None,
                plugin_id: None,
            },
        )
        .await
        .unwrap();
        crate::mcp::set_server_headers(
            &store,
            "row-remote",
            &[(
                "Authorization".to_string(),
                "Basic row-resolved-creds".to_string(),
            )],
        )
        .await
        .unwrap();

        let specs = crate::mcp::servers_for_session(&store, "native")
            .await
            .unwrap();
        assert_eq!(specs.len(), 1, "the http row must attach to the session");

        let _ = connect_mcp_tools(&store, &specs, &Default::default()).await;

        let requests = seen_auth.lock().unwrap().clone();
        let init = requests
            .first()
            .expect("the initialize request must have reached the server");
        assert_eq!(
            init.as_slice(),
            ["Basic row-resolved-creds".to_string()],
            "the row's stored header must reach the wire and win over the stored OAuth token: \
             {init:?}"
        );
    }

    #[test]
    fn connect_component_mcp_tools_is_a_no_op_with_no_components() {
        assert!(connect_component_mcp_tools(&[]).is_empty());
    }

    // -----------------------------------------------------------------
    // The credential-ownership predicate itself
    // -----------------------------------------------------------------

    fn http_spec_with(headers: &[(&str, &str)]) -> crate::domain::McpServerSpec {
        crate::domain::McpServerSpec {
            name: "remote".into(),
            transport: crate::domain::McpTransport::Http {
                url: "https://mcp.example.com".into(),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            },
        }
    }

    /// PROPERTY: [`mcp_http_credential`] answers exactly "would a stored MCP
    /// OAuth token be used for this server", which is the question BOTH
    /// `open_http_mcp` and the Apps card's `oauth_connect_available` need. The
    /// three assertions pin the three ways it has to be gotten right:
    ///
    /// * an `Authorization` header means the manifest owns the credential —
    ///   the case the UI got wrong for atlassian-rovo;
    /// * the match is case-INSENSITIVE (RFC 9110 §5.1), so a manifest written
    ///   `authorization` is not silently treated as no credential at all,
    ///   which would offer a connect flow whose token the transport then
    ///   drops in favour of the header;
    /// * a non-`Authorization` header (`X-Api-Key`) is NOT a manifest
    ///   credential for this purpose — it is sent alongside a stored token,
    ///   so the host still owns the `Authorization` slot and connect is real.
    #[test]
    fn credential_ownership_follows_the_authorization_header_case_insensitively() {
        assert_eq!(
            mcp_http_credential(&[]),
            McpHttpCredential::HostManaged,
            "no credential in the spec means a connected token is what authenticates"
        );
        assert_eq!(
            mcp_http_credential(&[("Authorization".into(), "Basic abc".into())]),
            McpHttpCredential::Manifest
        );
        assert_eq!(
            mcp_http_credential(&[("authorization".into(), "Basic abc".into())]),
            McpHttpCredential::Manifest,
            "a lower-case header name supplies a credential exactly as much as a capitalised one"
        );
        assert_eq!(
            mcp_http_credential(&[("X-Api-Key".into(), "abc".into())]),
            McpHttpCredential::HostManaged,
            "a non-Authorization header rides alongside a stored bearer rather than replacing it, \
             so the host still owns the credential slot"
        );
        assert!(McpHttpCredential::HostManaged.host_managed());
        assert!(!McpHttpCredential::Manifest.host_managed());
    }

    /// PROPERTY: a stdio server has no HTTP credential at all, so it must
    /// never classify as host-managed — `AppInfo.oauth_connect_available` is
    /// derived through this, and a stdio row that answered `Some(HostManaged)`
    /// would grow an OAuth Connect button for a local subprocess.
    #[test]
    fn a_stdio_spec_has_no_http_credential_to_classify() {
        let stdio = crate::domain::McpServerSpec {
            name: "local".into(),
            transport: crate::domain::McpTransport::Stdio {
                command: "acme-mcp".into(),
                args: vec![],
                env: vec![],
            },
        };
        assert_eq!(mcp_http_credential_of(&stdio), None);
        assert_eq!(
            mcp_http_credential_of(&http_spec_with(&[])),
            Some(McpHttpCredential::HostManaged)
        );
        assert_eq!(
            mcp_http_credential_of(&http_spec_with(&[("Authorization", "Bearer x")])),
            Some(McpHttpCredential::Manifest)
        );
    }

    /// PROPERTY: `open_http_mcp` refuses a stdio spec instead of silently
    /// doing something surprising — it is now reachable from `api::apps_api`
    /// (the Probe button), which is one `row.transport` typo away from
    /// handing it the wrong shape.
    #[tokio::test]
    async fn open_http_mcp_rejects_a_stdio_spec() {
        let store = mcp_test_store().await;
        // `McpHttpConnection` is not `Debug`, so this cannot use
        // `expect_err` — match instead of widening a production type's derives
        // for a test's convenience.
        let opened = open_http_mcp(
            &store,
            &crate::domain::McpServerSpec {
                name: "local".into(),
                transport: crate::domain::McpTransport::Stdio {
                    command: "acme-mcp".into(),
                    args: vec![],
                    env: vec![],
                },
            },
        )
        .await;
        match opened {
            Ok(_) => panic!("a stdio spec is not something this can open"),
            Err(err) => assert!(err.to_string().contains("not a remote (http)"), "{err:#}"),
        }
    }
}
