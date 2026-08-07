//! Task 11: install a plugin from a local folder or a git URL, with a
//! tiered-trust model — the ecosystem-opening counterpart to the signed
//! catalog (`plugins::remote_catalog`/`plugins::bundle`).
//!
//! # Trust model
//!
//! | Surface | Signed catalog | Unsigned (local folder / git URL) |
//! |---|---|---|
//! | skills, commands, `[[hooks]]`, `[[jobs]]` | active on install | active after the normal install confirm |
//! | `[[mcp]]` (stdio = arbitrary process!) | active on install | requires EXPLICIT trust acceptance |
//! | `[component]` (WASM, sandboxed) | active on install | requires EXPLICIT trust acceptance |
//! | `allow_self_auth`, `[gateway]` | first-party only | **never** (structural — `signing_key_id` for an unsigned install is never `first_party_key::FIRST_PARTY_KEY_ID`, see [`UNSIGNED_SIGNING_KEY_ID`]) |
//!
//! Trust acceptance is recorded as the setting `plugin.<id>.trusted =
//! "true"`, written ONLY by [`confirm_plugin_install`] and ONLY when the
//! caller passed `accept_trust: true`. If the user declines, the install
//! still proceeds — but the mcp/component surfaces stay inert
//! ([`crate::plugins::host::component_surfaces_trusted`] and
//! [`component_surfaces_trusted_for`] are the shared gate every consuming
//! surface checks).
//!
//! # Two-phase flow
//!
//! Mirrors `skills_install`'s `begin_install`/`confirm_install` gate
//! pattern (staged clone/copy held in a process-global map under a
//! single-use, TTL'd token) and reuses its git-clone/directory-copy
//! primitives via [`crate::install_common`] rather than re-implementing
//! them. Unlike skills, there is no "curated" fast path here — every plugin
//! source install stops at [`begin_plugin_install_from_source`]'s
//! [`PluginTrustPrompt`] before [`confirm_plugin_install`] ever touches the
//! live install dir, since a plugin folder can carry a manifest declaring
//! ANY surface (a skill pack cannot).
//!
//! # On-disk layout
//!
//! Same versioned-directory + `current`-pointer convention
//! [`crate::plugins::bundle::ComponentBundleInstaller`] uses for signed
//! bundles: `~/.config/ryuzi/plugins/<id>/<version>/` + a sibling `current`
//! text file naming the active version. A declarative-only manifest (no
//! `[component]`) still gets this exact layout — just without a component
//! file/`release.json`/release-ledger row — which is why
//! `control::lifecycle::ControlPlane::enabled_plugin_content_roots` must
//! union `bundle::load_active_bundles` (component-only) with
//! `PluginHost::list()`'s `Installed` entries (see that function's doc for
//! the fix).
//!
//! Provenance is stamped as `install.json` inside the version dir (never
//! written for a `Catalog` provenance — nothing here ever installs one) and
//! read back by every WASM bundle-discovery site via
//! [`read_install_provenance`], defaulting to `Catalog` when absent so every
//! pre-Task-11 install path (first-party embedded, signed catalog) is
//! trusted exactly as before.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, bail, Context, Result};
use ryuzi_plugin_sdk::{McpTransportDef, PluginManifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugins::host::{qualified_setting_key, CorePlugin, InstallProvenance, PluginSource};
use crate::settings::SettingsStore;
use crate::store::{ComponentPluginReleaseRecord, Store};

/// The `signing_key_id` stamped into the `component_plugin_releases` row for
/// every component installed through this module. Deliberately never equal
/// to `first_party_key::FIRST_PARTY_KEY_ID` — that inequality is what keeps
/// `HostPolicy::for_installed_bundle`'s `allow_self_auth`/`allow_gateway`
/// derivation (both gated on `signing_key_id ==
/// first_party_key::FIRST_PARTY_KEY_ID`, see `plugins::runtime`) structurally
/// `false` for every unsigned install, satisfying the trust table's "never"
/// column without this module needing its own copy of that gate.
pub const UNSIGNED_SIGNING_KEY_ID: &str = "unsigned-install";

/// How long a staged (unconfirmed) plugin install stays valid before
/// [`confirm_plugin_install`] rejects it and the caller must start over via
/// [`begin_plugin_install_from_source`]. Mirrors
/// `skills_install::STAGED_INSTALL_TTL_MS`.
const STAGED_INSTALL_TTL_MS: i64 = 10 * 60 * 1000;

/// One `[[mcp]]` entry, summarized for the trust prompt — stdio's command is
/// spelled out in full (never just "trusts a command exists"), matching the
/// brief's "list exactly what will run".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerSummary {
    pub name: String,
    pub transport: String,
    /// stdio: `"<command> <args...>"`; http: the URL.
    pub detail: String,
}

