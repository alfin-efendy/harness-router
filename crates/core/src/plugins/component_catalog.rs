//! The first-party WASM component bundles shipped from `plugins/<id>`,
//! surfaced as manifest-only [`CorePlugin`]s so they are enumerable through
//! the `list_plugins` RPC that backs Cockpit's Plugins hub.
//!
//! This replaces the removed declarative `plugins::catalog`. Two deliberate
//! differences from that module:
//!
//! - **Manifest-only, always.** A bundle's executable capability (gateway,
//!   connector, provider) is still discovered off disk in
//!   `daemon::build_daemon` from the *installed* bundle root; nothing here
//!   instantiates a component. These entries exist so a bundle is visible
//!   and enumerable even before it is installed.
//! - **Ids a provider builtin already owns are skipped.** Several bundles
//!   under `plugins/` are model providers whose bundle id also appears in
//!   `llm_router::registry::CATALOG`, which [`super::install_providers`]
//!   registers BEFORE this module. [`super::host::PluginHost::add`] is
//!   first-registration-wins, so registering such an id here would be dropped
//!   as a duplicate (and log a warning every boot) while also discarding the
//!   builtin's richer manifest — so [`component_catalog_plugins`] filters them
//!   out via [`provider_registry_owns`]. This covers both the twelve
//!   same-named provider bundles in [`COMPONENT_BACKED_PROVIDER_IDS`] AND
//!   `mimo`/`opencode`, whose bundle ids happen to sit in the CATALOG too.
//!   Every such id is still reported as component-backed by
//!   [`is_component_bundle`], so Cockpit can offer release management for it
//!   against whichever row won.

use ryuzi_plugin_sdk::bundle::{DeclaredTool, PluginBundleManifest};
use ryuzi_plugin_sdk::{AuthKind, AuthSpec, PluginManifest, CONTRACT_VERSION};

use super::host::{CorePlugin, PluginSource};

/// The non-colliding first-party bundles, embedded from their in-repo
/// manifest. Keep in sync with the component list in
/// `scripts/plugins/build-first-party.ts`: every id there is either here or
/// in [`COMPONENT_BACKED_PROVIDER_IDS`].
pub const COMPONENT_BUNDLE_MANIFESTS: &[(&str, &str)] = &[
    (
        "github",
        include_str!("../../../../plugins/github/ryuzi-plugin.toml"),
    ),
    (
        "atlassian",
        include_str!("../../../../plugins/atlassian/ryuzi-plugin.toml"),
    ),
    (
        "bitbucket",
        include_str!("../../../../plugins/bitbucket/ryuzi-plugin.toml"),
    ),
    (
        "discord",
        include_str!("../../../../plugins/discord/ryuzi-plugin.toml"),
    ),
    (
        "mimo",
        include_str!("../../../../plugins/mimo/ryuzi-plugin.toml"),
    ),
    (
        "opencode",
        include_str!("../../../../plugins/opencode/ryuzi-plugin.toml"),
    ),
];

/// Whether `id` is already owned by a `llm_router::registry::CATALOG`
/// provider, which [`super::install_providers`] registers BEFORE this module.
/// Such an id is skipped here rather than handed to `PluginHost::add`, which
/// would drop it as a duplicate and log a warning on every boot.
fn provider_registry_owns(id: &str) -> bool {
    crate::llm_router::registry::CATALOG
        .iter()
        .any(|d| d.id == id)
}

/// Every first-party component bundle id — the embedded manifests plus the
/// provider bundles represented by their builtin row. Used to flag a plugin
/// as component-backed in `PluginInfo` so Cockpit's release-management surface
/// (install / active version / rollback) can find it regardless of which
/// registration won the id.
pub fn is_component_bundle(id: &str) -> bool {
    COMPONENT_BUNDLE_MANIFESTS.iter().any(|(got, _)| *got == id)
        || COMPONENT_BACKED_PROVIDER_IDS.contains(&id)
}

/// Provider bundles that exist under `plugins/` but are represented in the
/// plugin list by their `install_providers` builtin instead, because the
/// bundle id and the router provider id are the same string (see module doc).
pub const COMPONENT_BACKED_PROVIDER_IDS: &[&str] = &[
    "anthropic",
    "anthropic-oauth",
    "openai",
    "openrouter",
    "groq",
    "deepseek",
    "mistral",
    "xai",
    "nvidia",
    "huggingface",
    "google",
    "qwen",
];

