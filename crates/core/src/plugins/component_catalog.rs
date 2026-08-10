//! The first-party WASM component bundles shipped from `plugins/<id>`,
//! surfaced as manifest-only [`CorePlugin`]s so they are enumerable through
//! the `list_plugins` RPC that backs Cockpit's Plugins hub.
//!
//! This replaces the removed declarative `plugins::catalog`. Two deliberate
//! differences from that module:
//!
//! - **Manifest-only for every COMPONENT surface.** A bundle's executable
//!   *component* capability (gateway, connector-over-wasm, provider) is still
//!   discovered off disk in `daemon::build_daemon` from the *installed* bundle
//!   root; nothing here instantiates a component. These entries exist so a
//!   bundle is visible and enumerable even before it is installed. The one
//!   capability that is NOT component-backed — a manifest's own `[[mcp]]`
//!   block, which needs no wasm at all — is wired up right here, see
//!   [`super::declarative_connector_for`] and the `atlassian-rovo` note below.
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
//!
//! # Declarative-only ("component-less") bundles
//!
//! Not every first-party bundle carries a `[component]`. `atlassian-rovo` is a
//! remote-MCP-over-HTTP manifest with no wasm at all — its whole behavior is
//! one `[[mcp]]` entry plus a `${setting:...}`-substituted `Authorization`
//! header. Such a bundle MUST be registered here with a real
//! [`crate::connector::Connector`] (built off its own manifest by
//! [`super::declarative_connector_for`]), because:
//!
//! - `mcp_sync::sync_plugin_mcp` — the only thing that ever creates a plugin's
//!   `mcp_servers` row — early-returns when `plugin.connector.is_none()`, so a
//!   manifest-only registration makes every enable/OAuth-completion sync a
//!   silent no-op and the plugin never gets an Apps row at all;
//! - there is nothing to discover off disk for it: `bundle::load_active_bundles`
//!   only yields component-backed bundles, and the embedded manifest here is
//!   the same bytes the signed release ships, so the connector is correct
//!   whether or not the release was ever installed.

use ryuzi_plugin_sdk::{AuthKind, AuthSpec, DeclaredTool, PluginManifest};

use super::host::{qualified_setting_key, CorePlugin, PluginSource};