/// One `[[tools]]` entry a `[component]` bundle declares, with its `writes`
/// flag — the exact thing the trust prompt must show for a WASM component
/// (brief: "tool names with their `writes` flags").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentToolSummary {
    pub name: String,
    pub writes: bool,
}

/// What a declared `[component]` would be granted, summarized for the trust
/// prompt.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentTrustSummary {
    pub network_hosts: Vec<String>,
    pub tools: Vec<ComponentToolSummary>,
}

/// Counts of the surfaces that are active immediately after the normal
/// install confirm, regardless of trust acceptance (skills/commands/hooks/
/// jobs — see this module's trust-table doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSurfacesSummary {
    pub commands: usize,
    pub skills: usize,
    pub hooks: usize,
    pub jobs: usize,
}

/// What [`begin_plugin_install_from_source`] found, shown to the user before
/// [`confirm_plugin_install`] touches the live install dir. `trust_required`
/// is `true` iff `mcp_servers` is non-empty or `component` is `Some` — the
/// caller's UI should only show the explicit-trust checkbox in that case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginTrustPrompt {
    pub token: String,
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub surfaces: PluginSurfacesSummary,
    pub mcp_servers: Vec<McpServerSummary>,
    pub component: Option<ComponentTrustSummary>,
    pub trust_required: bool,
}

/// What a completed [`confirm_plugin_install`] returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub dir: PathBuf,
    pub provenance: InstallProvenance,
    pub trust_required: bool,
    /// Whether `plugin.<id>.trusted` was actually written by this call
    /// (`accept_trust && trust_required`).
    pub trusted: bool,
}

/// A plugin source string is either an absolute local folder or a git
/// remote. `file://` is always treated as a git remote (git itself
/// understands the `file://` transport) — deliberately distinct from a bare
/// absolute path, which is staged by copying rather than cloning. This lets
/// a test exercise the real git-clone code path hermetically against a
/// local-only `file://` fixture repo without ever touching the network.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedPluginSource {
    LocalPath(PathBuf),
    GitUrl(String),
}

fn parse_plugin_source(source: &str) -> Result<ParsedPluginSource> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        bail!("plugin source must not be empty");
    }
    let looks_like_git_url = trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("git@")
        || trimmed.starts_with("ssh://")
        || trimmed.starts_with("file://")
        || trimmed.ends_with(".git");
    if looks_like_git_url {
        return Ok(ParsedPluginSource::GitUrl(trimmed.to_string()));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        bail!("plugin source must be an absolute local path or a git URL, got {trimmed:?}");
    }
    Ok(ParsedPluginSource::LocalPath(path))
}

/// Staged state for a plugin-source install awaiting [`confirm_plugin_install`].
/// Holds the staged directory alive (`_temp`'s `Drop` deletes it once the
/// token is removed from [`staging_map`], whether by a successful confirm,
/// an expired/rejected confirm, or — best-effort only — never, if the
/// process exits first).
struct StagedPluginInstall {
    source: String,
    parsed: ParsedPluginSource,
    _temp: tempfile::TempDir,
    staging_dir: PathBuf,
    manifest: PluginManifest,
    trust_required: bool,
    created_ms: i64,
}

fn staging_map() -> &'static Mutex<std::collections::HashMap<String, StagedPluginInstall>> {
    static MAP: OnceLock<Mutex<std::collections::HashMap<String, StagedPluginInstall>>> =
        OnceLock::new();
    MAP.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Count `*.md` files directly under `dir` (non-recursive summary — good
/// enough for a trust-prompt count; the real discovery at session-start,
/// `harness::native::commands::read_command_dir`, does recurse).
fn count_markdown_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md") && e.path().is_file())
        .count()
}

/// Count subdirectories of `dir` that carry a `SKILL.md` — the same
/// convention `harness::native::skills::read_skills` uses to discover an
/// installed skill.
fn count_skill_dirs(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir() && e.path().join("SKILL.md").is_file())
        .count()
}

