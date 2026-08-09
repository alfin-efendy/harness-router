//! Big-bang v2 first-upgrade migration.
//!
//! Contract 2 (Tasks 1-4) unified every plugin's install format, killed
//! Track D, and moved every plugin's enablement onto `plugin.<id>.enabled`.
//! Neither change is meaningful to a machine that already has plugins
//! installed under the OLD contract-1 on-disk layout or the old
//! `enabled_gateways` CSV setting — [`run`] is what actually carries an
//! upgrading install from v1 to v2, called once at daemon boot
//! (`daemon::build_daemon`, immediately before
//! `remote_catalog::bootstrap_first_party_components`).
//!
//! Destructive by design (spec 2026-08-01): a v1 component install is
//! dropped wholesale rather than upgraded in place — first-party bundles
//! (mimo/opencode) re-bootstrap automatically (see the `_v2` marker rename
//! below), everything else reinstalls from the hub the next time a user
//! visits the Plugins page. `load_active_bundles` (Task 2) already
//! skip-and-warns a contract-1 leftover rather than blinding the whole
//! plugin subsystem, so an upgrading user is never blocked on this
//! migration running — but leaving the stale directory and ledger rows
//! around forever would be a permanent, silent no-op install slot. Settings
//! and OAuth tokens (`plugin.<id>.*`) are never touched by the install-drop
//! step; only the `enabled_gateways` CSV migrates (into the same
//! `plugin.<id>.enabled` keys the toggle UI now writes) and is then deleted.
use crate::settings::SettingsStore;
use crate::store::Store;
use std::path::Path;

/// What [`run`] actually did, so the caller can log a single line instead of
/// staying silent about a potentially surprising, destructive cleanup.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Plugin ids whose on-disk install tree (and release-ledger rows) were
    /// deleted because they were not a valid v2 install.
    pub dropped_installs: Vec<String>,
    /// Plugin/gateway ids carried forward from the legacy `enabled_gateways`
    /// CSV into their own `plugin.<id>.enabled = "true"` setting.
    pub migrated_gateway_ids: Vec<String>,
}

impl MigrationReport {
    /// Nothing to report — the common case on every boot after the first
    /// (idempotence: a second run always produces this).
    pub fn is_empty(&self) -> bool {
        self.dropped_installs.is_empty() && self.migrated_gateway_ids.is_empty()
    }
}

/// Run the v1 -> v2 first-upgrade migration against `plugins_root` (the
/// [`crate::plugins::bundle::ComponentBundleInstaller`] root layout:
/// `<plugins_root>/<plugin_id>/<version>/` plus a `<plugins_root>/<plugin_id>/current`
/// pointer file).
///
/// Two independent cleanups, both idempotent and both best-effort against a
/// missing/absent input rather than erroring:
///
/// 1. **Install trees.** Every immediate subdirectory of `plugins_root` is a
///    plugin id. If it is not a valid v2 install (see [`install_is_v2`]) —
///    including a v1 leftover, a half-finished install with no `current`
///    pointer, or a directory that is missing its manifest entirely — the
///    whole directory is deleted and its release-ledger history
///    ([`Store::clear_component_releases`], which also clears the active
///    pointer since `active` lives on the same rows) is wiped. A directory
///    that already IS a valid v2 install is left completely alone. A
///    missing `plugins_root` (nothing ever installed) is not an error.
/// 2. **Gateway CSV.** If a legacy `enabled_gateways` setting exists (Task 4
///    retired writing it, but never migrated an existing value), every
///    comma-separated id in it gets `plugin.<id>.enabled = "true"` written
///    via the raw store — deliberately NOT through the validated
///    [`SettingsStore::set`], which would reject any id that has not (yet,
///    at this point in boot) registered its `plugin.<id>.enabled` field —
///    and the CSV key is then deleted. A missing CSV key is a no-op.
///
/// Idempotent: a second call against the same `store`/`plugins_root` finds
/// only already-v2 directories and no CSV key, so it returns an empty
/// [`MigrationReport`] and touches nothing.
pub async fn run(
    store: &Store,
    settings: &SettingsStore,
    plugins_root: &Path,
) -> anyhow::Result<MigrationReport> {
    let mut report = MigrationReport::default();

    if let Ok(entries) = std::fs::read_dir(plugins_root) {
        for entry in entries.filter_map(Result::ok) {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            match classify_install(&dir) {
                InstallVerdict::V2 => continue,
                InstallVerdict::Indeterminate(error) => {
                    // Leave it strictly alone. `load_active_bundles` already
                    // skip-and-warns anything unloadable, so the cost of not
                    // cleaning up now is a log line; the cost of guessing
                    // wrong is a destroyed install.
                    tracing::warn!(
                        plugin = %id,
                        "plugins: could not classify install, leaving it untouched: {error}"
                    );
                }
                InstallVerdict::NotV2 => {
                    // Per-directory best-effort: one locked file must not
                    // abort the sweep or the independent CSV migration below.
                    if let Err(error) = std::fs::remove_dir_all(&dir) {
                        tracing::warn!(
                            plugin = %id,
                            "plugins: could not remove v1 leftover (leaving it): {error}"
                        );
                        continue;
                    }
                    if let Err(error) = store.clear_component_releases(&id).await {
                        tracing::warn!(
                            plugin = %id,
                            "plugins: removed v1 leftover but could not clear its ledger rows: {error}"
                        );
                    }
                    report.dropped_installs.push(id);
                }
            }
        }
    }

    if let Some(csv) = settings.get("enabled_gateways").await? {
        for id in csv.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            // Raw write, not `settings.set` — the CSV can name an id whose
            // plugin fields have not been (or will never again be)
            // registered process-wide, and a validated write would bail
            // with "unknown setting" and abort the loop, silently losing
            // every id after it.
            store
                .set_setting_raw(&format!("plugin.{id}.enabled"), "true")
                .await?;
            report.migrated_gateway_ids.push(id.to_string());
        }
        store.delete_setting_raw("enabled_gateways").await?;
    }

    Ok(report)
}

