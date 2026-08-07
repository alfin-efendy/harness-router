//! Plugins screen RPC family: every installed plugin's identity/capabilities
//! (`list_plugins`), a single plugin's full detail (`plugin_detail`),
//! enable/disable (`set_plugin_enabled` — delegates to
//! [`crate::plugins::toggle_enabled`], the same helper `ryuzi plugins
//! enable/disable` uses, so the two surfaces can never drift), a validated
//! settings write (`set_plugin_setting`), plugin OAuth sign-in, the install
//! wizard resolution (`begin_plugin_install`/`cancel_plugin_install`/
//! `set_plugin_oauth_client_id`), kind-symmetric `uninstall_plugin`, and a
//! provider's effective model list (`plugin_models`). Moved (per the Move
//! Recipe) from `apps/cockpit/src-tauri/src/plugins_cmd.rs`.
//!
//! DTOs here are deliberate thin mirrors of `ryuzi_plugin_sdk::PluginManifest`
//! (and [`crate::plugins::CorePlugin`]) rather than re-exports: the manifest
//! is the engine's contract for plugin authors, while these shapes are the
//! Cockpit UI's contract, free to add UI-only fields (like
//! `value_set`/`configured` booleans) without perturbing the engine type.
//!
//! Secrets are never returned: `PluginAuthInfo.configured` and
//! `PluginFieldInfo.value_set` are booleans derived from whether a row is
//! persisted (or an auth env var is set), never the value itself.
//!
//! Behavior change from the Tauri original: `begin_plugin_oauth` /
//! `begin_plugin_install` no longer take an `AppHandle` or open the system
//! browser directly — they broadcast [`CoreEvent::PluginOauthAuthorizeUrl`]
//! via `state.cp.emit(..)` and Cockpit opens the browser on receipt. The
//! loopback callback server (bind 8976), the browser open, and the local
//! flow-cancel handles stay Cockpit-local in `plugins_cmd.rs`; the daemon
//! owns discovery/DCR/token exchange and the PKCE flow map.

use super::{ok, params, ApiError};
use crate::api::types::*;
use crate::control::ControlPlane;
use crate::domain::CoreEvent;
use crate::plugins::oauth::{
    discover_oauth_server_metadata, generate_pkce_verifier, pkce_challenge_s256,
    register_oauth_client, OauthServerMetadata, PluginOauthToken,
};
use crate::plugins::providers;
use crate::plugins::{CorePlugin, InstallProvenance, PluginSource};
use crate::serve::ApiState;
use crate::settings::SettingsStore;
use crate::store::{PluginOauthClient, RemoteCatalogRow, Store};
use reqwest::Url;
use ryuzi_plugin_sdk::{
    AuthKind, AuthSpec, FieldKind, McpServerDef, McpTransportDef, SettingField,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub(crate) const HANDLES: &[&str] = &[
    "list_plugins",
    "plugin_detail",
    "set_plugin_enabled",
    "set_plugin_setting",
    "begin_plugin_oauth",
    "complete_plugin_oauth",
    "disconnect_plugin_oauth",
    "plugin_models",
    "plugin_tools",
    "uninstall_plugin",
    "begin_plugin_install",
    "set_plugin_oauth_client_id",
    "cancel_plugin_install",
    "begin_skill_install",
    "confirm_skill_install",
    "update_plugin",
    "update_all_plugins",
    "set_plugin_pin",
    "plugin_doctor",
    "plugins_restart_required",
    "shutdown_engine",
    // Component-plugin (WASM bundle) release management — Task 11a.
    "plugin_release_detail",
    "install_component_plugin",
    "rollback_component_plugin",
    "component_bootstrap_status",
    // Thin, profile-aware wrappers over the Phase-3 OAuth profile engine
    // (`plugins::capabilities::oauth`) — Task 11a.
    "plugin_profile_begin_pkce",
    "plugin_profile_complete_pkce",
    "plugin_profile_disconnect",
    "plugin_profile_begin_device_flow",
    "plugin_profile_poll_device_flow",
];

#[derive(Clone)]
struct PluginOauthFlowState {
    verifier: String,
    redirect_uri: String,
    requested_scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct PluginOauthTokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
}

static PLUGIN_OAUTH_FLOWS: OnceLock<Mutex<HashMap<String, PluginOauthFlowState>>> = OnceLock::new();

fn plugin_oauth_flows() -> &'static Mutex<HashMap<String, PluginOauthFlowState>> {
    PLUGIN_OAUTH_FLOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Deserialize)]
struct IdP {
    id: String,
}
#[derive(Deserialize)]
struct SetPluginEnabledP {
    id: String,
    enabled: bool,
}
#[derive(Deserialize)]
struct SetPluginSettingP {
    key: String,
    value: String,
}
#[derive(Deserialize)]
struct PluginIdP {
    plugin_id: String,
}
#[derive(Deserialize)]
struct CompletePluginOauthP {
    plugin_id: String,
    code: String,
    state_token: String,
}
#[derive(Deserialize)]
struct SetPluginOauthClientIdP {
    plugin_id: String,
    client_id: String,
}
#[derive(Deserialize)]
struct CancelPluginInstallP {
    plugin_id: String,
    state_token: Option<String>,
}
#[derive(Deserialize)]
struct SourceP {
    source: String,
}
#[derive(Deserialize)]
struct TokenP {
    token: String,
}
#[derive(Deserialize)]
struct UpdatePluginP {
    id: String,
    force: bool,
}
#[derive(Deserialize)]
struct SetPluginPinP {
    id: String,
    pinned: bool,
    reason: Option<String>,
}
#[derive(Deserialize)]
struct InstallComponentP {
    id: String,
    #[serde(default)]
    version: Option<String>,
}
#[derive(Deserialize)]
struct RollbackComponentP {
    id: String,
    /// The bad version to revoke + deactivate.
    from_version: String,
    /// The prior good version to re-point the active pointer at.
    to_version: String,
}
#[derive(Deserialize)]
struct ProfileBeginPkceP {
    plugin_id: String,
    profile_id: String,
    redirect_uri: String,
}
#[derive(Deserialize)]
struct ProfileIdP {
    plugin_id: String,
    profile_id: String,
}
#[derive(Deserialize)]
struct ProfileCompletePkceP {
    plugin_id: String,
    profile_id: String,
    redirect_uri: String,
    code: String,
    verifier: String,
}
#[derive(Deserialize)]
struct ProfileDeviceFlowP {
    plugin_id: String,
    profile_id: String,
    device_authorization_url: String,
}
#[derive(Deserialize)]
struct ProfilePollDeviceP {
    plugin_id: String,
    profile_id: String,
    token_url: String,
    device_code: String,
    expires_at: i64,
}

pub(crate) async fn dispatch(state: &ApiState, method: &str, p: Value) -> Result<Value, ApiError> {
    let cp = &state.cp;
    match method {
        "list_plugins" => ok(assemble_list(cp).await?),
        "plugin_detail" => {
            let a: IdP = params(p)?;
            ok(assemble_detail(cp, &a.id).await?)
        }
        "set_plugin_enabled" => {
            let a: SetPluginEnabledP = params(p)?;
            set_plugin_enabled(cp, a.id, a.enabled).await?;
            ok(())
        }
        "set_plugin_setting" => {
            let a: SetPluginSettingP = params(p)?;
            set_plugin_setting(cp, a.key, a.value).await?;
            ok(())
        }
        "begin_plugin_oauth" => {
            let a: PluginIdP = params(p)?;
            ok(begin_plugin_oauth(cp, a.plugin_id).await?)
        }
        "complete_plugin_oauth" => {
            let a: CompletePluginOauthP = params(p)?;
            ok(complete_plugin_oauth(cp, a.plugin_id, a.code, a.state_token).await?)
        }
        "disconnect_plugin_oauth" => {
            let a: PluginIdP = params(p)?;
            ok(disconnect_plugin_oauth(cp, a.plugin_id).await?)
        }
        "plugin_models" => {
            let a: IdP = params(p)?;
            ok(providers::list_models(cp.store(), &a.id).await?)
        }
        "plugin_tools" => {
            let a: PluginIdP = params(p)?;
            ok(plugin_tools(cp, &a.plugin_id).await?)
        }
        "uninstall_plugin" => {
            let a: IdP = params(p)?;
            uninstall_and_reconcile(cp, &a.id).await?;
            ok(assemble_list(cp).await?)
        }
        "begin_plugin_install" => {
            let a: PluginIdP = params(p)?;
            ok(begin_plugin_install(cp, a.plugin_id).await?)
        }
        "set_plugin_oauth_client_id" => {
            let a: SetPluginOauthClientIdP = params(p)?;
            set_plugin_oauth_client_id(cp, a.plugin_id, a.client_id).await?;
            ok(())
        }
        "cancel_plugin_install" => {
            let a: CancelPluginInstallP = params(p)?;
            cancel_plugin_install(cp, a.plugin_id, a.state_token).await?;
            ok(())
        }
        "begin_skill_install" => {
            let a: SourceP = params(p)?;
            ok(begin_skill_install(cp, &a.source).await?)
        }
        "confirm_skill_install" => {
            let a: TokenP = params(p)?;
            ok(confirm_skill_install(cp, &a.token).await?)
        }
        "update_plugin" => {
            let a: UpdatePluginP = params(p)?;
            ok(update_plugin(cp, &a.id, a.force).await?)
        }
        "update_all_plugins" => ok(update_all_plugins(cp).await?),
        "set_plugin_pin" => {
            let a: SetPluginPinP = params(p)?;
            crate::skills_install::set_pack_pin(&a.id, a.pinned, a.reason.as_deref(), cp.store())
                .await?;
            ok(())
        }
        "plugin_doctor" => {
            let findings = crate::plugins::doctor::plugin_doctor(cp).await?;
            ok(findings
                .into_iter()
                .map(DoctorFinding::from)
                .collect::<Vec<_>>())
        }
        "plugins_restart_required" => ok(cp.plugins_restart_required()),
        // Spec B3: graceful process exit on request. The response is sent
        // before the signal loop unwinds (the RPC returns immediately; the
        // daemon's select-loop then runs `daemon.stop()` and exits), so the
        // caller gets a clean 200 rather than a dropped connection.
        "shutdown_engine" => {
            cp.request_shutdown();
            ok(())
        }
        "plugin_release_detail" => {
            let a: IdP = params(p)?;
            ok(plugin_release_detail(cp, &a.id).await?)
        }
        "install_component_plugin" => {
            let a: InstallComponentP = params(p)?;
            ok(install_component_plugin(cp, &a.id, a.version.as_deref()).await?)
        }
        "rollback_component_plugin" => {
            let a: RollbackComponentP = params(p)?;
            ok(rollback_component_plugin(cp, &a.id, &a.from_version, &a.to_version).await?)
        }
        "component_bootstrap_status" => ok(component_bootstrap_status(cp).await?),
        "plugin_profile_begin_pkce" => {
            let a: ProfileBeginPkceP = params(p)?;
            ok(plugin_profile_begin_pkce(cp, &a.plugin_id, &a.profile_id, &a.redirect_uri).await?)
        }
        "plugin_profile_complete_pkce" => {
            let a: ProfileCompletePkceP = params(p)?;
            ok(plugin_profile_complete_pkce(
                cp,
                &a.plugin_id,
                &a.profile_id,
                &a.redirect_uri,
                &a.code,
                &a.verifier,
            )
            .await?)
        }
        "plugin_profile_disconnect" => {
            let a: ProfileIdP = params(p)?;
            plugin_profile_disconnect(cp, &a.plugin_id, &a.profile_id).await?;
            ok(())
        }
        "plugin_profile_begin_device_flow" => {
            let a: ProfileDeviceFlowP = params(p)?;
            ok(plugin_profile_begin_device_flow(
                cp,
                &a.plugin_id,
                &a.profile_id,
                &a.device_authorization_url,
            )
            .await?)
        }
        "plugin_profile_poll_device_flow" => {
            let a: ProfilePollDeviceP = params(p)?;
            ok(plugin_profile_poll_device_flow(
                cp,
                &a.plugin_id,
                &a.profile_id,
                &a.token_url,
                &a.device_code,
                a.expires_at,
            )
            .await?)
        }
        _ => Err(ApiError::not_found(format!("unknown method: {method}"))),
    }
}

/// Phase 1 of the two-phase tiered trust gate (see
/// [`crate::skills_install::begin_install`]): curated sources install
/// immediately (`completed: true`); arbitrary sources stop at a trust prompt
/// the wizard must show before `confirm_skill_install` can proceed. Marks the
/// daemon dirty (`plugins_restart_required`) only when the install actually
/// completed — a `NeedsConfirmation` trust prompt hasn't touched disk yet.
async fn begin_skill_install(cp: &ControlPlane, source: &str) -> anyhow::Result<SkillInstallBegin> {
    let result = crate::skills_install::begin_install(source, cp.store()).await?;
    if matches!(result, crate::skills_install::BeginInstall::Completed(_)) {
        cp.mark_plugins_restart_required();
        cp.emit(CoreEvent::PluginsChanged);
    }
    Ok(SkillInstallBegin::from(result))
}

/// Phase 2: complete a staged install (or update) after the user has
/// acknowledged its trust prompt. The token is single-use. Always marks
/// `plugins_restart_required`: reaching this point always means an install (or
/// reack-triggered update) just completed.
async fn confirm_skill_install(
    cp: &ControlPlane,
    token: &str,
) -> anyhow::Result<crate::skills_install::InstalledSkillPack> {
    let pack = crate::skills_install::confirm_install(token, cp.store()).await?;
    cp.mark_plugins_restart_required();
    cp.emit(CoreEvent::PluginsChanged);
    Ok(pack)
}

/// Update one installed pack. `force` overrides the local-edits guard but
/// never the pinned guard or the hook-script re-ack gate. Marks a restart only
/// on an actual `Updated` outcome — the other outcomes are no-ops on disk.
async fn update_plugin(
    cp: &ControlPlane,
    id: &str,
    force: bool,
) -> anyhow::Result<UpdateOutcomeDto> {
    let outcome = crate::skills_install::update_installed_pack(id, force, cp.store()).await?;
    if matches!(outcome, crate::skills_install::UpdateOutcome::Updated) {
        cp.mark_plugins_restart_required();
        cp.emit(CoreEvent::PluginsChanged);
    }
    Ok(UpdateOutcomeDto::from(outcome))
}

/// Update every installed pack (skipping pinned ones); never fails as a whole
/// — a single pack's error surfaces as that pack's `Failed` entry. Marks a
/// restart if at least one pack actually reinstalled.
async fn update_all_plugins(cp: &ControlPlane) -> anyhow::Result<Vec<UpdateOutcomeEntry>> {
    let results = crate::skills_install::update_all_packs(cp.store()).await?;
    if results
        .iter()
        .any(|(_, o)| matches!(o, crate::skills_install::UpdateOutcome::Updated))
    {
        cp.mark_plugins_restart_required();
        cp.emit(CoreEvent::PluginsChanged);
    }
    Ok(results
        .into_iter()
        .map(|(id, outcome)| UpdateOutcomeEntry {
            id,
            outcome: UpdateOutcomeDto::from(outcome),
        })
        .collect())
}

fn source_label(source: &PluginSource) -> &'static str {
    match source {
        PluginSource::Builtin => "builtin",
        PluginSource::Installed { provenance, .. } => match provenance {
            InstallProvenance::Catalog => "catalog",
            InstallProvenance::LocalPath => "local-path",
            InstallProvenance::GitUrl(_) => "git-url",
        },
    }
}

/// The catalog kind for a plugin, or `None` when it must not be listed
/// (runtimes). Classification itself lives in [`crate::plugins::plugin_kind`].
fn derive_kind(plugin: &CorePlugin) -> Option<&'static str> {
    match crate::plugins::plugin_kind(plugin) {
        "runtime" => None,
        kind => Some(kind),
    }
}

/// Family head id for a provider plugin (`anthropic-oauth` → `anthropic`).
fn provider_family(id: &str) -> String {
    crate::llm_router::registry::descriptor(id)
        .map(|d| d.family.to_string())
        .unwrap_or_else(|| id.to_string())
}

/// Pure kind → installed decision. Inputs are pre-computed by the caller.
///
/// `component_active` — an active `component_plugin_releases` row exists for
/// this id, i.e. its WASM bundle IS installed on disk. It counts for the
/// integration/gateway arms (an auth-less, still-disabled component like
/// discord would otherwise read not-installed forever, re-offering Install
/// for an already-installed bundle) but NOT for providers: provider
/// installed-ness stays authoritative on the persisted installed set alone
/// (mimo/opencode bundles are bootstrapped for everyone, set or no set).
fn installed_flag(
    kind: &str,
    enabled: bool,
    configured: bool,
    provider_installed: bool,
    gateway_settings_complete: bool,
    skill_pack_installed: bool,
    component_active: bool,
) -> bool {
    match kind {
        "provider" => provider_installed,
        "gateway" => gateway_settings_complete || component_active,
        "skill-pack" => skill_pack_installed,
        _ => configured || enabled || component_active,
    }
}

/// Single-source status for a PluginInfo row. Priority: blocked >
/// not-installed > disabled > attach-failed > needs-setup >
/// update-available > ok. Pure so it stays unit-testable.
pub(crate) fn derive_plugin_status(
    installed: bool,
    enabled: bool,
    blocked: bool,
    auth_kind: &str,
    configured: bool,
    attach_failed: Option<&str>,
    update_available: bool,
) -> (&'static str, Option<String>) {
    if blocked {
        return ("blocked", None);
    }
    if !installed {
        return ("not-installed", None);
    }
    if !enabled {
        return ("disabled", None);
    }
    if let Some(reason) = attach_failed {
        return ("attach-failed", Some(reason.to_string()));
    }
    if auth_kind != "none" && !configured {
        return (
            "needs-setup",
            Some("authentication not configured".to_string()),
        );
    }
    if update_available {
        return ("update-available", None);
    }
    ("ok", None)
}

/// Coarse 3-way auth kind for `PluginInfo.auth_kind` / `derive_plugin_status`'s
/// needs-setup gate (spec §6: `none` | `token` | `oauth`). Distinct from
/// `auth_kind_label`'s 4-way `PluginAuthInfo.kind` label — `api-key` and
/// `token` both collapse to `"token"` here, since the status derivation only
/// cares whether SOME credential is required, not which shape.
fn plugin_info_auth_kind(auth: Option<&AuthSpec>) -> &'static str {
    match auth.map(|a| a.kind) {
        None | Some(AuthKind::None) => "none",
        Some(AuthKind::Oauth) => "oauth",
        Some(AuthKind::ApiKey) | Some(AuthKind::Token) => "token",
    }
}

/// Ledger-derived `PluginInfo` fields (`pinned`, `sourceSpec`,
/// `resolvedCommit`, `installedAt`, `updatedAt`, `trustTier`) drawn from an
/// optional `plugin_installs` row — `None` leaves them at their "no ledger
/// row" defaults (`pinned: false`, the rest `None`).
struct InstallLedgerFields {
    pinned: bool,
    source_spec: Option<String>,
    resolved_commit: Option<String>,
    installed_at: Option<i64>,
    updated_at: Option<i64>,
    trust_tier: Option<String>,
}

impl InstallLedgerFields {
    fn absent() -> Self {
        Self {
            pinned: false,
            source_spec: None,
            resolved_commit: None,
            installed_at: None,
            updated_at: None,
            trust_tier: None,
        }
    }

    fn from_record(rec: &crate::store::PluginInstallRecord) -> Self {
        Self {
            pinned: rec.pinned,
            source_spec: Some(rec.source_spec.clone()),
            resolved_commit: rec.resolved_commit.clone(),
            installed_at: Some(rec.installed_at),
            updated_at: Some(rec.updated_at),
            trust_tier: Some(rec.trust_tier.clone()),
        }
    }

    fn from_option(rec: Option<&crate::store::PluginInstallRecord>) -> Self {
        rec.map(Self::from_record).unwrap_or_else(Self::absent)
    }
}

/// Enrichment inputs `plugin_info` needs beyond the plugin itself: the
/// install ledger row, the cached remote-catalog row, whether this plugin
/// currently owns its manifest-claimed `slot` (Feature C2), the last recorded
/// attach failure (Task 3), the active component-release version (Task 3),
/// and the installed skill count for skill-pack rows (Task 3). Bundled into
/// one struct so `plugin_info` doesn't creep past clippy's too-many-arguments
/// lint as fields get added over time.
struct PluginInfoContext<'a> {
    install: Option<&'a crate::store::PluginInstallRecord>,
    remote: Option<&'a RemoteCatalogRow>,
    owns_slot: bool,
    /// Secret-free reason from the last recorded attach failure
    /// (`plugin_attach_status.outcome == "failed"`) — `None` when the last
    /// attach succeeded or none was ever recorded. Mirrors the doctor's
    /// `attach-failed` predicate (`doctor.rs:177-191`).
    attach_failed: Option<&'a str>,
    /// The currently active `component_plugin_releases` version for this
    /// plugin id, compared against `catalog_version` to derive
    /// `update_available`. `None` when never installed via the release
    /// pipeline.
    active_version: Option<&'a str>,
    /// Installed-skill-pack size (`InstalledSkillInfo.skill_count`) — only
    /// meaningful for `kind == "skill-pack"` rows; `plugin_info` ignores it
    /// (forces `None`) for every other kind.
    skill_count: Option<u32>,
}

fn plugin_info(
    plugin: &CorePlugin,
    enabled: bool,
    configured: bool,
    kind: &str,
    installed: bool,
    ctx: PluginInfoContext<'_>,
) -> PluginInfo {
    let m = &plugin.manifest;
    let ledger = InstallLedgerFields::from_option(ctx.install);
    let remote = ctx.remote;
    let owns_slot = ctx.owns_slot;
    let blocked_reason = remote.and_then(|r| r.blocked_reason.clone());
    let catalog_version = remote.map(|r| r.version.clone());
    let component_backed = crate::plugins::component_catalog::is_component_bundle(&m.id);
    let auth_kind = plugin_info_auth_kind(m.auth.as_ref());
    let update_available = component_backed
        && catalog_version.is_some()
        && catalog_version.as_deref() != ctx.active_version;
    let (status, status_detail) = derive_plugin_status(
        installed,
        enabled,
        blocked_reason.is_some(),
        auth_kind,
        configured,
        ctx.attach_failed,
        update_available,
    );
    PluginInfo {
        id: m.id.clone(),
        name: m.name.clone(),
        description: m.description.clone(),
        icon: m.icon.clone(),
        categories: m.categories.clone(),
        slot: m.slot.clone(),
        owns_slot,
        verified: m.verified,
        experimental: m.experimental,
        enabled,
        configured,
        source: source_label(&plugin.source).to_string(),
        capabilities: plugin
            .capabilities()
            .into_iter()
            .map(str::to_string)
            .collect(),
        kind: kind.to_string(),
        installed,
        family: (kind == "provider").then(|| provider_family(&m.id)),
        pinned: ledger.pinned,
        source_spec: ledger.source_spec,
        resolved_commit: ledger.resolved_commit,
        installed_at: ledger.installed_at,
        updated_at: ledger.updated_at,
        trust_tier: ledger.trust_tier,
        component_backed,
        catalog_version,
        blocked_reason,
        status: status.to_string(),
        status_detail,
        auth_kind: auth_kind.to_string(),
        tool_count: component_backed
            .then(|| crate::plugins::component_catalog::declared_tool_count(&m.id))
            .flatten(),
        skill_count: (kind == "skill-pack").then_some(ctx.skill_count).flatten(),
    }
}

fn auth_kind_label(kind: AuthKind) -> &'static str {
    match kind {
        AuthKind::None => "none",
        AuthKind::ApiKey => "api-key",
        AuthKind::Token => "token",
        AuthKind::Oauth => "oauth",
    }
}

/// `ryuzi_plugin_sdk::FieldKind` -> the camelCase-friendly label
/// `PluginFieldInfo.kind` carries across the Tauri IPC boundary.
fn field_kind_label(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::String => "string",
        FieldKind::Int => "int",
        FieldKind::Bool => "bool",
    }
}

fn plugin_oauth_flow_key(plugin_id: &str, state_token: &str) -> String {
    format!("{plugin_id}:{state_token}")
}

/// The install wizard's loopback callback server port. Registered redirect
/// URIs use it, so it can never change without re-registering every DCR
/// client. The daemon builds the redirect_uri string; Cockpit binds the same
/// port with `oauth_loopback::bind_fixed`.
const PLUGIN_OAUTH_CALLBACK_PORT: u16 = 8976;

fn plugin_oauth_callback_path(plugin_id: &str) -> String {
    format!("/plugin-oauth/{plugin_id}/callback")
}

fn plugin_oauth_redirect_uri(plugin_id: &str) -> String {
    format!(
        "http://127.0.0.1:{PLUGIN_OAUTH_CALLBACK_PORT}{}",
        plugin_oauth_callback_path(plugin_id)
    )
}

fn plugin_oauth_requested_scopes(auth: &AuthSpec) -> Vec<String> {
    auth.scopes.clone()
}

/// Drop pending daemon-side flow state for `plugin_id` — all of its flows
/// when `state_token` is `None`, else just that one. The loopback callback
/// server this feeds is Cockpit-local (`plugins_cmd.rs`); only the daemon's
/// PKCE/verifier map lives here.
fn drop_pending_plugin_flows(plugin_id: &str, state_token: Option<&str>) {
    let prefix = format!("{plugin_id}:");
    let mut flows = plugin_oauth_flows()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    match state_token {
        Some(token) => {
            flows.remove(&plugin_oauth_flow_key(plugin_id, token));
        }
        None => {
            let keys: Vec<String> = flows
                .keys()
                .filter(|k| k.starts_with(&prefix))
                .cloned()
                .collect();
            for key in keys {
                flows.remove(&key);
            }
        }
    }
}

impl PluginInstallBeginResult {
    fn new(auth_kind: &str) -> Self {
        Self {
            auth_kind: auth_kind.to_string(),
            env_var_present: false,
            env_var_name: None,
            oauth_available: false,
            oauth_external: false,
            needs_client_id: false,
            dcr_succeeded: false,
            callback_mode: "manual".to_string(),
            oauth_begin: None,
            dcr_error: None,
        }
    }
}

/// The effective OAuth config after the resolution order. Endpoints:
/// `plugin_oauth_clients` row (discovery/DCR/manual cache) → manifest.
/// Client id: row → saved value of the manifest's `auth.client_id_setting`
/// → for EXTERNAL OAuth plugins only, the saved `auth.setting` value
/// (google-workspace's client id key IS its auth.setting).
#[derive(Clone)]
struct ResolvedPluginOauth {
    authorize_url: Option<String>,
    token_url: Option<String>,
    client_id: Option<String>,
}