fn mcp_summary(def: &ryuzi_plugin_sdk::McpServerDef) -> McpServerSummary {
    let (transport, detail) = match def.transport {
        McpTransportDef::Stdio => {
            let mut parts = vec![def.command.clone().unwrap_or_default()];
            parts.extend(def.args.iter().cloned());
            ("stdio".to_string(), parts.join(" "))
        }
        McpTransportDef::Http => ("http".to_string(), def.url.clone().unwrap_or_default()),
    };
    McpServerSummary {
        name: def.name.clone(),
        transport,
        detail,
    }
}

fn trust_required_for(manifest: &PluginManifest) -> bool {
    !manifest.mcp.is_empty() || manifest.component.is_some()
}

/// Phase 1: stage `source` (cloning a git URL or copying a local folder into
/// a temp dir — the ORIGINAL local folder is never mutated or moved),
/// parse+validate its `ryuzi-plugin.toml` (v2), and build the
/// [`PluginTrustPrompt`] the caller must show before [`confirm_plugin_install`]
/// can proceed.
pub async fn begin_plugin_install_from_source(source: &str) -> Result<PluginTrustPrompt> {
    let parsed = parse_plugin_source(source)?;
    let temp = tempfile::tempdir().context("creating a temp staging directory")?;
    let staging_dir = temp.path().join("plugin");
    match &parsed {
        ParsedPluginSource::LocalPath(path) => {
            if !path.is_dir() {
                bail!("local plugin source is not a directory: {}", path.display());
            }
            crate::install_common::copy_dir_recursive(path, &staging_dir)
                .with_context(|| format!("copying local plugin source {}", path.display()))?;
        }
        ParsedPluginSource::GitUrl(url) => {
            crate::install_common::git_clone_repo(url, &staging_dir)
                .await
                .with_context(|| format!("cloning plugin source {url}"))?;
        }
    }

    let manifest_toml = std::fs::read_to_string(staging_dir.join("ryuzi-plugin.toml"))
        .with_context(|| format!("{source}: missing ryuzi-plugin.toml"))?;
    let manifest = PluginManifest::from_toml(&manifest_toml)
        .with_context(|| format!("{source}: invalid ryuzi-plugin.toml"))?;

    let surfaces = PluginSurfacesSummary {
        commands: count_markdown_files(&staging_dir.join("commands")),
        skills: count_skill_dirs(&staging_dir.join("skills")),
        hooks: manifest.hooks.len(),
        jobs: manifest.jobs.len(),
    };
    let mcp_servers: Vec<McpServerSummary> = manifest.mcp.iter().map(mcp_summary).collect();
    let component = manifest.component.as_ref().map(|_| ComponentTrustSummary {
        network_hosts: manifest
            .permissions
            .network
            .iter()
            .map(|entry| entry.0.clone())
            .collect(),
        tools: manifest
            .tools
            .iter()
            .map(|tool| ComponentToolSummary {
                name: tool.name.clone(),
                writes: tool.writes,
            })
            .collect(),
    });
    let trust_required = trust_required_for(&manifest);

    let token = crate::paths::new_id();
    let staged = StagedPluginInstall {
        source: source.to_string(),
        parsed,
        _temp: temp,
        staging_dir,
        manifest: manifest.clone(),
        trust_required,
        created_ms: crate::paths::now_ms(),
    };
    staging_map().lock().unwrap().insert(token.clone(), staged);

    Ok(PluginTrustPrompt {
        token,
        id: manifest.id,
        name: manifest.name,
        publisher: manifest.publisher,
        surfaces,
        mcp_servers,
        component,
        trust_required,
    })
}

/// Remove a staged install before it was ever confirmed, freeing its temp
/// directory immediately rather than waiting out the TTL. Mirrors
/// `skills_install::discard_staged_install`.
pub fn discard_staged_plugin_install(token: &str) {
    staging_map().lock().unwrap().remove(token);
}

