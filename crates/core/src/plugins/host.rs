//! `CorePlugin`/`PluginHost`: binds a `PluginManifest` to the runtime
//! capabilities it provides, and `Registries`, the composition root for all
//! four extension axes plus the plugin host itself.
//!
//! This replaces the old `Integration` trait (`crate::integration`, deleted).
//! Previously a host object implemented `Integration` and answered
//! `harness()`/`gateway()`/`connector()` by method; now every extension point
//! is a manifest (`CorePlugin.manifest`) paired with typed `Option<Arc<dyn
//! _>>` capability fields — the manifest is the thing a catalog/user plugin
//! can actually author (as TOML), while a Rust built-in constructs both the
//! manifest and its capabilities in code (see `harness::native`).
//!
//! `Registries::add_plugin` is the one place a `CorePlugin` becomes "live":
//! a `harness` capability replaces the single `Registries.harness` slot, a
//! `gateway` capability is fanned into `GatewayRegistry` under
//! `manifest.id`, AND the plugin is recorded in `self.plugins` so
//! `PluginHost` can answer identity/enablement questions later (e.g. for a
//! settings UI).
//! `connector` is deliberately NOT fanned into `ConnectorRegistry` here — a
//! `CorePlugin` carries a live `Arc<dyn Connector>` instance (not a
//! `ConnectorFactory`), consumed directly from the host by
//! `ControlPlane::start_harness_session` (`control::lifecycle`), which
//! attaches every enabled connector-capable plugin's MCP servers to the
//! session.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use ryuzi_plugin_sdk::{FieldKind, PluginManifest, SettingField};

use crate::connector::{Connector, ConnectorRegistry};
use crate::gateway::{GatewayFactory, GatewayRegistry};
use crate::harness::HarnessFactory;
use crate::plugins::extension::ExtensionFactory;
use crate::settings::{csv, SettingsStore};

/// Process-wide registry of every `plugin.*` settings key any installed
/// plugin has declared, populated by [`PluginHost::add`]. Backs
/// `crate::settings::store::validate_setting`'s acceptance of `plugin.*`
/// keys and `crate::settings::catalog::is_secret`'s secret-flagging for
/// plugin-owned fields.
///
/// Global rather than threaded through `SettingsStore`/`validate_setting`
/// because validation is a free function called from many places (CLI,
/// Tauri commands, the settings store itself) that don't otherwise carry a
/// `Registries` handle. Registration is add-only and idempotent — the same
/// key registered twice (e.g. across two `PluginHost`s in different tests)
/// simply overwrites; tests that care about isolation use unique plugin ids.
static PLUGIN_FIELDS: OnceLock<RwLock<HashMap<String, (String, SettingField)>>> = OnceLock::new();

fn plugin_fields() -> &'static RwLock<HashMap<String, (String, SettingField)>> {
    PLUGIN_FIELDS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Look up a `plugin.*` settings field previously registered by
/// [`PluginHost::add`] — `None` if no installed plugin declares `key`.
pub fn plugin_field(key: &str) -> Option<SettingField> {
    plugin_fields()
        .read()
        .expect("PLUGIN_FIELDS lock poisoned")
        .get(key)
        .map(|(_, field)| field.clone())
}

/// Every `plugin.*` settings field registered by an installed plugin,
/// paired with the id of the plugin that declared it. Used by
/// `crate::settings::store::SettingsStore::missing_required` to fold
/// plugin-declared `required` fields into the same required-field
/// aggregation used for global/gateway fields, gated on the owning plugin
/// actually being enabled (a disabled plugin's required fields must not
/// block onboarding).
pub fn plugin_fields_all() -> Vec<(String, SettingField)> {
    plugin_fields()
        .read()
        .expect("PLUGIN_FIELDS lock poisoned")
        .values()
        .cloned()
        .collect()
}

/// Register every `plugin.*` settings key `manifest` declares:
/// - each `manifest.settings[]` field, verbatim
/// - `manifest.auth.setting`, if present, as a synthetic secret `String`
///   field (the manifest's `[auth]` block only names the key; it has no
///   label/help of its own) — UNLESS `manifest.settings[]` already declared
///   that key (PR-3: a bridged component's derived `AuthKind::Token` setting
///   IS one of its own `settings[]` fields, e.g. `plugin.discord.token`, with
///   a real label/help/kind the manifest author wrote; the label-less
///   synthetic field must never clobber it)
/// - `plugin.<id>.enabled`, always, as a `Bool` field — this is what makes
///   `validate_setting("plugin.<id>.enabled", ...)` accept every installed
///   plugin, not just connector-capable ones (harmless for the others: they
///   just never read the key back via `is_enabled`)
fn register_plugin_fields(manifest: &PluginManifest) {
    let mut fields = plugin_fields()
        .write()
        .expect("PLUGIN_FIELDS lock poisoned");
    for f in &manifest.settings {
        fields.insert(f.key.clone(), (manifest.id.clone(), f.clone()));
    }
    if let Some(auth) = &manifest.auth {
        if let Some(key) = &auth.setting {
            if !fields.contains_key(key) {
                fields.insert(
                    key.clone(),
                    (
                        manifest.id.clone(),
                        SettingField {
                            key: key.clone(),
                            label: format!("{} auth", manifest.name),
                            help: String::new(),
                            secret: true,
                            required: false,
                            kind: FieldKind::String,
                            options: Vec::new(),
                            default: None,
                        },
                    ),
                );
            }
        }
    }
    let enabled_key = format!("plugin.{}.enabled", manifest.id);
    fields.insert(
        enabled_key.clone(),
        (
            manifest.id.clone(),
            SettingField {
                key: enabled_key,
                label: format!("Enable {}", manifest.name),
                help: String::new(),
                secret: false,
                required: false,
                kind: FieldKind::Bool,
                options: Vec::new(),
                default: None,
            },
        ),
    );
}

