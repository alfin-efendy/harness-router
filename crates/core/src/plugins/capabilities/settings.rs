//! `ryuzi:settings/settings` host adapter: a plugin's own `plugin.<id>.*`
//! settings slice.
//!
//! # Scoping convention
//! The WIT `key` a component passes is always a **bare field name** (e.g.
//! `"token"`, `"region"`) — never a fully-qualified settings key. The
//! effective key this adapter reads/writes is always computed as
//! `format!("plugin.{}.{}", ctx.plugin_id, key)`.
//!
//! A caller cannot smuggle a fully-qualified key (e.g.
//! `"plugin.atlassian.token"`) to reach another plugin's settings: any bare
//! key that already starts with `"plugin."` is rejected with
//! [`SettingsErr::Invalid`] before it is ever concatenated. Combined with the
//! prefix always being `ctx.plugin_id` (which the host — not the guest —
//! controls), cross-plugin settings access is structurally impossible, not
//! just policy-denied.

use super::PluginCapabilityContext;

/// A capability-adapter-local error, mapped to the generated WIT
/// `settings::SettingsError` by the runtime's `Host` trait impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsErr {
    NotFound,
    Invalid(String),
    Unavailable,
}

/// A plugin's scoped view of the shared settings store — see the module doc
/// for the scoping convention this enforces.
pub struct ScopedSettings<'a> {
    pub ctx: &'a PluginCapabilityContext,
}

impl<'a> ScopedSettings<'a> {
    pub fn new(ctx: &'a PluginCapabilityContext) -> Self {
        Self { ctx }
    }

    /// Reject a WIT `key` that is already fully-qualified (starts with
    /// `"plugin."`) — see the module doc. On success, returns the effective
    /// key: `plugin.<plugin_id>.<key>`.
    ///
    /// I2 fix: also rejects the host-owned control keys `enabled` and
    /// `trusted`. `host::register_plugin_fields` always registers
    /// `plugin.<id>.enabled`/`plugin.<id>.trusted` in `PLUGIN_FIELDS` for
    /// EVERY plugin (so the settings-tab schema recognizes them) — which
    /// means, absent this check, a WASM guest could call
    /// `settings.set("enabled", "true")` to silently undo a user's disable,
    /// or `settings.set("trusted", "true")` to grant itself the Task 11
    /// trust gate it was never given. Neither key is ever declared in a
    /// manifest's own `[[settings]]`, so a legitimate plugin has no reason
    /// to read or write either through this capability — every real write
    /// site for both is host-only (`crate::plugins::toggle_enabled`,
    /// `install_sources::confirm_plugin_install`).
    fn effective_key(&self, key: &str) -> Result<String, SettingsErr> {
        if key.starts_with("plugin.") {
            return Err(SettingsErr::Invalid(
                "settings key must be a bare field name".to_string(),
            ));
        }
        if key == "enabled" || key == "trusted" {
            return Err(SettingsErr::Invalid(format!(
                "{key} is a host-owned control key and cannot be read or written by a plugin"
            )));
        }
        let effective = format!("plugin.{}.{}", self.ctx.plugin_id, key);
        if crate::plugins::plugin_field(&effective).is_none() {
            return Err(SettingsErr::Invalid(format!(
                "undeclared plugin setting: {effective}"
            )));
        }
        Ok(effective)
    }

    /// Returns `(effective key, value, secret)`. A component may read back
    /// the REAL value of its own declared secret fields (PR-3) — the
    /// credential exists FOR this component (declared in its own signed
    /// manifest), and it needs it to build provider payloads itself (e.g.
    /// discord's gateway IDENTIFY, which the component — not the host —
    /// assembles). `effective_key`'s own-plugin+declared scoping already
    /// makes another plugin's secrets structurally unreachable (see the
    /// module doc), and reachability of THIS plugin's own secret is further
    /// bounded by the manifest's signed network allowlist: whatever the
    /// component does with the value, it can only ever send it to the hosts
    /// its manifest declared. `secret` still comes back `true` so a UI keeps
    /// masking the field.
    pub async fn get(&self, key: &str) -> Result<(String, String, bool), SettingsErr> {
        let effective = self.effective_key(key)?;
        let secret = crate::settings::is_secret(&effective);
        match self.ctx.settings.get(&effective).await {
            Ok(Some(value)) => Ok((effective, value, secret)),
            Ok(None) => Err(SettingsErr::NotFound),
            Err(_) => Err(SettingsErr::Unavailable),
        }
    }