/// Phase 2: complete a staged install after the user has reviewed its
/// [`PluginTrustPrompt`]. Single-use — the token is removed from
/// [`staging_map`] up front. `accept_trust` only has an effect when the
/// staged manifest actually needs it (`trust_required`); accepting trust for
/// a manifest that declares neither `[[mcp]]` nor `[component]` writes
/// nothing.
///
/// Moves the staged directory into
/// `~/.config/ryuzi/plugins/<id>/<version>/` + flips the `current` pointer
/// (the exact layout [`crate::plugins::bundle::load_active_bundles`] and
/// [`crate::control::lifecycle::ControlPlane::enabled_plugin_content_roots`]'s
/// union fallback both expect), stamps `install.json` with the resolved
/// provenance, seeds a `component_plugin_releases` ledger row when the
/// manifest declares a `[component]` (required for
/// `bundle::load_active_bundles` to admit it at all — that loader
/// re-verifies the component hash against this row regardless of signing),
/// writes `plugin.<id>.trusted` iff `accept_trust && trust_required`, then
/// runs the Task 7/8/9/10 syncs against a transiently-built, fully
/// behavioral [`CorePlugin`] (connector + hooks/jobs) — NOT the manifest-only
/// row `plugins::install_installed_plugins` keeps in `PluginHost` for
/// identity/enablement bookkeeping.
pub async fn confirm_plugin_install(
    token: &str,
    accept_trust: bool,
    store: &Store,
    settings: &SettingsStore,
) -> Result<InstalledPluginInfo> {
    confirm_plugin_install_at(
        token,
        accept_trust,
        store,
        settings,
        &crate::plugins::bundle::installed_bundle_root(),
    )
    .await
}

/// [`confirm_plugin_install`] with an injectable install root — production
/// always uses [`crate::plugins::bundle::installed_bundle_root`] (the real
/// per-user config dir); tests inject a hermetic tempdir so nothing here
/// ever touches — let alone deletes from — the real
/// `~/.config/ryuzi/plugins`.
async fn confirm_plugin_install_at(
    token: &str,
    accept_trust: bool,
    store: &Store,
    settings: &SettingsStore,
    root: &Path,
) -> Result<InstalledPluginInfo> {
    let staged = staging_map()
        .lock()
        .unwrap()
        .remove(token)
        .ok_or_else(|| anyhow!("install session expired — start the install again"))?;
    if crate::paths::now_ms() - staged.created_ms > STAGED_INSTALL_TTL_MS {
        bail!("install session expired — start the install again");
    }

    let manifest = staged.manifest;
    let id = manifest.id.clone();
    let version = if manifest.version.is_empty() {
        "0.0.0".to_string()
    } else {
        manifest.version.clone()
    };
    // `SettingsStore::set` validates every key against the process-wide
    // `plugin_field` registry, which normally only gains an entry for `id`
    // once a `CorePlugin` is registered into a `PluginHost`
    // (`Registries::add_plugin`) — which for THIS plugin won't happen until
    // the next daemon restart picks it up via `install_installed_plugins`.
    // Register its fields (incl. `plugin.<id>.trusted`) right now so the
    // trust-acceptance write below (and any settings write a caller makes
    // before that restart) is recognized immediately.
    crate::plugins::host::register_plugin_fields(&manifest);

    let plugin_root = root.join(&id);
    let version_dir = plugin_root.join(&version);
    std::fs::create_dir_all(&plugin_root)
        .with_context(|| format!("creating plugin root {}", plugin_root.display()))?;
    if version_dir.exists() {
        // A prior install of this exact version already claimed the
        // destination (a repeat confirm, or a reinstall after uninstall) —
        // replace it wholesale, mirroring
        // `ComponentBundleInstaller::install_verified`'s identical handling.
        std::fs::remove_dir_all(&version_dir).with_context(|| {
            format!(
                "removing existing plugin directory {} before reinstall",
                version_dir.display()
            )
        })?;
    }
    std::fs::rename(&staged.staging_dir, &version_dir)
        .with_context(|| format!("moving staged plugin into {}", version_dir.display()))?;

    let provenance = match &staged.parsed {
        ParsedPluginSource::LocalPath(_) => InstallProvenance::LocalPath,
        ParsedPluginSource::GitUrl(url) => InstallProvenance::GitUrl(url.clone()),
    };
    let installed_at = crate::paths::now_ms();
    write_install_stamp(&version_dir, &provenance, installed_at)
        .context("writing install.json provenance stamp")?;

    let pointer = plugin_root.join("current");
    crate::agents::transaction::atomic_write(&pointer, version.as_bytes())
        .context("writing the active plugin pointer")?;

    if let Some(component) = &manifest.component {
        let component_path = version_dir.join(&component.file);
        let bytes = std::fs::read(&component_path)
            .with_context(|| format!("reading component file {}", component_path.display()))?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        // `bundle::load_active_bundles` parses `release.json` (id/version/
        // component_sha256 must agree with the manifest and the ledger row)
        // — write a minimal one; nothing here claims a signature over it.
        let release = serde_json::json!({
            "id": id,
            "version": version,
            "wit-api": component.wit_api,
            "component_url": staged.source,
            "component_sha256": sha256,
        });
        std::fs::write(
            version_dir.join("release.json"),
            serde_json::to_vec(&release)?,
        )
        .context("writing release.json")?;

        let record = ComponentPluginReleaseRecord {
            plugin_id: id.clone(),
            version: version.clone(),
            source_url: staged.source.clone(),
            sha256,
            signing_key_id: UNSIGNED_SIGNING_KEY_ID.to_string(),
            installed_at,
            active: false,
            revoked: false,
            revocation_reason: None,
        };
        store.upsert_component_release(&record).await?;
        store.set_active_component_release(&id, &version).await?;
    }

    let trust_required = staged.trust_required;
    let trusted = accept_trust && trust_required;
    if trusted {
        settings
            .set(&qualified_setting_key(&id, "trusted"), "true")
            .await?;
    }

    // Task 7/8/9/10 syncs need a fully behavioral plugin (connector +
    // hooks/jobs), not the manifest-only row boot registration keeps in
    // `PluginHost` — build one transiently, with the real `Installed`
    // source so `component_surfaces_trusted` reads the correct provenance.
    let plugin: CorePlugin = crate::plugins::declarative::declarative_plugin(
        manifest.clone(),
        PluginSource::Installed {
            dir: version_dir.clone(),
            provenance: provenance.clone(),
        },
    )?;
    crate::plugins::mcp_sync::sync_plugin_mcp(store, settings, &plugin).await?;
    crate::plugins::automation_sync::sync_plugin_automations(store, &plugin).await?;

    Ok(InstalledPluginInfo {
        id,
        name: manifest.name,
        version,
        dir: version_dir,
        provenance,
        trust_required,
        trusted,
    })
}