/// A v2 install has a readable `current` pointer whose named version
/// directory holds a `ryuzi-plugin.toml` that parses as a NATIVE contract-2
/// manifest.
///
/// "Native" is load-bearing.
/// [`ryuzi_plugin_sdk::PluginManifest::from_toml`] also accepts a contract-1
/// bundle manifest through the compatibility shim (so a pre-v2 release feed
/// stays installable), which means parse-success alone no longer distinguishes
/// a v1 leftover from a v2 install. This classifier therefore reads the
/// `upgraded` flag from
/// [`ryuzi_plugin_sdk::PluginManifest::from_toml_detecting_legacy`] and treats
/// an upgraded manifest as NOT v2: the migration is about the provenance of
/// the on-disk tree — installed by the v1 installer, whose layout and ledger
/// semantics v2 changed — not merely about whether the manifest text can be
/// read today.
/// What a directory under the plugins root looks like to the migration.
#[derive(Debug)]
enum InstallVerdict {
    /// A readable pointer + manifest that parses as contract 2. Keep.
    V2,
    /// Definitively not a v2 install — the pointer or manifest is absent, or
    /// the manifest is present and does NOT parse as v2. Safe to drop.
    NotV2,
    /// Could not be determined right now (permission denied, file locked,
    /// antivirus scan, concurrent install). NEVER drop on this: an
    /// indeterminate read must not be mistaken for a v1 leftover.
    Indeterminate(std::io::Error),
}