/// The non-colliding first-party bundles, embedded from their in-repo
/// manifest. Keep in sync with the component list in
/// `scripts/plugins/build-first-party.ts`: every id there is either here or
/// in [`COMPONENT_BACKED_PROVIDER_IDS`] — pinned by
/// `tests::the_signer_component_list_is_exactly_the_registered_id_set`, which
/// parses that script's `COMPONENTS` array so adding a bundle to the signer
/// without registering it here fails.
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
    // Declarative-only (no `[component]`): a remote-MCP-over-HTTP manifest.
    // Registered WITH a connector — see this module's "Declarative-only
    // bundles" doc section for why that is load-bearing rather than cosmetic.
    (
        "atlassian-rovo",
        include_str!("../../../../plugins/atlassian-rovo/ryuzi-plugin.toml"),
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

/// Derive the coarse auth contract a manifest implies (PR-3): declared OAuth
/// profiles → oauth; else a secret+required setting (e.g. discord's bot
/// token) → token, pointing `setting` at its fully-qualified key; else none.
/// Presentation/status metadata ONLY — component connect flows run through
/// the profile RPCs, never the classic declarative OAuth engine.
///
/// v2 manifests still don't author an `[auth]` block for a component bundle
/// (the bridge derives it, same as v1's bundle bridge did) — this now reads
/// straight off `&PluginManifest` (`oauth` + `settings`, both present on
/// every v2 manifest) instead of the deleted `PluginBundleManifest`.
pub(crate) fn derive_manifest_auth(manifest: &PluginManifest) -> Option<AuthSpec> {
    if !manifest.oauth.is_empty() {
        return Some(AuthSpec {
            kind: AuthKind::Oauth,
            ..Default::default()
        });
    }
    manifest
        .settings
        .iter()
        .find(|f| f.secret && f.required)
        .map(|f| AuthSpec {
            kind: AuthKind::Token,
            setting: Some(qualified_setting_key(&manifest.id, &f.key)),
            ..Default::default()
        })
}

/// Parse `id`'s embedded first-party manifest (PR-1). The single lookup+parse
/// `declared_tool_count`/`declared_tools` always did, exposed whole so
/// `plugin_release_detail` can preview a never-installed component's full
/// manifest (tools, domains, oauth) without any fetch. `None` when `id` has
/// no embedded manifest in [`COMPONENT_BUNDLE_MANIFESTS`] or its TOML fails
/// to parse.
///
/// Unlike v1's `declared_bundle_manifest`, this is the RAW embedded manifest
/// (bare settings keys, no derived `[auth]`) — the same shape
/// [`component_catalog_plugins`] bridges into a `CorePlugin`. Callers that
/// need the derived/qualified presentation (e.g.
/// `host::component_required_settings_configured`) get it as-is here and
/// qualify keys themselves via `host::qualified_setting_key`.
pub fn declared_manifest(id: &str) -> Option<PluginManifest> {
    COMPONENT_BUNDLE_MANIFESTS
        .iter()
        .find(|(got, _)| *got == id)
        .and_then(|(_, src)| PluginManifest::from_toml(src).ok())
}

/// The number of tools `id`'s embedded first-party manifest declares
/// (Task 1) — feeds `PluginInfo.tool_count`. `None` when `id` has no embedded
/// manifest in [`COMPONENT_BUNDLE_MANIFESTS`] (e.g. a provider bundle
/// represented by its builtin row, see [`COMPONENT_BACKED_PROVIDER_IDS`]) or
/// its embedded TOML fails to parse.
pub fn declared_tool_count(id: &str) -> Option<u32> {
    declared_manifest(id).map(|m| m.tools.len() as u32)
}

/// The full declared-tool detail (name, description, writes) `id`'s embedded
/// first-party manifest declares (Task 1) — the same lookup
/// [`declared_tool_count`] uses, but the tools themselves rather than just
/// the count. Feeds `plugin_tools`' (Task 4) declared-manifest fallback.
/// Empty (never an error) when `id` has no embedded manifest here or its
/// embedded TOML fails to parse — same fallback `declared_tool_count` uses.
pub fn declared_tools(id: &str) -> Vec<DeclaredTool> {
    declared_manifest(id).map(|m| m.tools).unwrap_or_default()
}

/// Every embedded component bundle as a plugin whose only live capability is
/// the `[[mcp]]` connector its own manifest implies (usually none). A manifest
/// that fails to parse is logged and skipped rather than panicking, so one bad
/// embedded file can never take the daemon down at startup.
///
/// v2 manifests are the UI manifest already — no `manifest_from_bundle`
/// bridge step remains — but `auth`/settings-key qualification still happen
/// here, same as v1's bridge: the embedded TOML author never writes `[auth]`
/// for a component (it is derived), and `[[settings]]` keys stay BARE in the
/// manifest, qualified to `plugin.<id>.<key>` only at this registration
/// boundary.
///
/// ORDER IS LOAD-BEARING: the connector is built from the manifest while its
/// `[[settings]]` keys are still BARE, and only then are those keys qualified
/// for the registered row. `DeclarativeConnector` qualifies `settings[].key`
/// itself on every `ensure_auth` (see `declarative.rs`), and
/// `PluginManifest::validate` — which `declarative_plugin` re-runs — REJECTS a
/// `plugin.`-prefixed bare key outright, so handing it the qualified manifest
/// would both fail to build and, if it somehow built, look for
/// `plugin.<id>.plugin.<id>.<key>`. Same split `install_sources::
/// confirm_plugin_install_at` makes for the same reason (its C1 comment).
pub fn component_catalog_plugins() -> Vec<CorePlugin> {
    COMPONENT_BUNDLE_MANIFESTS
        .iter()
        .filter(|(id, _)| !provider_registry_owns(id))
        .filter_map(|(id, src)| match PluginManifest::from_toml(src) {
            Ok(mut manifest) => {
                if manifest.auth.is_none() {
                    manifest.auth = derive_manifest_auth(&manifest);
                }
                // Built off the still-BARE manifest (see this function's doc).
                let connector = super::declarative_connector_for(&manifest);
                for field in &mut manifest.settings {
                    field.key = qualified_setting_key(&manifest.id, &field.key);
                }
                if manifest.categories.is_empty() {
                    manifest.categories = vec!["component".to_string()];
                }
                manifest.verified = true;
                Some(CorePlugin {
                    manifest,
                    harness: None,
                    gateway: None,
                    connector,
                    provider: None,
                    source: PluginSource::Builtin,
                })
            }
            Err(error) => {
                tracing::error!("component catalog: manifest `{id}` failed to parse: {error}");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryuzi_plugin_sdk::{FieldKind, SettingField};
    use std::collections::BTreeSet;

    /// Repo-root-relative path from `crates/core` (this crate's manifest dir)
    /// — the same accessor shape `plugins::github_e2e::repo_path` uses to read
    /// committed, non-Rust repo files from a test.
    fn repo_path(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel)
    }

    /// Every `id` in `scripts/plugins/build-first-party.ts`'s `COMPONENTS`
    /// array — the authoritative list of what CI builds and SIGNS as a
    /// first-party release. PARSED out of the script rather than duplicated
    /// here, so the signer's list and this module's registration can never
    /// drift silently (which is exactly how `atlassian-rovo` shipped inert).
    ///
    /// Deliberately strict: a renamed array, a moved terminator, or a parse
    /// that recovers implausibly few ids all PANIC. A helper that quietly
    /// returned an empty set would make every coverage assertion below
    /// vacuous — the very failure mode this test exists to end.
    fn signer_component_ids() -> BTreeSet<String> {
        const PATH: &str = "scripts/plugins/build-first-party.ts";
        const MARKER: &str = "export const COMPONENTS: ComponentSpec[] = [";
        let src = std::fs::read_to_string(repo_path(PATH))
            .unwrap_or_else(|e| panic!("reading {PATH}: {e}"));
        let after = src
            .split_once(MARKER)
            .unwrap_or_else(|| panic!("{PATH} no longer declares `{MARKER}` — update this test"))
            .1;
        let body = after
            .split_once("\n];")
            .unwrap_or_else(|| panic!("{PATH}'s COMPONENTS array has no `];` terminator"))
            .0;
        let mut ids = BTreeSet::new();
        for line in body.lines() {
            // Strip `//` comments first, so prose inside the array can never
            // be misread as an entry.
            let code = line.split_once("//").map_or(line, |(before, _)| before);
            let Some((_, after_key)) = code.split_once("id: \"") else {
                continue;
            };
            let Some((id, _)) = after_key.split_once('"') else {
                continue;
            };
            ids.insert(id.to_string());
        }
        assert!(
            ids.len() >= 6,
            "parsed only {ids:?} out of {PATH}'s COMPONENTS array — this parser is broken, and a \
             broken parser makes every coverage assertion built on it vacuous"
        );
        ids
    }

    // PR-3: the bridge derives an auth kind so needs-setup/connect surfaces
    // light up. github/atlassian/bitbucket declare [[oauth]] → oauth;
    // discord declares a secret+required token setting (Task 10) → token —
    // until Task 10 lands, discord derives None here, so this test uses a
    // synthetic manifest for the token case.
    #[test]
    fn bridge_derives_auth_from_manifest() {
        let github = declared_manifest("github").unwrap();
        assert_eq!(
            derive_manifest_auth(&github).map(|a| a.kind),
            Some(AuthKind::Oauth)
        );

        let mut synthetic = declared_manifest("github").unwrap();
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
        let auth = derive_manifest_auth(&synthetic).expect("secret+required setting derives auth");
        assert_eq!(auth.kind, AuthKind::Token);
        assert_eq!(auth.setting.as_deref(), Some("plugin.synth.token"));
    }

    // Task 10: discord now declares its settings for real, so the bridge
    // derives AuthKind::Token from the embedded manifest itself (no more
    // synthetic-manifest workaround, see `bridge_derives_auth_from_manifest`
    // above). Goes through `component_catalog_plugins()` (not
    // `declared_manifest` directly) because settings-key qualification and
    // auth derivation both happen at that registration boundary.
    #[test]
    fn discord_manifest_declares_token_settings_and_derives_token_auth() {
        let plugins = component_catalog_plugins();
        let m = &plugins
            .iter()
            .find(|p| p.manifest.id == "discord")
            .expect("discord is registered")
            .manifest;
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
        let manifest = declared_manifest("atlassian").unwrap();
        let p = &manifest.oauth[0];
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
            let manifest = PluginManifest::from_toml(toml_src)
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
        assert_eq!(
            ids,
            [
                "atlassian",
                "atlassian-rovo",
                "bitbucket",
                "discord",
                "github"
            ]
        );
    }

    // Every bundle stays reachable for release management even when its
    // registration lost the id to a provider builtin.
    #[test]
    fn is_component_bundle_covers_embedded_and_provider_backed_ids() {
        for id in [
            "github",
            "atlassian",
            "atlassian-rovo",
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

    // Every embedded row is `Builtin`-sourced and carries NO component-backed
    // capability — those are discovered off disk from the installed bundle, not
    // instantiated here. The one exception, and the load-bearing half of this
    // test, is the connector: it comes from the manifest's own `[[mcp]]` block
    // (no wasm involved) and must be present EXACTLY when that block is
    // non-empty. `is_some()` for a bundle that declares `[[mcp]]` is what makes
    // `mcp_sync::sync_plugin_mcp` able to create its Apps row at all;
    // `is_none()` for one that declares none keeps a component bundle's
    // capability set honest (`capabilities()` feeds the hub).
    //
    // Was `assert!(plugin.manifest.component.is_some())` before this branch,
    // which is why registering the first component-LESS bundle broke it.
    // Component-less is a legitimate shape (`PluginManifest` always allowed it,
    // and the signer now publishes it) — what a registration here must still
    // never do is claim a capability it cannot back.
    #[test]
    fn every_plugin_is_builtin_sourced_with_a_connector_exactly_when_it_declares_mcp() {
        for plugin in component_catalog_plugins() {
            let id = &plugin.manifest.id;
            assert_eq!(plugin.source, PluginSource::Builtin, "{id}");
            assert!(plugin.gateway.is_none(), "{id}: component-backed, off disk");
            assert!(plugin.harness.is_none(), "{id}: never from a manifest");
            assert!(
                plugin.provider.is_none(),
                "{id}: component-backed, off disk"
            );
            assert_eq!(
                plugin.connector.is_some(),
                !plugin.manifest.mcp.is_empty(),
                "{id}: a connector must exist exactly when the manifest declares [[mcp]] — \
                 missing means sync_plugin_mcp silently creates no Apps row, spurious means the \
                 hub advertises a capability with nothing behind it"
            );
            assert!(
                plugin.manifest.component.is_some() || !plugin.manifest.mcp.is_empty(),
                "{id}: a component-less registration must at least declare [[mcp]], or it is an \
                 inert row with no capability at all"
            );
        }
    }

    // The embedded set and the excluded-provider set must together cover
    // EXACTLY the components `scripts/plugins/build-first-party.ts` builds and
    // signs — in both directions, against the signer's own list.
    //
    // Until this branch this test only asserted the two sets do not OVERLAP,
    // and so stayed green while `atlassian-rovo` was added to the signer,
    // built, and signed without ever being registered here. An id registered
    // nowhere in this module is INVISIBLE: `daemon::build_daemon` populates the
    // plugin registry from `component_catalog_plugins()` and
    // `api::plugins_api::list_plugins` iterates that registry, so the plugin
    // never reaches Cockpit's hub — no install, no enable, no settings surface
    // to type its credential into — and `is_component_bundle` reports false, so
    // even the release-management surface hides it.
    #[test]
    fn the_signer_component_list_is_exactly_the_registered_id_set() {
        let signed = signer_component_ids();
        let registered: BTreeSet<String> = COMPONENT_BUNDLE_MANIFESTS
            .iter()
            .map(|(id, _)| (*id).to_string())
            .chain(
                COMPONENT_BACKED_PROVIDER_IDS
                    .iter()
                    .map(|id| (*id).to_string()),
            )
            .collect();

        let unregistered: Vec<&String> = signed.difference(&registered).collect();
        assert!(
            unregistered.is_empty(),
            "scripts/plugins/build-first-party.ts builds and SIGNS {unregistered:?}, but nothing \
             in component_catalog registers them — they can never appear in the Plugins hub. Add \
             each to COMPONENT_BUNDLE_MANIFESTS, or (for a provider bundle whose id a router \
             CATALOG builtin already owns) to COMPONENT_BACKED_PROVIDER_IDS."
        );
        let unsigned: Vec<&String> = registered.difference(&signed).collect();
        assert!(
            unsigned.is_empty(),
            "{unsigned:?} are registered here as first-party bundles, but the signer publishes no \
             release for them — a stale registration offering an install that can never resolve."
        );

        // Kept from the original assertion: an id must never be in both sets.
        for (id, _) in COMPONENT_BUNDLE_MANIFESTS {
            assert!(
                !COMPONENT_BACKED_PROVIDER_IDS.contains(id),
                "`{id}` cannot be both embedded and excluded"
            );
        }
    }

    /// PROPERTY: `atlassian-rovo` — the first component-LESS first-party
    /// bundle — must register with a REAL connector. Manifest-only would leave
    /// it visible but inert: `mcp_sync::sync_plugin_mcp` is the only code that
    /// ever writes a plugin's `mcp_servers` row and it early-returns on
    /// `connector.is_none()`, so enabling the plugin (or completing its OAuth)
    /// would silently create nothing. The row itself is proven end to end in
    /// `mcp_sync`'s tests; this pins the registration shape it depends on,
    /// including the qualification ORDER — a connector built from the
    /// already-qualified manifest would look for
    /// `plugin.atlassian-rovo.plugin.atlassian-rovo.basic_credential`.
    #[test]
    fn a_declarative_only_bundle_registers_with_a_connector_and_qualified_settings() {
        let plugins = component_catalog_plugins();
        let rovo = plugins
            .iter()
            .find(|p| p.manifest.id == "atlassian-rovo")
            .expect("atlassian-rovo is a signed first-party bundle and must be registered");

        assert!(
            rovo.manifest.component.is_none(),
            "atlassian-rovo is declarative-only — it must declare no [component]"
        );
        assert_eq!(
            rovo.manifest.mcp.len(),
            1,
            "its whole behavior is one remote [[mcp]] entry"
        );
        assert!(
            rovo.connector.is_some(),
            "a declarative-only bundle MUST carry a connector or sync_plugin_mcp no-ops"
        );
        assert_eq!(
            rovo.capabilities(),
            vec!["connector"],
            "its only capability is the manifest's own [[mcp]] block"
        );
        assert!(rovo.manifest.verified, "first-party rows are verified");

        let keys: Vec<&str> = rovo
            .manifest
            .settings
            .iter()
            .map(|f| f.key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec!["plugin.atlassian-rovo.basic_credential"],
            "the settings key must be qualified at this registration boundary"
        );
        let auth = rovo.manifest.auth.as_ref().expect("derived token auth");
        assert_eq!(auth.kind, AuthKind::Token);
        assert_eq!(
            auth.setting.as_deref(),
            Some("plugin.atlassian-rovo.basic_credential")
        );
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
            let m = PluginManifest::from_toml(src)
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
        let m = PluginManifest::from_toml(src)
            .unwrap_or_else(|e| panic!("discord manifest failed to parse: {e}"));
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
        // A declarative remote-MCP plugin declares no `[[tools]]` — v2's
        // `validate()` forbids them without a `[component]` — and cannot: its
        // tools are whatever the remote MCP server reports at probe time.
        assert_eq!(declared_tool_count("atlassian-rovo"), Some(0));
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
        assert!(declared_tools("atlassian-rovo").is_empty());
        assert!(declared_tools("anthropic").is_empty());
        assert!(declared_tools("nope").is_empty());
    }
}