/// The on-disk `install.json` shape: `{ "provenance": "local-path" | {
/// "gitUrl": "..." }, "installedAt": <unix ms> }`. Never written for
/// [`InstallProvenance::Catalog`] — nothing in this module ever installs one
/// (a signed catalog install goes through
/// `bundle::ComponentBundleInstaller` instead, which never stamps this
/// file), so [`read_install_provenance`] safely defaults to `Catalog` when
/// the file is absent.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstallStamp {
    provenance: ProvenanceStamp,
    #[serde(rename = "installedAt")]
    installed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum ProvenanceStamp {
    LocalPath(String),
    GitUrl {
        #[serde(rename = "gitUrl")]
        git_url: String,
    },
}

pub(crate) fn write_install_stamp(
    version_dir: &Path,
    provenance: &InstallProvenance,
    installed_at: i64,
) -> Result<()> {
    let provenance = match provenance {
        InstallProvenance::LocalPath => ProvenanceStamp::LocalPath("local-path".to_string()),
        InstallProvenance::GitUrl(url) => ProvenanceStamp::GitUrl {
            git_url: url.clone(),
        },
        InstallProvenance::Catalog => {
            // Nothing in this module ever installs a Catalog-provenance
            // plugin; guard rather than silently stamp a wrong value.
            bail!("refusing to stamp install.json for a Catalog-provenance install");
        }
    };
    let stamp = InstallStamp {
        provenance,
        installed_at,
    };
    let json = serde_json::to_vec_pretty(&stamp).context("serializing install.json")?;
    crate::agents::transaction::atomic_write(&version_dir.join("install.json"), &json)
}