/// External OAuth: sign-in is brokered outside Cockpit by the child server —
/// kind=oauth with neither an `auth.resource` to discover against nor a
/// manifest `authorize_url` (google-workspace).
fn is_external_oauth(auth: &AuthSpec) -> bool {
    auth.kind == AuthKind::Oauth
        && auth.resource.as_deref().is_none_or(str::is_empty)
        && auth.authorize_url.as_deref().is_none_or(str::is_empty)
}

async fn resolve_plugin_oauth(
    store: &Store,
    plugin_id: &str,
    auth: &AuthSpec,
) -> anyhow::Result<ResolvedPluginOauth> {
    let row = store.get_plugin_oauth_client(plugin_id).await?;
    let (row_authorize, row_token, row_client) = match row {
        Some(row) => (row.authorize_url, row.token_url, row.client_id),
        None => (None, None, None),
    };
    let non_empty = |value: Option<String>| value.filter(|v| !v.is_empty());
    let authorize_url =
        non_empty(row_authorize).or_else(|| auth.authorize_url.clone().filter(|v| !v.is_empty()));
    let token_url =
        non_empty(row_token).or_else(|| auth.token_url.clone().filter(|v| !v.is_empty()));
    let mut client_id = non_empty(row_client);
    if client_id.is_none() {
        if let Some(key) = auth.client_id_setting.as_deref() {
            client_id = store.get_setting_raw(key).await?.filter(|v| !v.is_empty());
        }
    }
    if client_id.is_none() && is_external_oauth(auth) {
        if let Some(key) = auth.setting.as_deref() {
            client_id = store.get_setting_raw(key).await?.filter(|v| !v.is_empty());
        }
    }
    Ok(ResolvedPluginOauth {
        authorize_url,
        token_url,
        client_id,
    })
}

/// Prereq check over RESOLVED values (table already consulted). Two client-id
/// message variants preserved: missing `auth.client_id_setting` declaration
/// vs missing "saved value for {key}" — the wizard branches on structured
/// fields, never on this text.
fn plugin_oauth_prereq_error(
    plugin_id: &str,
    auth: &AuthSpec,
    resolved: &ResolvedPluginOauth,
) -> Option<String> {
    let mut missing = Vec::new();
    if resolved.authorize_url.is_none() {
        missing.push("auth.authorize_url".to_string());
    }
    if resolved.token_url.is_none() {
        missing.push("auth.token_url".to_string());
    }
    if resolved.client_id.is_none() {
        match auth.client_id_setting.as_deref() {
            Some(key) => missing.push(format!("saved value for {key}")),
            None => missing.push("auth.client_id_setting".to_string()),
        }
    }
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "{plugin_id} OAuth sign-in isn't ready in Cockpit yet: missing {}",
            missing.join(", ")
        ))
    }
}

async fn plugin_oauth_client_secret(
    store: &Store,
    auth: &AuthSpec,
) -> anyhow::Result<Option<String>> {
    let Some(key) = auth.client_secret_setting.as_deref() else {
        return Ok(None);
    };
    Ok(store
        .get_setting_raw(key)
        .await?
        .filter(|value| !value.is_empty()))
}

async fn build_plugin_oauth_begin_result(
    store: &Store,
    plugin_id: &str,
    auth: &AuthSpec,
    verifier: &str,
    state_token: &str,
) -> anyhow::Result<PluginOauthBeginResult> {
    let resolved = resolve_plugin_oauth(store, plugin_id, auth).await?;
    build_plugin_oauth_begin_result_with(plugin_id, auth, &resolved, verifier, state_token)
}

/// Build the authorize URL from already-resolved endpoints/client id — table
/// values take precedence over manifest fields (see [`resolve_plugin_oauth`]).
/// `begin_plugin_install` calls this directly with its post-DCR resolution;
/// `begin_plugin_oauth` goes through the async wrapper above.
fn build_plugin_oauth_begin_result_with(
    plugin_id: &str,
    auth: &AuthSpec,
    resolved: &ResolvedPluginOauth,
    verifier: &str,
    state_token: &str,
) -> anyhow::Result<PluginOauthBeginResult> {
    if let Some(message) = plugin_oauth_prereq_error(plugin_id, auth, resolved) {
        anyhow::bail!(message);
    }
    let client_id = resolved
        .client_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("{plugin_id} OAuth sign-in is missing a client id"))?;
    let authorize_url = resolved.authorize_url.as_deref().ok_or_else(|| {
        anyhow::anyhow!("{plugin_id} OAuth sign-in is missing auth.authorize_url")
    })?;
    let redirect_uri = plugin_oauth_redirect_uri(plugin_id);
    let requested_scopes = plugin_oauth_requested_scopes(auth);
    let mut url = Url::parse(authorize_url)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", client_id);
        query.append_pair("redirect_uri", &redirect_uri);
        query.append_pair("state", state_token);
        query.append_pair("code_challenge", &pkce_challenge_s256(verifier));
        query.append_pair("code_challenge_method", "S256");
        if !requested_scopes.is_empty() {
            query.append_pair("scope", &requested_scopes.join(" "));
        }
        if let Some(resource) = auth.resource.as_deref().filter(|value| !value.is_empty()) {
            query.append_pair("resource", resource);
        }
        for (key, value) in &auth.extra_authorize_params {
            query.append_pair(key, value);
        }
    }

    Ok(PluginOauthBeginResult {
        state_token: state_token.to_string(),
        authorize_url: url.into(),
        redirect_uri,
    })
}

/// Steps 1-6 of the install resolution order: env var → non-oauth kinds →
/// external OAuth → endpoint discovery (regardless of the
/// dynamic-registration flag) → client id / DCR → authorize URL + flow state.
/// Kept free of the browser/loopback steps so tests can drive it against a
/// mock vendor; the Cockpit-local `begin_plugin_install` proxy wraps it with
/// the callback server (step 7). The daemon RPC below emits the authorize URL
/// (step 8) as a `CoreEvent`.
async fn resolve_plugin_install(
    store: &Store,
    http: &reqwest::Client,
    plugin_id: &str,
    auth: Option<&AuthSpec>,
) -> anyhow::Result<PluginInstallBeginResult> {
    // A manifest without [auth] behaves as kind "none".
    let Some(auth) = auth else {
        return Ok(PluginInstallBeginResult::new("none"));
    };
    let mut result = PluginInstallBeginResult::new(auth_kind_label(auth.kind));
    result.env_var_name = auth.env.clone();

    // 1. Env var short-circuit: install completes with zero auth input (the
    // wizard still routes through the settings step before enabling).
    if auth
        .env
        .as_deref()
        .is_some_and(|e| std::env::var_os(e).is_some())
    {
        result.env_var_present = true;
        return Ok(result);
    }

    // 2. Non-OAuth kinds: the wizard routes to token input or settings.
    if auth.kind != AuthKind::Oauth {
        return Ok(result);
    }

    // 3. External OAuth (google-workspace): no discovery, no browser, no
    // callback — the child server brokers sign-in at first use. The wizard
    // only collects the client id when none is saved yet.
    if is_external_oauth(auth) {
        let resolved = resolve_plugin_oauth(store, plugin_id, auth).await?;
        result.oauth_external = true;
        result.needs_client_id = resolved.client_id.is_none();
        return Ok(result);
    }

    // 4. Endpoint resolution: discover when either endpoint COLUMN is missing
    // — regardless of the dynamic-registration flag (Slack needs endpoints
    // too). Manifest endpoints can still rescue a failure.
    let row = store.get_plugin_oauth_client(plugin_id).await?;
    let row_has_endpoints = row.as_ref().is_some_and(|row| {
        row.authorize_url.as_deref().is_some_and(|v| !v.is_empty())
            && row.token_url.as_deref().is_some_and(|v| !v.is_empty())
    });
    let mut discovered: Option<OauthServerMetadata> = None;
    if !row_has_endpoints {
        if let Some(resource) = auth.resource.as_deref().filter(|v| !v.is_empty()) {
            match discover_oauth_server_metadata(http, resource).await {
                Ok(metadata) => {
                    // Persist endpoints even when registration is impossible —
                    // the manual client-id path needs an authorize URL.
                    // Network I/O above, store write here: never inside
                    // with_conn.
                    store
                        .upsert_plugin_oauth_client(&PluginOauthClient {
                            plugin_id: plugin_id.to_string(),
                            authorize_url: Some(metadata.authorization_endpoint.clone()),
                            token_url: Some(metadata.token_endpoint.clone()),
                            client_id: None,
                        })
                        .await?;
                    discovered = Some(metadata);
                }
                Err(err) => result.dcr_error = Some(err.to_string()),
            }
        }
    }
    let mut resolved = resolve_plugin_oauth(store, plugin_id, auth).await?;
    if resolved.authorize_url.is_none() || resolved.token_url.is_none() {
        // Discovery failed and neither cache nor manifest supplies endpoints —
        // nothing else is possible; the wizard shows dcr_error with Retry.
        return Ok(result);
    }

    // 5. Client id: any existing id (row → client_id_setting) permanently
    // suppresses DCR. DCR runs only when the manifest opts in AND this call's
    // discovery document exposed a registration_endpoint.
    if resolved.client_id.is_none() {
        let registration_endpoint = discovered
            .as_ref()
            .and_then(|m| m.registration_endpoint.clone())
            .filter(|_| auth.dynamic_registration);
        let Some(registration_endpoint) = registration_endpoint else {
            result.needs_client_id = true;
            return Ok(result);
        };
        match register_oauth_client(
            http,
            &registration_endpoint,
            &plugin_oauth_redirect_uri(plugin_id),
        )
        .await
        {
            Ok(client_id) => {
                store
                    .upsert_plugin_oauth_client(&PluginOauthClient {
                        plugin_id: plugin_id.to_string(),
                        authorize_url: None,
                        token_url: None,
                        client_id: Some(client_id.clone()),
                    })
                    .await?;
                result.dcr_succeeded = true;
                resolved.client_id = Some(client_id);
            }
            Err(err) => {
                result.dcr_error = Some(err.to_string());
                result.needs_client_id = true;
                return Ok(result);
            }
        }
    }

    // 6. Authorize URL + flow state; a new begin cancels whatever flow was
    // pending for this plugin first.
    drop_pending_plugin_flows(plugin_id, None);
    let verifier = generate_pkce_verifier();
    let state_token = crate::paths::new_id();
    let begin =
        build_plugin_oauth_begin_result_with(plugin_id, auth, &resolved, &verifier, &state_token)?;
    plugin_oauth_flows()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(
            plugin_oauth_flow_key(plugin_id, &state_token),
            PluginOauthFlowState {
                verifier,
                redirect_uri: begin.redirect_uri.clone(),
                requested_scopes: plugin_oauth_requested_scopes(auth),
            },
        );
    result.oauth_available = true;
    result.oauth_begin = Some(begin);
    // Step 6 succeeded: any earlier dcr_error (e.g. discovery failed but the
    // manifest's endpoints rescued the flow) is stale — never let the DTO
    // carry oauthAvailable:true alongside a leftover dcrError.
    result.dcr_error = None;
    Ok(result)
}

/// Whether an auth block's credential is configured: a persisted, non-empty
/// value under `auth.setting`, or — fallback — the `auth.env` var set in the
/// process environment. Pure so it's testable without a `Store` or a real
/// process environment; callers resolve `setting_value`/`env_is_set` first
/// (see `build_auth_info`).
fn auth_configured(setting_value: Option<&str>, env_is_set: bool) -> bool {
    setting_value.is_some_and(|v| !v.is_empty()) || env_is_set
}

/// `PluginAuthInfo.configured` for the list payload without building the whole
/// auth DTO: oauth → a token is stored and reconnect isn't required; otherwise
/// the `auth.setting`-row / `auth.env` check. No `[auth]` → false.
///
/// PR-3: for a component id whose declared bundle manifest carries `[[oauth]]`
/// profiles (github/atlassian/bitbucket), oauth-configured means EVERY
/// declared profile has a live stored token — a single-profile bundle behaves
/// like the classic single-token check, but a multi-profile one is only
/// "configured" once every profile is connected. Non-component oauth (and any
/// component with no declared profiles) falls through to the classic
/// single-token check unchanged.
async fn plugin_auth_configured(
    store: &Store,
    plugin_id: &str,
    auth: Option<&AuthSpec>,
) -> anyhow::Result<bool> {
    let Some(auth) = auth else {
        return Ok(false);
    };
    if auth.kind == AuthKind::Oauth {
        if let Some(bundle) = crate::plugins::component_catalog::declared_manifest(plugin_id) {
            if !bundle.oauth.is_empty() {
                for profile in &bundle.oauth {
                    let live = store
                        .get_plugin_oauth_profile_token(plugin_id, &profile.id)
                        .await?
                        .is_some_and(|t| !t.reconnect_required);
                    if !live {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
        }
        let token = store.get_plugin_oauth_token(plugin_id).await?;
        return Ok(token.is_some_and(|token| !token.reconnect_required));
    }
    let setting_value = match &auth.setting {
        Some(key) => store.get_setting_raw(key).await?,
        None => None,
    };
    let env_is_set = auth
        .env
        .as_deref()
        .is_some_and(|e| std::env::var_os(e).is_some());
    Ok(auth_configured(setting_value.as_deref(), env_is_set))
}

async fn build_auth_info(
    store: &Store,
    plugin_id: &str,
    auth: &AuthSpec,
) -> anyhow::Result<PluginAuthInfo> {
    let setting_value = match &auth.setting {
        Some(key) => store.get_setting_raw(key).await?,
        None => None,
    };
    let env_is_set = auth
        .env
        .as_deref()
        .is_some_and(|e| std::env::var_os(e).is_some());
    let resolved_oauth = if auth.kind == AuthKind::Oauth {
        Some(resolve_plugin_oauth(store, plugin_id, auth).await?)
    } else {
        None
    };
    let oauth_token = if auth.kind == AuthKind::Oauth {
        store.get_plugin_oauth_token(plugin_id).await?
    } else {
        None
    };
    let oauth_reconnect_required = oauth_token
        .as_ref()
        .is_some_and(|token| token.reconnect_required);
    let oauth_token_stored = oauth_token.is_some();
    let oauth_connect_error = resolved_oauth
        .as_ref()
        .and_then(|resolved| plugin_oauth_prereq_error(plugin_id, auth, resolved));
    Ok(PluginAuthInfo {
        kind: auth_kind_label(auth.kind).to_string(),
        setting: auth.setting.clone(),
        env: auth.env.clone(),
        help_url: auth.help_url.clone(),
        configured: if auth.kind == AuthKind::Oauth {
            oauth_token_stored && !oauth_reconnect_required
        } else {
            auth_configured(setting_value.as_deref(), env_is_set)
        },
        oauth_connect_available: auth.kind == AuthKind::Oauth && oauth_connect_error.is_none(),
        oauth_connect_error,
        oauth_token_stored,
        oauth_reconnect_required,
    })
}

/// Whether a settings field's value is set: a persisted, non-empty row. Pure —
/// callers resolve the persisted row first (see `build_settings_info`).
fn field_value_set(persisted: Option<&str>) -> bool {
    persisted.is_some_and(|v| !v.is_empty())
}

async fn build_settings_info(
    store: &Store,
    fields: &[SettingField],
) -> anyhow::Result<Vec<PluginFieldInfo>> {
    let mut out = Vec::with_capacity(fields.len());
    for f in fields {
        let persisted = store.get_setting_raw(&f.key).await?;
        out.push(PluginFieldInfo {
            key: f.key.clone(),
            label: f.label.clone(),
            help: f.help.clone(),
            secret: f.secret,
            required: f.required,
            value_set: field_value_set(persisted.as_deref()),
            kind: field_kind_label(f.kind).to_string(),
            options: f.options.clone(),
            default: f.default.clone(),
        });
    }
    Ok(out)
}

fn mcp_transport_label(t: McpTransportDef) -> &'static str {
    match t {
        McpTransportDef::Stdio => "stdio",
        McpTransportDef::Http => "http",
    }
}

/// Raw manifest string, no `${auth}` substitution — command for stdio, url for
/// http.
fn mcp_info(server: &McpServerDef) -> PluginMcpInfo {
    PluginMcpInfo {
        name: server.name.clone(),
        transport: mcp_transport_label(server.transport).to_string(),
        command_or_url: server
            .command
            .clone()
            .or_else(|| server.url.clone())
            .unwrap_or_default(),
    }
}

fn plugin_oauth_auth(cp: &ControlPlane, plugin_id: &str) -> anyhow::Result<AuthSpec> {
    let plugin = cp
        .plugins()
        .get(plugin_id)
        .ok_or_else(|| anyhow::anyhow!("unknown plugin: {plugin_id}"))?;
    let auth = plugin
        .manifest
        .auth
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("{plugin_id} does not declare an auth block"))?;
    if auth.kind != AuthKind::Oauth {
        anyhow::bail!("{plugin_id} does not use OAuth")
    }
    Ok(auth.clone())
}

async fn exchange_plugin_oauth_code(
    store: &Store,
    plugin_id: &str,
    auth: &AuthSpec,
    flow: &PluginOauthFlowState,
    code: &str,
) -> anyhow::Result<PluginOauthToken> {
    let resolved = resolve_plugin_oauth(store, plugin_id, auth).await?;
    if let Some(message) = plugin_oauth_prereq_error(plugin_id, auth, &resolved) {
        anyhow::bail!(message);
    }
    let client_id = resolved
        .client_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("{plugin_id} OAuth sign-in is missing a client id"))?;
    let token_url = resolved
        .token_url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("{plugin_id} OAuth sign-in is missing auth.token_url"))?;
    let client_secret = plugin_oauth_client_secret(store, auth).await?;
    let mut form = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), flow.redirect_uri.clone()),
        ("client_id".to_string(), client_id),
        ("code_verifier".to_string(), flow.verifier.clone()),
    ];
    if !flow.requested_scopes.is_empty() {
        form.push(("scope".to_string(), flow.requested_scopes.join(" ")));
    }
    if let Some(resource) = auth.resource.as_deref().filter(|value| !value.is_empty()) {
        form.push(("resource".to_string(), resource.to_string()));
    }
    if let Some(secret) = client_secret {
        form.push(("client_secret".to_string(), secret));
    }
    for (key, value) in &auth.extra_token_params {
        form.push((key.clone(), value.clone()));
    }

    let http = reqwest::Client::new();
    let response = http.post(token_url).form(&form).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let detail = body.trim();
        if detail.is_empty() {
            anyhow::bail!("{plugin_id} OAuth token exchange failed with HTTP {status}");
        }
        anyhow::bail!("{plugin_id} OAuth token exchange failed with HTTP {status}: {detail}");
    }

    let payload: PluginOauthTokenResponse = response.json().await?;
    let access_token = payload
        .access_token
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("{plugin_id} OAuth token response is missing access_token")
        })?;
    let token_type = payload
        .token_type
        .filter(|token_type| !token_type.is_empty())
        .unwrap_or_else(|| "Bearer".to_string());
    let scopes = payload
        .scope
        .map(|scope| {
            scope
                .split_whitespace()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|scopes| !scopes.is_empty())
        .unwrap_or_else(|| flow.requested_scopes.clone());
    let expires_at = payload
        .expires_in
        .map(|seconds| crate::paths::now_ms() + seconds.saturating_mul(1000));

    Ok(PluginOauthToken {
        plugin_id: plugin_id.to_string(),
        access_token,
        refresh_token: payload.refresh_token.filter(|token| !token.is_empty()),
        token_type,
        expires_at,
        scopes,
        reconnect_required: false,
    })
}

struct InstalledCtx {
    installed_skills: Vec<crate::skills_install::InstalledSkillInfo>,
    installed_providers: Vec<String>,
}

async fn installed_ctx(store: &Store) -> anyhow::Result<InstalledCtx> {
    Ok(InstalledCtx {
        installed_skills: crate::skills_install::list_installed_skills().unwrap_or_default(),
        installed_providers: crate::llm_router::installed::list_installed_providers(store).await?,
    })
}

async fn compute_installed(
    store: &Store,
    plugin: &CorePlugin,
    kind: &str,
    enabled: bool,
    configured: bool,
    ctx: &InstalledCtx,
    component_active: bool,
) -> anyhow::Result<bool> {
    let id = &plugin.manifest.id;
    // Provider installed-ness is authoritative on the persisted installed set
    // ALONE — never on whether a connection exists. The Models list filters on
    // the same set, so both surfaces agree in lockstep. Existing-connection
    // families are unioned into the set at boot by
    // `ensure_default_installed_providers`, and connections are only ever added
    // to already-installed providers, so a real connection always has its
    // family in the set.
    let provider_installed = kind == "provider"
        && crate::llm_router::installed::is_installed(
            &ctx.installed_providers,
            &provider_family(id),
        );
    let gateway_settings_complete = if kind == "gateway" {
        // A gateway with no manifest settings has nothing to configure, so its
        // installed-ness is just whether it's enabled — otherwise it could
        // never leave Browse. A gateway that declares required settings takes
        // the all-present path below.
        if plugin.manifest.settings.is_empty() {
            enabled
        } else {
            let mut complete = true;
            for field in &plugin.manifest.settings {
                let value = store.get_setting_raw(&field.key).await?;
                if value.as_deref().map(str::trim).is_none_or(str::is_empty) {
                    complete = false;
                    break;
                }
            }
            complete
        }
    } else {
        false
    };
    let skill_pack_installed = kind == "skill-pack"
        && ctx
            .installed_skills
            .iter()
            .any(|s| s.plugin_id.as_deref() == Some(id.as_str()) || &s.id == id);
    Ok(installed_flag(
        kind,
        enabled,
        configured,
        provider_installed,
        gateway_settings_complete,
        skill_pack_installed,
        component_active,
    ))
}

/// Fetch every `plugin_installs` ledger row ONCE and index it by plugin id so
/// list assembly stays O(1) round-trips regardless of the plugin count (never
/// a per-plugin `get_plugin_install` inside the loop below).
async fn install_ledger_index(
    store: &Store,
) -> anyhow::Result<HashMap<String, crate::store::PluginInstallRecord>> {
    Ok(store
        .list_plugin_installs()
        .await?
        .into_iter()
        .map(|r| (r.plugin_id.clone(), r))
        .collect())
}

/// Fetch every cached `plugin_catalog_cache` row ONCE and index it by plugin
/// id — mirrors [`install_ledger_index`]'s O(1)-round-trip shape, so list
/// assembly never issues a per-plugin remote-catalog query.
async fn remote_catalog_index(store: &Store) -> anyhow::Result<HashMap<String, RemoteCatalogRow>> {
    Ok(store
        .list_remote_catalog()
        .await?
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect())
}

/// Fetch every `plugin_attach_status` row ONCE and index it by plugin id,
/// keeping only entries whose last recorded outcome was a failure — mirrors
/// [`install_ledger_index`]'s O(1)-round-trip shape and the doctor's
/// `attach-failed` predicate (`doctor.rs:177-191`). `plugin_id` is the
/// table's primary key (see `Store::record_plugin_attach`'s upsert), so there
/// is at most one row per id already.
async fn attach_failed_index(store: &Store) -> anyhow::Result<HashMap<String, String>> {
    Ok(store
        .list_plugin_attach()
        .await?
        .into_iter()
        .filter(|a| a.outcome == "failed")
        .map(|a| {
            let reason = a
                .reason
                .clone()
                .unwrap_or_else(|| format!("{} failed to attach", a.plugin_id));
            (a.plugin_id, reason)
        })
        .collect())
}

/// The currently active `component_plugin_releases` version for every plugin
/// id that has at least one row in that ledger, indexed by plugin id — built
/// from [`Store::list_component_release_plugin_ids`] plus the same
/// [`Store::active_component_release`] accessor `plugin_release_detail` uses,
/// so list assembly's `update_available` derivation reads the identical
/// source of truth as the release-management surface.
async fn active_release_version_index(store: &Store) -> anyhow::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for id in store.list_component_release_plugin_ids().await? {
        if let Some(rec) = store.active_component_release(&id).await? {
            out.insert(id, rec.version);
        }
    }
    Ok(out)
}

async fn assemble_list(cp: &ControlPlane) -> anyhow::Result<Vec<PluginInfo>> {
    let settings = SettingsStore::new(cp.store().clone());
    let ctx = installed_ctx(cp.store()).await?;
    let installs = install_ledger_index(cp.store()).await?;
    let remote = remote_catalog_index(cp.store()).await?;
    let attach_failed = attach_failed_index(cp.store()).await?;
    let active_releases = active_release_version_index(cp.store()).await?;
    let mut out = Vec::new();
    for plugin in cp.plugins().list() {
        let Some(kind) = derive_kind(&plugin) else {
            continue;
        };
        let enabled = cp
            .plugins()
            .is_enabled(&settings, &plugin.manifest.id)
            .await?;
        let configured = plugin_auth_configured(
            cp.store(),
            &plugin.manifest.id,
            plugin.manifest.auth.as_ref(),
        )
        .await?;
        let installed = compute_installed(
            cp.store(),
            &plugin,
            kind,
            enabled,
            configured,
            &ctx,
            active_releases.contains_key(&plugin.manifest.id),
        )
        .await?;
        let record = installs.get(&plugin.manifest.id);
        let remote_row = remote.get(&plugin.manifest.id);
        let owns_slot = plugin
            .manifest
            .slot
            .as_deref()
            .is_some_and(|s| cp.plugins().slot_owner(s) == Some(plugin.manifest.id.as_str()));
        let skill_count = (kind == "skill-pack")
            .then(|| {
                ctx.installed_skills
                    .iter()
                    .find(|s| {
                        s.plugin_id.as_deref() == Some(plugin.manifest.id.as_str())
                            || s.id == plugin.manifest.id
                    })
                    .map(|s| s.skill_count as u32)
            })
            .flatten();
        out.push(plugin_info(
            &plugin,
            enabled,
            configured,
            kind,
            installed,
            PluginInfoContext {
                install: record,
                remote: remote_row,
                owns_slot,
                attach_failed: attach_failed.get(&plugin.manifest.id).map(String::as_str),
                active_version: active_releases.get(&plugin.manifest.id).map(String::as_str),
                skill_count,
            },
        ));
    }
    for pack in crate::skills_install::curated_skill_packs() {
        if cp.plugins().get(pack.id).is_some() || out.iter().any(|p| p.id == pack.id) {
            continue;
        }
        let installed_skill = ctx
            .installed_skills
            .iter()
            .find(|s| s.id == pack.id || s.source == pack.id || s.source == pack.repo);
        let installed = installed_skill.is_some();
        let skill_count = installed_skill.map(|s| s.skill_count as u32);
        let ledger = InstallLedgerFields::from_option(installs.get(pack.id));
        out.push(curated_pack_row(pack, installed, skill_count, ledger));
    }
    Ok(out)
}