/// Classify one plugin directory. Only `NotFound` counts as evidence of
/// absence; every other I/O error is [`InstallVerdict::Indeterminate`] so a
/// transient failure can never destroy a healthy install.
fn classify_install(plugin_dir: &Path) -> InstallVerdict {
    let current = match std::fs::read_to_string(plugin_dir.join("current")) {
        Ok(current) => current,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return InstallVerdict::NotV2,
        Err(error) => return InstallVerdict::Indeterminate(error),
    };
    let manifest_path = plugin_dir.join(current.trim()).join("ryuzi-plugin.toml");
    let text = match std::fs::read_to_string(manifest_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return InstallVerdict::NotV2,
        Err(error) => return InstallVerdict::Indeterminate(error),
    };
    match ryuzi_plugin_sdk::PluginManifest::from_toml_detecting_legacy(&text) {
        // Parsed only via the contract-1 shim — a v1 leftover, not a v2 install.
        Ok((_, upgraded_from_contract_1)) if upgraded_from_contract_1 => InstallVerdict::NotV2,
        Ok(_) => InstallVerdict::V2,
        Err(_) => InstallVerdict::NotV2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ComponentPluginReleaseRecord;
    use std::sync::Arc;

    /// A genuine contract-1 bundle manifest: no `contract` key at all, and a
    /// bare `component` string instead of the v2 `[component]` table.
    ///
    /// This shape MATTERS. It is exactly what the contract-1 compatibility
    /// shim accepts, so an install written with it is the case that would
    /// silently survive the migration if `classify_install` went back to
    /// treating parse-success as proof of v2-ness.
    fn v1_manifest_toml(id: &str, name: &str, version: &str) -> String {
        format!(
            "id = \"{id}\"\nname = \"{name}\"\nversion = \"{version}\"\n\
             wit-api = \"^0.1.0\"\nlifecycle = \"per-session\"\ncomponent = \"{id}.wasm\"\n"
        )
    }

    /// A minimal, structurally valid v2 manifest — `contract = 2` plus the
    /// two other required fields (`id`, `name`); everything else defaults.
    fn v2_manifest_toml(id: &str, name: &str, version: &str) -> String {
        format!("contract = 2\nid = \"{id}\"\nname = \"{name}\"\nversion = \"{version}\"\n")
    }

    fn write_install(root: &Path, id: &str, version: &str, manifest_toml: &str) {
        let version_dir = root.join(id).join(version);
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(version_dir.join("ryuzi-plugin.toml"), manifest_toml).unwrap();
        std::fs::write(root.join(id).join("current"), version).unwrap();
    }

    fn component_release(plugin_id: &str, version: &str) -> ComponentPluginReleaseRecord {
        ComponentPluginReleaseRecord {
            plugin_id: plugin_id.into(),
            version: version.into(),
            source_url: format!("https://example.test/{plugin_id}/{version}.wasm"),
            sha256: format!("sha256-{plugin_id}-{version}"),
            signing_key_id: "key-1".into(),
            installed_at: 0,
            active: false,
            revoked: false,
            revocation_reason: None,
        }
    }

    /// `(store, settings backed by the SAME store, plugins-root tempdir,
    /// db-file tempfile [kept alive so the sqlite file isn't removed out
    /// from under the still-open pool])`.
    async fn test_harness() -> (
        Store,
        SettingsStore,
        tempfile::TempDir,
        tempfile::NamedTempFile,
    ) {
        let db = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(db.path()).await.unwrap();
        let settings = SettingsStore::new(Arc::new(store.clone()));
        let plugins_root = tempfile::tempdir().unwrap();
        (store, settings, plugins_root, db)
    }

    #[tokio::test]
    async fn drops_v1_installs_and_preserves_settings() {
        let (store, settings, tmp, _db) = test_harness().await;
        write_install(
            tmp.path(),
            "github",
            "0.1.1",
            &v1_manifest_toml("github", "GitHub", "0.1.1"),
        );
        // Settings/tokens AND a release-ledger active pointer, so the test
        // proves the ledger is really cleared (not just the directory).
        store
            .set_setting_raw("plugin.github.enabled", "true")
            .await
            .unwrap();
        store
            .set_setting_raw("plugin.github.token", "secret")
            .await
            .unwrap();
        store
            .upsert_component_release(&component_release("github", "0.1.1"))
            .await
            .unwrap();
        store
            .set_active_component_release("github", "0.1.1")
            .await
            .unwrap();

        let report = run(&store, &settings, tmp.path()).await.unwrap();

        assert_eq!(report.dropped_installs, vec!["github".to_string()]);
        assert!(report.migrated_gateway_ids.is_empty());
        assert!(!tmp.path().join("github").exists());
        // Ledger history (incl. the active pointer, which lives on the same
        // rows) is gone.
        assert!(store
            .active_component_release("github")
            .await
            .unwrap()
            .is_none());
        assert!(store
            .list_component_releases("github")
            .await
            .unwrap()
            .is_empty());
        // Settings and tokens survive untouched.
        assert_eq!(
            settings
                .get("plugin.github.enabled")
                .await
                .unwrap()
                .as_deref(),
            Some("true")
        );
        assert_eq!(
            settings
                .get("plugin.github.token")
                .await
                .unwrap()
                .as_deref(),
            Some("secret")
        );
    }

    /// The classifier must only treat ABSENCE as evidence of "not v2". Any
    /// other I/O error is indeterminate, and an indeterminate directory is
    /// never dropped — a transient lock or permission blip must not destroy a
    /// healthy install. (Regression: an earlier version mapped every `Err` to
    /// "not v2" and deleted real user installs.)
    #[test]
    fn classify_treats_only_absence_as_not_v2() {
        let tmp = tempfile::tempdir().unwrap();

        // Nothing at all -> definitively not a v2 install.
        let missing = tmp.path().join("gone");
        std::fs::create_dir_all(&missing).unwrap();
        assert!(matches!(classify_install(&missing), InstallVerdict::NotV2));

        // Pointer present, manifest absent -> still definitively not v2.
        let dangling = tmp.path().join("dangling");
        std::fs::create_dir_all(&dangling).unwrap();
        std::fs::write(dangling.join("current"), "0.1.0").unwrap();
        assert!(matches!(classify_install(&dangling), InstallVerdict::NotV2));

        // A contract-1 manifest parses as absent-from-v2 -> droppable.
        let v1 = tmp.path().join("v1");
        std::fs::create_dir_all(v1.join("0.1.0")).unwrap();
        std::fs::write(v1.join("current"), "0.1.0").unwrap();
        std::fs::write(
            v1.join("0.1.0/ryuzi-plugin.toml"),
            "contract = 1\nid = \"v1\"\nname = \"V1\"\n",
        )
        .unwrap();
        assert!(matches!(classify_install(&v1), InstallVerdict::NotV2));

        // `current` is a DIRECTORY, so reading it fails with an error that is
        // not NotFound -> indeterminate, must be left alone.
        let locked = tmp.path().join("locked");
        std::fs::create_dir_all(locked.join("current")).unwrap();
        assert!(
            matches!(classify_install(&locked), InstallVerdict::Indeterminate(_)),
            "a non-NotFound read error must be indeterminate, never NotV2"
        );
    }

    #[tokio::test]
    async fn indeterminate_installs_are_never_dropped() {
        let (store, settings, tmp, _db) = test_harness().await;
        // `current` as a directory reproduces a non-NotFound read error.
        std::fs::create_dir_all(tmp.path().join("github/current")).unwrap();

        let report = run(&store, &settings, tmp.path()).await.unwrap();

        assert!(
            report.dropped_installs.is_empty(),
            "an indeterminate install must not be dropped, got {:?}",
            report.dropped_installs
        );
        assert!(
            tmp.path().join("github").exists(),
            "the directory must be left on disk"
        );
    }

    #[tokio::test]
    async fn v2_installs_are_left_alone() {
        let (store, settings, tmp, _db) = test_harness().await;
        write_install(
            tmp.path(),
            "github",
            "0.1.1",
            &v2_manifest_toml("github", "GitHub", "0.1.1"),
        );
        store
            .upsert_component_release(&component_release("github", "0.1.1"))
            .await
            .unwrap();
        store
            .set_active_component_release("github", "0.1.1")
            .await
            .unwrap();

        let report = run(&store, &settings, tmp.path()).await.unwrap();

        assert!(
            report.dropped_installs.is_empty(),
            "a valid v2 install must not be dropped, got {:?}",
            report.dropped_installs
        );
        assert!(tmp.path().join("github").join("0.1.1").exists());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("github/current")).unwrap(),
            "0.1.1"
        );
        // Ledger untouched too.
        assert!(store
            .active_component_release("github")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn gateway_csv_migrates_to_per_plugin_keys() {
        let (store, settings, tmp, _db) = test_harness().await;
        // Two ids, with incidental whitespace, exercising the split/trim path.
        store
            .set_setting_raw("enabled_gateways", "discord, slack")
            .await
            .unwrap();

        let report = run(&store, &settings, tmp.path()).await.unwrap();

        assert_eq!(
            report.migrated_gateway_ids,
            vec!["discord".to_string(), "slack".to_string()]
        );
        assert_eq!(
            settings
                .get("plugin.discord.enabled")
                .await
                .unwrap()
                .as_deref(),
            Some("true")
        );
        assert_eq!(
            settings
                .get("plugin.slack.enabled")
                .await
                .unwrap()
                .as_deref(),
            Some("true")
        );
        assert_eq!(settings.get("enabled_gateways").await.unwrap(), None);
    }

    #[tokio::test]
    async fn run_is_idempotent() {
        let (store, settings, tmp, _db) = test_harness().await;
        write_install(
            tmp.path(),
            "github",
            "0.1.1",
            &v1_manifest_toml("github", "GitHub", "0.1.1"),
        );
        store
            .set_setting_raw("enabled_gateways", "discord")
            .await
            .unwrap();

        let first = run(&store, &settings, tmp.path()).await.unwrap();
        assert!(!first.is_empty(), "first run should have work to report");

        let second = run(&store, &settings, tmp.path()).await.unwrap();
        assert!(
            second.is_empty(),
            "second run must be a clean no-op, got {second:?}"
        );
        // The migrated setting from the first run is untouched by the
        // no-op second run.
        assert_eq!(
            settings
                .get("plugin.discord.enabled")
                .await
                .unwrap()
                .as_deref(),
            Some("true")
        );
    }

    #[tokio::test]
    async fn a_plugin_directory_missing_its_current_pointer_is_dropped() {
        // A half-finished/corrupted install: a version directory exists but
        // the `current` pointer was never written.
        let (store, settings, tmp, _db) = test_harness().await;
        std::fs::create_dir_all(tmp.path().join("orphan").join("0.1.0")).unwrap();

        let report = run(&store, &settings, tmp.path()).await.unwrap();

        assert_eq!(report.dropped_installs, vec!["orphan".to_string()]);
        assert!(!tmp.path().join("orphan").exists());
    }

    #[tokio::test]
    async fn a_missing_plugins_root_is_not_an_error() {
        let (store, settings, tmp, _db) = test_harness().await;
        let missing_root = tmp.path().join("does-not-exist");

        let report = run(&store, &settings, &missing_root).await.unwrap();

        assert!(report.is_empty());
    }
}