/// Read `version_dir`'s `install.json` stamp (see [`InstallStamp`]),
/// defaulting to [`InstallProvenance::Catalog`] when the file is missing or
/// unparseable — every pre-Task-11 install path (first-party embedded,
/// signed catalog) never wrote it, and both are trusted by construction.
pub fn read_install_provenance(version_dir: &Path) -> InstallProvenance {
    let Ok(raw) = std::fs::read_to_string(version_dir.join("install.json")) else {
        return InstallProvenance::Catalog;
    };
    match serde_json::from_str::<InstallStamp>(&raw) {
        Ok(stamp) => match stamp.provenance {
            ProvenanceStamp::LocalPath(_) => InstallProvenance::LocalPath,
            ProvenanceStamp::GitUrl { git_url } => InstallProvenance::GitUrl(git_url),
        },
        Err(_) => InstallProvenance::Catalog,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::Registries;

    async fn open_settings() -> (
        std::sync::Arc<Store>,
        SettingsStore,
        tempfile::NamedTempFile,
    ) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = std::sync::Arc::new(Store::open(tmp.path()).await.unwrap());
        let settings = SettingsStore::new(store.clone());
        (store, settings, tmp)
    }

    fn write_manifest(dir: &Path, toml: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("ryuzi-plugin.toml"), toml).unwrap();
    }

    fn write_command(dir: &Path, name: &str) {
        let commands = dir.join("commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(commands.join(format!("{name}.md")), "# do a thing").unwrap();
    }

    fn write_skill(dir: &Path, name: &str) {
        let skill_dir = dir.join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: test\n---\nbody").unwrap();
    }

    const DECLARATIVE_MANIFEST: &str = r#"
contract = 2
id = "acme-local"
name = "Acme Local"
publisher = "acme"
"#;

    // ---------- local-folder declarative-only install ----------

    #[tokio::test]
    async fn local_folder_declarative_only_plugin_installs_and_registers() {
        let (store, settings, _tmp) = open_settings().await;
        let source_dir = tempfile::tempdir().unwrap();
        write_manifest(source_dir.path(), DECLARATIVE_MANIFEST);
        write_command(source_dir.path(), "hello");
        write_skill(source_dir.path(), "greeting");

        let prompt = begin_plugin_install_from_source(source_dir.path().to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(prompt.id, "acme-local");
        assert!(!prompt.trust_required, "no mcp/component surfaces declared");
        assert_eq!(prompt.surfaces.commands, 1);
        assert_eq!(prompt.surfaces.skills, 1);

        let root = tempfile::tempdir().unwrap();
        let info = confirm_plugin_install_at(&prompt.token, false, &store, &settings, root.path())
            .await
            .unwrap();
        assert_eq!(info.id, "acme-local");
        assert_eq!(info.version, "0.0.0", "manifest declared no version");
        assert_eq!(info.provenance, InstallProvenance::LocalPath);
        assert!(!info.trusted);

        // Registers via the boot-scan path: `install_installed_plugins`
        // scanning the same root must find it as an Installed CorePlugin.
        let mut regs = Registries::new();
        crate::plugins::install_installed_plugins(&mut regs, root.path());
        let registered = regs.plugins.get("acme-local").expect("must register");
        assert!(matches!(registered.source, PluginSource::Installed { .. }));

        // Its commands/skills roots are discoverable: the install dir laid
        // out by confirm_plugin_install must contain the exact directories
        // `enabled_plugin_content_roots`'s union fallback (dir.join(...))
        // reads.
        let PluginSource::Installed { dir, .. } = &registered.source else {
            panic!("expected Installed source");
        };
        assert!(dir.join("commands").join("hello.md").is_file());
        assert!(dir
            .join("skills")
            .join("greeting")
            .join("SKILL.md")
            .is_file());

        // The original source folder is untouched — never moved or mutated.
        assert!(source_dir.path().join("ryuzi-plugin.toml").is_file());
    }

    // ---------- mcp trust gate ----------

    const MCP_MANIFEST: &str = r#"
contract = 2
id = "acme-mcp"
name = "Acme MCP"

[[mcp]]
name = "svc"
transport = "stdio"
command = "acme-mcp-server"
args = ["--flag"]
"#;

    #[tokio::test]
    async fn mcp_manifest_reports_trust_required_and_syncs_no_rows_without_acceptance() {
        let (store, settings, _tmp) = open_settings().await;
        let source_dir = tempfile::tempdir().unwrap();
        write_manifest(source_dir.path(), MCP_MANIFEST);

        let prompt = begin_plugin_install_from_source(source_dir.path().to_str().unwrap())
            .await
            .unwrap();
        assert!(prompt.trust_required);
        assert_eq!(prompt.mcp_servers.len(), 1);
        assert_eq!(prompt.mcp_servers[0].name, "svc");
        assert_eq!(prompt.mcp_servers[0].transport, "stdio");
        assert_eq!(prompt.mcp_servers[0].detail, "acme-mcp-server --flag");

        let root = tempfile::tempdir().unwrap();
        let info = confirm_plugin_install_at(&prompt.token, false, &store, &settings, root.path())
            .await
            .unwrap();
        assert!(info.trust_required);
        assert!(!info.trusted, "acceptance was withheld");
        assert_eq!(
            settings
                .get(&qualified_setting_key("acme-mcp", "trusted"))
                .await
                .unwrap(),
            None,
            "the trust setting must not be written without acceptance"
        );
        assert!(
            crate::mcp::list_servers(&store).await.unwrap().is_empty(),
            "an untrusted mcp entry must sync no rows"
        );
    }

    #[tokio::test]
    async fn mcp_manifest_with_acceptance_writes_trust_and_syncs() {
        let (store, settings, _tmp) = open_settings().await;
        let source_dir = tempfile::tempdir().unwrap();
        write_manifest(source_dir.path(), MCP_MANIFEST);

        let prompt = begin_plugin_install_from_source(source_dir.path().to_str().unwrap())
            .await
            .unwrap();

        let root = tempfile::tempdir().unwrap();
        let info = confirm_plugin_install_at(&prompt.token, true, &store, &settings, root.path())
            .await
            .unwrap();
        assert!(info.trusted);
        assert_eq!(
            settings
                .get(&qualified_setting_key("acme-mcp", "trusted"))
                .await
                .unwrap()
                .as_deref(),
            Some("true")
        );
        let rows = crate::mcp::list_servers(&store).await.unwrap();
        assert_eq!(rows.len(), 1, "a trusted mcp entry must sync its row");
        assert_eq!(rows[0].plugin_id.as_deref(), Some("acme-mcp"));
    }

    #[tokio::test]
    async fn accepting_trust_for_a_manifest_that_needs_none_writes_nothing() {
        let (store, settings, _tmp) = open_settings().await;
        let source_dir = tempfile::tempdir().unwrap();
        write_manifest(source_dir.path(), DECLARATIVE_MANIFEST);

        let prompt = begin_plugin_install_from_source(source_dir.path().to_str().unwrap())
            .await
            .unwrap();
        assert!(!prompt.trust_required);

        let root = tempfile::tempdir().unwrap();
        let info = confirm_plugin_install_at(&prompt.token, true, &store, &settings, root.path())
            .await
            .unwrap();
        assert!(
            !info.trusted,
            "trust_required was false — acceptance is a no-op"
        );
        assert_eq!(
            settings
                .get(&qualified_setting_key("acme-local", "trusted"))
                .await
                .unwrap(),
            None
        );
    }

    // ---------- git URL staging ----------

    fn run_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git must be on PATH for this test");
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }

    fn file_url(path: &Path) -> String {
        let mut s = path.display().to_string().replace('\\', "/");
        if !s.starts_with('/') {
            s = format!("/{s}");
        }
        format!("file://{s}")
    }

    #[tokio::test]
    async fn git_url_source_stages_via_clone() {
        let repo_dir = tempfile::tempdir().unwrap();
        write_manifest(repo_dir.path(), DECLARATIVE_MANIFEST);
        write_command(repo_dir.path(), "hello");
        run_git(repo_dir.path(), &["init", "-q"]);
        run_git(
            repo_dir.path(),
            &["config", "user.email", "test@example.com"],
        );
        run_git(repo_dir.path(), &["config", "user.name", "Test"]);
        run_git(repo_dir.path(), &["add", "-A"]);
        run_git(repo_dir.path(), &["commit", "-q", "-m", "initial"]);

        let url = file_url(repo_dir.path());
        let prompt = begin_plugin_install_from_source(&url).await.unwrap();
        assert_eq!(prompt.id, "acme-local");
        assert_eq!(prompt.surfaces.commands, 1);

        let (store, settings, _tmp) = open_settings().await;
        let root = tempfile::tempdir().unwrap();
        let info = confirm_plugin_install_at(&prompt.token, false, &store, &settings, root.path())
            .await
            .unwrap();
        assert!(matches!(info.provenance, InstallProvenance::GitUrl(ref got) if got == &url));
        assert!(root
            .path()
            .join("acme-local")
            .join("0.0.0")
            .join("commands")
            .join("hello.md")
            .is_file());
    }

    // ---------- component trust gate ----------

    const COMPONENT_MANIFEST_TEMPLATE: &str = r#"
contract = 2
id = "acme-component"
name = "Acme Component"
version = "0.1.0"

[component]
file = "plugin.wasm"
wit-api = "^0.1.0"
lifecycle = "per-call"

[[tools]]
name = "do_thing"
description = "Does a thing"
writes = true
"#;

    #[tokio::test]
    async fn component_manifest_reports_trust_required_with_tools_and_writes_flags() {
        let source_dir = tempfile::tempdir().unwrap();
        write_manifest(source_dir.path(), COMPONENT_MANIFEST_TEMPLATE);
        std::fs::write(source_dir.path().join("plugin.wasm"), b"fake wasm bytes").unwrap();

        let prompt = begin_plugin_install_from_source(source_dir.path().to_str().unwrap())
            .await
            .unwrap();
        assert!(prompt.trust_required);
        let component = prompt.component.expect("component summary");
        assert_eq!(component.tools.len(), 1);
        assert_eq!(component.tools[0].name, "do_thing");
        assert!(component.tools[0].writes);
    }

    #[tokio::test]
    async fn unsigned_component_seeds_a_release_row_with_a_non_first_party_signing_key() {
        let (store, settings, _tmp) = open_settings().await;
        let source_dir = tempfile::tempdir().unwrap();
        write_manifest(source_dir.path(), COMPONENT_MANIFEST_TEMPLATE);
        std::fs::write(source_dir.path().join("plugin.wasm"), b"fake wasm bytes").unwrap();

        let prompt = begin_plugin_install_from_source(source_dir.path().to_str().unwrap())
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        confirm_plugin_install_at(&prompt.token, true, &store, &settings, root.path())
            .await
            .unwrap();

        let record = store
            .active_component_release("acme-component")
            .await
            .unwrap()
            .expect("a release row must be seeded for a component-backed install");
        assert_eq!(record.signing_key_id, UNSIGNED_SIGNING_KEY_ID);
        assert_ne!(
            record.signing_key_id,
            crate::plugins::first_party_key::FIRST_PARTY_KEY_ID,
            "an unsigned install must never carry the first-party signing key id"
        );
    }

    // ---------- provenance stamp round-trip ----------

    #[test]
    fn install_provenance_round_trips_local_path_and_git_url() {
        let dir = tempfile::tempdir().unwrap();
        write_install_stamp(dir.path(), &InstallProvenance::LocalPath, 123).unwrap();
        assert_eq!(
            read_install_provenance(dir.path()),
            InstallProvenance::LocalPath
        );

        let dir2 = tempfile::tempdir().unwrap();
        write_install_stamp(
            dir2.path(),
            &InstallProvenance::GitUrl("https://example.com/acme.git".into()),
            456,
        )
        .unwrap();
        assert_eq!(
            read_install_provenance(dir2.path()),
            InstallProvenance::GitUrl("https://example.com/acme.git".into())
        );
    }

    #[test]
    fn install_provenance_defaults_to_catalog_when_the_stamp_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_install_provenance(dir.path()),
            InstallProvenance::Catalog
        );
    }

    // ---------- staged-token hygiene ----------

    #[tokio::test]
    async fn confirm_rejects_an_unknown_or_already_consumed_token() {
        let (store, settings, _tmp) = open_settings().await;
        let root = tempfile::tempdir().unwrap();
        let err = confirm_plugin_install_at("no-such-token", false, &store, &settings, root.path())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    #[tokio::test]
    async fn confirm_is_single_use() {
        let (store, settings, _tmp) = open_settings().await;
        let source_dir = tempfile::tempdir().unwrap();
        write_manifest(source_dir.path(), DECLARATIVE_MANIFEST);
        let prompt = begin_plugin_install_from_source(source_dir.path().to_str().unwrap())
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        confirm_plugin_install_at(&prompt.token, false, &store, &settings, root.path())
            .await
            .unwrap();
        let err = confirm_plugin_install_at(&prompt.token, false, &store, &settings, root.path())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    #[tokio::test]
    async fn discard_frees_a_staged_install_before_confirm() {
        let (store, settings, _tmp) = open_settings().await;
        let source_dir = tempfile::tempdir().unwrap();
        write_manifest(source_dir.path(), DECLARATIVE_MANIFEST);
        let prompt = begin_plugin_install_from_source(source_dir.path().to_str().unwrap())
            .await
            .unwrap();
        discard_staged_plugin_install(&prompt.token);
        let root = tempfile::tempdir().unwrap();
        let err = confirm_plugin_install_at(&prompt.token, false, &store, &settings, root.path())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    // ---------- source parsing ----------

    #[test]
    fn parse_plugin_source_rejects_a_relative_path() {
        let err = parse_plugin_source("relative/path").unwrap_err();
        assert!(err.to_string().contains("absolute"));
    }

    #[test]
    fn parse_plugin_source_rejects_empty_input() {
        assert!(parse_plugin_source("").is_err());
        assert!(parse_plugin_source("   ").is_err());
    }

    #[test]
    fn parse_plugin_source_classifies_https_and_ssh_as_git() {
        assert!(matches!(
            parse_plugin_source("https://example.com/acme.git").unwrap(),
            ParsedPluginSource::GitUrl(_)
        ));
        assert!(matches!(
            parse_plugin_source("git@example.com:acme/plugin.git").unwrap(),
            ParsedPluginSource::GitUrl(_)
        ));
    }
}