/// Synthesizes the `PluginInfo` row for a curated skill pack (e.g.
/// `superpowers`) that has no registered `CorePlugin`/manifest — shared by
/// `assemble_list`'s curated-pack loop (a Browse tile before install) and
/// `assemble_detail`'s not-a-registered-plugin fallback (Task 5), so the two
/// surfaces can never drift.
///
/// This row bypasses `derive_plugin_status` by design (there is no manifest
/// to derive from) — `status`/`skill_count` are set directly from `installed`
/// / `skill_count` instead (Finding 4, final-review fix): a curated pack the
/// caller reports as already installed must read `"ok"`, not the hardcoded
/// `"not-installed"` the Browse-tile default used to send back even for an
/// installed pack — Cockpit's Install/Open split reads `status`, so the
/// installed case wrongly kept offering an Install button. `skill_count`
/// mirrors the same field's derivation on a normal skill-pack `PluginInfo`
/// row (`assemble_list`/`assemble_detail`'s own `(kind == "skill-pack")`
/// lookups against `ctx.installed_skills`), populated by both call sites only
/// when `installed` is true.
fn curated_pack_row(
    pack: &crate::skills_install::CuratedSkillPack,
    installed: bool,
    skill_count: Option<u32>,
    ledger: InstallLedgerFields,
) -> PluginInfo {
    PluginInfo {
        id: pack.id.to_string(),
        name: pack.name.to_string(),
        description: pack.description.to_string(),
        icon: Some("sparkles".to_string()),
        categories: vec!["skills".to_string()],
        // A synthesized curated pack has no manifest to declare a slot.
        slot: None,
        owns_slot: false,
        verified: true,
        experimental: false,
        // A synthesized pack isn't a registered plugin, so `enabled` /
        // `configured` are meaningless here — only `installed` drives the
        // Browse/Installed split.
        enabled: false,
        configured: false,
        source: "skill-pack".to_string(),
        capabilities: vec![],
        kind: "skill-pack".to_string(),
        installed,
        family: None,
        pinned: ledger.pinned,
        source_spec: ledger.source_spec,
        resolved_commit: ledger.resolved_commit,
        installed_at: ledger.installed_at,
        updated_at: ledger.updated_at,
        trust_tier: ledger.trust_tier,
        // A synthesized curated pack resolves via git clone, so it is
        // never a component bundle and never came from a manifest feed.
        component_backed: false,
        catalog_version: None,
        blocked_reason: None,
        // Synthesized rows have no manifest-backed status machinery
        // (`enabled`/`configured` are meaningless here too, see above), so
        // this bypasses `derive_plugin_status` entirely: `"ok"` once
        // installed (Finding 4), else the Browse-tile default per Task 3's
        // spec.
        status: if installed { "ok" } else { "not-installed" }.to_string(),
        status_detail: None,
        auth_kind: "none".to_string(),
        tool_count: None,
        skill_count: installed.then_some(skill_count).flatten(),
    }
}

async fn assemble_detail(cp: &ControlPlane, id: &str) -> anyhow::Result<PluginDetail> {
    let Some(plugin) = cp.plugins().get(id) else {
        return curated_pack_detail(cp, id).await;
    };
    let settings = SettingsStore::new(cp.store().clone());
    let enabled = cp.plugins().is_enabled(&settings, id).await?;
    let m = &plugin.manifest;

    let auth = match &m.auth {
        Some(auth) => Some(build_auth_info(cp.store(), id, auth).await?),
        None => None,
    };
    let settings_info = build_settings_info(cp.store(), &m.settings).await?;
    let mcp = m.mcp.iter().map(mcp_info).collect();
    let models = providers::list_models(cp.store(), id).await?;
    let configured = plugin_auth_configured(cp.store(), id, m.auth.as_ref()).await?;
    let kind = derive_kind(&plugin).unwrap_or("integration");
    let ctx = installed_ctx(cp.store()).await?;
    // Fetched before `compute_installed` — an active release makes an
    // integration/gateway row installed regardless of enable/configure state.
    let active_release = cp.store().active_component_release(id).await?;
    let installed = compute_installed(
        cp.store(),
        &plugin,
        kind,
        enabled,
        configured,
        &ctx,
        active_release.is_some(),
    )
    .await?;
    // Single-plugin lookup is fine here — unlike `assemble_list`, there is
    // only ever one id to resolve for a detail view.
    let record = cp.store().get_plugin_install(id).await?;
    let remote_row = cp
        .store()
        .list_remote_catalog()
        .await?
        .into_iter()
        .find(|r| r.id == id);

    let owns_slot = m
        .slot
        .as_deref()
        .is_some_and(|s| cp.plugins().slot_owner(s) == Some(id));

    // Same single-plugin-lookup shape as `record`/`remote_row` above — a
    // detail view only ever resolves one id, so there is no index to build.
    let attach = cp.store().get_plugin_attach(id).await?;
    let attach_failed = attach.as_ref().filter(|a| a.outcome == "failed").map(|a| {
        a.reason
            .clone()
            .unwrap_or_else(|| format!("{id} failed to attach"))
    });
    let skill_count = (kind == "skill-pack")
        .then(|| {
            ctx.installed_skills
                .iter()
                .find(|s| s.plugin_id.as_deref() == Some(id) || s.id == id)
                .map(|s| s.skill_count as u32)
        })
        .flatten();

    Ok(PluginDetail {
        info: plugin_info(
            &plugin,
            enabled,
            configured,
            kind,
            installed,
            PluginInfoContext {
                install: record.as_ref(),
                remote: remote_row.as_ref(),
                owns_slot,
                attach_failed: attach_failed.as_deref(),
                active_version: active_release.as_ref().map(|r| r.version.as_str()),
                skill_count,
            },
        ),
        auth,
        settings: settings_info,
        mcp,
        models,
        homepage: m.homepage.clone(),
        publisher: m.publisher.clone(),
    })
}

/// `assemble_detail`'s fallback when `id` isn't a registered `CorePlugin`: an
/// uninstalled curated skill pack (e.g. `superpowers`, see
/// `crate::skills_install::curated_skill_packs`) has no manifest, but
/// `assemble_list` already synthesizes a Browse-tile row for it via
/// `curated_pack_row` — navigating into that tile before install must resolve
/// the same row wrapped in a minimal `PluginDetail`, not 500. A truly unknown
/// id (matches no curated pack either) still bails `unknown plugin: {id}`,
/// same text/mechanism `assemble_detail` used before this fallback existed.
async fn curated_pack_detail(cp: &ControlPlane, id: &str) -> anyhow::Result<PluginDetail> {
    let Some(pack) = crate::skills_install::curated_skill_packs()
        .iter()
        .find(|p| p.id == id)
    else {
        anyhow::bail!("unknown plugin: {id}");
    };
    let ctx = installed_ctx(cp.store()).await?;
    let installed_skill = ctx
        .installed_skills
        .iter()
        .find(|s| s.id == pack.id || s.source == pack.id || s.source == pack.repo);
    let installed = installed_skill.is_some();
    let skill_count = installed_skill.map(|s| s.skill_count as u32);
    let record = cp.store().get_plugin_install(id).await?;
    let ledger = InstallLedgerFields::from_option(record.as_ref());
    Ok(PluginDetail {
        info: curated_pack_row(pack, installed, skill_count, ledger),
        auth: None,
        settings: vec![],
        mcp: vec![],
        models: vec![],
        homepage: Some(pack.repo.to_string()),
        publisher: String::new(),
    })
}

/// `plugin_tools` RPC: everything a plugin currently offers — an agent-facing
/// tool, an installed skill, or a provider's model — as one flat list. The
/// sources below are tried in order; a branch "wins" (returns) only when it
/// actually has at least one entry, EXCEPT the last one, which always wins
/// (it's the terminal fallback):
///
/// 1. **Declared component tools** — `id` is a first-party WASM bundle
///    ([`crate::plugins::component_catalog::is_component_bundle`]). Prefers
///    the currently-*installed* release's on-disk manifest (same read
///    `plugin_release_detail` performs) so a post-install listing reflects
///    the running release; falls back to the embedded manifest
///    ([`crate::plugins::component_catalog::declared_tools`]) so a bundle
///    not yet installed still shows "what you'll get". `live: false`. A
///    component-backed id with ZERO declared tools does NOT return here — it
///    falls through to steps 2-3, because a component-backed *provider*
///    (e.g. `mimo`, `opencode`: `is_component_bundle` is true for them, but
///    their embedded bundle manifest declares no `[[tools]]`) must still
///    reach step 3's model list rather than short-circuit to an empty
///    result. A gateway like `discord` (also zero declared tools, no
///    provider capability) still ends up empty — it just gets there via
///    step 3's empty-list fallback instead of step 1's early return.
/// 2. **Skill packs** — `id` is an installed skill pack
///    ([`crate::skills_install::get_installed_skill_pack`]): one entry per
///    installed skill, description-less (a skill's prose lives in its own
///    `SKILL.md`, not surfaced here). `live: false`.
/// 3. **Providers** — `id`'s effective model list, via the same
///    [`providers::list_models`] internals `plugin_models` uses. This also
///    covers "anything else": `list_models` never errors for a non-provider
///    id, it just returns an empty list, so a known plugin id that matches
///    none of the branches above still resolves to an empty, well-formed
///    result rather than an error. `live: false`.
///
/// Unknown id -> the same `unknown plugin: {id}` `ApiError` `plugin_detail`
/// (`assemble_detail`) uses.
async fn plugin_tools(cp: &ControlPlane, id: &str) -> Result<PluginToolsResult, ApiError> {
    cp.plugins()
        .get(id)
        .ok_or_else(|| ApiError::not_found(format!("unknown plugin: {id}")))?;

    // 1. Declared component tools. Only short-circuits when there's actually
    // something to show — a component-backed provider (mimo, opencode) has
    // no declared tools and must fall through to step 3's model list instead
    // of resolving to a misleadingly empty result.
    if crate::plugins::component_catalog::is_component_bundle(id) {
        let entries = declared_component_tool_entries(cp, id).await?;
        if !entries.is_empty() {
            return Ok(PluginToolsResult {
                plugin_id: id.to_string(),
                live: false,
                entries,
            });
        }
    }

    // 2. Skill packs.
    if let Some(pack) = crate::skills_install::get_installed_skill_pack(id) {
        let entries = pack
            .skills
            .into_iter()
            .map(|s| PluginToolEntry {
                name: s.name,
                description: String::new(),
                kind: "skill".to_string(),
                writes: None,
            })
            .collect();
        return Ok(PluginToolsResult {
            plugin_id: id.to_string(),
            live: false,
            entries,
        });
    }

    // 3. Providers (and, by `list_models`'s own never-errors-just-empty
    // contract, every other "known but nothing to list" id).
    let entries = providers::list_models(cp.store(), id)
        .await?
        .into_iter()
        .map(|model_id| PluginToolEntry {
            name: model_id,
            description: String::new(),
            kind: "model".to_string(),
            writes: None,
        })
        .collect();
    Ok(PluginToolsResult {
        plugin_id: id.to_string(),
        live: false,
        entries,
    })
}

/// Step 2's tool source for [`plugin_tools`]: prefer the currently-active
/// installed bundle's on-disk manifest, falling back to the embedded
/// first-party manifest. Mirrors `plugin_release_detail`'s own
/// active-release-then-disk read exactly (including gating the disk read on
/// the store actually recording an active release), so the two RPCs can
/// never disagree about which manifest is "the" active one.
async fn declared_component_tool_entries(
    cp: &ControlPlane,
    id: &str,
) -> anyhow::Result<Vec<PluginToolEntry>> {
    let has_active_release = cp.store().active_component_release(id).await?.is_some();
    let installed_tools = if has_active_release {
        let root = crate::plugins::bundle::installed_bundle_root();
        crate::plugins::bundle::load_active_bundles(&root, cp.store())
            .await
            .ok()
            .and_then(|bundles| bundles.into_iter().find(|b| b.manifest.id == id))
            .map(|b| b.manifest.tools)
    } else {
        None
    };
    let tools =
        installed_tools.unwrap_or_else(|| crate::plugins::component_catalog::declared_tools(id));
    Ok(tools
        .into_iter()
        .map(|t| PluginToolEntry {
            name: t.name,
            description: t.description,
            kind: "tool".to_string(),
            writes: Some(t.writes),
        })
        .collect())
}

/// Re-run WASM provider discovery against the installed-bundle root (spec B1).
/// The transport registry is a live `RwLock` with insert-or-replace semantics
/// (`wasm_provider::register_wasm_provider`), so this is safe after any
/// install/rollback/enable: an ENABLED provider bundle's transport is usable
/// immediately, with no daemon restart. Disabled bundles and non-provider
/// bundles are skipped inside discovery itself, so this is a no-op on the
/// paths that shouldn't change anything (e.g. a fresh mimo install that the
/// user hasn't enabled — the Phase-7 native default stays untouched).
async fn hot_reload_provider_transports(cp: &ControlPlane) {
    let settings = SettingsStore::new(cp.store().clone());
    let registered = crate::plugins::wasm_provider::discover_provider_components(
        cp.store().clone(),
        &settings,
        cp.telemetry(),
        &crate::plugins::bundle::installed_bundle_root(),
    )
    .await;
    if !registered.is_empty() {
        tracing::info!(providers = ?registered, "hot-registered wasm provider transport(s)");
    }
}

/// True when `plugin_id`'s ACTIVE installed bundle exports the wasm gateway
/// interface. The granular restart latch needs this because a component
/// gateway's host row is the manifest-only catalog stand-in — `derive_kind`
/// says "integration", but the gateway supervisor + Router map stay frozen
/// until reboot. Fail-closed: any load/compile error latches conservatively
/// (returns true); a missing bundle root or no active bundle for this id
/// returns false (classic rows, and a first-time install with nothing on
/// disk yet, have nothing frozen to reload).
///
/// `root` is a parameter (rather than always
/// [`crate::plugins::bundle::installed_bundle_root`] internally) for the
/// same reason [`crate::plugins::wasm_provider::discover_provider_components`]
/// takes one: it lets a unit test point this at a hermetic temp dir instead
/// of this machine's real, possibly-populated per-user install root —
/// `installed_bundle_root()` has no env-var test seam of its own (see
/// `InstalledBundleFixture`'s doc in this module's tests), and `load_active_
/// bundles` fails closed (this fn's own contract) on ANY mismatch anywhere
/// under the root, so a real root with unrelated real installs would make a
/// hermetic-root-less test of this function flaky on a machine that has
/// ever manually installed a component bundle.
async fn installed_bundle_is_gateway(
    store: &std::sync::Arc<Store>,
    root: &std::path::Path,
    plugin_id: &str,
) -> bool {
    use crate::plugins::runtime::{ComponentRuntime, HostPolicy};
    // Mirrors `discover_provider_components`'s own pre-check: `load_active_
    // bundles` calls `root.canonicalize()` first, which errors on a
    // nonexistent path — so an absent bundle root (nothing ever installed)
    // must be treated as "no active bundle", not a fail-closed error.
    if !root.exists() {
        return false;
    }
    let bundles = match crate::plugins::bundle::load_active_bundles(root, store).await {
        Ok(bundles) => bundles,
        Err(_) => return true,
    };
    let Some(bundle) = bundles.into_iter().find(|b| b.manifest.id == plugin_id) else {
        return false;
    };
    let Ok(runtime) = ComponentRuntime::new() else {
        return true;
    };
    let policy = HostPolicy::for_installed_bundle(&bundle);
    match runtime.compile(&bundle, policy) {
        Ok(compiled) => compiled.exports_gateway(),
        Err(_) => true,
    }
}

/// Same semantics as `ryuzi plugins enable/disable` — delegates to the shared
/// core helper so the two surfaces never drift.
async fn set_plugin_enabled(cp: &ControlPlane, id: String, enabled: bool) -> Result<(), ApiError> {
    let settings = SettingsStore::new(cp.store().clone());
    crate::plugins::toggle_enabled(cp.plugins(), &settings, &id, enabled).await?;
    // Spec B1: provider transports are hot — enabling registers the transport
    // now, disabling unregisters it. Other kinds keep their existing axes
    // (connectors re-discover per session; gateways gate on the same
    // `plugin.<id>.enabled` key, read fresh on the next attach/restart).
    let kind = cp.plugins().get(&id);
    let is_provider = kind.as_deref().and_then(derive_kind) == Some("provider");
    if is_provider && crate::plugins::component_catalog::is_component_bundle(&id) {
        if enabled {
            hot_reload_provider_transports(cp).await;
        } else {
            crate::plugins::wasm_provider::unregister_wasm_providers_for_plugin(&id);
        }
    }
    cp.emit(CoreEvent::PluginsChanged);
    Ok(())
}

/// Validated write through `SettingsStore::set` — rejects unknown keys and
/// type-mismatched values the same way `ryuzi config set` does. Never returns
/// a value, so no secret can leak back through this command.
async fn set_plugin_setting(cp: &ControlPlane, key: String, value: String) -> Result<(), ApiError> {
    let settings = SettingsStore::new(cp.store().clone());
    settings.set(&key, &value).await?;
    Ok(())
}

/// Kind-symmetric uninstall: after this the entry's `installed` flips false and
/// it reappears in Browse.
async fn uninstall(cp: &ControlPlane, id: &str) -> anyhow::Result<()> {
    let settings = SettingsStore::new(cp.store().clone());
    let Some(plugin) = cp.plugins().get(id) else {
        // Synthesized curated pack or a pack installed without a manifest —
        // resolve through the skills installer.
        let installed = crate::skills_install::list_installed_skills()?;
        let Some(pack) = installed
            .iter()
            .find(|s| s.id == id || s.source == id || s.plugin_id.as_deref() == Some(id))
        else {
            anyhow::bail!("unknown plugin: {id}");
        };
        // Recorded variant: also drop the pack's `plugin_installs` +
        // `plugin_attach_status` rows, so an uninstalled pack doesn't leave a
        // ghost ledger row (which would make every future `update_all_packs`
        // report `Failed("unknown installed skill: <id>")` for it and bleed
        // stale trust/pin/attach metadata into the reappeared Browse card).
        return crate::skills_install::remove_installed_skill_recorded(&pack.id, cp.store()).await;
    };
    match derive_kind(&plugin) {
        Some("provider") => {
            let family = provider_family(id);
            for row in crate::llm_router::connections::list_connections(cp.store()).await? {
                if provider_family(&row.provider) != family {
                    continue;
                }
                if crate::llm_router::connections::is_builtin_free(&row.provider, &row.auth_type) {
                    continue; // spec A2: builtin free rows are infrastructure and survive uninstall
                }
                crate::llm_router::connections::remove_connection(cp.store(), &row.id).await?;
            }
            // Spec B1: an uninstalled provider must stay off — clear the
            // transport key or the next hot reload would re-register it.
            cp.store()
                .delete_setting_raw(&crate::plugins::qualified_setting_key(id, "enabled"))
                .await?;
            Ok(())
        }
        Some("gateway") => {
            for field in &plugin.manifest.settings {
                cp.store().delete_setting_raw(&field.key).await?;
            }
            // A component-backed gateway also sheds its installed bundle:
            // installed-ness reads the active-release ledger, so leaving the
            // release active would keep the row "installed" forever. No-op
            // for native gateways (no release rows). Deactivated, NOT
            // revoked — a reinstall stays possible.
            cp.store().deactivate_component_releases(id).await?;
            crate::plugins::toggle_enabled(cp.plugins(), &settings, id, false).await
        }
        Some("skill-pack") => {
            let installed = crate::skills_install::list_installed_skills()?;
            let Some(pack) = installed
                .iter()
                .find(|s| s.plugin_id.as_deref() == Some(id) || s.id == id)
            else {
                anyhow::bail!("skill pack not installed: {id}");
            };
            // Recorded variant — drops the ledger row too (see the
            // not-in-host fallback above for why a ghost row is harmful).
            crate::skills_install::remove_installed_skill_recorded(&pack.id, cp.store()).await
        }
        _ => {
            if let Some(auth) = &plugin.manifest.auth {
                if let Some(setting) = &auth.setting {
                    cp.store().delete_setting_raw(setting).await?;
                }
                if auth.kind == AuthKind::Oauth {
                    cp.store().delete_plugin_oauth_token(id).await?;
                }
            }
            for field in &plugin.manifest.settings {
                cp.store().delete_setting_raw(&field.key).await?;
            }
            if plugin.connector.is_some() && !plugin.manifest.experimental {
                crate::plugins::toggle_enabled(cp.plugins(), &settings, id, false).await?;
            }
            // Component-backed integrations (and the catalog stand-in a
            // component gateway wears pre-restart) deactivate their installed
            // bundle too — same rationale as the "gateway" arm above; the
            // reconcile's `was_gateway_bundle` check runs BEFORE this for
            // exactly that reason. No-op for rows without release ledger rows.
            cp.store().deactivate_component_releases(id).await?;
            Ok(())
        }
    }
}

/// Kind-aware post-uninstall reconcile — the `uninstall_plugin` dispatch
/// arm's body, extracted so the latch-granularity tests can call it directly
/// against a bare `ControlPlane` (this module's tests never go through
/// `dispatch`/`ApiState` for `uninstall`). Drops a live provider transport
/// immediately (hot); leaves a connector alone (it re-discovers per
/// session, hot next session); and falls back to the conservative restart
/// latch for a gateway (frozen Router map), skill pack, or any id the host
/// doesn't know. Always emits `PluginsChanged`.
async fn uninstall_and_reconcile(cp: &ControlPlane, id: &str) -> anyhow::Result<()> {
    // Kind decides the reconcile below; captured up front for clarity
    // (host rows are startup-frozen, so before/after is equivalent).
    let kind = cp
        .plugins()
        .get(id)
        .as_deref()
        .and_then(derive_kind)
        .map(str::to_owned);
    // A component GATEWAY's host row still reads "integration" via the
    // catalog stand-in (see `installed_bundle_is_gateway`'s doc comment), so
    // that arm needs the bundle capability check. Computed BEFORE `uninstall`
    // runs below — uninstall may deactivate the bundle, after which the
    // check could no longer answer. Only paid for on the "integration" arm
    // (the one case that needs it), so a provider/gateway/skill-pack
    // uninstall never touches the bundle root at all.
    let was_gateway_bundle = if kind.as_deref() == Some("integration") {
        installed_bundle_is_gateway(
            cp.store(),
            &crate::plugins::bundle::installed_bundle_root(),
            id,
        )
        .await
    } else {
        false
    };
    uninstall(cp, id).await?;
    match kind.as_deref() {
        // Hot: drop the live transport (no-op when none registered).
        Some("provider") => crate::plugins::wasm_provider::unregister_wasm_providers_for_plugin(id),
        // Hot next session UNLESS the installed bundle is actually a gateway
        // (frozen Router map) — see the capability check above.
        Some("integration") => {
            if was_gateway_bundle {
                cp.mark_plugins_restart_required();
            }
        }
        // Gateway (frozen Router map), skill packs, and anything the
        // host doesn't know keep the conservative latch.
        _ => cp.mark_plugins_restart_required(),
    }
    cp.emit(CoreEvent::PluginsChanged);
    Ok(())
}

async fn begin_plugin_oauth(
    cp: &ControlPlane,
    plugin_id: String,
) -> Result<PluginOauthBeginResult, ApiError> {
    let auth = plugin_oauth_auth(cp, &plugin_id)?;
    let verifier = generate_pkce_verifier();
    let state_token = crate::paths::new_id();
    let begin =
        build_plugin_oauth_begin_result(cp.store(), &plugin_id, &auth, &verifier, &state_token)
            .await?;
    plugin_oauth_flows()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(
            plugin_oauth_flow_key(&plugin_id, &state_token),
            PluginOauthFlowState {
                verifier,
                redirect_uri: begin.redirect_uri.clone(),
                requested_scopes: plugin_oauth_requested_scopes(&auth),
            },
        );
    cp.emit(CoreEvent::PluginOauthAuthorizeUrl {
        plugin_id,
        authorize_url: begin.authorize_url.clone(),
    });
    Ok(begin)
}

async fn complete_plugin_oauth(
    cp: &ControlPlane,
    plugin_id: String,
    code: String,
    state_token: String,
) -> Result<PluginAuthInfo, ApiError> {
    let auth = plugin_oauth_auth(cp, &plugin_id)?;
    let flow_key = plugin_oauth_flow_key(&plugin_id, &state_token);
    let flow = plugin_oauth_flows()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .remove(&flow_key)
        .ok_or_else(|| ApiError::bad_request("plugin sign-in flow not found — start again"))?;
    let token =
        match exchange_plugin_oauth_code(cp.store(), &plugin_id, &auth, &flow, code.trim()).await {
            Ok(token) => token,
            Err(err) => {
                plugin_oauth_flows()
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(flow_key, flow);
                return Err(err.into());
            }
        };
    cp.store().upsert_plugin_oauth_token(&token).await?;
    Ok(build_auth_info(cp.store(), &plugin_id, &auth).await?)
}

async fn disconnect_plugin_oauth(
    cp: &ControlPlane,
    plugin_id: String,
) -> Result<PluginAuthInfo, ApiError> {
    let auth = plugin_oauth_auth(cp, &plugin_id)?;
    cp.store().delete_plugin_oauth_token(&plugin_id).await?;
    Ok(build_auth_info(cp.store(), &plugin_id, &auth).await?)
}

/// The install wizard's entry point (spec 8-step resolution order). Steps 1-6
/// live in `resolve_plugin_install`; the daemon adds step 8 here (emit
/// `CoreEvent::PluginOauthAuthorizeUrl`, which the Cockpit SSE bridge maps to
/// a browser open). Step 7 (bind 8976 + background callback/exchange task)
/// stays Cockpit-local in the `begin_plugin_install` proxy, so
/// `callback_mode` is left `"manual"` here — Cockpit flips it to `"auto"`
/// after a successful local bind.
async fn begin_plugin_install(
    cp: &ControlPlane,
    plugin_id: String,
) -> Result<PluginInstallBeginResult, ApiError> {
    let plugin = cp
        .plugins()
        .get(&plugin_id)
        .ok_or_else(|| ApiError::not_found(format!("unknown plugin: {plugin_id}")))?;
    if plugin.manifest.component.is_some() {
        return Err(ApiError::bad_request(
            "component plugins connect via their oauth profiles, not the declarative install flow"
                .to_string(),
        ));
    }
    let auth = plugin.manifest.auth.clone();
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| ApiError {
            status: 500,
            message: e.to_string(),
        })?;
    let result = resolve_plugin_install(cp.store(), &http, &plugin_id, auth.as_ref()).await?;
    if let Some(begin) = result.oauth_begin.clone() {
        cp.emit(CoreEvent::PluginOauthAuthorizeUrl {
            plugin_id: plugin_id.clone(),
            authorize_url: begin.authorize_url.clone(),
        });
    }
    Ok(result)
}