/// Derive the coarse auth contract a bundle implies (PR-3): declared OAuth
/// profiles → oauth; else a secret+required setting (e.g. discord's bot
/// token) → token, pointing `setting` at its fully-qualified key; else none.
/// Presentation/status metadata ONLY — component connect flows run through
/// the profile RPCs, never the classic declarative OAuth engine.
fn derive_bundle_auth(id: &str, bundle: &PluginBundleManifest) -> Option<AuthSpec> {
    if !bundle.oauth.is_empty() {
        return Some(AuthSpec {
            kind: AuthKind::Oauth,
            ..Default::default()
        });
    }
    bundle
        .settings
        .iter()
        .find(|f| f.secret && f.required)
        .map(|f| AuthSpec {
            kind: AuthKind::Token,
            setting: Some(format!("plugin.{id}.{}", f.key)),
            ..Default::default()
        })
}

/// Map a bundle manifest onto the declarative [`PluginManifest`] shape the
/// plugin list speaks. `mcp`/`skills`/`provider` stay empty — the bundle
/// format has no concept of them, and a component's behavior is contributed
/// by the running component, never by this manifest. `auth`/`settings` ARE
/// bridged (PR-3, [`derive_bundle_auth`]): the bundle's declared OAuth
/// profiles / settings drive the wizard connect step, needs-setup status,
/// and the setup checklist for component plugins, with every bare bundle
/// settings key namespaced `plugin.<id>.<key>` to match the settings store.
fn manifest_from_bundle(bundle: PluginBundleManifest) -> PluginManifest {
    let auth = derive_bundle_auth(&bundle.id, &bundle);
    let settings = bundle
        .settings
        .iter()
        .map(|f| {
            let mut f = f.clone();
            f.key = format!("plugin.{}.{}", bundle.id, f.key);
            f
        })
        .collect();
    PluginManifest {
        contract: CONTRACT_VERSION,
        id: bundle.id,
        name: bundle.name,
        version: bundle.version,
        publisher: if bundle.publisher.is_empty() {
            "Ryuzi".to_string()
        } else {
            bundle.publisher
        },
        description: bundle.description,
        homepage: None,
        icon: None,
        categories: vec!["component".to_string()],
        slot: None,
        verified: true,
        experimental: false,
        auth,
        settings,
        mcp: vec![],
        extensions: vec![],
        skills: vec![],
        provider: None,
    }
}

/// Parse `id`'s embedded first-party bundle manifest (PR-1). The single
/// lookup+parse `declared_tool_count`/`declared_tools` always did, exposed
/// whole so `plugin_release_detail` can preview a never-installed
/// component's full manifest (tools, domains, oauth) without any fetch.
/// `None` when `id` has no embedded manifest in
/// [`COMPONENT_BUNDLE_MANIFESTS`] or its TOML fails to parse.
pub fn declared_bundle_manifest(id: &str) -> Option<PluginBundleManifest> {
    COMPONENT_BUNDLE_MANIFESTS
        .iter()
        .find(|(got, _)| *got == id)
        .and_then(|(_, src)| toml::from_str::<PluginBundleManifest>(src).ok())
}

/// The number of tools `id`'s embedded first-party bundle manifest declares
/// (Task 1) — feeds `PluginInfo.tool_count`. `None` when `id` has no embedded
/// manifest in [`COMPONENT_BUNDLE_MANIFESTS`] (e.g. a provider bundle
/// represented by its builtin row, see [`COMPONENT_BACKED_PROVIDER_IDS`]) or
/// its embedded TOML fails to parse.
pub fn declared_tool_count(id: &str) -> Option<u32> {
    declared_bundle_manifest(id).map(|m| m.tools.len() as u32)
}

/// The full declared-tool detail (name, description, writes) `id`'s embedded
/// first-party bundle manifest declares (Task 1) — the same lookup
/// [`declared_tool_count`] uses, but the tools themselves rather than just
/// the count. Feeds `plugin_tools`' (Task 4) declared-manifest fallback.
/// Empty (never an error) when `id` has no embedded manifest here or its
/// embedded TOML fails to parse — same fallback `declared_tool_count` uses.
pub fn declared_tools(id: &str) -> Vec<DeclaredTool> {
    declared_bundle_manifest(id)
        .map(|m| m.tools)
        .unwrap_or_default()
}