/// Where a plugin's manifest/behavior came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// Shipped inside the `ryuzi` binary (the native harness, the discord gateway).
    Builtin,
    /// Installed as a skill pack by the skills installer
    /// (`crate::skills_install`) — carries the manifest's own on-disk
    /// directory.
    SkillPack(std::path::PathBuf),
    /// A signed WASM component bundle shipped first-party from `plugins/<id>`
    /// (see `crate::plugins::component_catalog`). Registered manifest-only:
    /// the executable capability is still discovered off disk in
    /// `daemon::build_daemon` from the *installed* bundle root, so this entry
    /// exists purely to make the bundle visible and enumerable through
    /// `list_plugins`.
    Component,
}

/// A manifest bound to the behavioral capabilities it advertises. Each
/// `Some` axis is wired into the matching registry by
/// [`Registries::add_plugin`].
pub struct CorePlugin {
    pub manifest: PluginManifest,
    pub harness: Option<Arc<dyn HarnessFactory>>,
    pub gateway: Option<Arc<dyn GatewayFactory>>,
    pub connector: Option<Arc<dyn Connector>>,
    /// Supervised subprocess "code plugin" capability (Track D). Mirrors
    /// `connector` in every way that matters here: a live instance (not a
    /// factory-by-config), gated by [`PluginHost::is_enabled`] the same way,
    /// and — like `connector` — deliberately NOT fanned into a registry by
    /// [`Registries::add_plugin`]; it is consumed directly from the host
    /// (`plugins::extension::ExtensionHost::spawn_all`).
    pub extension: Option<Arc<dyn ExtensionFactory>>,
    /// Live WASM component model-provider capability (Task 10). Like
    /// `connector`/`extension`, a live instance rather than a factory-by-config,
    /// and deliberately NOT fanned into `Registries` by
    /// [`Registries::add_plugin`]: it is consumed directly — the router looks it
    /// up through the process-wide registry in
    /// [`crate::plugins::wasm_provider`], not off this field — so `Registries`
    /// stays unaware of provider bundles. Distinct from the metadata-only
    /// `manifest.provider` string (which merely marks a catalog entry as a
    /// provider): this is the executable component.
    pub provider: Option<Arc<dyn crate::plugins::wasm_provider::WasmProviderRuntime>>,
    pub source: PluginSource,
}

impl CorePlugin {
    /// Which of the five extension axes this plugin advertises. `runtime`
    /// means a live `HarnessFactory` (the native runtime).
    ///
    /// Single source of truth for `ryuzi_core::serve`'s `GET /plugins`
    /// endpoint, the Cockpit `list_plugins`/`plugin_detail` commands, and
    /// `ryuzi plugins info` — all three call this instead of re-deriving the
    /// convention themselves.
    pub fn capabilities(&self) -> Vec<&'static str> {
        let mut caps = Vec::new();
        // A single "provider" capability whether it is a metadata-only catalog
        // entry (`manifest.provider`) or a live component provider
        // (`self.provider`, Task 10) — reported once, never double-counted, so a
        // bundle carrying both still lists "provider" a single time.
        if self.manifest.provider.is_some() || self.provider.is_some() {
            caps.push("provider");
        }
        if self.harness.is_some() {
            caps.push("runtime");
        }
        if self.gateway.is_some() {
            caps.push("gateway");
        }
        if self.connector.is_some() {
            caps.push("connector");
        }
        if self.extension.is_some() {
            caps.push("extension");
        }
        caps
    }
}

/// A losing claim for an already-owned [`PluginManifest::slot`]: `winner_id`
/// registered first and owns `slot`; `loser_id` claimed the same slot later
/// and was NOT registered as owner (it is still installed as a normal
/// plugin — only its slot claim lost). Surfaced by
/// `crate::plugins::doctor::plugin_doctor` as a `"slot-conflict"` finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotConflict {
    pub slot: String,
    pub winner_id: String,
    pub loser_id: String,
}