    /// Returns `(effective key, secret)` on success.
    pub async fn set(&self, key: &str, value: &str) -> Result<(String, bool), SettingsErr> {
        let effective = self.effective_key(key)?;
        self.ctx
            .settings
            .set(&effective, value)
            .await
            .map_err(|error| SettingsErr::Invalid(error.to_string()))?;
        Ok((effective.clone(), crate::settings::is_secret(&effective)))
    }

    /// Returns whether a row existed and was removed.
    pub async fn remove(&self, key: &str) -> Result<bool, SettingsErr> {
        let effective = self.effective_key(key)?;
        let existed = self
            .ctx
            .store
            .get_setting_raw(&effective)
            .await
            .map_err(|_| SettingsErr::Unavailable)?
            .is_some();
        self.ctx
            .store
            .delete_setting_raw(&effective)
            .await
            .map_err(|_| SettingsErr::Unavailable)?;
        Ok(existed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{CorePlugin, PluginHost, PluginSource};
    use crate::settings::SettingsStore;
    use crate::store::Store;
    use crate::telemetry::NoopTelemetry;
    use ryuzi_plugin_sdk::{AuthKind, AuthSpec, FieldKind, PluginManifest, SettingField};
    use std::sync::Arc;

    async fn open_test_store() -> (Store, tempfile::NamedTempFile) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        (store, tmp)
    }

    /// Registers a plugin with a plain `region` field (not secret) and an
    /// auth-backed `token` field (always secret) in the process-wide
    /// `plugin_field` registry — mirrors `settings::store`'s
    /// `register_test_plugin` helper.
    fn register_test_plugin(id: &str) {
        let manifest = PluginManifest {
            contract: ryuzi_plugin_sdk::CONTRACT_VERSION,
            id: id.to_string(),
            name: format!("Task7 Test Plugin {id}"),
            version: String::new(),
            publisher: String::new(),
            description: String::new(),
            homepage: None,
            icon: None,
            categories: vec![],
            slot: None,
            verified: false,
            experimental: false,
            auth: Some(AuthSpec {
                kind: AuthKind::Token,
                setting: Some(format!("plugin.{id}.token")),
                ..Default::default()
            }),
            settings: vec![SettingField {
                key: format!("plugin.{id}.region"),
                label: "Region".to_string(),
                help: String::new(),
                secret: false,
                required: false,
                kind: FieldKind::String,
                options: Vec::new(),
                default: None,
            }],
            component: None,
            permissions: Default::default(),
            oauth: vec![],
            provider: None,
            tools: vec![],
            mcp: vec![],
            hooks: vec![],
            jobs: vec![],
            gateway: false,
        };
        let mut host = PluginHost::new();
        host.add(CorePlugin {
            manifest,
            harness: None,
            gateway: None,
            connector: None,
            provider: None,
            source: PluginSource::Builtin,
        });
    }

    async fn ctx_for(store: Arc<Store>, plugin_id: &str) -> PluginCapabilityContext {
        PluginCapabilityContext {
            plugin_id: plugin_id.to_string(),
            version: "0.1.0".to_string(),
            settings: SettingsStore::new(store.clone()),
            store,
            telemetry: Arc::new(NoopTelemetry),
            network_allowlist: vec![],
            oauth_profile_ids: vec![],
            provider_ids: vec![],
        }
    }

    // PR-3: a component may read back its OWN declared secret's real value —
    // it needs the credential to build provider payloads (e.g. discord's
    // gateway IDENTIFY). Scoping + declaration checks are unchanged. This
    // replaces the old `secret_field_round_trip_never_returns_the_raw_value`,
    // which asserted the opposite (blanking) policy this task removes.
    #[tokio::test]
    async fn own_declared_secret_reads_back_real_value() {
        let id = "task7-github";
        register_test_plugin(id);
        let (store, _tmp) = open_test_store().await;
        let ctx = ctx_for(Arc::new(store), id).await;
        let settings = ScopedSettings::new(&ctx);

        settings.set("token", "s3cr3t").await.unwrap();
        let (effective, value, secret) = settings.get("token").await.unwrap();
        assert_eq!(effective, format!("plugin.{id}.token"));
        assert!(secret, "secret still reports true so a UI keeps masking");
        assert_eq!(
            value, "s3cr3t",
            "a component may read back its own declared secret's real value"
        );
    }

    #[tokio::test]
    async fn non_secret_field_round_trips_the_value() {
        let id = "task7-github-region";
        register_test_plugin(id);
        let (store, _tmp) = open_test_store().await;
        let ctx = ctx_for(Arc::new(store), id).await;
        let settings = ScopedSettings::new(&ctx);

        settings.set("region", "us-east-1").await.unwrap();
        let (effective, value, secret) = settings.get("region").await.unwrap();
        assert_eq!(effective, format!("plugin.{id}.region"));
        assert!(!secret);
        assert_eq!(value, "us-east-1");
    }

    #[tokio::test]
    async fn a_fully_qualified_key_targeting_another_plugin_is_rejected() {
        let github_id = "task7-github-cross";
        let atlassian_id = "task7-atlassian-cross";
        register_test_plugin(github_id);
        register_test_plugin(atlassian_id);
        let (store, _tmp) = open_test_store().await;
        let store = Arc::new(store);
        let ctx = ctx_for(store, github_id).await;
        let settings = ScopedSettings::new(&ctx);

        let smuggled = format!("plugin.{atlassian_id}.token");
        let get_result = settings.get(&smuggled).await;
        assert!(
            matches!(get_result, Err(SettingsErr::Invalid(_))),
            "expected Invalid, got {get_result:?}"
        );
        let set_result = settings.set(&smuggled, "stolen").await;
        assert!(
            matches!(set_result, Err(SettingsErr::Invalid(_))),
            "expected Invalid, got {set_result:?}"
        );
    }

    #[tokio::test]
    async fn an_undeclared_bare_key_is_rejected() {
        let id = "task7-github-undeclared";
        register_test_plugin(id);
        let (store, _tmp) = open_test_store().await;
        let ctx = ctx_for(Arc::new(store), id).await;
        let settings = ScopedSettings::new(&ctx);

        for result in [
            settings.get("nope").await.map(|_| ()),
            settings.set("nope", "x").await.map(|_| ()),
            settings.remove("nope").await.map(|_| ()),
        ] {
            assert!(
                matches!(result, Err(SettingsErr::Invalid(_))),
                "expected Invalid, got {result:?}"
            );
        }
    }

    // ---------- I2: a guest must never reach the host-owned `enabled`/
    // `trusted` control keys through this capability ----------

    #[tokio::test]
    async fn a_guest_cannot_self_enable_or_self_trust_via_the_bare_control_keys() {
        let id = "task7-github-self-enable";
        register_test_plugin(id);
        let (store, _tmp) = open_test_store().await;
        let store = Arc::new(store);
        let ctx = ctx_for(store.clone(), id).await;
        let settings = ScopedSettings::new(&ctx);

        for key in ["enabled", "trusted"] {
            let get_result = settings.get(key).await;
            assert!(
                matches!(get_result, Err(SettingsErr::Invalid(_))),
                "{key}: get should be Invalid, got {get_result:?}"
            );
            let set_result = settings.set(key, "true").await;
            assert!(
                matches!(set_result, Err(SettingsErr::Invalid(_))),
                "{key}: a guest must not be able to write its own {key} flag, got {set_result:?}"
            );
            let remove_result = settings.remove(key).await;
            assert!(
                matches!(remove_result, Err(SettingsErr::Invalid(_))),
                "{key}: remove should be Invalid, got {remove_result:?}"
            );
        }

        // The write attempts above must have left no trace: a host-side
        // disable/un-trust set directly on the row (bypassing this
        // capability, the way `toggle_enabled`/uninstall actually do it)
        // must survive untouched.
        let qualified_enabled = format!("plugin.{id}.enabled");
        let qualified_trusted = format!("plugin.{id}.trusted");
        store
            .set_setting_raw(&qualified_enabled, "false")
            .await
            .unwrap();
        store
            .set_setting_raw(&qualified_trusted, "false")
            .await
            .unwrap();
        assert_eq!(
            store.get_setting_raw(&qualified_enabled).await.unwrap(),
            Some("false".to_string())
        );
        assert_eq!(
            store.get_setting_raw(&qualified_trusted).await.unwrap(),
            Some("false".to_string())
        );
    }
}