/// Every embedded component bundle as a manifest-only plugin. A manifest that
/// fails to parse is logged and skipped rather than panicking, so one bad
/// embedded file can never take the daemon down at startup.
pub fn component_catalog_plugins() -> Vec<CorePlugin> {
    COMPONENT_BUNDLE_MANIFESTS
        .iter()
        .filter(|(id, _)| !provider_registry_owns(id))
        .filter_map(
            |(id, src)| match toml::from_str::<PluginBundleManifest>(src) {
                Ok(bundle) => Some(CorePlugin {
                    manifest: manifest_from_bundle(bundle),
                    harness: None,
                    gateway: None,
                    connector: None,
                    extension: None,
                    provider: None,
                    source: PluginSource::Component,
                }),
                Err(error) => {
                    tracing::error!("component catalog: manifest `{id}` failed to parse: {error}");
                    None
                }
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryuzi_plugin_sdk::{FieldKind, SettingField};

    // PR-3: the bridge derives an auth kind so needs-setup/connect surfaces
    // light up. github/atlassian/bitbucket declare [[oauth]] → oauth;
    // discord declares a secret+required token setting (Task 10) → token —
    // until Task 10 lands, discord derives None here, so this test uses a
    // synthetic bundle for the token case.
    #[test]
    fn bridge_derives_auth_from_bundle() {
        let github = manifest_from_bundle(declared_bundle_manifest("github").unwrap());
        assert_eq!(github.auth.as_ref().map(|a| a.kind), Some(AuthKind::Oauth));

        let mut synthetic = declared_bundle_manifest("github").unwrap();
        synthetic.id = "synth".into();
        synthetic.oauth.clear();
        synthetic.settings = vec![SettingField {
            key: "token".into(),
            label: "Token".into(),
            help: String::new(),
            secret: true,
            required: true,
            kind: FieldKind::String,
            options: vec![],
            default: None,
        }];
        let m = manifest_from_bundle(synthetic);
        assert_eq!(m.auth.as_ref().map(|a| a.kind), Some(AuthKind::Token));
        assert_eq!(
            m.auth.as_ref().and_then(|a| a.setting.as_deref()),
            Some("plugin.synth.token")
        );
        assert_eq!(
            m.settings[0].key, "plugin.synth.token",
            "bridge prefixes bare keys"
        );
    }

    // Task 10: discord now declares its settings for real, so the bridge
    // derives AuthKind::Token from the embedded manifest itself (no more
    // synthetic-bundle workaround, see `bridge_derives_auth_from_bundle`
    // above).
    #[test]
    fn discord_bundle_declares_token_settings_and_derives_token_auth() {
        let m = manifest_from_bundle(declared_bundle_manifest("discord").unwrap());
        assert_eq!(m.auth.as_ref().map(|a| a.kind), Some(AuthKind::Token));
        assert_eq!(
            m.auth.as_ref().and_then(|a| a.setting.as_deref()),
            Some("plugin.discord.token")
        );
        let keys: Vec<&str> = m.settings.iter().map(|f| f.key.as_str()).collect();
        assert!(keys.contains(&"plugin.discord.token"));
        assert!(keys.contains(&"plugin.discord.app_id"));
        assert!(keys.contains(&"plugin.discord.guild_id"));
    }

    // Task 10: atlassian's [[oauth]] profile now carries the PKCE extras +
    // client-id-setting the host-side 3LO wiring needs.
    #[test]
    fn atlassian_profile_carries_pkce_extras_and_client_id_setting() {
        let bundle = declared_bundle_manifest("atlassian").unwrap();
        let p = &bundle.oauth[0];
        assert_eq!(
            p.client_id_setting.as_deref(),
            Some("plugin.atlassian.oauth_client_id")
        );
        assert_eq!(
            p.extra_authorize_params.get("audience").map(String::as_str),
            Some("api.atlassian.com")
        );
    }

    #[test]
    fn every_embedded_manifest_parses_and_matches_its_declared_id() {
        for (id, toml_src) in COMPONENT_BUNDLE_MANIFESTS {
            let manifest: PluginBundleManifest = toml::from_str(toml_src)
                .unwrap_or_else(|e| panic!("component manifest `{id}` failed to parse: {e}"));
            assert_eq!(&manifest.id, id, "declared id must match the embedded slot");
        }
    }

    // `mimo`/`opencode` are embedded (their manifests are real) but ALSO live
    // in the router CATALOG, so they are represented by their provider builtin
    // and skipped here rather than dropped as duplicates at registration.
    #[test]
    fn registers_only_components_no_provider_builtin_already_owns() {
        let plugins = component_catalog_plugins();
        let mut ids: Vec<&str> = plugins.iter().map(|p| p.manifest.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["atlassian", "bitbucket", "discord", "github"]);
    }

    // Every bundle stays reachable for release management even when its
    // registration lost the id to a provider builtin.
    #[test]
    fn is_component_bundle_covers_embedded_and_provider_backed_ids() {
        for id in [
            "github",
            "atlassian",
            "bitbucket",
            "discord",
            "mimo",
            "opencode",
        ] {
            assert!(is_component_bundle(id), "`{id}` is a first-party bundle");
        }
        for id in COMPONENT_BACKED_PROVIDER_IDS {
            assert!(is_component_bundle(id), "`{id}` is a first-party bundle");
        }
        assert!(!is_component_bundle("native"));
        assert!(!is_component_bundle("nope"));
    }

    // A colliding provider component shares an id with an `install_providers`
    // plugin, which registers FIRST and wins. Registering one here would be
    // silently dropped by `PluginHost::add`, so they are excluded by design.
    #[test]
    fn colliding_provider_components_are_not_registered() {
        let plugins = component_catalog_plugins();
        for id in COMPONENT_BACKED_PROVIDER_IDS {
            assert!(
                !plugins.iter().any(|p| p.manifest.id == *id),
                "provider component `{id}` must not be registered — it collides with its builtin"
            );
        }
    }

    #[test]
    fn every_plugin_is_manifest_only_and_component_sourced() {
        for plugin in component_catalog_plugins() {
            assert_eq!(plugin.source, PluginSource::Component);
            assert!(plugin.connector.is_none(), "manifest-only registration");
            assert!(plugin.gateway.is_none(), "manifest-only registration");
            assert!(plugin.harness.is_none(), "manifest-only registration");
            assert!(plugin.provider.is_none(), "manifest-only registration");
        }
    }

    // The embedded set and the excluded-provider set must together cover every
    // component `scripts/plugins/build-first-party.ts` builds and signs, or a
    // newly added bundle would silently never appear in the Plugins hub.
    #[test]
    fn embedded_and_excluded_sets_are_disjoint() {
        for (id, _) in COMPONENT_BUNDLE_MANIFESTS {
            assert!(
                !COMPONENT_BACKED_PROVIDER_IDS.contains(id),
                "`{id}` cannot be both embedded and excluded"
            );
        }
    }

    // Every first-party connector/gateway manifest must declare the tools its
    // component registers, with no duplicate names, so Cockpit can show "what
    // you'll get" before the bundle is ever installed (Task 1).
    #[test]
    fn first_party_connector_manifests_declare_tools() {
        for id in ["github", "atlassian", "bitbucket"] {
            let (_, src) = COMPONENT_BUNDLE_MANIFESTS
                .iter()
                .find(|(got, _)| *got == id)
                .unwrap_or_else(|| panic!("`{id}` must be an embedded component manifest"));
            let m: PluginBundleManifest = toml::from_str(src)
                .unwrap_or_else(|e| panic!("component manifest `{id}` failed to parse: {e}"));
            assert!(!m.tools.is_empty(), "{id} must declare its tools");
            let mut names: Vec<_> = m.tools.iter().map(|t| t.name.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), m.tools.len(), "{id} tool names must be unique");
        }

        // Discord is a gateway component with no agent-facing tools; slash commands are not tools.
        let (_, src) = COMPONENT_BUNDLE_MANIFESTS
            .iter()
            .find(|(got, _)| *got == "discord")
            .unwrap_or_else(|| panic!("`discord` must be an embedded component manifest"));
        let m: PluginBundleManifest =
            toml::from_str(src).unwrap_or_else(|e| panic!("discord manifest failed to parse: {e}"));
        assert!(
            m.tools.is_empty(),
            "discord gateway must declare no agent tools"
        );
    }

    // Task 3: `PluginInfo.tool_count` reads through this lookup for
    // component-backed rows.
    #[test]
    fn declared_tool_count_matches_embedded_manifest_and_none_for_unknown_ids() {
        assert_eq!(declared_tool_count("github"), Some(12));
        assert_eq!(declared_tool_count("atlassian"), Some(10));
        assert_eq!(declared_tool_count("bitbucket"), Some(9));
        // Discord is a gateway component with no agent-facing tools.
        assert_eq!(declared_tool_count("discord"), Some(0));
        // No embedded manifest here (represented by its builtin provider row).
        assert_eq!(declared_tool_count("anthropic"), None);
        assert_eq!(declared_tool_count("nope"), None);
    }

    // Task 4: `plugin_tools`' declared-manifest fallback reads through this
    // lookup for component-backed rows with no live extension/running
    // instance.
    #[test]
    fn declared_tools_matches_the_embedded_manifest_and_is_empty_for_unknown_ids() {
        assert_eq!(declared_tools("github").len(), 12);
        assert_eq!(declared_tools("atlassian").len(), 10);
        assert_eq!(declared_tools("bitbucket").len(), 9);
        assert!(declared_tools("discord").is_empty());
        assert!(declared_tools("anthropic").is_empty());
        assert!(declared_tools("nope").is_empty());
    }
}