/// Every installed plugin, keyed by `manifest.id`, kept in insertion order.
/// Also arbitrates [`PluginManifest::slot`] claims: `slots` records the
/// first plugin to claim each named slot (first-registration-wins, the same
/// rule `add` itself uses for duplicate ids), and `slot_conflicts` records
/// every later claimant for an already-owned slot instead of silently
/// dropping it.
#[derive(Clone, Default)]
pub struct PluginHost {
    order: Vec<Arc<CorePlugin>>,
    by_id: HashMap<String, usize>,
    slots: HashMap<String, String>,
    slot_conflicts: Vec<SlotConflict>,
}

impl PluginHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plugin. Returns `false` (and logs a warning) without
    /// installing it if `manifest.id` is already taken — the first
    /// registration for an id wins.
    ///
    /// If the manifest claims a `slot`, arbitrate it the same way: the first
    /// plugin to claim a given slot name wins ([`PluginHost::slot_owner`]);
    /// a later claimant is still registered as a normal plugin (its manifest
    /// is unaffected), but the claim itself is recorded as a
    /// [`SlotConflict`] rather than silently overwriting the owner.
    pub fn add(&mut self, plugin: CorePlugin) -> bool {
        if self.by_id.contains_key(&plugin.manifest.id) {
            tracing::warn!(
                "plugin id `{}` already registered — ignoring duplicate",
                plugin.manifest.id
            );
            return false;
        }
        register_plugin_fields(&plugin.manifest);
        if let Some(slot) = &plugin.manifest.slot {
            match self.slots.get(slot) {
                None => {
                    self.slots.insert(slot.clone(), plugin.manifest.id.clone());
                }
                Some(winner_id) => {
                    tracing::warn!(
                        "plugin `{}` claims slot `{slot}`, already owned by `{winner_id}` — recording a slot conflict",
                        plugin.manifest.id
                    );
                    self.slot_conflicts.push(SlotConflict {
                        slot: slot.clone(),
                        winner_id: winner_id.clone(),
                        loser_id: plugin.manifest.id.clone(),
                    });
                }
            }
        }
        self.by_id
            .insert(plugin.manifest.id.clone(), self.order.len());
        self.order.push(Arc::new(plugin));
        true
    }

    /// The plugin id that won a named slot's arbitration (first
    /// registration wins), or `None` if no installed plugin has claimed
    /// `slot`.
    pub fn slot_owner(&self, slot: &str) -> Option<&str> {
        self.slots.get(slot).map(String::as_str)
    }

    /// Every losing slot claim recorded by [`PluginHost::add`], in the
    /// order the conflicting plugin was registered.
    pub fn slot_conflicts(&self) -> &[SlotConflict] {
        &self.slot_conflicts
    }

    /// Look up an installed plugin by id.
    pub fn get(&self, id: &str) -> Option<Arc<CorePlugin>> {
        self.by_id.get(id).map(|&i| self.order[i].clone())
    }

    /// All installed plugins, in insertion order.
    pub fn list(&self) -> Vec<Arc<CorePlugin>> {
        self.order.clone()
    }

    /// Whether `id` is enabled, in priority order:
    /// - unknown id → `false`
    /// - harness-capable → always `true` (the native runtime cannot be
    ///   disabled)
    /// - gateway-capable → the `enabled_gateways` CSV setting contains `id`
    /// - component bundle → explicit `plugin.<id>.enabled` wins in either
    ///   direction (`"false"` disables, `"true"` enables); with no setting,
    ///   enabled iff an active release exists on disk
    ///   (`Store::active_component_release`) — a successful install counts as
    ///   enabled without a separate enable write
    /// - experimental → always `false` (see below)
    /// - manifest-only (no harness/gateway/connector/extension capability)
    ///   → always `true`
    /// - connector- and/or extension-capable → the setting
    ///   `plugin.<id>.enabled == "true"` (defaults to `false`)
    pub async fn is_enabled(&self, settings: &SettingsStore, id: &str) -> anyhow::Result<bool> {
        let Some(plugin) = self.get(id) else {
            return Ok(false);
        };
        if plugin.harness.is_some() {
            // The native runtime is the only harness and is always enabled.
            return Ok(true);
        }
        if plugin.source == PluginSource::Component {
            // PR-2 fix A — one rule, one implementation: the catalog gate and
            // the runtime-attach gate must never disagree (a wedged install
            // would be bindable but never attach), so this delegates to the
            // same function the daemon-build path uses.
            return component_plugin_enabled(settings, id).await;
        }
        if plugin.gateway.is_some() {
            let enabled = csv(settings.get("enabled_gateways").await?.as_deref());
            return Ok(enabled.iter().any(|g| g == id));
        }
        if plugin.manifest.experimental {
            // Experimental catalog entries (ngrok/zep/vercel-sandbox) are
            // docs-only: no harness/gateway/connector/extension capability
            // backs them, so there is nothing to actually enable. Report
            // disabled unconditionally — this wins over the manifest-only
            // fallback below even if a stray `plugin.<id>.enabled = true`
            // setting exists. Real capabilities (providers) always hardcode
            // `experimental = false`, so this never affects them.
            return Ok(false);
        }
        if plugin.connector.is_none() && plugin.extension.is_none() {
            // Manifest-only plugin (e.g. a provider metadata entry
            // with no behavioral capability of its own) — always enabled.
            return Ok(true);
        }
        let key = format!("plugin.{id}.enabled");
        Ok(settings.get(&key).await?.as_deref() == Some("true"))
    }
}