/// Persist a manually-entered client id. External-OAuth plugins store it under
/// the declared `auth.setting` via the validated SettingsStore path
/// (`validate_setting`/`register_plugin_fields` only accept manifest-declared
/// keys); everyone else upserts `plugin_oauth_clients.client_id` — deliberately
/// NOT a `plugin.*` setting, since none of these manifests declare one.
async fn set_plugin_oauth_client_id(
    cp: &ControlPlane,
    plugin_id: String,
    client_id: String,
) -> Result<(), ApiError> {
    let auth = plugin_oauth_auth(cp, &plugin_id)?;
    let client_id = client_id.trim();
    if client_id.is_empty() {
        return Err(ApiError::bad_request("client id must not be empty"));
    }
    if is_external_oauth(&auth) {
        let Some(key) = auth.setting.as_deref() else {
            return Err(ApiError::bad_request(format!(
                "{plugin_id} declares no auth.setting to hold a client id"
            )));
        };
        let settings = SettingsStore::new(cp.store().clone());
        settings.set(key, client_id).await?;
        return Ok(());
    }
    cp.store()
        .upsert_plugin_oauth_client(&PluginOauthClient {
            plugin_id: plugin_id.clone(),
            authorize_url: None,
            token_url: None,
            client_id: Some(client_id.to_string()),
        })
        .await?;
    Ok(())
}

/// Cancel the pending OAuth flow for this plugin, if any (daemon half): drops
/// the flow-map entry. `state_token` narrows to a specific flow when known;
/// `None` cancels whatever is pending for the id. Shutting down the local
/// loopback callback listener is the Cockpit half (`plugins_cmd.rs`).
async fn cancel_plugin_install(
    cp: &ControlPlane,
    plugin_id: String,
    state_token: Option<String>,
) -> Result<(), ApiError> {
    if cp.plugins().get(&plugin_id).is_none() {
        return Err(ApiError::not_found(format!("unknown plugin: {plugin_id}")));
    }
    drop_pending_plugin_flows(&plugin_id, state_token.as_deref());
    Ok(())
}

// ===========================================================================
// Component-plugin (WASM bundle) release management — Task 11a.
// ===========================================================================

/// The release ledger for a component plugin: every recorded release (oldest
/// first) plus the active version. Read-only; the template is `plugin_detail`.
///
/// Task 12 addition: when a version is active, also resolves that version's
/// on-disk bundle manifest (publisher/lifecycle/domains/oauth) for the
/// permission-confirmation summary — see [`ComponentManifestInfo`]'s doc for
/// why this is read-only-disk, not a new network fetch, and why it is `None`
/// for a never-installed plugin. Best-effort: any I/O error (most commonly,
/// the bundle root not existing yet) degrades to `None` rather than failing
/// the whole RPC, since this is read-only display data. PR-1: additionally
/// carries the embedded first-party manifest as `declared_manifest` for
/// component bundles — see `ComponentReleaseDetail`.
async fn plugin_release_detail(
    cp: &ControlPlane,
    plugin_id: &str,
) -> anyhow::Result<ComponentReleaseDetail> {
    let releases = cp.store().list_component_releases(plugin_id).await?;
    let active_version = cp
        .store()
        .active_component_release(plugin_id)
        .await?
        .map(|r| r.version);
    let active_manifest = if active_version.is_some() {
        let root = crate::plugins::bundle::installed_bundle_root();
        crate::plugins::bundle::load_active_bundles(&root, cp.store())
            .await
            .ok()
            .and_then(|bundles| {
                bundles
                    .into_iter()
                    .find(|b| b.manifest.id == plugin_id)
                    .map(|b| ComponentManifestInfo::from(b.manifest))
            })
    } else {
        None
    };
    let active_manifest = match active_manifest {
        Some(mut manifest) => {
            enrich_oauth_profile_status(cp.store(), plugin_id, &mut manifest).await;
            Some(manifest)
        }
        None => None,
    };
    let declared_manifest = match crate::plugins::component_catalog::declared_manifest(plugin_id)
        .map(ComponentManifestInfo::from)
    {
        Some(mut manifest) => {
            // Same store enrichment the active manifest gets: a token or
            // client-id override stored from a previous install survives
            // uninstall, so the pre-install preview reflects it too.
            enrich_oauth_profile_status(cp.store(), plugin_id, &mut manifest).await;
            Some(manifest)
        }
        None => None,
    };
    Ok(ComponentReleaseDetail {
        plugin_id: plugin_id.to_string(),
        releases: releases
            .into_iter()
            .map(ComponentReleaseInfo::from)
            .collect(),
        active_version,
        active_manifest,
        declared_manifest,
    })
}

/// Enrich each OAuth profile with live connection status from the store: whether
/// a usable token is stored (`connected`) and whether a client id resolves
/// (`client_id_configured` — manifest baked-in OR a stored per-install
/// override). The pure manifest `From` cannot see the store, so it leaves
/// `connected=false` and `client_id_configured` = the manifest's baked-in value
/// only; this ORs in the stored-override and token facts.
async fn enrich_oauth_profile_status(
    store: &Store,
    plugin_id: &str,
    manifest: &mut ComponentManifestInfo,
) {
    for profile in &mut manifest.oauth_profiles {
        profile.connected = store
            .get_plugin_oauth_profile_token(plugin_id, &profile.id)
            .await
            .ok()
            .flatten()
            .is_some_and(|tok| !tok.reconnect_required);
        if !profile.client_id_configured {
            profile.client_id_configured = store
                .get_plugin_oauth_profile_client(plugin_id, &profile.id)
                .await
                .ok()
                .flatten()
                .and_then(|c| c.client_id)
                .is_some_and(|id| !id.is_empty());
        }
        if !profile.client_id_configured {
            // A client-id-setting with a stored non-empty value also counts
            // (PR-3: user-supplied client id until first-party ids are baked).
            let setting_key = crate::plugins::component_catalog::declared_manifest(plugin_id)
                .and_then(|bundle| {
                    bundle
                        .oauth
                        .into_iter()
                        .find(|p| p.id == profile.id)
                        .and_then(|p| p.client_id_setting)
                });
            if let Some(key) = setting_key {
                profile.client_id_configured = store
                    .get_setting_raw(&key)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|v| !v.is_empty());
            }
        }
    }
}

/// Install (or update to) a component plugin's signed release via the Task 11a
/// pipeline (resolve+download+stage+verify_bundle+install+activate), then mark
/// the host restart-required so the newly activated bundle is picked up.
/// Returns the release ledger after the install. Fail-closed: on a build with
/// no trusted first-party signing key — e.g. a zeroed fork or a dev build
/// that hasn't been given the real key — this refuses before any network I/O
/// rather than staging an unverifiable bundle.
async fn install_component_plugin(
    cp: &ControlPlane,
    plugin_id: &str,
    version: Option<&str>,
) -> anyhow::Result<ComponentReleaseDetail> {
    let store = cp.store();
    let trusted_keys = crate::plugins::first_party_key::first_party_trusted_keys();
    if trusted_keys.is_empty() {
        anyhow::bail!(
            "component plugin installs are disabled until the first-party signing key is configured"
        );
    }
    let base_url = store
        .get_setting_raw("component_release_base_url")
        .await?
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            // Unversioned installs pin to this build's own release tag; an
            // explicit version (the catalog-feed update flow) resolves its
            // pinned stem against `latest`, where newer releases live.
            crate::plugins::remote_catalog::default_component_release_base_url_for(version)
        });
    let http = crate::plugins::remote_catalog::ReqwestCatalogHttp::new();
    let installer = crate::plugins::bundle::ComponentBundleInstaller::new(
        crate::plugins::bundle::installed_bundle_root(),
        store.as_ref().clone(),
    );
    crate::plugins::remote_catalog::install_component_release(
        &http,
        &installer,
        &trusted_keys,
        &base_url,
        plugin_id,
        version,
    )
    .await?;
    // Spec B1 granular latch: only a GATEWAY bundle still needs a process
    // restart (the Router's gateway map is immutable). Provider bundles are
    // hot-reloaded just below; connector bundles re-discover per session. An
    // id the host doesn't know latches conservatively. A component GATEWAY's
    // host row is the manifest-only catalog stand-in on a first-time install
    // — `derive_kind` reports "integration" for it, not "gateway" — so the
    // "integration" arm must fall through to the bundle capability check
    // rather than assume "not a gateway" (see `installed_bundle_is_gateway`).
    match cp.plugins().get(plugin_id).as_deref().and_then(derive_kind) {
        Some("provider") => {}
        Some("integration") => {
            if installed_bundle_is_gateway(
                cp.store(),
                &crate::plugins::bundle::installed_bundle_root(),
                plugin_id,
            )
            .await
            {
                cp.mark_plugins_restart_required();
            }
        }
        _ => cp.mark_plugins_restart_required(),
    }
    // Fail-closed: drop this plugin's live transports BEFORE re-discovery, so
    // a version whose compile fails leaves NO transport (router falls back to
    // the native/generic path) instead of silently keeping the previous —
    // for rollback, the just-revoked — version serving traffic.
    crate::plugins::wasm_provider::unregister_wasm_providers_for_plugin(plugin_id);
    hot_reload_provider_transports(cp).await;
    cp.emit(CoreEvent::PluginsChanged);
    plugin_release_detail(cp, plugin_id).await
}

/// Roll a component plugin off a bad release: re-point the active release to the
/// prior-good `to_version`, revoke + deactivate `from_version`, and mark the
/// host restart-required so the rolled-back bundle is loaded fresh on the next
/// session/boot (the same reload signal `uninstall_plugin` uses).
///
/// ORDER MATTERS: `set_active_component_release` runs first. It validates — in
/// one transaction, before mutating anything — that `to_version` exists and is
/// not revoked, so a missing/revoked target is a clean no-op that leaves
/// `from_version` still active. Only once the good version is active do we
/// revoke the bad one, so a failed reactivation can NEVER strand the plugin with
/// no active release (the non-atomic revoke-first ordering could).
async fn rollback_component_plugin(
    cp: &ControlPlane,
    plugin_id: &str,
    from_version: &str,
    to_version: &str,
) -> anyhow::Result<ComponentReleaseDetail> {
    if from_version == to_version {
        anyhow::bail!(
            "cannot roll back {plugin_id} to the same version being revoked ({from_version})"
        );
    }
    let store = cp.store();
    store
        .set_active_component_release(plugin_id, to_version)
        .await?;
    store
        .mark_component_release_revoked(
            plugin_id,
            from_version,
            &format!("rolled back to {to_version}"),
        )
        .await?;
    // Spec B1 granular latch: same rule as `install_component_plugin` — only
    // a gateway (or an id the host doesn't know) still needs a process
    // restart; a provider hot-swaps below, a connector re-discovers per
    // session. Same "integration" caveat as `install_component_plugin`: a
    // component gateway's host row still reads as "integration" via the
    // catalog stand-in, so that arm defers to the bundle capability check.
    match cp.plugins().get(plugin_id).as_deref().and_then(derive_kind) {
        Some("provider") => {}
        Some("integration") => {
            if installed_bundle_is_gateway(
                cp.store(),
                &crate::plugins::bundle::installed_bundle_root(),
                plugin_id,
            )
            .await
            {
                cp.mark_plugins_restart_required();
            }
        }
        _ => cp.mark_plugins_restart_required(),
    }
    // If the rolled-back plugin is a currently-running gateway, stop its
    // supervisor MID-SESSION at a safe boundary — the restart flag above only
    // reloads the (now good) bundle on the NEXT boot, so the revoked one would
    // otherwise keep running until then. Keyed by plugin id; a no-op for a
    // connector/provider or a gateway not presently supervised.
    cp.stop_revoked_running_gateways(&std::iter::once(plugin_id.to_string()).collect())
        .await;
    // Fail-closed: drop this plugin's live transports BEFORE re-discovery, so
    // a version whose compile fails leaves NO transport (router falls back to
    // the native/generic path) instead of silently keeping the previous —
    // for rollback, the just-revoked — version serving traffic.
    crate::plugins::wasm_provider::unregister_wasm_providers_for_plugin(plugin_id);
    // A rolled-back ENABLED provider bundle hot-swaps to the restored release
    // (discovery compiles the now-active version; replace semantics).
    hot_reload_provider_transports(cp).await;
    cp.emit(CoreEvent::PluginsChanged);
    plugin_release_detail(cp, plugin_id).await
}

/// The first-party component bootstrap's retryable status: pending when the
/// last bootstrap attempt landed nothing AND bootstrap has not since completed,
/// so Cockpit (Task 12) can surface a retry banner.
async fn component_bootstrap_status(cp: &ControlPlane) -> anyhow::Result<ComponentBootstrapStatus> {
    let store = cp.store();
    let message = store
        .get_setting_raw(crate::plugins::remote_catalog::FIRST_PARTY_BOOTSTRAP_RETRY)
        .await?
        .filter(|m| !m.is_empty());
    let completed = store
        .get_setting_raw(crate::plugins::remote_catalog::FIRST_PARTY_BOOTSTRAP_MARKER)
        .await?
        .is_some();
    let pending = message.is_some() && !completed;
    Ok(ComponentBootstrapStatus {
        message: pending.then(|| message.clone().unwrap_or_default()),
        pending,
    })
}

// ---------------------------------------------------------------------------
// Thin, profile-aware wrappers over the Phase-3 OAuth profile engine
// (`plugins::capabilities::oauth::ProfileOauth`). No new OAuth engine logic —
// each handler just builds the plugin's capability context from its installed
// bundle (so the network allowlist and declared profile set come from the
// signed manifest, never the caller) and dispatches one method. Deliberately
// minimal: mimo/opencode don't use OAuth (that lands with Task 13/GitHub), and
// `authorized_request` is a component-runtime-facing HTTP proxy, not a Cockpit
// surface, so it is intentionally NOT exposed here.
// ---------------------------------------------------------------------------

/// Map a capability-adapter `OauthErr` to an `ApiError` status.
fn oauth_err(err: crate::plugins::capabilities::oauth::OauthErr) -> ApiError {
    use crate::plugins::capabilities::oauth::OauthErr;
    match err {
        OauthErr::InvalidRequest(message) => ApiError::bad_request(message),
        OauthErr::Denied => ApiError {
            status: 403,
            message: "oauth profile denied".to_string(),
        },
        OauthErr::Expired => ApiError::conflict("oauth token expired"),
        OauthErr::Failed(message) => ApiError {
            status: 502,
            message,
        },
    }
}

/// Load the active installed bundle for `plugin_id` and build its capability
/// context (+ return the manifest). The context's network allowlist and OAuth
/// profile ids come from the signed bundle manifest, so a component can never
/// widen its own permissions through these RPCs. Telemetry is a no-op here (the
/// wrapped `ProfileOauth` methods don't emit).
async fn profile_capability_context(
    cp: &ControlPlane,
    plugin_id: &str,
) -> Result<
    (
        crate::plugins::capabilities::PluginCapabilityContext,
        ryuzi_plugin_sdk::PluginManifest,
    ),
    ApiError,
> {
    let root = crate::plugins::bundle::installed_bundle_root();
    let bundles = crate::plugins::bundle::load_active_bundles(&root, cp.store())
        .await
        .map_err(ApiError::from)?;
    let bundle = bundles
        .into_iter()
        .find(|b| b.manifest.id == plugin_id)
        .ok_or_else(|| {
            ApiError::not_found(format!("no active component bundle for {plugin_id}"))
        })?;
    let manifest = bundle.manifest.clone();
    let ctx = crate::plugins::capabilities::PluginCapabilityContext {
        plugin_id: manifest.id.clone(),
        version: manifest.version.clone(),
        settings: SettingsStore::new(cp.store().clone()),
        store: cp.store().clone(),
        telemetry: std::sync::Arc::new(crate::telemetry::NoopTelemetry),
        network_allowlist: manifest
            .permissions
            .network
            .iter()
            .map(|entry| entry.0.clone())
            .collect(),
        oauth_profile_ids: manifest.oauth.iter().map(|p| p.id.clone()).collect(),
        provider_ids: manifest.resolved_provider_ids(),
    };
    Ok((ctx, manifest))
}

fn find_oauth_profile(
    manifest: &ryuzi_plugin_sdk::PluginManifest,
    profile_id: &str,
) -> Result<ryuzi_plugin_sdk::OAuthProfile, ApiError> {
    manifest
        .oauth
        .iter()
        .find(|p| p.id == profile_id)
        .cloned()
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "plugin does not declare oauth profile {profile_id:?}"
            ))
        })
}

fn device_poll_outcome_label(
    outcome: crate::plugins::capabilities::oauth::DevicePollOutcome,
) -> &'static str {
    use crate::plugins::capabilities::oauth::DevicePollOutcome;
    match outcome {
        DevicePollOutcome::Pending => "pending",
        DevicePollOutcome::SlowDown => "slow-down",
        DevicePollOutcome::Ready => "ready",
        DevicePollOutcome::Expired => "expired",
        DevicePollOutcome::Denied => "denied",
    }
}

async fn plugin_profile_begin_pkce(
    cp: &ControlPlane,
    plugin_id: &str,
    profile_id: &str,
    redirect_uri: &str,
) -> Result<PluginProfilePkceStart, ApiError> {
    let (ctx, manifest) = profile_capability_context(cp, plugin_id).await?;
    let profile = find_oauth_profile(&manifest, profile_id)?;
    let start = crate::plugins::capabilities::oauth::ProfileOauth::new(&ctx)
        .begin_pkce(&profile, redirect_uri)
        .await
        .map_err(oauth_err)?;
    Ok(start.into())
}

/// Completes a PKCE authorization-code exchange begun by
/// `plugin_profile_begin_pkce`. Cockpit's loopback callback is the only
/// caller of this RPC. On success the new token is live immediately, so this
/// emits `CoreEvent::PluginsChanged` (mirroring `install_component_plugin`)
/// so Cockpit refreshes the profile's connected status.
async fn plugin_profile_complete_pkce(
    cp: &ControlPlane,
    plugin_id: &str,
    profile_id: &str,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) -> Result<(), ApiError> {
    let (ctx, manifest) = profile_capability_context(cp, plugin_id).await?;
    let profile = find_oauth_profile(&manifest, profile_id)?;
    crate::plugins::capabilities::oauth::ProfileOauth::new(&ctx)
        .complete_pkce(&profile, redirect_uri, code, verifier)
        .await
        .map_err(oauth_err)?;
    cp.emit(CoreEvent::PluginsChanged);
    Ok(())
}

async fn plugin_profile_disconnect(
    cp: &ControlPlane,
    plugin_id: &str,
    profile_id: &str,
) -> Result<(), ApiError> {
    let (ctx, _manifest) = profile_capability_context(cp, plugin_id).await?;
    crate::plugins::capabilities::oauth::ProfileOauth::new(&ctx)
        .disconnect_profile(profile_id)
        .await
        .map_err(oauth_err)
}

async fn plugin_profile_begin_device_flow(
    cp: &ControlPlane,
    plugin_id: &str,
    profile_id: &str,
    device_authorization_url: &str,
) -> Result<PluginProfileDeviceFlowStart, ApiError> {
    let (ctx, manifest) = profile_capability_context(cp, plugin_id).await?;
    let profile = find_oauth_profile(&manifest, profile_id)?;
    let start = crate::plugins::capabilities::oauth::ProfileOauth::new(&ctx)
        .begin_device_flow(&profile, device_authorization_url)
        .await
        .map_err(oauth_err)?;
    Ok(start.into())
}

