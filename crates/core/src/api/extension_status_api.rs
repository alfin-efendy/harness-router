//! `extension_status` rpc (Track D observability — DT8): a read-only,
//! params-free snapshot of every extension (code plugin) the daemon's
//! `ExtensionHost` currently knows about, mirroring
//! `remote_catalog_api::catalog_status`'s own params-free-status-snapshot
//! shape and `plugins::doctor::plugin_doctor`'s "only report on host state
//! that's actually there" emptiness discipline (see both for the pattern
//! this reuses).
//!
//! Never mutates anything: no spawn, no restart, no shutdown. Cockpit's
//! `PluginDetailView` calls this (via the `extension_status` Tauri thin
//! command) to render an extension-capable plugin's live state, restart
//! count, and sanitized last error.

use super::{ok, ApiError};
use crate::api::types::ExtensionStatusEntry;
use crate::control::ControlPlane;
use crate::plugins::extension::ExtensionStatus;
use crate::serve::ApiState;
use crate::settings::SettingsStore;
use serde_json::Value;

pub(crate) const HANDLES: &[&str] = &["extension_status"];

pub(crate) async fn dispatch(state: &ApiState, method: &str, _p: Value) -> Result<Value, ApiError> {
    let cp = &state.cp;
    match method {
        "extension_status" => ok(extension_status(cp).await?),
        _ => Err(ApiError::not_found(format!("unknown method: {method}"))),
    }
}

/// One entry per spawned extension, across every enabled extension-capable
/// plugin, plus a synthetic `not-running` entry for an enabled
/// extension-capable plugin the host has no spawned entry for at all — same
/// enumeration `plugins::doctor::plugin_doctor` uses (see its "Extension...
/// health" section), just projected into the full status DTO instead of only
/// the unhealthy branches. Gated on `ExtensionHost::is_empty()` the same way:
/// a control plane that never spawned anything (every test `ControlPlane`,
/// or a process that isn't the daemon's spawn host) reports an empty list
/// rather than a `not-running` entry per enabled extension plugin — an
/// unspawned host is not evidence any specific extension failed.
async fn extension_status(cp: &ControlPlane) -> anyhow::Result<Vec<ExtensionStatusEntry>> {
    let mut out = Vec::new();
    if cp.extension_host().is_empty().await {
        return Ok(out);
    }
    let settings = SettingsStore::new(cp.store().clone());
    for plugin in cp.plugins().list() {
        // `CorePlugin.extension` no longer exists — the v2 SDK manifest has
        // no `[[extension]]` surface, so no plugin can ever be
        // extension-capable anymore. This always skips every plugin (the RPC
        // always returns an empty list) pending Task 3's full deletion of
        // Track D subprocess extensions.
        if true {
            continue;
        }
        let id = &plugin.manifest.id;
        if !cp
            .plugins()
            .is_enabled(&settings, id)
            .await
            .unwrap_or(false)
        {
            continue;
        }
        let snapshots = cp.extension_host().get(id).await;
        if snapshots.is_empty() {
            out.push(ExtensionStatusEntry {
                plugin_id: id.clone(),
                name: plugin.manifest.name.clone(),
                status: "not-running".to_string(),
                restart_count: 0,
                last_error: None,
                confirmed_events: Vec::new(),
                tool_count: 0,
            });
            continue;
        }
        for snap in snapshots {
            let (status, last_error) = match &snap.status {
                ExtensionStatus::Starting => ("starting".to_string(), None),
                ExtensionStatus::Running => ("running".to_string(), None),
                ExtensionStatus::Restarting => ("restarting".to_string(), None),
                ExtensionStatus::Stopped => ("stopped".to_string(), None),
                ExtensionStatus::Failed(reason) => ("failed".to_string(), Some(reason.clone())),
            };
            out.push(ExtensionStatusEntry {
                plugin_id: id.clone(),
                name: snap.name,
                status,
                restart_count: snap.restart_count,
                last_error,
                confirmed_events: snap.confirmed_events,
                tool_count: snap.tools.len() as u32,
            });
        }
    }
    out.sort_by(|a, b| {
        (a.plugin_id.as_str(), a.name.as_str()).cmp(&(b.plugin_id.as_str(), b.name.as_str()))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::api::{dispatch, ApiState};
    use crate::plugins::Registries;
    use serde_json::json;
    use std::sync::Arc;

    /// Like `api::tests_support::state`, but seeded with `plugins` at
    /// `ControlPlane::new` time (that function doesn't take a `Registries`
    /// param, so every extension_status test needing a real extension-
    /// capable `CorePlugin` builds its own `ApiState` this way rather than
    /// reaching for a Registries mutator that doesn't exist post-construction).
    async fn state_with_plugins(plugins: Vec<crate::plugins::CorePlugin>) -> ApiState {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(crate::store::Store::open(tmp.path()).await.unwrap());
        let mut regs = Registries::new();
        for plugin in plugins {
            regs.add_plugin(plugin);
        }
        let config = tempfile::tempdir().unwrap();
        let persistence = crate::agents::bootstrap::initialize_agent_persistence(
            config.path().to_path_buf(),
            Arc::clone(&store),
        )
        .await
        .unwrap();
        let cp = crate::control::ControlPlane::new(store, regs, persistence.clone()).await;
        std::mem::forget(config);
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

    // NOTE: `NoopExtensionFactory`/`extension_plugin`/`fake_spec` and the 3
    // tests that used them (`reports_a_running_entry_with_confirmed_events_and_tool_count`,
    // `a_failed_entrys_last_error_is_the_sanitized_reason_never_a_raw_secret`,
    // `an_enabled_extension_plugin_with_nothing_spawned_reports_not_running_when_the_host_is_otherwise_active`)
    // were deleted here: each proved an extension-capable plugin (built via
    // `extension_plugin`'s `extension: Some(...)`) shows up in this RPC's
    // results. `CorePlugin.extension` no longer exists (the v2 SDK manifest
    // has no `[[extension]]` surface), the handler above now always skips
    // every plugin, and that behavior is categorically impossible now —
    // pending Task 3's full deletion of Track D subprocess extensions.

    #[tokio::test]
    async fn empty_host_reports_an_empty_list() {
        let s = state_with_plugins(vec![]).await;
        let out = dispatch(&s, "extension_status", json!({})).await.unwrap();
        assert_eq!(out, json!([]));
    }
}