/// Whether an installed WASM component bundle (Task 9+) is enabled. Mirrors
/// [`PluginHost::is_enabled`]'s logic for Component bundles: an installed
/// (active-release-on-disk) component is enabled unless the user explicitly
/// disabled it (explicit `plugin.<id>.enabled == "false"`). Explicit settings
/// always win, in either direction.
///
/// Component bundles are discovered off-disk
/// ([`crate::plugins::bundle::load_active_bundles`]) rather than registered as
/// [`CorePlugin`]s, so this is a standalone check keyed on the bundle's
/// manifest id rather than a `PluginHost` method — but it deliberately reuses
/// the identical settings key so a single "enable this plugin" toggle governs
/// a plugin regardless of whether it ships as a subprocess extension or a WASM
/// component.
pub async fn component_plugin_enabled(settings: &SettingsStore, id: &str) -> anyhow::Result<bool> {
    let key = format!("plugin.{id}.enabled");
    match settings.get(&key).await?.as_deref() {
        Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => {
            let active = settings
                .store()
                .active_component_release(id)
                .await?
                .is_some();
            Ok(active)
        }
    }
}

/// Whether `id`'s installed component has every setting its derived auth
/// requires (the `component_catalog::derive_bundle_auth` contract: an
/// embedded manifest secret+required field, e.g. discord's bot token). `true`
/// when the embedded bundle manifest declares no such field —
/// there is nothing to configure, so an enabled component is free to attach —
/// or when every declared secret+required field has a stored non-empty value.
///
/// This is deliberately SEPARATE from [`component_plugin_enabled`]: enabled
/// answers "did the user turn this on" (default yes, opt-out), while this
/// answers "can it actually start" (needs-setup is a real, honest state, not
/// a supervisor-restart-loop failure). Callers gate an attach path on BOTH —
/// an enabled-but-unconfigured component must skip attaching, not fail
/// `start()` and get retried forever.
pub async fn component_required_settings_configured(
    settings: &SettingsStore,
    id: &str,
) -> anyhow::Result<bool> {
    let Some(bundle) = crate::plugins::component_catalog::declared_bundle_manifest(id) else {
        return Ok(true);
    };
    for field in bundle.settings.iter().filter(|f| f.secret && f.required) {
        let key = format!("plugin.{id}.{}", field.key);
        let configured = settings
            .get(&key)
            .await?
            .is_some_and(|value| !value.is_empty());
        if !configured {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The extension registries, plus the plugin host recording every
/// installed [`CorePlugin`]. `harness` is a single slot: the native
/// runtime is the only harness — production leaves the default, tests
/// swap in fakes (directly, or via a harness-capable plugin's
/// `add_plugin`, which overwrites the slot).
pub struct Registries {
    pub harness: Arc<dyn HarnessFactory>,
    pub gateway: GatewayRegistry,
    pub connector: ConnectorRegistry,
    pub plugins: PluginHost,
}

impl Default for Registries {
    fn default() -> Self {
        Registries {
            harness: Arc::new(crate::harness::native::NativeHarnessFactory::new()),
            gateway: GatewayRegistry::default(),
            connector: ConnectorRegistry::default(),
            plugins: PluginHost::default(),
        }
    }
}

impl Registries {
    pub fn new() -> Self {
        Registries::default()
    }

    /// Install a plugin: a harness capability replaces the single harness
    /// slot, a gateway capability registers under `manifest.id`, and the
    /// plugin is recorded in `self.plugins`. A duplicate `manifest.id` is
    /// rejected entirely (no registry is touched) — see [`PluginHost::add`].
    pub fn add_plugin(&mut self, plugin: CorePlugin) {
        let id = plugin.manifest.id.clone();
        if self.plugins.get(&id).is_some() {
            tracing::warn!("plugin id `{id}` already registered — ignoring duplicate");
            return;
        }
        if let Some(h) = &plugin.harness {
            self.harness = h.clone();
        }
        if let Some(g) = &plugin.gateway {
            self.gateway.register(id.clone(), g.clone());
        }
        self.plugins.add(plugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::ConnectorCtx;
    use crate::domain::{ApprovalDecision, ApprovalRequest, McpServerSpec, Surface};
    use crate::gateway::{Gateway, MessageRef};
    use crate::harness::{Harness, HarnessSession, SessionCtx};
    use crate::store::Store;
    use async_trait::async_trait;

    // ---- minimal fakes for each axis (self-contained to this test module) ----

    struct FakeHarness;
    #[async_trait]
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
    #[async_trait]
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
        async fn post_status(&self, surface: &Surface, _text: &str) -> anyhow::Result<MessageRef> {
            Ok(MessageRef {
                surface: surface.clone(),
                message_id: "m1".to_string(),
            })
        }
        async fn edit_status(&self, _msg: &MessageRef, _text: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn post_result(&self, _surface: &Surface, _chunks: &[String]) -> anyhow::Result<()> {
            Ok(())
        }
        async fn post_error(&self, _surface: &Surface, _message: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn request_approval(
            &self,
            _s: &Surface,
            _r: &ApprovalRequest,
        ) -> anyhow::Result<ApprovalDecision> {
            Ok(ApprovalDecision::Cancel)
        }
    }
    struct FakeGatewayFactory;
    impl GatewayFactory for FakeGatewayFactory {
        fn create(&self, _c: &serde_json::Value) -> anyhow::Result<Arc<dyn Gateway>> {
            Ok(Arc::new(FakeGateway))
        }
    }

    struct FakeConnector;
    #[async_trait]
    impl Connector for FakeConnector {
        async fn mcp_servers(&self, _ctx: &ConnectorCtx) -> anyhow::Result<Vec<McpServerSpec>> {
            Ok(vec![])
        }
    }

    fn manifest(id: &str) -> PluginManifest {
        PluginManifest {
            contract: 1,
            id: id.to_string(),
            name: id.to_string(),
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
            mcp: vec![],
            extensions: vec![],
            skills: vec![],
            provider: None,
        }
    }

    fn harness_only(id: &str) -> CorePlugin {
        CorePlugin {
            manifest: manifest(id),
            harness: Some(Arc::new(FakeHarnessFactory)),
            gateway: None,
            connector: None,
            extension: None,
            provider: None,
            source: PluginSource::Builtin,
        }
    }

    fn gateway_only(id: &str) -> CorePlugin {
        CorePlugin {
            manifest: manifest(id),
            harness: None,
            gateway: Some(Arc::new(FakeGatewayFactory)),
            connector: None,
            extension: None,
            provider: None,
            source: PluginSource::Builtin,
        }
    }

    fn connector_only(id: &str) -> CorePlugin {
        CorePlugin {
            manifest: manifest(id),
            harness: None,
            gateway: None,
            connector: Some(Arc::new(FakeConnector)),
            extension: None,
            provider: None,
            source: PluginSource::Builtin,
        }
    }

    fn manifest_only(id: &str) -> CorePlugin {
        CorePlugin {
            manifest: manifest(id),
            harness: None,
            gateway: None,
            connector: None,
            extension: None,
            provider: None,
            source: PluginSource::Builtin,
        }
    }

    fn manifest_only_with_slot(id: &str, slot: &str) -> CorePlugin {
        CorePlugin {
            manifest: PluginManifest {
                slot: Some(slot.to_string()),
                ..manifest(id)
            },
            harness: None,
            gateway: None,
            connector: None,
            extension: None,
            provider: None,
            source: PluginSource::Builtin,
        }
    }

    struct FakeExtensionFactory;
    #[async_trait]
    impl crate::plugins::extension::ExtensionFactory for FakeExtensionFactory {
        async fn extensions(
            &self,
            _ctx: &crate::plugins::extension::ExtensionCtx,
        ) -> anyhow::Result<Vec<crate::plugins::extension::ExtensionSpec>> {
            Ok(vec![])
        }
    }

    fn extension_only(id: &str) -> CorePlugin {
        CorePlugin {
            manifest: manifest(id),
            harness: None,
            gateway: None,
            connector: None,
            extension: Some(Arc::new(FakeExtensionFactory)),
            provider: None,
            source: PluginSource::Builtin,
        }
    }

    fn component_only(id: &str) -> CorePlugin {
        CorePlugin {
            manifest: manifest(id),
            harness: None,
            gateway: None,
            connector: None,
            extension: None,
            provider: None,
            source: PluginSource::Component,
        }
    }

    async fn open_settings() -> (Arc<Store>, SettingsStore, tempfile::NamedTempFile) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(tmp.path()).await.unwrap());
        let settings = SettingsStore::new(store.clone());
        (store, settings, tmp)
    }

    /// Seeds an installed, active `component_plugin_releases` row for `id` —
    /// the ledger state a successful component-bundle install leaves behind.
    /// Mirrors `plugins_api.rs`'s `seed_active_component_release` test helper.
    async fn seed_active_component_release(store: &Store, id: &str) {
        store
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
        store
            .set_active_component_release(id, "0.1.0")
            .await
            .unwrap();
    }

    // ---------- PluginHost::add/get/list ----------

    #[test]
    fn add_get_list_preserve_insertion_order() {
        let mut host = PluginHost::new();
        assert!(host.add(harness_only("a")));
        assert!(host.add(gateway_only("b")));

        assert_eq!(host.get("a").unwrap().manifest.id, "a");
        assert_eq!(host.get("b").unwrap().manifest.id, "b");
        assert!(host.get("missing").is_none());

        let ids: Vec<String> = host.list().iter().map(|p| p.manifest.id.clone()).collect();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn duplicate_id_is_rejected_and_first_registration_wins() {
        let mut host = PluginHost::new();
        assert!(host.add(harness_only("dup")));
        assert!(
            !host.add(gateway_only("dup")),
            "second registration of the same id must be rejected"
        );

        let kept = host.get("dup").unwrap();
        assert!(kept.harness.is_some(), "the FIRST registration must win");
        assert!(kept.gateway.is_none());
        assert_eq!(host.list().len(), 1);
    }

    // ---------- slot arbitration (Feature C2) ----------

    #[test]
    fn first_plugin_to_claim_a_slot_becomes_its_owner() {
        let mut host = PluginHost::new();
        assert!(host.add(manifest_only_with_slot("hermes-native", "memory")));

        assert_eq!(host.slot_owner("memory"), Some("hermes-native"));
        assert!(host.slot_conflicts().is_empty());
    }

    #[test]
    fn slot_owner_is_none_for_an_unclaimed_slot() {
        let host = PluginHost::new();
        assert_eq!(host.slot_owner("memory"), None);
    }

    #[test]
    fn second_claimant_for_an_owned_slot_is_recorded_as_a_conflict_not_owner() {
        let mut host = PluginHost::new();
        assert!(host.add(manifest_only_with_slot("mem0", "memory")));
        assert!(
            host.add(manifest_only_with_slot("cavemem", "memory")),
            "a losing slot claim must not block normal plugin registration"
        );

        // First registration still owns the slot.
        assert_eq!(host.slot_owner("memory"), Some("mem0"));

        // The loser is recorded as a conflict, not registered as owner.
        let conflicts = host.slot_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].slot, "memory");
        assert_eq!(conflicts[0].winner_id, "mem0");
        assert_eq!(conflicts[0].loser_id, "cavemem");

        // The loser is still installed as a normal plugin — only its slot
        // claim lost, its own registration did not.
        assert!(host.get("cavemem").is_some());
        assert_eq!(host.list().len(), 2);
    }

    #[test]
    fn distinct_slots_have_independent_owners() {
        let mut host = PluginHost::new();
        assert!(host.add(manifest_only_with_slot("mem0", "memory")));
        assert!(host.add(manifest_only_with_slot("graphiti", "knowledge-graph")));

        assert_eq!(host.slot_owner("memory"), Some("mem0"));
        assert_eq!(host.slot_owner("knowledge-graph"), Some("graphiti"));
        assert!(host.slot_conflicts().is_empty());
    }

    #[test]
    fn plugins_without_a_slot_claim_never_produce_a_conflict() {
        let mut host = PluginHost::new();
        assert!(host.add(manifest_only("a")));
        assert!(host.add(manifest_only("b")));

        assert!(host.slot_conflicts().is_empty());
        assert_eq!(host.slot_owner("memory"), None);
    }

    // ---------- Registries::add_plugin ----------

    #[test]
    fn add_plugin_fans_harness_and_gateway_into_registries_under_manifest_id() {
        let mut regs = Registries::new();
        regs.add_plugin(harness_only("claude-code"));
        regs.add_plugin(gateway_only("discord"));

        assert!(regs.gateway.get("claude-code").is_none());
        assert!(regs.gateway.get("discord").is_some());

        // both recorded in the host too
        assert!(regs.plugins.get("claude-code").is_some());
        assert!(regs.plugins.get("discord").is_some());
        assert_eq!(regs.gateway.names(), vec!["discord".to_string()]);
    }

    #[test]
    fn add_plugin_does_not_fan_connector_into_connector_registry() {
        let mut regs = Registries::new();
        regs.add_plugin(connector_only("notion"));

        assert!(
            regs.connector.get("notion").is_none(),
            "connector must be consumed directly from the host, not fanned into ConnectorRegistry"
        );
        assert!(regs.plugins.get("notion").is_some());
    }

    #[test]
    fn add_plugin_rejects_duplicate_id_without_touching_any_registry() {
        let mut regs = Registries::new();
        regs.add_plugin(harness_only("dup"));
        regs.add_plugin(gateway_only("dup"));

        assert!(
            regs.plugins.get("dup").unwrap().harness.is_some(),
            "first registration wins"
        );
        assert!(
            regs.gateway.get("dup").is_none(),
            "the duplicate's gateway factory must never be registered"
        );
    }

    // ---------- PluginHost::is_enabled ----------

    #[tokio::test]
    async fn is_enabled_unknown_id_is_false() {
        let (_store, settings, _tmp) = open_settings().await;
        let host = PluginHost::new();
        assert!(!host.is_enabled(&settings, "nope").await.unwrap());
    }

    #[tokio::test]
    async fn is_enabled_harness_capability_is_always_true() {
        let (_store, settings, _tmp) = open_settings().await;
        let mut host = PluginHost::new();
        host.add(harness_only("native"));
        assert!(host.is_enabled(&settings, "native").await.unwrap());
    }

    #[tokio::test]
    async fn is_enabled_gateway_capability_follows_enabled_gateways() {
        let (_store, settings, _tmp) = open_settings().await;
        let mut host = PluginHost::new();
        // Deliberately NOT "discord" — a fresh `Store` seeds
        // `enabled_gateways = "discord"` (see `store.rs`'s migration seed),
        // which would make this id enabled from the start and defeat the
        // "off by default" half of this test.
        host.add(gateway_only("slack"));

        assert!(!host.is_enabled(&settings, "slack").await.unwrap());
        settings.set("enabled_gateways", "slack").await.unwrap();
        assert!(host.is_enabled(&settings, "slack").await.unwrap());
    }

    #[tokio::test]
    async fn is_enabled_manifest_only_plugin_is_always_true() {
        let (_store, settings, _tmp) = open_settings().await;
        let mut host = PluginHost::new();
        host.add(manifest_only("anthropic"));

        assert!(host.is_enabled(&settings, "anthropic").await.unwrap());
    }

    #[tokio::test]
    async fn is_enabled_experimental_manifest_only_plugin_is_always_false() {
        let (store, settings, _tmp) = open_settings().await;
        let mut host = PluginHost::new();
        host.add(CorePlugin {
            manifest: PluginManifest {
                experimental: true,
                ..manifest("ngrok-like")
            },
            ..manifest_only("ngrok-like")
        });

        assert!(
            !host.is_enabled(&settings, "ngrok-like").await.unwrap(),
            "experimental plugins have nothing to enable — must report disabled"
        );

        // Writing `plugin.<id>.enabled = true` must not flip it: experimental
        // wins over the manifest-only "always enabled" fallback.
        store
            .set_setting_raw("plugin.ngrok-like.enabled", "true")
            .await
            .unwrap();
        assert!(
            !host.is_enabled(&settings, "ngrok-like").await.unwrap(),
            "experimental still wins even once plugin.<id>.enabled is set"
        );
    }

    #[tokio::test]
    async fn is_enabled_connector_only_plugin_defaults_false_until_setting_flips_true() {
        let (store, settings, _tmp) = open_settings().await;
        let mut host = PluginHost::new();
        host.add(connector_only("github"));

        assert!(!host.is_enabled(&settings, "github").await.unwrap());

        // `settings.set` would work equally well now that `validate_setting`
        // accepts `plugin.<id>.enabled` (see `settings::store`) — writing the
        // raw row directly here just mirrors the CLI/Tauri path least
        // coupled to that validation.
        store
            .set_setting_raw("plugin.github.enabled", "true")
            .await
            .unwrap();
        assert!(host.is_enabled(&settings, "github").await.unwrap());
    }

    #[tokio::test]
    async fn is_enabled_extension_only_plugin_defaults_false_until_setting_flips_true() {
        let (store, settings, _tmp) = open_settings().await;
        let mut host = PluginHost::new();
        host.add(extension_only("acme-ext"));

        assert!(
            !host.is_enabled(&settings, "acme-ext").await.unwrap(),
            "an extension-capable plugin (like a connector-capable one) must default to disabled"
        );

        store
            .set_setting_raw("plugin.acme-ext.enabled", "true")
            .await
            .unwrap();
        assert!(host.is_enabled(&settings, "acme-ext").await.unwrap());
    }

    // PR-2 fix A: a component bundle with an ACTIVE release on disk counts as
    // enabled even when `plugin.<id>.enabled` was never written — the install
    // succeeded, so agents may bind it. Explicit "false" still disables.
    #[tokio::test]
    async fn component_with_active_release_defaults_enabled() {
        let (store, settings, _tmp) = open_settings().await;
        let mut host = PluginHost::new();
        host.add(component_only("acme-component"));

        // No active release and no `plugin.<id>.enabled` setting → disabled.
        assert!(
            !host.is_enabled(&settings, "acme-component").await.unwrap(),
            "no active release and no setting must default to disabled"
        );

        // An active release with no setting written → enabled by default:
        // the install succeeded, so agents may bind it without an explicit
        // enable write.
        seed_active_component_release(&store, "acme-component").await;
        assert!(
            host.is_enabled(&settings, "acme-component").await.unwrap(),
            "an active release on disk must default to enabled"
        );

        // Explicit "false" still disables, even with an active release.
        store
            .set_setting_raw("plugin.acme-component.enabled", "false")
            .await
            .unwrap();
        assert!(
            !host.is_enabled(&settings, "acme-component").await.unwrap(),
            "an explicit false setting must win over an active release"
        );

        // Explicit "true" enables (unchanged from before this fix).
        store
            .set_setting_raw("plugin.acme-component.enabled", "true")
            .await
            .unwrap();
        assert!(
            host.is_enabled(&settings, "acme-component").await.unwrap(),
            "an explicit true setting must enable"
        );
    }

    // ---------- component_plugin_enabled ----------

    // Mirrors component_with_active_release_defaults_enabled but tests the
    // standalone function rather than the PluginHost::is_enabled path.
    #[tokio::test]
    async fn component_plugin_enabled_defaults_to_active_release_state() {
        let (store, settings, _tmp) = open_settings().await;

        // No active release and no `plugin.<id>.enabled` setting → disabled.
        assert!(
            !component_plugin_enabled(&settings, "acme-component")
                .await
                .unwrap(),
            "no active release and no setting must default to disabled"
        );

        // An active release with no setting written → enabled by default:
        // the install succeeded, so runtime may bind it without an explicit
        // enable write.
        seed_active_component_release(&store, "acme-component").await;
        assert!(
            component_plugin_enabled(&settings, "acme-component")
                .await
                .unwrap(),
            "an active release on disk must default to enabled"
        );

        // Explicit "false" still disables, even with an active release.
        store
            .set_setting_raw("plugin.acme-component.enabled", "false")
            .await
            .unwrap();
        assert!(
            !component_plugin_enabled(&settings, "acme-component")
                .await
                .unwrap(),
            "an explicit false setting must win over an active release"
        );

        // Explicit "true" enables (unchanged from before this fix).
        store
            .set_setting_raw("plugin.acme-component.enabled", "true")
            .await
            .unwrap();
        assert!(
            component_plugin_enabled(&settings, "acme-component")
                .await
                .unwrap(),
            "an explicit true setting must enable"
        );
    }

    // ---------- component_required_settings_configured ----------

    // Final-review fix: enabled ("should this run") and configured ("can it
    // actually start") are separate axes. `discord` is the one embedded
    // catalog manifest (`component_catalog::declared_bundle_manifest`) with a
    // `secret + required` settings field (its bot token), so it is the id
    // that exercises the real gate; a synthetic id with no embedded manifest
    // must report configured unconditionally (nothing to configure).
    #[tokio::test]
    async fn component_required_settings_configured_gates_on_the_embedded_secret_field() {
        let (store, settings, _tmp) = open_settings().await;

        // No embedded manifest at all (e.g. a generic test/gateway bundle) —
        // nothing declared, so nothing blocks attaching.
        assert!(
            component_required_settings_configured(&settings, "acme-no-manifest")
                .await
                .unwrap(),
            "an id with no embedded manifest has nothing to configure"
        );

        // `discord` declares its bot token as `secret = true, required = true`
        // — with no stored value, it must report NOT configured.
        assert!(
            !component_required_settings_configured(&settings, "discord")
                .await
                .unwrap(),
            "discord's required bot-token setting has no stored value yet"
        );

        // Storing the required setting flips it to configured.
        store
            .set_setting_raw("plugin.discord.token", "test-bot-token")
            .await
            .unwrap();
        assert!(
            component_required_settings_configured(&settings, "discord")
                .await
                .unwrap(),
            "once the required setting is stored, the component is configured"
        );

        // An empty stored value is the same as unset — still not configured.
        store
            .set_setting_raw("plugin.discord.token", "")
            .await
            .unwrap();
        assert!(
            !component_required_settings_configured(&settings, "discord")
                .await
                .unwrap(),
            "an empty stored value must not count as configured"
        );
    }

    #[test]
    fn capabilities_reports_extension_when_the_axis_is_present() {
        let plugin = extension_only("acme-ext");
        assert_eq!(plugin.capabilities(), vec!["extension"]);
    }

    // Task 10's first real collision: a bundle-bridged manifest (discord) now
    // declares `auth.setting` pointing at one of its OWN `settings[]` keys
    // (the derived token auth setting IS the declared field). The doc-stated
    // guard at `register_plugin_fields` must keep the declared field's real
    // label/required — never let the label-less auth-synthetic clobber it.
    #[test]
    fn register_plugin_fields_keeps_declared_field_when_auth_setting_collides() {
        use ryuzi_plugin_sdk::{AuthKind, AuthSpec};

        let id = "acme-collision-guard";
        let key = format!("plugin.{id}.token");
        let plugin = CorePlugin {
            manifest: PluginManifest {
                auth: Some(AuthSpec {
                    kind: AuthKind::Token,
                    setting: Some(key.clone()),
                    ..Default::default()
                }),
                settings: vec![SettingField {
                    key: key.clone(),
                    label: "Bot token".to_string(),
                    help: "declared by the manifest author, not synthesized".to_string(),
                    secret: true,
                    required: true,
                    kind: FieldKind::String,
                    options: Vec::new(),
                    default: None,
                }],
                ..manifest(id)
            },
            harness: None,
            gateway: None,
            connector: None,
            extension: None,
            provider: None,
            source: PluginSource::Builtin,
        };

        let mut host = PluginHost::new();
        assert!(host.add(plugin));

        let field = plugin_field(&key).expect("the declared field must be registered");
        assert_eq!(
            field.label, "Bot token",
            "the declared settings[] field's label must win over the label-less auth synthetic"
        );
        assert!(
            field.required,
            "the declared field's required flag must survive, not the synthetic's always-false"
        );
    }
}