async fn plugin_profile_poll_device_flow(
    cp: &ControlPlane,
    plugin_id: &str,
    profile_id: &str,
    token_url: &str,
    device_code: &str,
    expires_at: i64,
) -> Result<String, ApiError> {
    let (ctx, manifest) = profile_capability_context(cp, plugin_id).await?;
    let profile = find_oauth_profile(&manifest, profile_id)?;
    let outcome = crate::plugins::capabilities::oauth::ProfileOauth::new(&ctx)
        .poll_device_flow(&profile, token_url, device_code, expires_at)
        .await
        .map_err(oauth_err)?;
    Ok(device_poll_outcome_label(outcome).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{dispatch, tests_support::state};
    use crate::connector::{Connector, ConnectorCtx};
    use crate::domain::McpServerSpec;
    use crate::gateway::{Gateway, GatewayFactory};
    use crate::harness::{Harness, HarnessFactory, HarnessSession, SessionCtx};
    use crate::Registries;
    use ryuzi_plugin_sdk::{
        AuthSpec, ComponentSpec, ModelDef, PluginLifecycle, PluginManifest, ProviderSpec,
    };
    use serde_json::json;
    use serial_test::serial;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // ---- minimal fakes, self-contained to this test module ----

    struct FakeHarness;
    #[async_trait::async_trait]
    impl Harness for FakeHarness {
        async fn start_session(&self, _ctx: SessionCtx) -> anyhow::Result<Box<dyn HarnessSession>> {
            anyhow::bail!("not needed in this test")
        }
    }
    struct FakeHarnessFactory;
    impl HarnessFactory for FakeHarnessFactory {
        fn create(&self) -> anyhow::Result<Arc<dyn Harness>> {
            Ok(Arc::new(FakeHarness))
        }
    }

    struct FakeGateway;
    #[async_trait::async_trait]
    impl Gateway for FakeGateway {
        fn id(&self) -> &str {
            "fake"
        }
        async fn start(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn stop(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn create_workspace(&self, name: &str) -> anyhow::Result<String> {
            Ok(format!("ws-{name}"))
        }
        async fn create_conversation(
            &self,
            _workspace_id: &str,
            _title: &str,
        ) -> anyhow::Result<String> {
            Ok("conv".to_string())
        }
        async fn post_status(
            &self,
            surface: &crate::domain::Surface,
            _text: &str,
        ) -> anyhow::Result<crate::gateway::MessageRef> {
            Ok(crate::gateway::MessageRef {
                surface: surface.clone(),
                message_id: "m1".to_string(),
            })
        }
        async fn edit_status(
            &self,
            _msg: &crate::gateway::MessageRef,
            _text: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn post_result(
            &self,
            _surface: &crate::domain::Surface,
            _chunks: &[String],
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn post_error(
            &self,
            _surface: &crate::domain::Surface,
            _message: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn request_approval(
            &self,
            _s: &crate::domain::Surface,
            _r: &crate::domain::ApprovalRequest,
        ) -> anyhow::Result<crate::domain::ApprovalDecision> {
            Ok(crate::domain::ApprovalDecision::Cancel)
        }
    }
    struct FakeGatewayFactory;
    impl GatewayFactory for FakeGatewayFactory {
        fn create(&self, _c: &serde_json::Value) -> anyhow::Result<Arc<dyn Gateway>> {
            Ok(Arc::new(FakeGateway))
        }
    }

    struct FakeConnector;
    #[async_trait::async_trait]
    impl Connector for FakeConnector {
        async fn mcp_servers(&self, _ctx: &ConnectorCtx) -> anyhow::Result<Vec<McpServerSpec>> {
            Ok(vec![])
        }
    }

    fn manifest(id: &str) -> PluginManifest {
        PluginManifest {
            contract: ryuzi_plugin_sdk::CONTRACT_VERSION,
            id: id.to_string(),
            name: format!("Plugin {id}"),
            version: String::new(),
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
            component: None,
            permissions: Default::default(),
            oauth: vec![],
            provider: None,
            tools: vec![],
            mcp: vec![],
            hooks: vec![],
            jobs: vec![],
            gateway: false,
        }
    }

    fn harness_only(id: &str) -> CorePlugin {
        CorePlugin {
            manifest: manifest(id),
            harness: Some(Arc::new(FakeHarnessFactory)),
            gateway: None,
            connector: None,
            provider: None,
            source: PluginSource::Builtin,
        }
    }

    fn gateway_only(id: &str) -> CorePlugin {
        CorePlugin {
            manifest: PluginManifest {
                component: Some(ComponentSpec {
                    file: format!("{id}.wasm"),
                    wit_api: "^0.1.0".to_string(),
                    lifecycle: PluginLifecycle::Singleton,
                }),
                ..manifest(id)
            },
            harness: None,
            gateway: Some(Arc::new(FakeGatewayFactory)),
            connector: None,
            provider: None,
            // Real component-catalog plugins are `PluginSource::Builtin`-
            // registered too (see `component_catalog.rs`) — "component-ness"
            // is a manifest property (`manifest.component.is_some()`) now,
            // not a `PluginSource` variant.
            source: PluginSource::Builtin,
        }
    }

    fn connector_only(id: &str) -> CorePlugin {
        CorePlugin {
            manifest: manifest(id),
            harness: None,
            gateway: None,
            connector: Some(Arc::new(FakeConnector)),
            provider: None,
            source: PluginSource::Installed {
                dir: std::path::PathBuf::from("/tmp/whatever"),
                provenance: InstallProvenance::LocalPath,
            },
        }
    }

    fn provider_only(id: &str) -> CorePlugin {
        CorePlugin {
            manifest: PluginManifest {
                provider: Some(ProviderSpec {
                    ids: vec![],
                    format: Some("openai".to_string()),
                    base_url: None,
                    models: vec![ModelDef {
                        id: "m1".to_string(),
                        label: None,
                        default: true,
                    }],
                }),
                ..manifest(id)
            },
            harness: None,
            gateway: None,
            connector: None,
            provider: None,
            source: PluginSource::Builtin,
        }
    }

    // ---------- capabilities ----------

    #[test]
    fn capabilities_provider_from_manifest() {
        assert_eq!(provider_only("p").capabilities(), vec!["provider"]);
    }

    #[test]
    fn capabilities_runtime_from_live_harness() {
        assert_eq!(harness_only("h").capabilities(), vec!["runtime"]);
    }

    #[test]
    fn capabilities_gateway_from_live_gateway() {
        assert_eq!(gateway_only("g").capabilities(), vec!["gateway"]);
    }

    #[test]
    fn capabilities_connector_from_live_connector() {
        assert_eq!(connector_only("c").capabilities(), vec!["connector"]);
    }

    #[test]
    fn capabilities_empty_for_manifest_only_plugin() {
        assert!(CorePlugin {
            manifest: manifest("m"),
            harness: None,
            gateway: None,
            connector: None,
            provider: None,
            source: PluginSource::Builtin,
        }
        .capabilities()
        .is_empty());
    }

    // ---------- source_label ----------

    #[test]
    fn source_label_maps_every_variant() {
        assert_eq!(source_label(&PluginSource::Builtin), "builtin");
        assert_eq!(
            source_label(&PluginSource::Installed {
                dir: std::path::PathBuf::from("/x"),
                provenance: InstallProvenance::Catalog,
            }),
            "catalog"
        );
        assert_eq!(
            source_label(&PluginSource::Installed {
                dir: std::path::PathBuf::from("/x"),
                provenance: InstallProvenance::LocalPath,
            }),
            "local-path"
        );
        assert_eq!(
            source_label(&PluginSource::Installed {
                dir: std::path::PathBuf::from("/x"),
                provenance: InstallProvenance::GitUrl("https://example.com/repo.git".to_string()),
            }),
            "git-url"
        );
    }

    // ---------- derive_kind ----------

    #[test]
    fn derive_kind_classifies_each_capability_shape() {
        assert_eq!(derive_kind(&provider_only("anthropic")), Some("provider"));
        assert_eq!(derive_kind(&gateway_only("discord")), Some("gateway"));
        assert_eq!(derive_kind(&connector_only("slack")), Some("integration"));
        assert_eq!(derive_kind(&harness_only("native")), None);
    }

    // ---------- installed_flag ----------

    #[test]
    fn installed_flag_per_kind() {
        // (kind, enabled, configured, provider_installed,
        //  gateway_settings_complete, skill_pack_installed, component_active)
        let f = |k, e, c, p, g, s, ca| installed_flag(k, e, c, p, g, s, ca);
        assert!(f("integration", true, false, false, false, false, false));
        assert!(f("integration", false, true, false, false, false, false));
        assert!(!f("integration", false, false, true, true, true, false));
        assert!(f("provider", false, false, true, false, false, false));
        assert!(!f("provider", true, true, false, false, false, false));
        assert!(f("gateway", false, false, false, true, false, false));
        assert!(!f("gateway", true, false, false, false, false, false));
        assert!(f("skill-pack", false, false, false, false, true, false));
        assert!(!f("skill-pack", true, true, false, false, false, false));
    }

    #[test]
    fn installed_flag_counts_an_active_component_release_for_integration_and_gateway() {
        let f = |k| installed_flag(k, false, false, false, false, false, true);
        // The discord shape: integration stand-in, auth none (never
        // configured), disabled — the active bundle alone means installed.
        assert!(f("integration"));
        assert!(f("gateway"));
        // Providers stay authoritative on the installed set; skill packs on
        // the skills ledger — an active bundle row changes neither.
        assert!(!f("provider"));
        assert!(!f("skill-pack"));
    }

    // ---------- derive_plugin_status ----------

    #[test]
    fn derive_plugin_status_priority_order() {
        // (installed, enabled, blocked, auth_kind, configured, attach_failed, update_available)
        let s =
            |i, e, b, a: &str, c, af: Option<&str>, u| derive_plugin_status(i, e, b, a, c, af, u);
        assert_eq!(
            s(false, false, true, "none", false, None, false).0,
            "blocked"
        );
        assert_eq!(
            s(false, false, false, "none", false, None, false).0,
            "not-installed"
        );
        assert_eq!(
            s(true, false, false, "none", true, None, false).0,
            "disabled"
        );
        let (st, detail) = s(
            true,
            true,
            false,
            "oauth",
            true,
            Some("token rejected"),
            false,
        );
        assert_eq!(st, "attach-failed");
        assert_eq!(detail.as_deref(), Some("token rejected"));
        assert_eq!(
            s(true, true, false, "oauth", false, None, false).0,
            "needs-setup"
        );
        assert_eq!(
            s(true, true, false, "none", true, None, true).0,
            "update-available"
        );
        assert_eq!(s(true, true, false, "token", true, None, false).0, "ok");
    }

    #[test]
    fn provider_family_falls_back_to_id() {
        assert_eq!(provider_family("anthropic-oauth"), "anthropic");
        assert_eq!(provider_family("not-a-provider"), "not-a-provider");
    }

    // ---------- compute_installed: settings-less gateway ----------

    #[tokio::test]
    async fn compute_installed_gateway_without_settings_follows_enabled() {
        // A gateway with no manifest settings has nothing to configure, so its
        // installed-ness must track `enabled` — otherwise it could never leave
        // Browse. `gateway_only` builds a manifest with empty `settings`.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(crate::Store::open(tmp.path()).await.unwrap());
        let plugin = gateway_only("bare-gateway");
        let ctx = InstalledCtx {
            installed_skills: vec![],
            installed_providers: vec![],
        };

        let installed_when_enabled =
            compute_installed(&store, &plugin, "gateway", true, false, &ctx, false)
                .await
                .unwrap();
        assert!(
            installed_when_enabled,
            "enabled settings-less gateway is installed"
        );

        let installed_when_disabled =
            compute_installed(&store, &plugin, "gateway", false, false, &ctx, false)
                .await
                .unwrap();
        assert!(
            !installed_when_disabled,
            "disabled settings-less gateway is not installed"
        );
    }

    #[tokio::test]
    async fn compute_installed_provider_follows_installed_set_without_connection() {
        // A default-installed provider with zero connections is "installed"
        // because it is in the persisted set; a provider that is neither seeded
        // nor connected is not.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(crate::Store::open(tmp.path()).await.unwrap());
        crate::llm_router::installed::ensure_default_installed_providers(&store)
            .await
            .unwrap();
        let ctx = installed_ctx(&store).await.unwrap();

        // `mimo-free` is in DEFAULT_INSTALLED; no connection exists.
        let mimo_free = provider_only("mimo-free");
        assert!(
            compute_installed(&store, &mimo_free, "provider", false, false, &ctx, false)
                .await
                .unwrap(),
            "a default-installed provider is installed with zero connections"
        );

        // `xai` is not a default and has no connection.
        let xai = provider_only("xai");
        assert!(
            !compute_installed(&store, &xai, "provider", false, false, &ctx, false)
                .await
                .unwrap(),
            "a non-installed, connectionless provider is not installed"
        );
    }

    #[tokio::test]
    async fn compute_installed_provider_is_set_authoritative_ignoring_connections() {
        // Provider installed-ness is authoritative on the persisted set ONLY,
        // matching the Models list (which filters on the set). A family in the
        // set is installed; a family absent from the set is NOT installed even
        // when a connection row for it exists — so the Plugins card and the
        // Models list can never disagree.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(crate::Store::open(tmp.path()).await.unwrap());

        // A live connection for `xai`, but `xai` is deliberately NOT in the set.
        crate::llm_router::connections::add_connection(
            &store,
            crate::llm_router::connections::ConnectionRow {
                id: "x1".into(),
                provider: "xai".into(),
                auth_type: "api_key".into(),
                label: "xAI".into(),
                priority: 0,
                enabled: true,
                data: Default::default(),
                created_at: 0,
                updated_at: 0,
            },
        )
        .await
        .unwrap();
        // `openai` is installed via the set alone (no connection needed).
        crate::llm_router::installed::install_provider(&store, "openai")
            .await
            .unwrap();

        let ctx = installed_ctx(&store).await.unwrap();

        let openai = provider_only("openai");
        assert!(
            compute_installed(&store, &openai, "provider", false, false, &ctx, false)
                .await
                .unwrap(),
            "a family in the installed set is installed"
        );

        let xai = provider_only("xai");
        assert!(
            !compute_installed(&store, &xai, "provider", false, false, &ctx, false)
                .await
                .unwrap(),
            "a family with a connection but absent from the set is NOT installed"
        );
    }

    // ---------- compute_installed: active component release ----------

    /// Seeds an installed, active `component_plugin_releases` row for `id` —
    /// the ledger state a successful `install_component_plugin` leaves behind.
    async fn seed_active_component_release(cp: &ControlPlane, id: &str) {
        cp.store()
            .upsert_component_release(&crate::store::ComponentPluginReleaseRecord {
                plugin_id: id.into(),
                version: "0.1.0".into(),
                source_url: format!("https://example.com/{id}.wasm"),
                sha256: "aa".into(),
                signing_key_id: "first-party".into(),
                installed_at: 1,
                active: false,
                revoked: false,
                revocation_reason: None,
            })
            .await
            .unwrap();
        cp.store()
            .set_active_component_release(id, "0.1.0")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn component_integration_with_active_release_and_no_setting_reports_installed_enabled() {
        // PR-2 fix A (Task 1): the install itself succeeded (active release
        // in the ledger, bundle on disk) and `plugin.discord.enabled` was
        // never written. `PluginHost::is_enabled` now treats an installed
        // component with no explicit setting as enabled, so the row reads
        // installed AND enabled.
        //
        // Before this fix, `is_enabled` defaulted an unset component to
        // `false`, and this same scenario asserted `status == "disabled"` —
        // that was the "install succeeded but the final enable write didn't"
        // wedge this task closes.
        //
        // Since Task 10, discord's bundle DOES declare a secret+required
        // `token` setting, from which the bridge derives `AuthKind::Token` —
        // so this scenario also needs the token configured to isolate the
        // enabled-by-default behavior under test from the (separately
        // covered, see `component_needs_setup_when_declared_auth_setting_is_unconfigured`)
        // needs-setup gate.
        let cp = test_cp().await;
        seed_active_component_release(&cp, "discord").await;
        cp.store()
            .set_setting_raw("plugin.discord.token", "test-token")
            .await
            .unwrap();

        let list = assemble_list(&cp).await.unwrap();
        let row = list
            .iter()
            .find(|p| p.id == "discord")
            .expect("discord catalog row");
        assert!(
            row.installed,
            "an active component release means the plugin IS installed"
        );
        assert!(
            row.enabled,
            "an active release with no plugin.<id>.enabled setting now defaults to enabled"
        );
        assert_eq!(row.status, "ok");

        let detail = assemble_detail(&cp, "discord").await.unwrap();
        assert!(detail.info.installed, "detail must agree with the list");
        assert!(detail.info.enabled, "detail must agree with the list");
        assert_eq!(detail.info.status, "ok");
    }

    // Task 10: discord's bundle now declares a secret+required `token`
    // setting, from which the bridge derives `AuthKind::Token` — an active,
    // enabled install with NO token configured must report "needs-setup",
    // the mirror image of the "ok" case above (which seeds the token).
    #[tokio::test]
    async fn component_needs_setup_when_declared_auth_setting_is_unconfigured() {
        let cp = test_cp().await;
        seed_active_component_release(&cp, "discord").await;

        let list = assemble_list(&cp).await.unwrap();
        let row = list
            .iter()
            .find(|p| p.id == "discord")
            .expect("discord catalog row");
        assert!(row.enabled, "an active release defaults to enabled");
        assert_eq!(
            row.status, "needs-setup",
            "discord's derived token auth has no configured value, so this must be needs-setup"
        );
    }

    // ---------- plugin_info ----------

    fn no_ctx(owns_slot: bool) -> PluginInfoContext<'static> {
        PluginInfoContext {
            install: None,
            remote: None,
            owns_slot,
            attach_failed: None,
            active_version: None,
            skill_count: None,
        }
    }

    #[test]
    fn plugin_info_maps_identity_and_enabled_flag_through() {
        let plugin = harness_only("native");
        let info = plugin_info(&plugin, true, false, "integration", false, no_ctx(false));
        assert_eq!(info.id, "native");
        assert_eq!(info.name, "Plugin native");
        assert!(info.enabled);
        assert_eq!(info.source, "builtin");
        assert_eq!(info.capabilities, vec!["runtime".to_string()]);
        assert!(!info.configured);
        assert_eq!(info.kind, "integration");
        assert!(!info.installed);
        assert!(info.family.is_none());
        // No `plugin_installs` ledger row → ledger fields carry their defaults.
        assert!(!info.pinned);
        assert!(info.source_spec.is_none());
        // Builtin source, no cached remote-catalog row → all three enrichment
        // fields stay unset.
        assert!(info.catalog_version.is_none());
        assert!(info.blocked_reason.is_none());
        // No manifest `slot` claim → neither field is set.
        assert!(info.slot.is_none());
        assert!(!info.owns_slot);

        let info_disabled = plugin_info(&plugin, false, false, "integration", false, no_ctx(false));
        assert!(!info_disabled.enabled);
    }

    #[test]
    fn plugin_info_reports_slot_and_owns_slot() {
        let plugin = CorePlugin {
            manifest: PluginManifest {
                slot: Some("memory".to_string()),
                ..manifest("mem0")
            },
            ..harness_only("mem0")
        };
        let owner = plugin_info(&plugin, true, false, "integration", false, no_ctx(true));
        assert_eq!(owner.slot.as_deref(), Some("memory"));
        assert!(owner.owns_slot);

        let loser = plugin_info(&plugin, true, false, "integration", false, no_ctx(false));
        assert_eq!(
            loser.slot.as_deref(),
            Some("memory"),
            "the claim itself is still reported even when the plugin lost arbitration"
        );
        assert!(!loser.owns_slot);
    }

    // ---------- status/statusDetail/authKind/counts (Task 3) ----------

    #[test]
    fn plugin_info_status_ok_for_enabled_configured_builtin() {
        let plugin = harness_only("native");
        let info = plugin_info(&plugin, true, true, "integration", true, no_ctx(false));
        assert_eq!(info.status, "ok");
        assert!(info.status_detail.is_none());
        assert_eq!(info.auth_kind, "none");
    }

    #[test]
    fn plugin_info_status_not_installed_for_not_installed_catalog_row() {
        // `gateway_only` declares `manifest.component` — a component-catalog
        // row not yet installed.
        let plugin = gateway_only("acme-catalog");
        let info = plugin_info(&plugin, false, false, "gateway", false, no_ctx(false));
        assert_eq!(info.status, "not-installed");
        assert!(info.status_detail.is_none());
    }

    #[test]
    fn plugin_info_status_attach_failed_carries_reason_through_context() {
        let plugin = connector_only("acme-connector");
        let ctx = PluginInfoContext {
            attach_failed: Some("token rejected"),
            ..no_ctx(false)
        };
        let info = plugin_info(&plugin, true, true, "integration", true, ctx);
        assert_eq!(info.status, "attach-failed");
        assert_eq!(info.status_detail.as_deref(), Some("token rejected"));
    }

    #[test]
    fn plugin_info_status_needs_setup_when_auth_required_and_unconfigured() {
        let plugin = auth_connector("acme-oauth", AuthKind::Oauth, None);
        let info = plugin_info(&plugin, true, false, "integration", true, no_ctx(false));
        assert_eq!(info.status, "needs-setup");
        assert_eq!(info.auth_kind, "oauth");
    }

    #[test]
    fn plugin_info_tool_count_reads_embedded_manifest_for_component_backed_rows() {
        let mut plugin = connector_only("github");
        // `component_backed`/`tool_count` are keyed off the real embedded
        // component catalog by id (`is_component_bundle("github")`), not
        // `plugin.source` — this just documents the fixture's intent.
        plugin.manifest.component = Some(ComponentSpec {
            file: "github.wasm".to_string(),
            wit_api: "^0.1.0".to_string(),
            lifecycle: PluginLifecycle::Singleton,
        });
        let info = plugin_info(&plugin, true, true, "integration", true, no_ctx(false));
        assert!(info.component_backed);
        assert_eq!(info.tool_count, Some(12));

        // Non-component-backed rows never get a tool count.
        let native = harness_only("native");
        let native_info = plugin_info(&native, true, true, "integration", true, no_ctx(false));
        assert!(!native_info.component_backed);
        assert!(native_info.tool_count.is_none());
    }

    #[test]
    fn plugin_info_skill_count_only_applies_to_skill_pack_kind() {
        // `kind` is passed explicitly below — `plugin.source` plays no part.
        let plugin = connector_only("acme-pack");
        let ctx = PluginInfoContext {
            skill_count: Some(7),
            ..no_ctx(false)
        };
        let info = plugin_info(&plugin, false, false, "skill-pack", true, ctx);
        assert_eq!(info.skill_count, Some(7));

        // Same `skill_count` supplied, but a non-skill-pack kind ignores it.
        let ctx2 = PluginInfoContext {
            skill_count: Some(7),
            ..no_ctx(false)
        };
        let info2 = plugin_info(&plugin, false, false, "integration", true, ctx2);
        assert!(info2.skill_count.is_none());
    }

    #[test]
    fn plugin_info_update_available_only_for_component_backed_rows_with_newer_catalog_version() {
        let mut plugin = connector_only("github");
        plugin.manifest.component = Some(ComponentSpec {
            file: "github.wasm".to_string(),
            wit_api: "^0.1.0".to_string(),
            lifecycle: PluginLifecycle::Singleton,
        });
        let remote = RemoteCatalogRow {
            id: "github".to_string(),
            manifest_toml: String::new(),
            version: "2.0.0".to_string(),
            sequence: 1,
            blocked: false,
            blocked_reason: None,
            fetched_at: 0,
        };
        let ctx = PluginInfoContext {
            remote: Some(&remote),
            active_version: Some("1.0.0"),
            ..no_ctx(false)
        };
        let info = plugin_info(&plugin, true, true, "integration", true, ctx);
        assert_eq!(info.status, "update-available");

        // Same catalog version as active — no update available, status is ok.
        let ctx_same = PluginInfoContext {
            remote: Some(&remote),
            active_version: Some("2.0.0"),
            ..no_ctx(false)
        };
        let info_same = plugin_info(&plugin, true, true, "integration", true, ctx_same);
        assert_eq!(info_same.status, "ok");
    }

    // ---------- remote-catalog enrichment (assemble_list) ----------

    // A plugin whose `CorePlugin.source` is `RemoteCatalog` (the merged-catalog
    // path a real daemon boot takes via `catalog::merged_catalog_plugins`) must
    // report `catalogSource: "remote"`, and a matching (blocked) cached row
    // must surface as `catalogVersion`/`blockedReason` on the SAME list entry —
    // exercising the `remote_catalog_index` lookup `assemble_list` builds once,
    // not a per-plugin query.
    #[tokio::test]
    async fn assemble_list_enriches_remote_catalog_plugin_with_blocked_reason() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(crate::Store::open(tmp.path()).await.unwrap());
        let mut regs = Registries::new();
        let plugin = gateway_only("acme-remote");
        regs.add_plugin(plugin);
        let cp = {
            let persistence = crate::agents::bootstrap::AgentPersistence::temporary(store.clone())
                .await
                .unwrap();
            ControlPlane::new(store, regs, persistence).await
        };

        cp.store()
            .upsert_remote_catalog(&[crate::store::RemoteCatalogRow {
                id: "acme-remote".to_string(),
                manifest_toml: String::new(),
                version: "2.0.0".to_string(),
                sequence: 1,
                blocked: true,
                blocked_reason: Some("revoked: CVE-2026-0001".to_string()),
                fetched_at: 0,
            }])
            .await
            .unwrap();

        let list = assemble_list(&cp).await.unwrap();
        let info = list
            .iter()
            .find(|p| p.id == "acme-remote")
            .expect("acme-remote present in the list");
        assert_eq!(info.catalog_version.as_deref(), Some("2.0.0"));
        assert_eq!(
            info.blocked_reason.as_deref(),
            Some("revoked: CVE-2026-0001")
        );
    }

    // A real failed-attach row in `plugin_attach_status` for an
    // installed+enabled component-backed plugin must surface as
    // `attach-failed` with the recorded reason on the SAME list entry —
    // exercising the `attach_failed_index` lookup `assemble_list` builds
    // once, not a hand-built `PluginInfoContext`. `github` is `test_cp`'s
    // first-party component bundle (`PluginSource::Component`, no auth), so
    // flipping its `plugin.github.enabled` setting is enough to make it
    // installed+enabled without any other ledger row.
    #[tokio::test]
    async fn assemble_list_marks_attach_failed_from_store_attach_status() {
        let cp = test_cp().await;
        let settings = SettingsStore::new(cp.store().clone());
        settings.set("plugin.github.enabled", "true").await.unwrap();

        cp.store()
            .record_plugin_attach(&crate::store::PluginAttachStatus {
                plugin_id: "github".to_string(),
                last_attach_at: crate::paths::now_ms(),
                outcome: "failed".to_string(),
                reason: Some("token rejected".to_string()),
            })
            .await
            .unwrap();

        let list = assemble_list(&cp).await.unwrap();
        let github = list
            .iter()
            .find(|p| p.id == "github")
            .expect("github present in the list");
        assert_eq!(github.status, "attach-failed");
        assert_eq!(github.status_detail.as_deref(), Some("token rejected"));
    }

    // A component-backed plugin whose cached remote-catalog version differs
    // from the ACTIVE row in `component_plugin_releases` must report
    // `update-available` on the SAME list entry — exercising the real
    // `active_release_version_index` lookup (not a hand-built
    // `PluginInfoContext.active_version`). Once the active release is
    // reactivated to match the catalog version, the same plugin must fall
    // back to `ok`.
    //
    // PR-3: the bridge now derives `authKind: "oauth"` for github (it
    // declares `[[oauth]] id = "github"`), so an unconnected github would
    // read `needs-setup` and mask the update-available/ok assertions this
    // test is actually about — seed a live token for its one declared
    // profile so `configured` stays true throughout, isolating the
    // update-available logic from the (separately covered) auth-configured
    // gate.
    #[tokio::test]
    async fn assemble_list_marks_update_available_from_component_release_ledger() {
        let cp = test_cp().await;
        let settings = SettingsStore::new(cp.store().clone());
        settings.set("plugin.github.enabled", "true").await.unwrap();
        cp.store()
            .upsert_plugin_oauth_profile_token(
                "github",
                "github",
                &PluginOauthToken {
                    plugin_id: "github".into(),
                    access_token: "tok".into(),
                    refresh_token: None,
                    token_type: "Bearer".into(),
                    expires_at: None,
                    scopes: vec![],
                    reconnect_required: false,
                },
            )
            .await
            .unwrap();

        cp.store()
            .upsert_remote_catalog(&[crate::store::RemoteCatalogRow {
                id: "github".to_string(),
                manifest_toml: String::new(),
                version: "2.0.0".to_string(),
                sequence: 1,
                blocked: false,
                blocked_reason: None,
                fetched_at: 0,
            }])
            .await
            .unwrap();

        for version in ["1.0.0", "2.0.0"] {
            cp.store()
                .upsert_component_release(&crate::store::ComponentPluginReleaseRecord {
                    plugin_id: "github".to_string(),
                    version: version.to_string(),
                    source_url: format!("https://feed.test/github/{version}"),
                    sha256: "0".repeat(64),
                    signing_key_id: "first-party".to_string(),
                    installed_at: crate::paths::now_ms(),
                    active: false,
                    revoked: false,
                    revocation_reason: None,
                })
                .await
                .unwrap();
        }
        cp.store()
            .set_active_component_release("github", "1.0.0")
            .await
            .unwrap();

        let stale = assemble_list(&cp).await.unwrap();
        let github_stale = stale
            .iter()
            .find(|p| p.id == "github")
            .expect("github present in the list");
        assert_eq!(github_stale.status, "update-available");

        cp.store()
            .set_active_component_release("github", "2.0.0")
            .await
            .unwrap();

        let current = assemble_list(&cp).await.unwrap();
        let github_current = current
            .iter()
            .find(|p| p.id == "github")
            .expect("github present in the list");
        assert_eq!(github_current.status, "ok");
    }

    // ---------- auth_kind_label / auth_configured ----------

    #[test]
    fn auth_kind_label_maps_every_variant() {
        assert_eq!(auth_kind_label(AuthKind::None), "none");
        assert_eq!(auth_kind_label(AuthKind::ApiKey), "api-key");
        assert_eq!(auth_kind_label(AuthKind::Token), "token");
        assert_eq!(auth_kind_label(AuthKind::Oauth), "oauth");
    }

    #[test]
    fn field_kind_label_maps_every_variant() {
        assert_eq!(field_kind_label(FieldKind::String), "string");
        assert_eq!(field_kind_label(FieldKind::Int), "int");
        assert_eq!(field_kind_label(FieldKind::Bool), "bool");
    }

    // ---------- build_settings_info (Feature C3: kind/options/default) ----------

    #[tokio::test]
    async fn build_settings_info_carries_kind_options_and_default() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(crate::Store::open(tmp.path()).await.unwrap());
        let fields = vec![
            SettingField {
                key: "plugin.acme.tier".to_string(),
                label: "Tier".to_string(),
                help: String::new(),
                secret: false,
                required: false,
                kind: FieldKind::String,
                options: vec!["free".to_string(), "pro".to_string()],
                default: Some("free".to_string()),
            },
            SettingField {
                key: "plugin.acme.retries".to_string(),
                label: "Retries".to_string(),
                help: String::new(),
                secret: false,
                required: false,
                kind: FieldKind::Int,
                options: vec![],
                default: Some("3".to_string()),
            },
            SettingField {
                key: "plugin.acme.verbose".to_string(),
                label: "Verbose".to_string(),
                help: String::new(),
                secret: false,
                required: false,
                kind: FieldKind::Bool,
                options: vec![],
                default: None,
            },
        ];

        let out = build_settings_info(&store, &fields).await.unwrap();
        assert_eq!(out.len(), 3);

        assert_eq!(out[0].kind, "string");
        assert_eq!(out[0].options, vec!["free".to_string(), "pro".to_string()]);
        assert_eq!(out[0].default.as_deref(), Some("free"));

        assert_eq!(out[1].kind, "int");
        assert!(out[1].options.is_empty());
        assert_eq!(out[1].default.as_deref(), Some("3"));

        assert_eq!(out[2].kind, "bool");
        assert!(out[2].options.is_empty());
        assert_eq!(out[2].default, None);
    }

    #[test]
    fn auth_configured_true_when_setting_value_is_non_empty() {
        assert!(auth_configured(Some("sk-secret"), false));
    }

    #[test]
    fn auth_configured_true_when_env_fallback_is_set() {
        assert!(auth_configured(None, true));
        assert!(auth_configured(Some(""), true));
    }

    #[test]
    fn auth_configured_false_when_neither_setting_nor_env_present() {
        assert!(!auth_configured(None, false));
        assert!(!auth_configured(Some(""), false));
    }

    // PR-3: a component with declared oauth profiles is configured only when
    // every profile has a live stored token.
    #[tokio::test]
    async fn component_oauth_configured_requires_profile_tokens() {
        let cp = test_cp().await;
        let auth = Some(AuthSpec {
            kind: AuthKind::Oauth,
            ..Default::default()
        });
        assert!(!plugin_auth_configured(cp.store(), "github", auth.as_ref())
            .await
            .unwrap());
        // Store a token for github's single declared profile ("github",
        // matching plugins/github/ryuzi-plugin.toml's `[[oauth]] id =
        // "github"`) → configured.
        cp.store()
            .upsert_plugin_oauth_profile_token(
                "github",
                "github",
                &PluginOauthToken {
                    plugin_id: "github".into(),
                    access_token: "tok".into(),
                    refresh_token: None,
                    token_type: "Bearer".into(),
                    expires_at: None,
                    scopes: vec![],
                    reconnect_required: false,
                },
            )
            .await
            .unwrap();
        assert!(plugin_auth_configured(cp.store(), "github", auth.as_ref())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn plugin_oauth_authorize_url_uses_pkce_scopes_and_client_id_from_settings() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(crate::Store::open(tmp.path()).await.unwrap());
        store
            .set_setting_raw("plugin.acme.client_id", "acme-client-123")
            .await
            .unwrap();

        let auth = AuthSpec {
            kind: AuthKind::Oauth,
            authorize_url: Some("https://acme.example.com/oauth/authorize".into()),
            token_url: Some("https://acme.example.com/oauth/token".into()),
            scopes: vec!["repo".into(), "issues:read".into()],
            client_id_setting: Some("plugin.acme.client_id".into()),
            extra_authorize_params: BTreeMap::from([("prompt".into(), "consent".into())]),
            ..Default::default()
        };

        let begin = build_plugin_oauth_begin_result(
            &store,
            "acme-oauth",
            &auth,
            "verifier-test-123",
            "state-test-123",
        )
        .await
        .unwrap();

        let url = reqwest::Url::parse(&begin.authorize_url).unwrap();
        let query: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(
            url.as_str().split('?').next().unwrap(),
            "https://acme.example.com/oauth/authorize"
        );
        assert_eq!(
            query.get("client_id").map(String::as_str),
            Some("acme-client-123")
        );
        assert_eq!(
            query.get("code_challenge").map(String::as_str),
            Some(crate::plugins::oauth::pkce_challenge_s256("verifier-test-123").as_str())
        );
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(query.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(
            query.get("state").map(String::as_str),
            Some("state-test-123")
        );
        assert_eq!(
            query.get("scope").map(String::as_str),
            Some("repo issues:read")
        );
        assert_eq!(query.get("prompt").map(String::as_str), Some("consent"));
        assert_eq!(
            query.get("redirect_uri").map(String::as_str),
            Some(begin.redirect_uri.as_str())
        );
    }

    #[tokio::test]
    async fn resolve_plugin_oauth_orders_row_then_setting_then_external_auth_setting() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(crate::Store::open(tmp.path()).await.unwrap());
        store
            .set_setting_raw("plugin.gw.client_id", "setting-client")
            .await
            .unwrap();

        // External plugin (no resource, no authorize_url): auth.setting IS the
        // client id key (google-workspace shape).
        let external = AuthSpec {
            kind: AuthKind::Oauth,
            setting: Some("plugin.gw.client_id".into()),
            ..Default::default()
        };
        assert!(is_external_oauth(&external));
        let resolved = resolve_plugin_oauth(&store, "gw", &external).await.unwrap();
        assert_eq!(resolved.client_id.as_deref(), Some("setting-client"));

        // Non-external (resource declared): auth.setting is NOT consulted.
        let non_external = AuthSpec {
            kind: AuthKind::Oauth,
            setting: Some("plugin.gw.client_id".into()),
            resource: Some("https://vendor.test/mcp".into()),
            ..Default::default()
        };
        assert!(!is_external_oauth(&non_external));
        let resolved = resolve_plugin_oauth(&store, "gw", &non_external)
            .await
            .unwrap();
        assert_eq!(resolved.client_id, None);

        // client_id_setting is second in the order…
        let with_setting = AuthSpec {
            client_id_setting: Some("plugin.gw.client_id".into()),
            ..non_external.clone()
        };
        let resolved = resolve_plugin_oauth(&store, "gw", &with_setting)
            .await
            .unwrap();
        assert_eq!(resolved.client_id.as_deref(), Some("setting-client"));

        // …and the plugin_oauth_clients row wins over everything, endpoints
        // included (table → manifest).
        store
            .upsert_plugin_oauth_client(&PluginOauthClient {
                plugin_id: "gw".into(),
                authorize_url: Some("https://discovered.test/authorize".into()),
                token_url: Some("https://discovered.test/token".into()),
                client_id: Some("row-client".into()),
            })
            .await
            .unwrap();
        let resolved = resolve_plugin_oauth(&store, "gw", &with_setting)
            .await
            .unwrap();
        assert_eq!(resolved.client_id.as_deref(), Some("row-client"));
        assert_eq!(
            resolved.authorize_url.as_deref(),
            Some("https://discovered.test/authorize")
        );
        assert_eq!(
            resolved.token_url.as_deref(),
            Some("https://discovered.test/token")
        );
    }

    #[tokio::test]
    async fn begin_result_prefers_table_endpoints_over_manifest() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(crate::Store::open(tmp.path()).await.unwrap());
        store
            .upsert_plugin_oauth_client(&PluginOauthClient {
                plugin_id: "acme-table".into(),
                authorize_url: Some("https://discovered.test/authorize".into()),
                token_url: Some("https://discovered.test/token".into()),
                client_id: Some("row-client".into()),
            })
            .await
            .unwrap();
        let auth = AuthSpec {
            kind: AuthKind::Oauth,
            authorize_url: Some("https://manifest.test/authorize".into()),
            token_url: Some("https://manifest.test/token".into()),
            ..Default::default()
        };
        let begin = build_plugin_oauth_begin_result(&store, "acme-table", &auth, "v-1", "s-1")
            .await
            .unwrap();
        assert!(
            begin
                .authorize_url
                .starts_with("https://discovered.test/authorize?"),
            "{}",
            begin.authorize_url
        );
        assert!(begin.authorize_url.contains("client_id=row-client"));
    }

    // ---------- field_value_set ----------

    #[test]
    fn field_value_set_true_only_for_non_empty_persisted_value() {
        assert!(field_value_set(Some("x")));
        assert!(!field_value_set(Some("")));
        assert!(!field_value_set(None));
    }

    // ---------- mcp_transport_label / mcp_info ----------

    #[test]
    fn mcp_transport_label_maps_both_variants() {
        assert_eq!(mcp_transport_label(McpTransportDef::Stdio), "stdio");
        assert_eq!(mcp_transport_label(McpTransportDef::Http), "http");
    }

    #[test]
    fn mcp_info_uses_command_for_stdio_and_url_for_http() {
        let stdio = McpServerDef {
            name: "svc".to_string(),
            transport: McpTransportDef::Stdio,
            command: Some("npx".to_string()),
            args: vec![],
            env: Default::default(),
            url: None,
            headers: Default::default(),
        };
        let info = mcp_info(&stdio);
        assert_eq!(info.transport, "stdio");
        assert_eq!(info.command_or_url, "npx");

        let http = McpServerDef {
            name: "svc2".to_string(),
            transport: McpTransportDef::Http,
            command: None,
            args: vec![],
            env: Default::default(),
            url: Some("https://example.com/mcp".to_string()),
            headers: Default::default(),
        };
        let info2 = mcp_info(&http);
        assert_eq!(info2.transport, "http");
        assert_eq!(info2.command_or_url, "https://example.com/mcp");
    }

    // ---------- assemble_list / assemble_detail (ControlPlane-backed) ----------

    /// A connector-capable plugin that authenticates the given way. Tests that
    /// need a specific plugin SHAPE (oauth, a token setting, a real connector
    /// capability) build one here rather than depending on a shipped
    /// integration id — the embedded declarative catalog that used to supply
    /// `notion`/`github` for this purpose no longer exists.
    fn auth_connector(id: &str, kind: AuthKind, setting: Option<&str>) -> CorePlugin {
        auth_connector_full(id, kind, setting, None)
    }

    /// Like [`auth_connector`] but also sets `auth.resource`, which flips an
    /// OAuth plugin from "external" (client id → its `auth.setting`) to
    /// "resource-declared" (client id → the `plugin_oauth_clients` row) — the
    /// distinction `set_plugin_oauth_client_id` routes on.
    fn auth_connector_full(
        id: &str,
        kind: AuthKind,
        setting: Option<&str>,
        resource: Option<&str>,
    ) -> CorePlugin {
        let mut manifest = manifest(id);
        manifest.auth = Some(AuthSpec {
            kind,
            setting: setting.map(|s| s.to_string()),
            resource: resource.map(|s| s.to_string()),
            ..Default::default()
        });
        CorePlugin {
            manifest,
            harness: None,
            gateway: None,
            connector: Some(Arc::new(FakeConnector)),
            provider: None,
            source: PluginSource::Builtin,
        }
    }

    /// [`test_cp`] plus caller-supplied plugins, registered FIRST so they win
    /// their ids over anything `install_builtins` adds.
    async fn test_cp_with(extra: Vec<CorePlugin>) -> Arc<ControlPlane> {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(crate::Store::open(tmp.path()).await.unwrap());
        let mut regs = Registries::new();
        regs.add_plugin(crate::harness::native::native_plugin());
        for plugin in extra {
            regs.add_plugin(plugin);
        }
        crate::plugins::install_builtins(&mut regs);
        {
            let persistence =
                crate::agents::bootstrap::AgentPersistence::temporary(Arc::clone(&store))
                    .await
                    .unwrap();
            ControlPlane::new(store, regs, persistence).await
        }
    }

    async fn test_cp() -> Arc<ControlPlane> {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(crate::Store::open(tmp.path()).await.unwrap());
        let mut regs = Registries::new();
        // Mirror the composition root: the `native` runtime is registered
        // explicitly before `install_builtins` adds providers, CLI agents, and
        // the catalog (see `install_builtins`'s doc — those builtins win
        // same-id collisions).
        regs.add_plugin(crate::harness::native::native_plugin());
        crate::plugins::install_builtins(&mut regs);
        {
            let persistence =
                crate::agents::bootstrap::AgentPersistence::temporary(Arc::clone(&store))
                    .await
                    .unwrap();
            ControlPlane::new(store, regs, persistence).await
        }
    }

    #[tokio::test]
    async fn list_includes_anthropic_enabled_with_provider_capability() {
        let cp = test_cp().await;
        let list = assemble_list(&cp).await.unwrap();
        let anthropic = list
            .iter()
            .find(|p| p.id == "anthropic")
            .expect("anthropic plugin present");
        assert!(
            anthropic.enabled,
            "manifest-only plugins are always enabled"
        );
        assert_eq!(anthropic.capabilities, vec!["provider".to_string()]);
        assert_eq!(anthropic.source, "builtin");
    }

    #[tokio::test]
    async fn assemble_list_excludes_runtimes_and_synthesizes_curated_packs() {
        // `installed_ctx` reads `RYUZI_TEST_CONFIG_ROOT` via
        // `InstallRoots::for_user` (see `skills_install.rs`), a process-wide
        // env var — an empty-but-guarded root keeps this deterministic
        // against a concurrently-running test that points it at a fixture
        // WITH an installed "superpowers" pack (`InstalledCuratedPackFixture`
        // below), and against whatever the real machine's home directory
        // happens to have installed when no guard is held at all.
        let empty_root = tempfile::tempdir().unwrap();
        let _config_root = crate::api::tests_support::TestConfigRootGuard::set(empty_root.path());
        let cp = test_cp().await;
        let list = assemble_list(&cp).await.unwrap();
        assert!(list
            .iter()
            .all(|p| p.id != "native" && p.id != "claude-code"));
        let superpowers = list
            .iter()
            .find(|p| p.kind == "skill-pack" && p.id == "superpowers")
            .expect("curated pack row");
        // Finding 4 baseline: an uninstalled curated pack still reports the
        // Browse-tile default, not the installed row's "ok"/skill_count.
        assert_eq!(superpowers.status, "not-installed");
        assert!(superpowers.skill_count.is_none());
        let anthropic = list.iter().find(|p| p.id == "anthropic").expect("provider");
        assert_eq!(anthropic.kind, "provider");
        assert_eq!(anthropic.family.as_deref(), Some("anthropic"));
    }

    /// Test-only guard: simulates an INSTALLED curated skill pack
    /// (`superpowers`) with no registered `CorePlugin`/manifest — a single
    /// materialized skill directory under a tempdir `skills/` whose
    /// provenance `source` matches `CuratedSkillPack::repo`, the same shape a
    /// real git-clone install of a repo with no `ryuzi-plugin.toml` of its
    /// own leaves on disk. `test_cp()`/`test_cp_with` never scan
    /// `RYUZI_TEST_CONFIG_ROOT`'s `plugins/` dir (only `install_builtins`
    /// registers plugins for them), so this can never accidentally register
    /// "superpowers" as a real plugin — `curated_pack_row`'s synthesized-row
    /// path keeps applying, only now with `installed: true`.
    struct InstalledCuratedPackFixture {
        _temp_dir: tempfile::TempDir,
        _config_root: crate::api::tests_support::TestConfigRootGuard,
    }

    impl InstalledCuratedPackFixture {
        fn install() -> Self {
            let temp_dir = tempfile::tempdir().unwrap();
            let skill_dir = temp_dir.path().join("skills").join("superpowers");
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: superpowers\ndescription: Curated workflow and development skills\n---\nbody",
            )
            .unwrap();
            std::fs::write(
                skill_dir.join(".ryuzi-skill.json"),
                r#"{"source":"https://github.com/obra/superpowers","plugin_id":null,"installed_at":"2026-01-01T00:00:00.000Z"}"#,
            )
            .unwrap();
            // Set the env var AFTER the fixture is fully written to disk —
            // avoids a window where a concurrently-running test could read a
            // half-written directory through the same process-wide env var.
            let config_root = crate::api::tests_support::TestConfigRootGuard::set(temp_dir.path());
            Self {
                _temp_dir: temp_dir,
                _config_root: config_root,
            }
        }
    }

    // Finding 4 (final-review fix): an INSTALLED curated pack must report
    // `status: "ok"` (not the hardcoded `"not-installed"` the Browse-tile
    // default used to send back regardless of `installed`) and a populated
    // `skill_count` — both `assemble_list` (this test) and `assemble_detail`
    // (`detail_resolves_installed_curated_skill_pack_...` below) synthesize
    // this row through the same `curated_pack_row`, so both call sites must
    // agree.
    #[tokio::test]
    async fn assemble_list_reports_installed_curated_pack_as_ok_with_skill_count() {
        let _fixture = InstalledCuratedPackFixture::install();
        let cp = test_cp().await;
        assert!(
            cp.plugins().get("superpowers").is_none(),
            "precondition: still no registered CorePlugin for the curated pack"
        );
        let list = assemble_list(&cp).await.unwrap();
        let superpowers = list
            .iter()
            .find(|p| p.id == "superpowers")
            .expect("curated pack row");
        assert_eq!(superpowers.kind, "skill-pack");
        assert!(superpowers.installed);
        assert_eq!(superpowers.status, "ok");
        assert_eq!(superpowers.skill_count, Some(1));
    }

    #[tokio::test]
    async fn detail_unknown_id_errors() {
        let cp = test_cp().await;
        match assemble_detail(&cp, "nope").await {
            Ok(_) => panic!("expected an error for an unknown plugin id"),
            Err(e) => assert_eq!(e.to_string(), "unknown plugin: nope"),
        }
    }

    /// Task 5: an uninstalled curated skill pack (e.g. `superpowers`) has no
    /// registered `CorePlugin` — before this fix, `assemble_detail` bailed
    /// `unknown plugin: superpowers` for it even though `assemble_list`
    /// already synthesizes a Browse-tile row (see
    /// `assemble_list_excludes_runtimes_and_synthesizes_curated_packs`).
    /// Navigating into that tile must resolve, not 500.
    #[tokio::test]
    async fn detail_resolves_uninstalled_curated_skill_pack() {
        // See the matching comment on
        // `assemble_list_excludes_runtimes_and_synthesizes_curated_packs` —
        // an empty-but-guarded config root keeps `installed_ctx`'s read
        // deterministic against a concurrently-running installed-pack
        // fixture (same "superpowers" id) elsewhere in this test binary.
        let empty_root = tempfile::tempdir().unwrap();
        let _config_root = crate::api::tests_support::TestConfigRootGuard::set(empty_root.path());
        let cp = test_cp().await;
        assert!(
            cp.plugins().get("superpowers").is_none(),
            "precondition: superpowers is a synthesized curated pack, not a registered plugin"
        );
        let detail = assemble_detail(&cp, "superpowers")
            .await
            .expect("uninstalled curated skill pack should resolve, not error");
        assert_eq!(detail.info.id, "superpowers");
        assert_eq!(detail.info.kind, "skill-pack");
        assert!(!detail.info.installed);
        assert_eq!(detail.info.status, "not-installed");
        assert!(detail.auth.is_none());
        assert!(detail.settings.is_empty());
        assert!(detail.mcp.is_empty());
        assert!(detail.models.is_empty());
        assert_eq!(detail.publisher, "");
        assert_eq!(
            detail.homepage.as_deref(),
            Some("https://github.com/obra/superpowers")
        );
    }

    // Finding 4 (final-review fix): the detail-view sibling of
    // `assemble_list_reports_installed_curated_pack_as_ok_with_skill_count`
    // — `curated_pack_detail` must resolve the same `"ok"`/`skill_count` shape
    // for an installed curated pack, not just `assemble_list`'s row.
    #[tokio::test]
    async fn detail_resolves_installed_curated_skill_pack_as_ok_with_skill_count() {
        let _fixture = InstalledCuratedPackFixture::install();
        let cp = test_cp().await;
        let detail = assemble_detail(&cp, "superpowers")
            .await
            .expect("installed curated skill pack should resolve");
        assert_eq!(detail.info.id, "superpowers");
        assert_eq!(detail.info.kind, "skill-pack");
        assert!(detail.info.installed);
        assert_eq!(detail.info.status, "ok");
        assert_eq!(detail.info.skill_count, Some(1));
    }

    #[tokio::test]
    async fn detail_anthropic_has_provider_models_and_unconfigured_api_key_auth() {
        let cp = test_cp().await;
        let detail = assemble_detail(&cp, "anthropic").await.unwrap();
        assert_eq!(detail.info.id, "anthropic");
        assert!(!detail.models.is_empty());
        assert!(detail.settings.is_empty());
        assert!(detail.mcp.is_empty());
        assert_eq!(detail.publisher, "ryuzi");

        let auth = detail
            .auth
            .expect("anthropic manifest declares an auth block");
        assert_eq!(auth.kind, "api-key");
        assert!(
            !auth.configured,
            "no connection/env configured in a fresh store"
        );
    }

    #[tokio::test]
    async fn plugin_info_configured_matches_auth_info_semantics_for_non_oauth() {
        let cp = test_cp().await;
        let list = assemble_list(&cp).await.unwrap();
        let anthropic = list.iter().find(|p| p.id == "anthropic").unwrap();
        assert!(!anthropic.configured, "fresh store: nothing configured");
        let detail = assemble_detail(&cp, "anthropic").await.unwrap();
        assert_eq!(
            detail.info.configured,
            detail.auth.expect("anthropic declares auth").configured
        );
    }

    #[tokio::test]
    async fn plugin_info_configured_for_oauth_requires_stored_token_without_reconnect() {
        crate::llm_router::secrets::use_test_key_file();
        let cp = test_cp_with(vec![auth_connector("acme-oauth", AuthKind::Oauth, None)]).await;
        let before = assemble_detail(&cp, "acme-oauth").await.unwrap();
        assert!(!before.info.configured);

        cp.store()
            .upsert_plugin_oauth_token(&PluginOauthToken {
                plugin_id: "acme-oauth".into(),
                access_token: "tok".into(),
                refresh_token: None,
                token_type: "Bearer".into(),
                expires_at: None,
                scopes: vec![],
                reconnect_required: false,
            })
            .await
            .unwrap();
        let with_token = assemble_detail(&cp, "acme-oauth").await.unwrap();
        assert!(with_token.info.configured);

        cp.store()
            .mark_plugin_oauth_reconnect_required("acme-oauth")
            .await
            .unwrap();
        let reconnect = assemble_detail(&cp, "acme-oauth").await.unwrap();
        assert!(
            !reconnect.info.configured,
            "reconnect_required must unset configured"
        );
    }

    #[tokio::test]
    async fn set_plugin_enabled_and_setting_round_trip_through_the_control_plane() {
        let cp = test_cp().await;
        let settings = SettingsStore::new(cp.store().clone());

        // "kiro" is a manifest-only CATALOG provider with no component/bundle
        // backing (`is_component_bundle("kiro")` is false, unlike anthropic —
        // see `plugin_tools_provider_lists_models`'s doc): `is_enabled`
        // always reports it enabled regardless of any `plugin.<id>.enabled`
        // write, so toggling it must error rather than silently no-op (see
        // `toggle_enabled`'s doc). Anthropic itself is the wrong fixture
        // here post-Task-2: it's in `COMPONENT_BACKED_PROVIDER_IDS`, so
        // `toggle_enabled` now flips its transport-enable key instead of
        // erroring.
        let err = crate::plugins::toggle_enabled(cp.plugins(), &settings, "kiro", true)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "kiro is always available");

        settings
            .set("default_perm_mode", "acceptEdits")
            .await
            .unwrap();
        assert_eq!(
            settings.get("default_perm_mode").await.unwrap().as_deref(),
            Some("acceptEdits")
        );
    }

    // ---------- uninstall (kind-symmetric teardown) ----------

    #[tokio::test]
    async fn uninstall_provider_removes_every_family_connection() {
        let cp = test_cp().await;
        let now = crate::paths::now_ms();
        for (id, provider) in [
            ("c1", "anthropic"),
            ("c2", "anthropic-oauth"),
            ("c3", "openai"),
        ] {
            crate::llm_router::connections::add_connection(
                cp.store(),
                crate::llm_router::connections::ConnectionRow {
                    id: id.into(),
                    provider: provider.into(),
                    auth_type: "api_key".into(),
                    label: id.into(),
                    priority: 0,
                    enabled: true,
                    data: Default::default(),
                    created_at: now,
                    updated_at: now,
                },
            )
            .await
            .unwrap();
        }

        uninstall(&cp, "anthropic").await.unwrap();

        let left = crate::llm_router::connections::list_connections(cp.store())
            .await
            .unwrap();
        let providers: Vec<_> = left.iter().map(|c| c.provider.as_str()).collect();
        assert_eq!(
            providers,
            vec!["openai"],
            "family (anthropic + anthropic-oauth) removed"
        );
    }

    #[tokio::test]
    async fn uninstall_provider_survives_builtin_free_row_but_removes_paid_family_connection() {
        // Regression for the guard added in connections.rs: `mimo`'s family
        // is `mimo-free` (see registry::family_of), the same family as the
        // built-in free row. Before this fix, uninstalling the `mimo`
        // provider plugin tried to delete the builtin row via
        // `remove_connection` and the whole uninstall failed with "MiMo
        // (free) is a built-in connection and cannot be removed".
        let cp = test_cp().await;
        let now = crate::paths::now_ms();
        crate::llm_router::connections::add_connection(
            cp.store(),
            crate::llm_router::connections::ConnectionRow {
                id: "builtin-mimo".into(),
                provider: "mimo-free".into(),
                auth_type: "free".into(),
                label: "MiMo (free)".into(),
                priority: 0,
                enabled: true,
                data: Default::default(),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        crate::llm_router::connections::add_connection(
            cp.store(),
            crate::llm_router::connections::ConnectionRow {
                id: "paid-mimo".into(),
                provider: "mimo".into(),
                auth_type: "api_key".into(),
                label: "My MiMo account".into(),
                priority: 0,
                enabled: true,
                data: Default::default(),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();

        uninstall(&cp, "mimo").await.unwrap();

        let left = crate::llm_router::connections::list_connections(cp.store())
            .await
            .unwrap();
        let ids: Vec<_> = left.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["builtin-mimo"],
            "builtin free row survives; paid family row is removed"
        );
    }

    #[tokio::test]
    async fn uninstall_provider_clears_transport_enable_key() {
        // Spec B1 / Fix 2: an uninstalled provider must stay off. Before this
        // fix, `uninstall`'s provider arm only cleaned up connection rows —
        // the `plugin.<id>.enabled` row (what `discover_provider_components`
        // reads to decide whether to (re-)register a live transport) was left
        // "true", so a later unrelated `hot_reload_provider_transports` call
        // (from ANY other install/enable) could resurrect the uninstalled
        // plugin's transport out of the still-active-on-disk bundle.
        let cp = test_cp().await;
        cp.store()
            .set_setting_raw("plugin.mimo.enabled", "true")
            .await
            .unwrap();

        uninstall(&cp, "mimo").await.unwrap();

        assert_eq!(
            cp.store()
                .get_setting_raw("plugin.mimo.enabled")
                .await
                .unwrap(),
            None,
            "uninstalling a provider must clear its transport-enable key"
        );
    }

    #[tokio::test]
    async fn uninstall_integration_clears_credential_and_disables() {
        // A connector-capable, token-authenticated plugin: the shape `github`
        // used to have as a declarative catalog entry. It is a WASM component
        // now (manifest-only, so always-enabled), hence the synthetic stand-in.
        let cp = test_cp_with(vec![auth_connector(
            "acme-token",
            AuthKind::Token,
            Some("plugin.acme-token.token"),
        )])
        .await;
        cp.store()
            .set_setting_raw("plugin.acme-token.token", "tok")
            .await
            .unwrap();
        let settings = SettingsStore::new(cp.store().clone());
        crate::plugins::toggle_enabled(cp.plugins(), &settings, "acme-token", true)
            .await
            .unwrap();

        uninstall(&cp, "acme-token").await.unwrap();

        assert_eq!(
            cp.store()
                .get_setting_raw("plugin.acme-token.token")
                .await
                .unwrap(),
            None
        );
        assert!(!cp
            .plugins()
            .is_enabled(&settings, "acme-token")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn uninstall_component_integration_deactivates_the_active_release() {
        // With installed-ness now derived from the active release ledger, an
        // uninstall must deactivate the release or the row would keep reading
        // "installed" forever (the reconcile's own comment already promises
        // "uninstall may deactivate the bundle"). Uses the catalog's discord
        // stand-in — `derive_kind` sees "integration" for it.
        let cp = test_cp().await;
        seed_active_component_release(&cp, "discord").await;

        uninstall(&cp, "discord").await.unwrap();

        assert!(
            cp.store()
                .active_component_release("discord")
                .await
                .unwrap()
                .is_none(),
            "uninstall must deactivate the component release"
        );
        // Deactivated, not revoked — a reinstall stays possible.
        let releases = cp.store().list_component_releases("discord").await.unwrap();
        assert!(releases.iter().all(|r| !r.revoked));
    }

    #[tokio::test]
    async fn uninstall_gateway_deactivates_the_active_release() {
        // Same contract through the "gateway" arm — the shape discord's host
        // row takes once the engine has restarted with the bundle enabled.
        let cp = test_cp_with(vec![gateway_only("discord")]).await;
        seed_active_component_release(&cp, "discord").await;

        uninstall(&cp, "discord").await.unwrap();

        assert!(
            cp.store()
                .active_component_release("discord")
                .await
                .unwrap()
                .is_none(),
            "gateway uninstall must deactivate the component release"
        );
    }

    #[tokio::test]
    async fn uninstall_unknown_id_errors() {
        let cp = test_cp().await;
        assert!(uninstall(&cp, "definitely-not-a-plugin").await.is_err());
    }

    // ---------- uninstall latch granularity (spec B1) ----------
    //
    // These call `uninstall_and_reconcile` directly rather than through
    // `dispatch`/`ApiState`: every other test in this module drives the
    // handler fns (`uninstall`, `install_component_plugin`, ...) against a
    // bare `ControlPlane` from `test_cp()`, not the RPC layer, so the
    // dispatch arm's kind-aware reconcile logic is extracted into
    // `uninstall_and_reconcile` to stay directly testable the same way.

    #[tokio::test]
    async fn uninstall_provider_does_not_latch_restart() {
        // "mimo" is registered as a provider builtin (`install_providers`,
        // via `test_cp()`'s `install_builtins`) — `derive_kind` reports it
        // "provider" because `manifest.provider.is_some()`.
        let cp = test_cp().await;
        assert!(!cp.plugins_restart_required());
        uninstall_and_reconcile(&cp, "mimo").await.unwrap();
        assert!(
            !cp.plugins_restart_required(),
            "provider uninstall is hot (unregister) — must not latch"
        );
    }

    #[tokio::test]
    async fn uninstall_gateway_latches_restart() {
        // `gateway_only("discord")` gives a real gateway capability (unlike
        // `component_catalog_plugins()`'s manifest-only catalog entry for the
        // same id), so `derive_kind` reports "gateway" — the same fixture
        // `derive_kind_classifies_each_capability_shape` uses for this id.
        // Registered via `test_cp_with` so it wins the "discord" id over the
        // catalog's own manifest-only entry (first-registration-wins).
        let cp = test_cp_with(vec![gateway_only("discord")]).await;
        assert!(!cp.plugins_restart_required());
        uninstall_and_reconcile(&cp, "discord").await.unwrap();
        assert!(
            cp.plugins_restart_required(),
            "gateway uninstall still needs a restart"
        );
    }

    // A first-time component GATEWAY install/rollback/uninstall has NO host row
    // yet other than the manifest-only catalog stand-in (`derive_kind` sees
    // `Some("integration")` for it, never `Some("gateway")` — see
    // `installed_bundle_is_gateway`'s doc comment). These pin the capability
    // check the granular latch now runs for that `Some("integration")` case.

    #[tokio::test]
    async fn installed_bundle_is_gateway_is_false_without_active_bundle() {
        // A hermetic, empty temp dir (not the real per-user
        // `installed_bundle_root()`, which may hold real installs left over
        // from manual smoke-testing on a dev machine — `load_active_bundles`
        // fails closed on ANY mismatched entry anywhere under the root, so a
        // populated real root would make this test flaky depending on
        // ambient machine state; see this fn's own doc). No active bundle on
        // disk for this id — a classic integration row has nothing frozen to
        // reload, so this must be false, not fail-closed.
        let cp = test_cp().await;
        let empty_root = tempfile::tempdir().unwrap();
        assert!(
            !installed_bundle_is_gateway(
                cp.store(),
                empty_root.path(),
                "definitely-not-an-installed-bundle-id"
            )
            .await
        );
    }

    #[tokio::test]
    async fn installed_bundle_is_gateway_is_false_when_the_bundle_root_does_not_exist_yet() {
        // The bundle root itself doesn't exist at all — the common case for
        // a first-time daemon that has never installed ANY component bundle.
        // `load_active_bundles` errors on `root.canonicalize()` for a
        // nonexistent path, so this pins the pre-check that treats "no root"
        // as "no active bundle" rather than a fail-closed error.
        let cp = test_cp().await;
        let root = tempfile::tempdir().unwrap();
        let nonexistent = root.path().join("never-created");
        assert!(!installed_bundle_is_gateway(cp.store(), &nonexistent, "discord").await);
    }

    // Positive path: a REAL bundle that genuinely exports `ryuzi:gateway/
    // gateway`, staged on a hermetic on-disk root, must report `true`. The
    // two tests above only pin the "nothing to find" false paths and the
    // fail-closed error paths — none of them prove the actual
    // `compiled.exports_gateway()` read at the end of the function ever
    // returns `true` for a genuine gateway component. Reuses
    // `wasm_provider`'s own gateway fixture + on-disk staging helper (the
    // exact fixture `non_provider_bundle_registers_nothing` in that module
    // uses to prove the OPPOSITE thing — that a gateway bundle must NOT
    // register as a provider) rather than compiling a second throwaway
    // gateway component just for this assertion.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn installed_bundle_is_gateway_is_true_for_a_real_gateway_bundle() {
        crate::plugins::build_fixture_components_once();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(crate::Store::open(tmp.path()).await.unwrap());
        let root = tempfile::tempdir().unwrap();
        crate::plugins::wasm_provider::install_bundle_on_disk(
            root.path(),
            &store,
            "acme-gateway-fixture",
            &crate::plugins::wasm_provider::gateway_fixture_artifact(),
            &[],
        )
        .await;

        assert!(
            installed_bundle_is_gateway(&store, root.path(), "acme-gateway-fixture").await,
            "a real gateway-exporting bundle must report true, not fail-closed-adjacent false"
        );
    }

    // No end-to-end test of `uninstall_and_reconcile`/`install_component_plugin`/
    // `rollback_component_plugin`'s "integration" arm through the REAL
    // `installed_bundle_root()` is added here, deliberately: those call
    // sites always pass the real root (by design — see their own comments),
    // and `load_active_bundles`'s fail-closed contract means ANY mismatch
    // between that real root's contents and a test's fresh, unrelated store
    // trips the conservative "true" path — not a bug, but a mismatch that
    // can ONLY occur in a test harness (production always pairs the real
    // root with the SAME real store the install pipeline wrote both
    // through). `installed_bundle_is_gateway_is_false_without_active_bundle`
    // and its bundle-root-missing sibling above already pin that function's
    // own correctness hermetically (root injected); the three call sites'
    // wiring onto it is a single `if` one-liner each, verified by reading.

    // The positive skill-pack uninstall path (a real pack on disk removed via
    // `remove_installed_skill`) resolves through `InstallRoots::for_user()`,
    // i.e. the real user skills dir — environment-dependent — so only the
    // hermetic bail path is asserted here; the rest is covered by
    // `crate::skills_install` unit tests.

    #[tokio::test]
    async fn uninstall_skill_pack_unknown_id_errors() {
        let cp = test_cp().await;
        assert!(uninstall(&cp, "definitely-not-installed-pack")
            .await
            .is_err());
    }

    // ---------- begin_plugin_install resolution (steps 1-6) ----------

    /// Minimal hand-rolled HTTP mock on std::net. Serves the RFC 8414 root +
    /// path-inserted documents pointing endpoints (and, when
    /// `with_registration`, the registration endpoint) at itself, plus an RFC
    /// 7591 register endpoint; counts hits per route.
    fn spawn_mock_vendor(
        with_registration: bool,
        discovery_hits: Arc<AtomicUsize>,
        register_hits: Arc<AtomicUsize>,
    ) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let served_base = base.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let base = served_base.clone();
                let discovery_hits = discovery_hits.clone();
                let register_hits = register_hits.clone();
                std::thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    let header_end = loop {
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break pos + 4;
                        }
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
                    // Drain any request body so the client never sees a reset
                    // while still writing.
                    if let Some(len) = head.lines().find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    }) {
                        let mut have = buf.len() - header_end;
                        while have < len {
                            match stream.read(&mut chunk) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => have += n,
                            }
                        }
                    }
                    let request_line = head.lines().next().unwrap_or_default().to_string();
                    let (status, body) = if request_line
                        .starts_with("GET /.well-known/oauth-authorization-server")
                    {
                        discovery_hits.fetch_add(1, Ordering::SeqCst);
                        let registration = if with_registration {
                            format!(r#","registration_endpoint":"{base}/register""#)
                        } else {
                            String::new()
                        };
                        (
                            "200 OK",
                            format!(
                                r#"{{"authorization_endpoint":"{base}/authorize","token_endpoint":"{base}/token"{registration}}}"#
                            ),
                        )
                    } else if request_line.starts_with("POST /register") {
                        register_hits.fetch_add(1, Ordering::SeqCst);
                        ("200 OK", r#"{"client_id":"dcr-client-123"}"#.to_string())
                    } else {
                        ("404 Not Found", String::new())
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                });
            }
        });
        base
    }

    async fn test_store() -> (crate::Store, tempfile::NamedTempFile) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::Store::open(tmp.path()).await.unwrap();
        (store, tmp)
    }

    #[tokio::test]
    async fn begin_env_var_short_circuits_before_any_oauth_work() {
        let (store, _tmp) = test_store().await;
        let var = "RYUZI_TEST_WIZ_ENV_7a91";
        std::env::set_var(var, "present");
        let auth = AuthSpec {
            kind: AuthKind::ApiKey,
            env: Some(var.to_string()),
            ..Default::default()
        };
        let http = reqwest::Client::new();
        let result = resolve_plugin_install(&store, &http, "wiz-env", Some(&auth))
            .await
            .unwrap();
        assert_eq!(result.auth_kind, "api-key");
        assert!(result.env_var_present);
        assert_eq!(result.env_var_name.as_deref(), Some(var));
        assert!(result.oauth_begin.is_none());
        std::env::remove_var(var);
    }

    #[tokio::test]
    async fn begin_non_oauth_kind_reports_kind_only() {
        let (store, _tmp) = test_store().await;
        let auth = AuthSpec {
            kind: AuthKind::Token,
            setting: Some("plugin.wiz-token.token".into()),
            ..Default::default()
        };
        let http = reqwest::Client::new();
        let result = resolve_plugin_install(&store, &http, "wiz-token", Some(&auth))
            .await
            .unwrap();
        assert_eq!(result.auth_kind, "token");
        assert!(!result.env_var_present);
        assert!(!result.oauth_available && !result.oauth_external && !result.needs_client_id);
        // And no [auth] block at all behaves as "none".
        let result = resolve_plugin_install(&store, &http, "wiz-none", None)
            .await
            .unwrap();
        assert_eq!(result.auth_kind, "none");
    }

    #[tokio::test]
    async fn begin_external_oauth_never_discovers_and_tracks_saved_client_id() {
        let (store, _tmp) = test_store().await;
        let auth = AuthSpec {
            kind: AuthKind::Oauth,
            setting: Some("plugin.wiz-external.client_id".into()),
            ..Default::default()
        };
        let http = reqwest::Client::new();
        let result = resolve_plugin_install(&store, &http, "wiz-external", Some(&auth))
            .await
            .unwrap();
        assert!(result.oauth_external);
        assert!(result.needs_client_id, "no saved auth.setting value yet");
        assert!(!result.oauth_available);
        assert!(result.oauth_begin.is_none());

        store
            .set_setting_raw("plugin.wiz-external.client_id", "google-client")
            .await
            .unwrap();
        let result = resolve_plugin_install(&store, &http, "wiz-external", Some(&auth))
            .await
            .unwrap();
        assert!(result.oauth_external);
        assert!(!result.needs_client_id);
        assert!(
            result.oauth_begin.is_none(),
            "external never opens a browser"
        );
    }

    #[tokio::test]
    async fn begin_runs_discovery_then_dcr_then_reuses_the_cache() {
        let (store, _tmp) = test_store().await;
        let discovery_hits = Arc::new(AtomicUsize::new(0));
        let register_hits = Arc::new(AtomicUsize::new(0));
        let base = spawn_mock_vendor(true, discovery_hits.clone(), register_hits.clone());
        let auth = AuthSpec {
            kind: AuthKind::Oauth,
            resource: Some(format!("{base}/mcp")),
            dynamic_registration: true,
            ..Default::default()
        };
        let http = reqwest::Client::new();

        let result = resolve_plugin_install(&store, &http, "wiz-dcr", Some(&auth))
            .await
            .unwrap();
        assert!(result.dcr_succeeded);
        assert!(result.oauth_available);
        assert!(!result.needs_client_id);
        let begin = result.oauth_begin.expect("browser flow prepared");
        assert!(
            begin
                .authorize_url
                .starts_with(&format!("{base}/authorize?")),
            "{}",
            begin.authorize_url
        );
        assert!(begin.authorize_url.contains("client_id=dcr-client-123"));
        assert_eq!(discovery_hits.load(Ordering::SeqCst), 1);
        assert_eq!(register_hits.load(Ordering::SeqCst), 1);
        // Flow state was stored for the callback/exchange.
        assert!(plugin_oauth_flows()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .contains_key(&plugin_oauth_flow_key("wiz-dcr", &begin.state_token)));

        // Endpoints + client id persisted.
        let row = store
            .get_plugin_oauth_client("wiz-dcr")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.authorize_url.as_deref(),
            Some(format!("{base}/authorize").as_str())
        );
        assert_eq!(
            row.token_url.as_deref(),
            Some(format!("{base}/token").as_str())
        );
        assert_eq!(row.client_id.as_deref(), Some("dcr-client-123"));

        // Second begin: cached endpoints reused (no second discovery) and a
        // client id on the row permanently suppresses DCR.
        let result2 = resolve_plugin_install(&store, &http, "wiz-dcr", Some(&auth))
            .await
            .unwrap();
        assert!(result2.oauth_available);
        assert!(!result2.dcr_succeeded);
        assert_eq!(
            discovery_hits.load(Ordering::SeqCst),
            1,
            "no second discovery"
        );
        assert_eq!(
            register_hits.load(Ordering::SeqCst),
            1,
            "no second registration"
        );
    }

    #[tokio::test]
    async fn begin_without_registration_endpoint_persists_endpoints_then_manual_id_skips_dcr() {
        let (store, _tmp) = test_store().await;
        let discovery_hits = Arc::new(AtomicUsize::new(0));
        let register_hits = Arc::new(AtomicUsize::new(0));
        // Slack shape: endpoints discoverable, registration closed, manifest
        // does not opt into dynamic-registration.
        let base = spawn_mock_vendor(false, discovery_hits.clone(), register_hits.clone());
        let auth = AuthSpec {
            kind: AuthKind::Oauth,
            resource: Some(format!("{base}/mcp")),
            ..Default::default()
        };
        let http = reqwest::Client::new();

        let result = resolve_plugin_install(&store, &http, "wiz-slack", Some(&auth))
            .await
            .unwrap();
        assert!(result.needs_client_id);
        assert!(!result.oauth_available);
        assert!(!result.dcr_succeeded);
        assert_eq!(register_hits.load(Ordering::SeqCst), 0);
        // Endpoints survive even though registration is impossible.
        let row = store
            .get_plugin_oauth_client("wiz-slack")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.token_url.as_deref(),
            Some(format!("{base}/token").as_str())
        );
        assert!(row.client_id.is_none());

        // Manual client id → re-begin goes straight to the browser flow.
        store
            .upsert_plugin_oauth_client(&PluginOauthClient {
                plugin_id: "wiz-slack".into(),
                authorize_url: None,
                token_url: None,
                client_id: Some("manual-client".into()),
            })
            .await
            .unwrap();
        let result = resolve_plugin_install(&store, &http, "wiz-slack", Some(&auth))
            .await
            .unwrap();
        assert!(result.oauth_available);
        assert!(!result.needs_client_id);
        assert!(result
            .oauth_begin
            .unwrap()
            .authorize_url
            .contains("client_id=manual-client"));
        assert_eq!(
            discovery_hits.load(Ordering::SeqCst),
            1,
            "cached endpoints reused"
        );
        assert_eq!(
            register_hits.load(Ordering::SeqCst),
            0,
            "DCR never attempted"
        );
    }

    #[tokio::test]
    async fn begin_discovery_failure_with_no_endpoints_reports_only_the_error() {
        let (store, _tmp) = test_store().await;
        // Bind then drop: requests to this port are refused.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let auth = AuthSpec {
            kind: AuthKind::Oauth,
            resource: Some(format!("{base}/mcp")),
            dynamic_registration: true,
            ..Default::default()
        };
        let http = reqwest::Client::new();
        let result = resolve_plugin_install(&store, &http, "wiz-down", Some(&auth))
            .await
            .unwrap();
        assert!(!result.oauth_available);
        assert!(
            !result.needs_client_id,
            "nothing to enter without endpoints"
        );
        assert!(result.dcr_error.is_some());
        assert!(result.oauth_begin.is_none());
    }

    #[tokio::test]
    async fn set_plugin_oauth_client_id_routes_external_to_auth_setting_and_others_to_the_row() {
        // Two synthetic shapes stand in for the removed catalog entries:
        // `acme-external` declares an `auth.setting`, so its client id IS that
        // setting; `acme-row` declares none, so it falls through to the
        // `plugin_oauth_clients` row.
        let cp = test_cp_with(vec![
            auth_connector(
                "acme-external",
                AuthKind::Oauth,
                Some("plugin.acme-external.client_id"),
            ),
            auth_connector_full("acme-row", AuthKind::Oauth, None, Some("https://acme/api")),
        ])
        .await;
        set_plugin_oauth_client_id(
            &cp,
            "acme-external".to_string(),
            " acme-client-1 ".to_string(),
        )
        .await
        .unwrap();
        assert_eq!(
            cp.store()
                .get_setting_raw("plugin.acme-external.client_id")
                .await
                .unwrap()
                .as_deref(),
            Some("acme-client-1"),
            "trimmed value stored under the declared auth.setting"
        );
        assert!(
            cp.store()
                .get_plugin_oauth_client("acme-external")
                .await
                .unwrap()
                .is_none(),
            "external plugins never write the row"
        );

        // A plugin with no auth.setting goes to plugin_oauth_clients.
        set_plugin_oauth_client_id(&cp, "acme-row".to_string(), "acme-row-client-1".to_string())
            .await
            .unwrap();
        let row = cp
            .store()
            .get_plugin_oauth_client("acme-row")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.client_id.as_deref(), Some("acme-row-client-1"));
        assert!(row.authorize_url.is_none());

        // Empty input is rejected.
        assert!(
            set_plugin_oauth_client_id(&cp, "acme-row".to_string(), "  ".to_string())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn drop_pending_plugin_flows_narrows_by_state_token_or_sweeps_the_plugin() {
        let insert = |token: &str| {
            plugin_oauth_flows()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .insert(
                    plugin_oauth_flow_key("wiz-cancel", token),
                    PluginOauthFlowState {
                        verifier: "v".into(),
                        redirect_uri: plugin_oauth_redirect_uri("wiz-cancel"),
                        requested_scopes: vec![],
                    },
                );
        };
        insert("s1");
        insert("s2");
        drop_pending_plugin_flows("wiz-cancel", Some("s1"));
        {
            let flows = plugin_oauth_flows()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            assert!(!flows.contains_key(&plugin_oauth_flow_key("wiz-cancel", "s1")));
            assert!(flows.contains_key(&plugin_oauth_flow_key("wiz-cancel", "s2")));
        }
        drop_pending_plugin_flows("wiz-cancel", None);
        let flows = plugin_oauth_flows()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        assert!(!flows.contains_key(&plugin_oauth_flow_key("wiz-cancel", "s2")));
    }

    #[tokio::test]
    async fn begin_plugin_install_rejects_component_source() {
        let cp = test_cp().await;
        let result = begin_plugin_install(&cp, "github".to_string()).await;
        match result {
            Err(err) => {
                assert!(
                    err.message.contains("oauth profiles"),
                    "error message must mention oauth profiles, got: {}",
                    err.message
                );
            }
            Ok(_) => panic!("expected begin_plugin_install to reject component plugin github"),
        }
    }

    #[tokio::test]
    async fn list_plugins_dispatches_as_array() {
        let s = state().await;
        let out = dispatch(&s, "list_plugins", json!({})).await.unwrap();
        assert!(out.is_array());
    }

    // ---------- component-plugin release management (Task 11a) ----------

    fn component_release(version: &str) -> crate::store::ComponentPluginReleaseRecord {
        crate::store::ComponentPluginReleaseRecord {
            plugin_id: "mimo".into(),
            version: version.into(),
            source_url: format!("https://feed.test/mimo/{version}"),
            sha256: "0".repeat(64),
            signing_key_id: "first-party".into(),
            installed_at: crate::paths::now_ms(),
            active: false,
            revoked: false,
            revocation_reason: None,
        }
    }

    #[tokio::test]
    async fn plugin_release_detail_lists_releases_and_active_version() {
        let cp = test_cp().await;
        for v in ["0.1.0", "0.2.0"] {
            cp.store()
                .upsert_component_release(&component_release(v))
                .await
                .unwrap();
        }
        cp.store()
            .set_active_component_release("mimo", "0.2.0")
            .await
            .unwrap();

        let detail = plugin_release_detail(&cp, "mimo").await.unwrap();
        assert_eq!(detail.plugin_id, "mimo");
        assert_eq!(detail.releases.len(), 2);
        assert_eq!(detail.active_version.as_deref(), Some("0.2.0"));
        assert!(detail
            .releases
            .iter()
            .any(|r| r.version == "0.2.0" && r.active));
        // Task 12: every release here was signed with the first-party test
        // fixture's key id ("first-party"), so `first_party` must be true for
        // all of them.
        assert!(detail.releases.iter().all(|r| r.first_party));
        // Task 12: no bundle is installed on disk in this test environment
        // (only the ledger row exists), so the manifest-derived permission
        // summary must be absent rather than guessed.
        assert!(detail.active_manifest.is_none());
    }

    // Task 12: a release signed by a key other than the first-party constant
    // must report `first_party: false` — the UI's publisher-verification
    // badge relies on this being computed server-side, never string-matched
    // client-side.
    #[tokio::test]
    async fn plugin_release_detail_marks_non_first_party_releases() {
        let cp = test_cp().await;
        let mut third_party = component_release("0.1.0");
        third_party.signing_key_id = "some-other-key".into();
        cp.store()
            .upsert_component_release(&third_party)
            .await
            .unwrap();

        let detail = plugin_release_detail(&cp, "mimo").await.unwrap();
        let release = detail.releases.first().unwrap();
        assert!(!release.first_party);
        assert_eq!(release.signing_key_id, "some-other-key");
    }

    // Task 12: a component id with no recorded releases at all (never
    // installed) must return an empty, well-formed detail rather than an
    // error — this is the shape Cockpit's PluginDetailView sees for a
    // never-installed component plugin.
    #[tokio::test]
    async fn plugin_release_detail_is_empty_for_a_never_installed_plugin() {
        let cp = test_cp().await;
        let detail = plugin_release_detail(&cp, "opencode").await.unwrap();
        assert_eq!(detail.plugin_id, "opencode");
        assert!(detail.releases.is_empty());
        assert!(detail.active_version.is_none());
        assert!(detail.active_manifest.is_none());
    }

    // PR-1 (pre-install metadata): a component-bundle id that has never been
    // installed must still carry its embedded first-party manifest as
    // `declared_manifest`, so the wizard's overview/permissions steps can
    // render tools and permissions before anything is fetched. `active_*`
    // stays None — "verified release on disk" semantics are untouched.
    #[tokio::test]
    async fn plugin_release_detail_carries_declared_manifest_pre_install() {
        let cp = test_cp().await;
        let res = plugin_release_detail(&cp, "github").await.unwrap();
        assert!(res.active_version.is_none());
        assert!(res.active_manifest.is_none());
        let declared = res
            .declared_manifest
            .expect("github has an embedded first-party bundle manifest");
        assert_eq!(declared.tools.len(), 12, "github declares exactly 12 tools");
        assert!(!declared.domains.is_empty());
        assert_eq!(declared.oauth_profiles.len(), 1);
        // Store enrichment ran against an empty store: not connected.
        assert!(!declared.oauth_profiles[0].connected);
    }

    // A non-component id (catalog provider, no embedded bundle) must not
    // grow a declared manifest.
    #[tokio::test]
    async fn plugin_release_detail_declared_manifest_absent_for_non_component() {
        let cp = test_cp().await;
        let res = plugin_release_detail(&cp, "kiro").await.unwrap();
        assert!(res.declared_manifest.is_none());
    }

    // The pure manifest -> `ComponentManifestInfo` conversion must carry the
    // device-flow profile fields (token_url, device_authorization_url) and mark
    // `client_id_configured` from a baked manifest client-id, leaving
    // `connected` false (it cannot see the store).
    #[test]
    fn component_manifest_from_carries_profile_urls_and_baked_client_id() {
        let manifest = ryuzi_plugin_sdk::PluginManifest::from_toml(
            r#"
contract = 2
id = "github"
name = "GitHub"
version = "0.1.0"

[component]
file = "github.wasm"
wit-api = "^0.1.0"
lifecycle = "per-call"

[[oauth]]
id = "github"
token-url = "https://github.com/login/oauth/access_token"
device-authorization-url = "https://github.com/login/device/code"
client-id = "Iv1.public"
"#,
        )
        .unwrap();
        let info = ComponentManifestInfo::from(manifest);
        let p = &info.oauth_profiles[0];
        assert_eq!(
            p.token_url.as_deref(),
            Some("https://github.com/login/oauth/access_token")
        );
        assert_eq!(
            p.device_authorization_url.as_deref(),
            Some("https://github.com/login/device/code")
        );
        assert!(
            p.client_id_configured,
            "baked manifest client-id => configured"
        );
        assert!(!p.connected, "pure From cannot see the store");
    }

    // The pure manifest -> `ComponentManifestInfo` conversion must carry the
    // declared tools (name, description, writes) from the bundle manifest.
    #[test]
    fn component_manifest_info_carries_tools() {
        let manifest = ryuzi_plugin_sdk::PluginManifest::from_toml(
            r#"
contract = 2
id = "github"
name = "GitHub"
version = "0.1.0"

[component]
file = "github.wasm"
wit-api = "^0.1.0"
lifecycle = "per-call"

[[tools]]
name = "create_issue"
description = "Open an issue"
writes = true
"#,
        )
        .unwrap();
        let info = ComponentManifestInfo::from(manifest);
        assert_eq!(info.tools.len(), 1);
        assert_eq!(info.tools[0].name, "create_issue");
        assert_eq!(info.tools[0].description, "Open an issue");
        assert!(info.tools[0].writes);
    }

    // Store enrichment: a usable stored token flips `connected`; a stored client
    // id flips `client_id_configured` for a profile the manifest did not bake
    // one into; a `reconnect_required` token is NOT connected; per-profile rows
    // never bleed across ids.
    #[tokio::test]
    async fn enrich_oauth_profile_status_reflects_stored_token_and_client() {
        let cp = test_cp().await;
        let store = cp.store();
        store
            .upsert_plugin_oauth_profile_token(
                "github",
                "github",
                &PluginOauthToken {
                    plugin_id: "github".into(),
                    access_token: "tok".into(),
                    refresh_token: None,
                    token_type: "Bearer".into(),
                    expires_at: None,
                    scopes: vec![],
                    reconnect_required: false,
                },
            )
            .await
            .unwrap();
        store
            .upsert_plugin_oauth_profile_client(&crate::store::PluginOauthProfileClient {
                plugin_id: "github".into(),
                profile_id: "custom".into(),
                authorize_url: None,
                token_url: None,
                client_id: Some("stored-id".into()),
                client_secret_setting: None,
            })
            .await
            .unwrap();

        let profile = |id: &str, baked: bool| ComponentOauthProfileInfo {
            id: id.to_string(),
            scopes: vec![],
            token_url: None,
            device_authorization_url: None,
            connected: false,
            authorize_url: None,
            client_id_configured: baked,
        };
        let mut manifest = ComponentManifestInfo {
            publisher: "Ryuzi".into(),
            description: String::new(),
            lifecycle: "per-call".into(),
            domains: vec![],
            oauth_profiles: vec![
                profile("github", true),
                profile("custom", false),
                profile("unset", false),
            ],
            tools: vec![],
        };

        enrich_oauth_profile_status(store, "github", &mut manifest).await;

        let github = &manifest.oauth_profiles[0];
        assert!(github.connected, "seeded token => connected");
        assert!(github.client_id_configured);
        let custom = &manifest.oauth_profiles[1];
        assert!(!custom.connected, "no token for this profile");
        assert!(custom.client_id_configured, "stored client => configured");
        let unset = &manifest.oauth_profiles[2];
        assert!(!unset.connected);
        assert!(
            !unset.client_id_configured,
            "no manifest baked-in and no stored client => not configured"
        );
    }

    // The `client_id_setting` fallback (PR-3) only applies to profiles the
    // installed bundle's declared manifest actually names one for. The
    // embedded `github` bundle bakes a first-party client id instead (no
    // `client-id-setting`), so this profile must stay unconfigured rather
    // than the new lookup panicking or false-positiving on a plugin id that
    // resolves via `component_catalog::declared_manifest`.
    #[tokio::test]
    async fn enrich_client_id_setting_fallback_leaves_unconfigured_when_none_declared() {
        let cp = test_cp().await;
        let store = cp.store();
        let mut manifest = ComponentManifestInfo {
            publisher: "Ryuzi".into(),
            description: String::new(),
            lifecycle: "per-call".into(),
            domains: vec![],
            oauth_profiles: vec![ComponentOauthProfileInfo {
                id: "github".into(),
                scopes: vec![],
                token_url: None,
                device_authorization_url: None,
                connected: false,
                authorize_url: None,
                client_id_configured: false,
            }],
            tools: vec![],
        };
        enrich_oauth_profile_status(store, "github", &mut manifest).await;
        assert!(
            !manifest.oauth_profiles[0].client_id_configured,
            "github's declared profile has no client_id_setting, so this must stay false"
        );
    }

    // T7-deferred positive case, now landed by Task 10: atlassian's embedded
    // bundle declares `client-id-setting = "plugin.atlassian.oauth_client_id"`
    // (see `component_catalog`'s `atlassian_profile_carries_pkce_extras_and_client_id_setting`).
    // A non-empty stored value at that key must flip `client_id_configured`
    // through `plugin_release_detail`/`enrich_oauth_profile_status`'s
    // `declared_manifest` fallback — the mirror image of
    // `enrich_client_id_setting_fallback_leaves_unconfigured_when_none_declared`.
    #[tokio::test]
    async fn enrich_client_id_setting_fallback_flips_configured_when_value_is_stored() {
        let cp = test_cp().await;
        let store = cp.store();
        store
            .set_setting_raw("plugin.atlassian.oauth_client_id", "abc123")
            .await
            .unwrap();
        let mut manifest = ComponentManifestInfo {
            publisher: "Ryuzi".into(),
            description: String::new(),
            lifecycle: "per-call".into(),
            domains: vec![],
            oauth_profiles: vec![ComponentOauthProfileInfo {
                id: "atlassian-cloud".into(),
                scopes: vec![],
                token_url: None,
                device_authorization_url: None,
                connected: false,
                authorize_url: None,
                client_id_configured: false,
            }],
            tools: vec![],
        };
        enrich_oauth_profile_status(store, "atlassian", &mut manifest).await;
        assert!(
            manifest.oauth_profiles[0].client_id_configured,
            "atlassian's declared client-id-setting with a stored value must flip client_id_configured"
        );
    }

    #[tokio::test]
    async fn enrich_marks_a_reconnect_required_token_as_not_connected() {
        let cp = test_cp().await;
        let store = cp.store();
        store
            .upsert_plugin_oauth_profile_token(
                "github",
                "github",
                &PluginOauthToken {
                    plugin_id: "github".into(),
                    access_token: "tok".into(),
                    refresh_token: None,
                    token_type: "Bearer".into(),
                    expires_at: None,
                    scopes: vec![],
                    reconnect_required: true,
                },
            )
            .await
            .unwrap();
        let mut manifest = ComponentManifestInfo {
            publisher: "Ryuzi".into(),
            description: String::new(),
            lifecycle: "per-call".into(),
            domains: vec![],
            oauth_profiles: vec![ComponentOauthProfileInfo {
                id: "github".into(),
                scopes: vec![],
                token_url: None,
                device_authorization_url: None,
                connected: false,
                authorize_url: None,
                client_id_configured: true,
            }],
            tools: vec![],
        };
        enrich_oauth_profile_status(store, "github", &mut manifest).await;
        assert!(
            !manifest.oauth_profiles[0].connected,
            "a reconnect_required token must not read as connected"
        );
    }

    #[tokio::test]
    async fn rollback_component_plugin_revokes_bad_and_reactivates_prior_good() {
        let cp = test_cp().await;
        for v in ["0.1.0", "0.2.0"] {
            cp.store()
                .upsert_component_release(&component_release(v))
                .await
                .unwrap();
        }
        cp.store()
            .set_active_component_release("mimo", "0.2.0")
            .await
            .unwrap();

        let detail = rollback_component_plugin(&cp, "mimo", "0.2.0", "0.1.0")
            .await
            .unwrap();
        assert_eq!(detail.active_version.as_deref(), Some("0.1.0"));
        let bad = detail
            .releases
            .iter()
            .find(|r| r.version == "0.2.0")
            .unwrap();
        assert!(bad.revoked, "the bad version must be revoked");
        assert!(!bad.active);
        // Spec B1 granular latch: "mimo" is a provider (`derive_kind` sees
        // `manifest.provider.is_some()`), so its rollback hot-swaps the WASM
        // transport instead of latching a restart — see the identical
        // provider/integration match in `install_component_plugin`.
        assert!(
            !cp.plugins_restart_required(),
            "a provider rollback hot-swaps the transport — must not latch"
        );
    }

    // A gateway rollback is the conservative counterpart to the provider case
    // above: the Router's gateway map is built once at startup, so rolling a
    // gateway bundle back still needs the restart latch.
    #[tokio::test]
    async fn rollback_gateway_still_latches_restart() {
        let cp = test_cp_with(vec![gateway_only("discord")]).await;
        for v in ["0.1.0", "0.2.0"] {
            cp.store()
                .upsert_component_release(&crate::store::ComponentPluginReleaseRecord {
                    plugin_id: "discord".into(),
                    ..component_release(v)
                })
                .await
                .unwrap();
        }
        cp.store()
            .set_active_component_release("discord", "0.2.0")
            .await
            .unwrap();

        assert!(!cp.plugins_restart_required());
        rollback_component_plugin(&cp, "discord", "0.2.0", "0.1.0")
            .await
            .unwrap();
        assert!(
            cp.plugins_restart_required(),
            "gateway rollback still needs a restart"
        );
    }

    // IMP-1: rollback whose target does NOT exist must be a clean no-op — the
    // bad version stays ACTIVE and un-revoked, never leaving the plugin with no
    // active release despite the RPC reporting failure.
    #[tokio::test]
    async fn rollback_is_a_no_op_when_target_version_is_missing() {
        let cp = test_cp().await;
        cp.store()
            .upsert_component_release(&component_release("0.2.0"))
            .await
            .unwrap();
        cp.store()
            .set_active_component_release("mimo", "0.2.0")
            .await
            .unwrap();

        match rollback_component_plugin(&cp, "mimo", "0.2.0", "9.9.9").await {
            Ok(_) => panic!("rollback to a missing target version must fail"),
            Err(err) => assert!(
                err.to_string().contains("no component release"),
                "unexpected error: {err}"
            ),
        }
        let active = cp
            .store()
            .active_component_release("mimo")
            .await
            .unwrap()
            .expect("the bad version must remain active after a failed rollback");
        assert_eq!(active.version, "0.2.0");
        assert!(
            !active.revoked,
            "the bad version must not have been revoked"
        );
    }

    // IMP-1: rollback to a REVOKED target is likewise a clean no-op.
    #[tokio::test]
    async fn rollback_is_a_no_op_when_target_version_is_revoked() {
        let cp = test_cp().await;
        for v in ["0.1.0", "0.2.0"] {
            cp.store()
                .upsert_component_release(&component_release(v))
                .await
                .unwrap();
        }
        cp.store()
            .set_active_component_release("mimo", "0.2.0")
            .await
            .unwrap();
        cp.store()
            .mark_component_release_revoked("mimo", "0.1.0", "bad")
            .await
            .unwrap();

        match rollback_component_plugin(&cp, "mimo", "0.2.0", "0.1.0").await {
            Ok(_) => panic!("rollback to a revoked target version must fail"),
            Err(err) => assert!(
                err.to_string().contains("revoked"),
                "unexpected error: {err}"
            ),
        }
        let active = cp
            .store()
            .active_component_release("mimo")
            .await
            .unwrap()
            .expect("the bad version must remain active after a failed rollback");
        assert_eq!(active.version, "0.2.0");
        assert!(
            !active.revoked,
            "the bad version must not have been revoked on a failed rollback"
        );
    }

    // Key-state-agnostic: with no first-party signing key compiled in this
    // must refuse with the "disabled until" message BEFORE any network I/O
    // (the fail-closed guard `install_component_plugin` opens with); with a
    // real key compiled in that guard is bypassed and the call proceeds to
    // resolve+download, which we point at a closed local port so the
    // assertion stays hermetic (no live network dependency, no dependence on
    // a real "mimo" release existing) rather than reasserting the "no key"
    // message the live-key build can no longer produce.
    #[tokio::test]
    async fn install_component_plugin_is_fail_closed_without_a_signing_key() {
        let cp = test_cp().await;
        cp.store()
            .set_setting_raw("component_release_base_url", "http://127.0.0.1:1")
            .await
            .unwrap();
        match install_component_plugin(&cp, "mimo", None).await {
            Ok(_) => panic!(
                "expected mimo install to fail (no signing key, or nothing listening on the closed port)"
            ),
            Err(err) => {
                if crate::plugins::first_party_key::FIRST_PARTY_PUBKEY == [0u8; 32] {
                    assert!(
                        err.to_string().contains("disabled until"),
                        "unexpected error: {err}"
                    );
                } else {
                    assert!(
                        !err.to_string().contains("disabled until"),
                        "a real signing key must bypass the fail-closed guard: {err}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn component_bootstrap_status_reports_pending_retry_until_completed() {
        let cp = test_cp().await;
        assert!(!component_bootstrap_status(&cp).await.unwrap().pending);

        cp.store()
            .set_setting_raw(
                crate::plugins::remote_catalog::FIRST_PARTY_BOOTSTRAP_RETRY,
                "download failed",
            )
            .await
            .unwrap();
        let pending = component_bootstrap_status(&cp).await.unwrap();
        assert!(pending.pending);
        assert_eq!(pending.message.as_deref(), Some("download failed"));

        // Completion clears the pending state even if the retry row lingers.
        cp.store()
            .set_setting_raw(
                crate::plugins::remote_catalog::FIRST_PARTY_BOOTSTRAP_MARKER,
                "1",
            )
            .await
            .unwrap();
        assert!(!component_bootstrap_status(&cp).await.unwrap().pending);
    }

    #[tokio::test]
    async fn component_bootstrap_status_dispatches() {
        let s = state().await;
        let out = dispatch(&s, "component_bootstrap_status", json!({}))
            .await
            .unwrap();
        assert_eq!(out["pending"], json!(false));
    }

    // ---------- plugin_tools (Task 4) ----------

    // No registry, no plugins — `plugin_tools`' initial existence gate must
    // reject an id `cp.plugins()` has never heard of, with the exact same
    // error `plugin_detail` (`assemble_detail`) gives that same id, so the
    // two RPCs never disagree about what "known" means.
    #[tokio::test]
    async fn plugin_tools_unknown_id_errors() {
        let cp = test_cp().await;
        match plugin_tools(&cp, "nope").await {
            Ok(_) => panic!("expected an error for an unknown plugin id"),
            Err(e) => assert_eq!(e.message, "unknown plugin: nope"),
        }
    }

    // `install_builtins` registers every embedded component bundle
    // (`component_catalog::component_catalog_plugins`), github included, with
    // no release ever installed — so this must fall through to branch 1's
    // embedded-manifest fallback.
    #[tokio::test]
    async fn plugin_tools_falls_back_to_declared_manifest_tools() {
        let cp = test_cp().await;
        let res = plugin_tools(&cp, "github").await.unwrap();
        assert!(!res.live);
        assert_eq!(res.entries.len(), 12, "github declares exactly 12 tools");
        assert!(res.entries.iter().all(|e| e.kind == "tool"));
        assert!(res.entries.iter().all(|e| e.writes.is_some()));
    }

    // Discord is a gateway component that declares zero agent-facing tools
    // (Task 1) — a known, component-backed id with nothing to list must
    // still resolve to an empty, well-formed result, never an error.
    #[tokio::test]
    async fn plugin_tools_discord_component_has_no_declared_tools() {
        let cp = test_cp().await;
        let res = plugin_tools(&cp, "discord").await.unwrap();
        assert!(!res.live);
        assert!(res.entries.is_empty());
    }

    // "kiro" is a CATALOG provider with seeded models and no bundle/component
    // backing (`is_component_bundle("kiro")` is false) — branch 4 must surface
    // its effective model list, the same one `plugin_models` returns.
    #[tokio::test]
    async fn plugin_tools_provider_lists_models() {
        let cp = test_cp().await;
        let models = providers::list_models(cp.store(), "kiro").await.unwrap();
        assert!(!models.is_empty(), "fixture assumption: kiro seeds models");
        let res = plugin_tools(&cp, "kiro").await.unwrap();
        assert!(!res.live);
        assert_eq!(res.entries.len(), models.len());
        assert!(res.entries.iter().all(|e| e.kind == "model"));
        assert!(res.entries.iter().all(|e| e.writes.is_none()));
        let names: Vec<&str> = res.entries.iter().map(|e| e.name.as_str()).collect();
        for model in &models {
            assert!(names.contains(&model.as_str()));
        }
    }

    // "mimo" is the exact regression this test guards: `is_component_bundle`
    // is true (it's an embedded first-party bundle, see
    // `component_catalog::COMPONENT_BUNDLE_MANIFESTS`) AND it's a
    // component-BACKED PROVIDER (`install_providers` registers it — its bundle
    // id also sits in `llm_router::registry::CATALOG` — so its `CorePlugin`
    // has `manifest.provider.is_some()`), AND its embedded bundle manifest
    // declares zero `[[tools]]`. Before this fix, branch 2 returned that empty
    // tool list and never reached branch 4, hiding the provider's models from
    // the Tools & Skills surface entirely. Also re-asserts the two contracts
    // this fix must NOT disturb: github (non-empty declared tools) still
    // resolves at branch 2 with exactly its 12 declared tools, and discord (a
    // gateway component with zero declared tools and no provider capability)
    // still falls through to an empty, well-formed result rather than an
    // error.
    #[tokio::test]
    async fn plugin_tools_component_backed_provider_lists_models() {
        let cp = test_cp().await;

        // Precondition: mimo really is component-backed with nothing declared.
        assert!(crate::plugins::component_catalog::is_component_bundle(
            "mimo"
        ));
        assert!(crate::plugins::component_catalog::declared_tools("mimo").is_empty());
        let plugin = cp.plugins().get("mimo").expect("mimo registered");
        assert!(
            plugin.manifest.provider.is_some(),
            "mimo must be registered as a provider plugin, not a bare component"
        );

        let models = providers::list_models(cp.store(), "mimo").await.unwrap();
        assert!(!models.is_empty(), "fixture assumption: mimo seeds models");

        let res = plugin_tools(&cp, "mimo").await.unwrap();
        assert!(!res.live);
        assert_eq!(res.entries.len(), models.len());
        assert!(res.entries.iter().all(|e| e.kind == "model"));
        assert!(res.entries.iter().all(|e| e.writes.is_none()));
        let names: Vec<&str> = res.entries.iter().map(|e| e.name.as_str()).collect();
        for model in &models {
            assert!(names.contains(&model.as_str()));
        }

        // github: unchanged — non-empty declared tools still short-circuit at
        // branch 2, with exactly its 12 declared tools.
        let github = plugin_tools(&cp, "github").await.unwrap();
        assert!(!github.live);
        assert_eq!(github.entries.len(), 12);
        assert!(github.entries.iter().all(|e| e.kind == "tool"));

        // discord: unchanged — zero declared tools, no provider capability,
        // still resolves to an empty, well-formed result via the fallthrough.
        let discord = plugin_tools(&cp, "discord").await.unwrap();
        assert!(!discord.live);
        assert!(discord.entries.is_empty());
    }

    /// Stages a REAL, `load_active_bundles`-verifiable component bundle
    /// directly at the production [`crate::plugins::bundle::installed_bundle_root`]
    /// path — the exact root `declared_component_tool_entries` reads
    /// (`plugins_api.rs`'s step-2 tool source). Unlike `doctor.rs`'s
    /// `WasmComponentDoctorInputs::bundle_root`, `declared_component_tool_entries`
    /// has no injected-root test seam, so pinning its "installed manifest wins
    /// over embedded" precedence has no choice but to write the real per-user
    /// install root. Mirrors `doctor.rs`'s `wasm_component_findings::write_bundle`
    /// fixture (same signed-envelope shape) rather than inventing a new signing
    /// path; the signature itself is inert for THIS test (`load_active_bundles`
    /// never reads `plugin.sig` — only `verify_bundle`, a different call path,
    /// does), but it is staged anyway to keep the fixture a faithful "real
    /// installed bundle".
    ///
    /// `installed_bundle_root()` is process-global (same per-user path every
    /// test run resolves to — see `StateDirGuard` in `daemon.rs`/`control/tests.rs`
    /// for the same-shaped precedent), so every test using this fixture must be
    /// `#[serial]`. It also NEVER hijacks a plugin id that is already installed
    /// for real on this machine: `stage` refuses (returning `None`, skip-style)
    /// rather than repointing a real install's `current` pointer at placeholder
    /// bytes, since a SIGKILL mid-test could otherwise strand the real install.
    struct InstalledBundleFixture {
        plugin_root: std::path::PathBuf,
    }

    impl InstalledBundleFixture {
        /// Stage `plugin_id`@`version` declaring exactly one tool named
        /// `tool_name`. Returns `Some((fixture, component_sha256))` — the
        /// caller still owns recording + activating the release in the store
        /// (this only touches disk, matching `write_bundle` + `seed_active`'s
        /// split in `doctor.rs`) — or `None` if `plugin_id` already has a real
        /// install at this root, in which case nothing on disk is touched and
        /// the caller must skip the test rather than proceed.
        fn stage(plugin_id: &str, version: &str, tool_name: &str) -> Option<(Self, String)> {
            use base64::Engine as _;
            use ed25519_dalek::{Signer, SigningKey};
            use sha2::{Digest, Sha256};

            let root = crate::plugins::bundle::installed_bundle_root();
            let plugin_root = root.join(plugin_id);
            if plugin_root.exists() {
                // A real install already lives here — refuse to touch it.
                eprintln!(
                    "skipping plugin_tools_prefers_installed_manifest_over_embedded: \
                     a real install already exists at {} — refusing to hijack it",
                    plugin_root.display()
                );
                return None;
            }
            let pointer_path = plugin_root.join("current");
            let version_dir = plugin_root.join(version);
            std::fs::create_dir_all(&version_dir)
                .expect("create installed-bundle fixture version dir");

            let component_bytes = b"plugin_tools installed-manifest-precedence fixture component";
            std::fs::write(version_dir.join("plugin.wasm"), component_bytes)
                .expect("write fixture component");
            let sha = format!("{:x}", Sha256::digest(component_bytes));

            let manifest_toml = format!(
                "contract = 2\nid = \"{plugin_id}\"\nname = \"{plugin_id}\"\nversion = \"{version}\"\n\n[component]\nfile = \"plugin.wasm\"\nwit-api = \"^0.1.0\"\nlifecycle = \"singleton\"\n\n[[tools]]\nname = \"{tool_name}\"\ndescription = \"Only present in the installed release.\"\n"
            );
            std::fs::write(version_dir.join("ryuzi-plugin.toml"), &manifest_toml)
                .expect("write fixture manifest");

            let release_bytes = format!(
                "{{\"id\":\"{plugin_id}\",\"version\":\"{version}\",\"wit-api\":\"0.1.0\",\"component_url\":\"https://registry.example.test/{plugin_id}/{version}/plugin.wasm\",\"component_sha256\":\"{sha}\"}}"
            )
            .into_bytes();
            std::fs::write(version_dir.join("release.json"), &release_bytes)
                .expect("write fixture release.json");

            let key = SigningKey::from_bytes(&[9u8; 32]);
            let signature = key.sign(&release_bytes);
            let envelope = serde_json::json!({
                "key_id": "first-party",
                "signature": base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(signature.to_bytes()),
            });
            std::fs::write(
                version_dir.join("plugin.sig"),
                serde_json::to_vec(&envelope).unwrap(),
            )
            .expect("write fixture signature envelope");

            std::fs::write(&pointer_path, version).expect("write fixture current pointer");

            Some((Self { plugin_root }, sha))
        }
    }

    impl Drop for InstalledBundleFixture {
        fn drop(&mut self) {
            // `stage`'s guard above guarantees `plugin_root` did NOT exist
            // before this fixture created it, so it's always safe (and
            // sufficient) to remove the whole thing on the way out.
            // Best-effort: a cleanup failure must never mask (or panic over)
            // the test's own assertion outcome.
            let _ = std::fs::remove_dir_all(&self.plugin_root);
        }
    }

    // Task 4 follow-up (review finding): pins branch 2's "prefer the
    // currently-installed release's on-disk manifest over the embedded one"
    // precedence in `declared_component_tool_entries` — a brief-mandated
    // behavior (post-install tool listing must reflect the running release)
    // that was previously unexercised by any test, resting entirely on manual
    // mirroring of `plugin_release_detail`'s own active-release-then-disk
    // read. Stages a real signed bundle for `github` — an embedded id whose
    // baked-in manifest declares 12 tools including `auth_status` — at an
    // ACTIVE installed release declaring a totally different single tool
    // (`installed_only_tool`), then asserts `plugin_tools` returns the
    // installed manifest's tool, not the embedded one's.
    //
    // `InstalledBundleFixture` writes the real, process-global
    // `installed_bundle_root()` (see its doc) — `#[serial]` per this crate's
    // convention for any test touching a process-global resource (mirrors
    // `daemon.rs`'s/`control/tests.rs`'s `StateDirGuard` tests).
    #[tokio::test]
    #[serial]
    async fn plugin_tools_prefers_installed_manifest_over_embedded() {
        let cp = test_cp().await;

        // Fixture premise: github's EMBEDDED manifest has `auth_status` among
        // its 12 tools and no `installed_only_tool` — if this ever stops
        // being true the installed-vs-embedded divergence this test relies on
        // is gone, and it must be revisited rather than silently pass.
        let embedded = crate::plugins::component_catalog::declared_tools("github");
        assert!(embedded.iter().any(|t| t.name == "auth_status"));
        assert!(!embedded.iter().any(|t| t.name == "installed_only_tool"));

        let Some((_fixture, sha)) = InstalledBundleFixture::stage(
            "github",
            "9.9.9-installed-fixture",
            "installed_only_tool",
        ) else {
            // A real `github` component install already exists on this
            // machine — `stage` already printed why, and it refused to touch
            // it. Skip rather than assert anything.
            return;
        };
        cp.store()
            .upsert_component_release(&crate::store::ComponentPluginReleaseRecord {
                plugin_id: "github".into(),
                version: "9.9.9-installed-fixture".into(),
                source_url: "https://registry.example.test/github/9.9.9-installed-fixture".into(),
                sha256: sha,
                signing_key_id: "first-party".into(),
                installed_at: crate::paths::now_ms(),
                active: false,
                revoked: false,
                revocation_reason: None,
            })
            .await
            .unwrap();
        cp.store()
            .set_active_component_release("github", "9.9.9-installed-fixture")
            .await
            .unwrap();

        let res = plugin_tools(&cp, "github").await.unwrap();
        assert!(!res.live);
        let names: Vec<&str> = res.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["installed_only_tool"],
            "the active installed release's manifest must win over the embedded one, \
             got {names:?}"
        );
    }

    /// Like `api::tests_support::state`, but with `install_builtins` run
    /// against the `Registries` first (that helper deliberately starts from
    /// an empty registry — see its own doc — so a dispatch test that needs a
    /// real component/provider id, like `plugin_tools`' camelCase wire
    /// check below, builds its own `ApiState` this way instead).
    async fn state_with_builtins() -> ApiState {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(crate::store::Store::open(tmp.path()).await.unwrap());
        let mut regs = Registries::new();
        regs.add_plugin(crate::harness::native::native_plugin());
        crate::plugins::install_builtins(&mut regs);
        let persistence = crate::agents::bootstrap::AgentPersistence::temporary(Arc::clone(&store))
            .await
            .unwrap();
        let cp = ControlPlane::new(store, regs, persistence.clone()).await;
        std::mem::forget(tmp);
        ApiState {
            router_server: Arc::new(crate::llm_router::server::RouterServer::new(
                cp.store().clone(),
            )),
            cp,
            agents: persistence.registry,
            agent_knowledge: persistence.knowledge,
            learning_queue: persistence.learning,
            control_token: "t".into(),
        }
    }

    // Wire-level check: dispatch decodes the RPC's snake_case `plugin_id`
    // param (matching every other handler in this family — see
    // `plugins_cmd.rs`'s proxies, which all send `{ "plugin_id": ... }`) and
    // encodes the camelCase response DTO on the way out.
    #[tokio::test]
    async fn plugin_tools_dispatches_and_encodes_camel_case() {
        let s = state_with_builtins().await;
        let out = dispatch(&s, "plugin_tools", json!({ "plugin_id": "github" }))
            .await
            .unwrap();
        assert_eq!(out["pluginId"], json!("github"));
        assert_eq!(out["live"], json!(false));
        assert!(out["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| { e.get("name").is_some() && e.get("kind") == Some(&json!("tool")) }));
    }

    // `SkillPackFixture` and its `plugin_tools_skill_pack_lists_skills` test
    // were deleted here: both exercised `plugins::load_skill_pack_plugins_from`
    // registering a disk-sourced `CorePlugin` (`PluginSource::SkillPack`) for
    // `plugin_tools`' step-3 skill-pack branch. That loader was deleted in
    // this same v2 manifest migration — full plugin-folder installs are
    // deferred to a later task ("Task 11") — so the scenario is categorically
    // impossible for now. `plugin_tools`' step 3
    // (`skills_install::get_installed_skill_pack`) itself is untouched and
    // still covered elsewhere (e.g. `InstalledCuratedPackFixture`-backed
    // tests), since that ledger is independent of the deleted loader.
}
