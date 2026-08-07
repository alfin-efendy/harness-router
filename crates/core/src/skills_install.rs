//! Installer for git-backed native skills and plugin-bundled skill packs.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub(crate) const PROVENANCE_FILE: &str = ".ryuzi-skill.json";
const CURATED_SKILL_SOURCES: &[(&str, &str)] = &[
    ("superpowers", "https://github.com/obra/superpowers"),
    ("obra/superpowers", "https://github.com/obra/superpowers"),
];

/// A curated skill pack the Cockpit catalog offers before it's installed.
/// Distinct from `CURATED_SKILL_SOURCES`, which is an alias table for
/// `parse_skill_source` (several aliases may map to one repo) — this list
/// has exactly one entry per unique repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratedSkillPack {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub repo: &'static str,
}

const CURATED_SKILL_PACKS: &[CuratedSkillPack] = &[CuratedSkillPack {
    id: "superpowers",
    name: "Superpowers",
    description: "Curated workflow and development skills",
    repo: "https://github.com/obra/superpowers",
}];

pub fn curated_skill_packs() -> &'static [CuratedSkillPack] {
    CURATED_SKILL_PACKS
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkillEntry {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkillPack {
    pub id: String,
    pub name: String,
    pub source: String,
    pub plugin_id: Option<String>,
    pub installed_at: String,
    pub skills: Vec<InstalledSkillEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkillInfo {
    pub id: String,
    pub name: String,
    pub source: String,
    pub plugin_id: Option<String>,
    pub installed_at: String,
    pub skill_count: usize,
}

/// One agent-catalog row per installed skill (PR-2 fix D). `pack` carries the
/// owning pack's display name for plugin-installed packs, `None` for
/// standalone skills — the frontend groups by it. `skill_name` is the SKILL.md
/// frontmatter name, i.e. the exact string the runtime's by-name matching
/// (`skill_guidance`, `global_skill_listed`) compares against — unlike
/// [`InstalledSkillInfo`]'s `id`/pack-level grouping (still used by the hub
/// RPC, `list_installed_skills`), which collapses a whole pack to one row
/// keyed by an id nothing at runtime matches by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSkillCatalogRow {
    pub skill_name: String,
    pub label: String,
    pub pack: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SkillInstallProvenance {
    source: String,
    plugin_id: Option<String>,
    installed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedSkillSource {
    repo: String,
    repo_name: String,
}

#[derive(Debug, Clone)]
struct SkillDescriptor {
    display_name: String,
    normalized_name: String,
    source_dir: PathBuf,
}

/// A repo that installs as multiple skills sharing one plugin id, discovered
/// either from `.codex-plugin/plugin.json` or a bare `skills/*/SKILL.md`
/// layout. Unlike the pre-v2 shape, this carries no `PluginManifest` at all —
/// the v2 SDK manifest dropped `skills`/`extensions` entirely, so there is
/// nothing left to synthesize, write, or read back for a skill pack; a full
/// plugin-folder install (a real, possibly-manifest-bearing `PluginSource`)
/// is deferred to a later task. `skills` are already resolved
/// `SkillDescriptor`s, each carrying its own absolute `source_dir` into the
/// temp clone.
#[derive(Debug, Clone)]
struct PackDescriptor {
    plugin_id: String,
    /// Display name for the pack, shown as `InstalledSkillPack::name`.
    name: String,
    skills: Vec<SkillDescriptor>,
}

#[derive(Debug, Clone)]
enum Discovery {
    Single(SkillDescriptor),
    // Boxed: PackDescriptor is ~800 bytes vs Single's ~72
    // (clippy::large_enum_variant).
    Pack(Box<PackDescriptor>),
}

#[derive(Debug, Clone)]
struct InstallRoots {
    config_root: PathBuf,
    skills_root: PathBuf,
    plugins_root: PathBuf,
}

impl InstallRoots {
    fn new(config_root: PathBuf) -> Self {
        Self {
            skills_root: config_root.join("skills"),
            plugins_root: config_root.join("plugins"),
            config_root,
        }
    }

    fn for_user() -> Result<Self> {
        #[cfg(test)]
        if let Some(root) = std::env::var_os("RYUZI_TEST_CONFIG_ROOT") {
            return Ok(Self::new(PathBuf::from(root)));
        }

        let home = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve home directory"))?;
        Ok(Self::new(home.join(".config/ryuzi")))
    }

    fn ensure_exists(&self) -> Result<()> {
        std::fs::create_dir_all(&self.config_root)?;
        std::fs::create_dir_all(&self.skills_root)?;
        std::fs::create_dir_all(&self.plugins_root)?;
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CodexPluginJson {
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    repository: Option<String>,
    skills: Option<String>,
    author: Option<CodexPluginAuthor>,
    interface: Option<CodexPluginInterface>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CodexPluginAuthor {
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct CodexPluginInterface {
    display_name: Option<String>,
    short_description: Option<String>,
    developer_name: Option<String>,
    website_url: Option<String>,
}

#[async_trait::async_trait]
trait RepoCloner {
    /// Clone `source` into `dest`. Returns the resolved commit SHA when it can
    /// be determined (`None` for test doubles that don't produce a git repo).
    async fn clone_repo(&self, source: &ParsedSkillSource, dest: &Path) -> Result<Option<String>>;
}

struct GitRepoCloner;

#[async_trait::async_trait]
impl RepoCloner for GitRepoCloner {
    async fn clone_repo(&self, source: &ParsedSkillSource, dest: &Path) -> Result<Option<String>> {
        // The clone still has `.git` at this point; `copy_dir_recursive`
        // strips it later when the tree is installed.
        crate::install_common::git_clone_repo(&source.repo, dest).await
    }
}

// NOTE: the ungated `install_skill_source` / `install_skill_source_recorded`
// convenience wrappers were removed — every production install path now routes
// through the trust gate (`begin_install`/`confirm_install` or
// `install_skill_source_gated`). The injectable core `install_skill_source_with`
// remains for the refresh path and tests; `install_skill_source_with_recorded`
// is now test-only (the gated/begin paths inline the equivalent ledger write).

/// The live skills root (`~/.config/ryuzi/skills`, or the injected root under
/// `RYUZI_TEST_CONFIG_ROOT` in tests) — where every installed native skill
/// and materialized skill-pack skill lives on disk. A thin public accessor
/// over the otherwise-private [`InstallRoots`], so callers outside this
/// module never need to duplicate the home-dir/env-override resolution
/// logic.
pub fn skills_root() -> Result<PathBuf> {
    Ok(InstallRoots::for_user()?.skills_root)
}

pub fn list_installed_skills() -> Result<Vec<InstalledSkillInfo>> {
    let roots = InstallRoots::for_user()?;
    list_installed_skills_in(&roots)
}

/// One agent-catalog row per installed skill (PR-2 fix D) — feeds
/// `crate::agents::catalog::build_live_catalog`'s skills source. Unlike
/// `list_installed_skills` (kept as-is for the hub RPC), a multi-skill pack
/// contributes one row per member skill instead of collapsing to a single
/// pack-level row.
pub fn list_installed_skill_catalog_rows() -> Result<Vec<InstalledSkillCatalogRow>> {
    let roots = InstallRoots::for_user()?;
    list_installed_skill_catalog_rows_in(&roots)
}

/// PR-2 fix E compat shim: profiles written before per-skill binding stored
/// the PACK id in `skills` (the only id the old collapsed catalog row
/// offered). Expand any bound element that equals an installed pack id to
/// that pack's member skill names; anything else (an already-per-skill
/// binding, or a stale id naming neither) passes through untouched. Those
/// legacy pack-id bindings matched nothing at runtime before (the runtime
/// only ever compares by skill name), so expansion only ever adds what the
/// user visibly toggled on — it can't newly grant something they didn't
/// already ask for.
pub fn expand_skill_bindings(bound: &[String]) -> Vec<String> {
    match InstallRoots::for_user() {
        Ok(roots) => expand_skill_bindings_in(&roots, bound),
        Err(_) => bound.to_vec(),
    }
}

/// One installed pack's full detail (including its individual
/// [`InstalledSkillEntry`] list — [`InstalledSkillInfo`]'s `skill_count`
/// alone can't name them), by pack id. Feeds `plugin_tools`' (Task 4)
/// skill-pack branch. `None` — never an error — when `id` is not an
/// installed pack, or the on-disk scan fails for any reason: this is
/// best-effort display data, same discipline `plugin_release_detail` gives
/// its own best-effort disk reads.
pub fn get_installed_skill_pack(id: &str) -> Option<InstalledSkillPack> {
    let roots = InstallRoots::for_user().ok()?;
    read_installed_pack(&roots, id).ok()
}

pub fn remove_installed_skill(id: &str) -> Result<()> {
    let roots = InstallRoots::for_user()?;
    remove_installed_skill_in(&roots, id)
}

/// Like `remove_installed_skill`, but also deletes the pack's `plugin_installs`
/// ledger row and any `plugin_attach_status` row, so a reinstall starts from a
/// clean ledger state instead of resurrecting stale trust/pin metadata.
pub async fn remove_installed_skill_recorded(id: &str, store: &crate::store::Store) -> Result<()> {
    let roots = InstallRoots::for_user()?;
    remove_installed_skill_recorded_with(id, &roots, store).await
}

/// Injectable core of `remove_installed_skill_recorded` — takes explicit
/// `roots` so a hermetic test can install a pack into a tempdir and prove the
/// ledger + attach rows are deleted alongside its artifacts (the public
/// wrapper resolves `InstallRoots::for_user()`, which reads the operator's
/// real `$HOME`; the install seam that would set one up is crate-private, so
/// this deletion can only be tested in-crate).
async fn remove_installed_skill_recorded_with(
    id: &str,
    roots: &InstallRoots,
    store: &crate::store::Store,
) -> Result<()> {
    remove_installed_skill_in(roots, id)?;
    store.delete_plugin_install(id).await?;
    store
        .with_conn({
            let id = id.to_string();
            move |c| {
                c.execute(
                    "DELETE FROM plugin_attach_status WHERE plugin_id=?1",
                    rusqlite::params![id],
                )
                .map(|_| ())
            }
        })
        .await?;
    Ok(())
}

/// Refreshes an installed skill pack and keeps the pack's `plugin_installs`
/// ledger row in sync with the refreshed on-disk content. A bare refresh
/// re-clones and reinstalls the CURRENTLY RECORDED source (not a new one),
/// but still writes a fresh tree, so without this the ledger's `fingerprint`
/// goes stale — the next `update_installed_pack`/`update_all_packs`
/// local-edit guard would then false-positive a `LocalEdits` result for a
/// refresh that changed nothing the user asked for. `resolved_commit` is left
/// at its prior value (never nulled): the refresh path doesn't expose the
/// freshly cloned commit the way `update_installed_pack_with` does. When no
/// ledger record exists yet (a legacy pack refreshed before this ledger
/// existed), this backfills one — same trust-tier rule as
/// `install_skill_source_with_recorded`.
pub async fn refresh_installed_skill_recorded(
    id: &str,
    store: &crate::store::Store,
) -> Result<InstalledSkillPack> {
    let roots = InstallRoots::for_user()?;
    let cloner = GitRepoCloner;
    refresh_installed_skill_recorded_with(id, &roots, &cloner, store).await
}

async fn refresh_installed_skill_recorded_with(
    id: &str,
    roots: &InstallRoots,
    cloner: &impl RepoCloner,
    store: &crate::store::Store,
) -> Result<InstalledSkillPack> {
    let prior = store.get_plugin_install(id).await?;
    let refreshed = refresh_installed_skill_with(id, roots, cloner).await?;
    let fingerprint = pack_fingerprint(roots, &refreshed)?;
    let now = crate::paths::now_ms();
    let kind = if refreshed.plugin_id.is_some() {
        "plugin_pack"
    } else {
        "single_skill"
    };
    let record = match &prior {
        Some(rec) => crate::store::PluginInstallRecord {
            plugin_id: refreshed.id.clone(),
            kind: kind.into(),
            source_spec: rec.source_spec.clone(),
            resolved_commit: rec.resolved_commit.clone(),
            fingerprint,
            installed_at: rec.installed_at,
            updated_at: now,
            pinned: rec.pinned,
            pin_reason: rec.pin_reason.clone(),
            trust_tier: rec.trust_tier.clone(),
            trust_ack_at: rec.trust_ack_at,
            trust_ack_summary: rec.trust_ack_summary.clone(),
        },
        None => {
            let trust_tier = if is_curated_source(&refreshed.source) {
                "curated"
            } else {
                "acknowledged"
            };
            crate::store::PluginInstallRecord {
                plugin_id: refreshed.id.clone(),
                kind: kind.into(),
                source_spec: refreshed.source.clone(),
                resolved_commit: None,
                fingerprint,
                installed_at: now,
                updated_at: now,
                pinned: false,
                pin_reason: None,
                trust_tier: trust_tier.into(),
                trust_ack_at: if trust_tier == "acknowledged" {
                    Some(now)
                } else {
                    None
                },
                trust_ack_summary: None,
            }
        }
    };
    // The pack's identity (id) can change across a refresh (e.g. an upstream
    // rename) — same handling as `update_installed_pack_with`: drop the old
    // row instead of leaving a stale duplicate behind.
    if let Some(rec) = &prior {
        if record.plugin_id != rec.plugin_id {
            store.delete_plugin_install(&rec.plugin_id).await?;
        }
    }
    store.upsert_plugin_install(&record).await?;
    Ok(refreshed)
}

async fn install_skill_source_with(
    source: &str,
    roots: &InstallRoots,
    cloner: &impl RepoCloner,
) -> Result<InstalledSkillPack> {
    let (pack, _commit) = install_skill_source_with_commit(source, roots, cloner).await?;
    Ok(pack)
}

/// Shared install orchestration for both the plain and ledger-recording entry
/// points: parses the source, clones it, discovers the install target, and
/// installs it — returning the resolved commit (from `RepoCloner::clone_repo`)
/// alongside the installed pack, so `install_skill_source_with_recorded` can
/// write it into the ledger without cloning the repo a second time.
async fn install_skill_source_with_commit(
    source: &str,
    roots: &InstallRoots,
    cloner: &impl RepoCloner,
) -> Result<(InstalledSkillPack, Option<String>)> {
    roots.ensure_exists()?;
    let source = parse_skill_source(source)?;
    let temp = tempfile::tempdir()?;
    let repo_dir = temp.path().join("repo");
    let commit = cloner.clone_repo(&source, &repo_dir).await?;
    let discovered = discover_install_target(&repo_dir, &source)?;
    let pack = match discovered {
        Discovery::Single(skill) => install_single_skill(roots, &source, skill)?,
        Discovery::Pack(pack) => install_plugin_pack(roots, &source, *pack)?,
    };
    Ok((pack, commit))
}

/// Like `install_skill_source_with`, but also writes a `plugin_installs`
/// ledger row: `resolved_commit` from the cloner, `fingerprint` from
/// `fingerprint_dir` on the installed pack's on-disk directory, and
/// `trust_tier` = `"curated"` for `CURATED_SKILL_SOURCES` repos, otherwise
/// `"acknowledged"` (immediately acked, since an explicit install is itself
/// the acknowledgement).
///
/// Test-only: production ledger-recording installs go through the trust gate
/// (`install_skill_source_gated_with` / `begin_install_with`), which inline the
/// equivalent recording; this helper is retained to exercise that recording
/// logic directly.
#[cfg(test)]
async fn install_skill_source_with_recorded(
    source: &str,
    roots: &InstallRoots,
    cloner: &impl RepoCloner,
    store: &crate::store::Store,
) -> Result<InstalledSkillPack> {
    let (pack, commit) = install_skill_source_with_commit(source, roots, cloner).await?;
    let fingerprint = pack_fingerprint(roots, &pack)?;
    let now = crate::paths::now_ms();
    let trust_tier = if is_curated_source(&pack.source) {
        "curated"
    } else {
        "acknowledged"
    };
    store
        .upsert_plugin_install(&crate::store::PluginInstallRecord {
            plugin_id: pack.id.clone(),
            kind: if pack.plugin_id.is_some() {
                "plugin_pack".into()
            } else {
                "single_skill".into()
            },
            source_spec: source.to_string(),
            resolved_commit: commit,
            fingerprint,
            installed_at: now,
            updated_at: now,
            pinned: false,
            pin_reason: None,
            trust_tier: trust_tier.into(),
            trust_ack_at: if trust_tier == "acknowledged" {
                Some(now)
            } else {
                None
            },
            trust_ack_summary: None,
        })
        .await?;
    Ok(pack)
}

/// The on-disk directory that holds a standalone (non-pack) installed
/// skill's content — the single-skill dir under `skills_root`. Only
/// meaningful for `pack.plugin_id == None`; a plugin-pack install's content
/// lives across several sibling directories instead (one per member skill,
/// see `fingerprint_pack`), so there is no longer a single directory that
/// represents "the pack" on disk.
fn installed_pack_dir(roots: &InstallRoots, pack: &InstalledSkillPack) -> PathBuf {
    roots.skills_root.join(&pack.id)
}

/// The ledger fingerprint for an installed pack, covering whichever
/// directories actually hold its content: the single-skill dir for a
/// standalone skill, or the combined content of every materialized member
/// skill directory (`{plugin_id}--{skill}` under `skills_root`) for a
/// plugin pack — `install_plugin_pack` no longer writes anything under
/// `plugins_root`, so there is no single directory left to fingerprint for a
/// pack.
fn pack_fingerprint(roots: &InstallRoots, pack: &InstalledSkillPack) -> Result<String> {
    match &pack.plugin_id {
        Some(plugin_id) => fingerprint_pack(roots, plugin_id),
        None => fingerprint_dir(&installed_pack_dir(roots, pack)),
    }
}

/// Combined fingerprint of every materialized skill directory belonging to
/// `plugin_id` under `skills_root`, hashed in a stable (sorted-by-id) order
/// so the same on-disk content always yields the same digest. See
/// `pack_fingerprint`'s doc comment for why a plugin pack needs this instead
/// of `fingerprint_dir` on a single directory.
fn fingerprint_pack(roots: &InstallRoots, plugin_id: &str) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut ids = materialized_skill_ids_for_plugin(roots, plugin_id)?;
    ids.sort();
    let mut hasher = Sha256::new();
    for id in ids {
        let dir_fingerprint = fingerprint_dir(&roots.skills_root.join(&id))?;
        hasher.update(id.as_bytes());
        hasher.update([0u8]);
        hasher.update(dir_fingerprint.as_bytes());
        hasher.update([0u8]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Whether `canonical_repo` (already resolved by `parse_skill_source`) names
/// one of the curated skill sources — i.e. whether an install of it should
/// land at the `"curated"` trust tier rather than `"acknowledged"`.
pub(crate) fn is_curated_source(canonical_repo: &str) -> bool {
    CURATED_SKILL_SOURCES
        .iter()
        .any(|(_, repo)| *repo == canonical_repo)
}

/// Staged state for an arbitrary-source install (or update) awaiting
/// `confirm_install`. Holds the temp clone alive (`temp`'s `Drop` deletes it
/// once the token is removed from `staging_map()`, whether by a successful
/// confirm, an expired/rejected confirm, or — currently — never, if the
/// process exits first; staged installs are best-effort and don't survive a
/// restart).
///
/// `roots` is carried here rather than re-resolved in `confirm_install` so
/// the phase that stages a clone and the phase that installs it always agree
/// on where "the live install dir" is — re-resolving `InstallRoots::for_user()`
/// in `confirm_install` would silently install into the real user config dir
/// even when `begin_install_with`/`update_installed_pack_with` were called
/// with injected (e.g. test) roots.
struct StagedInstall {
    parsed: ParsedSkillSource,
    source_spec: String,
    roots: InstallRoots,
    // Never read directly — kept only so its `Drop` doesn't delete `repo_dir`
    // out from under the staged install while the token is still valid.
    _temp: tempfile::TempDir,
    repo_dir: PathBuf,
    commit: Option<String>,
    ack_summary: String, // JSON snapshot shown to the user; persisted verbatim as trust_ack_summary
    created_ms: i64,
    /// The `plugin_installs` record id being updated, when this staged state
    /// came from `update_installed_pack_with`'s re-ack-on-hook branch —
    /// `None` for a fresh `begin_install` (nothing prior to reconcile).
    /// `confirm_install` uses this to detect an identity change (the
    /// confirmed pack's id differs from `prior_id`) and clean up the old
    /// pack's artifacts/ledger row, mirroring what a normal (non-reack)
    /// update already does via `remove_stale_refresh_artifacts` +
    /// `delete_plugin_install`.
    prior_id: Option<String>,
}

/// How long a staged (unconfirmed) install stays valid before `confirm_install`
/// rejects it and the caller must start over via `begin_install`.
const STAGED_INSTALL_TTL_MS: i64 = 10 * 60 * 1000;

/// Process-global staging area for arbitrary-source installs/updates awaiting
/// confirmation (mirrors the shape of `PLUGIN_INSTALL_CANCELS` elsewhere in
/// the codebase). Keyed by a random token (`crate::paths::new_id()`), so
/// concurrent callers — including parallel tests — never collide.
fn staging_map() -> &'static Mutex<HashMap<String, StagedInstall>> {
    static MAP: OnceLock<Mutex<HashMap<String, StagedInstall>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A snapshot of what an arbitrary (non-curated) source install or update
/// would do, shown to the user before anything touches the live install dir.
/// `token` round-trips through `confirm_install` to complete the staged
/// install. Also carried inside `UpdateOutcome::NeedsReack` when an update
/// introduces a hook script the user hasn't already acknowledged.
///
/// Derives `PartialEq, Eq` (beyond the minimal `Debug, Clone, Serialize,
/// Deserialize` a prompt payload would otherwise need) so that
/// `UpdateOutcome`, which embeds a `TrustPrompt` in its `NeedsReack` variant,
/// can keep deriving `PartialEq, Eq` for its existing equality-based tests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrustPrompt {
    pub token: String,
    pub source_spec: String,
    pub owner_repo: String,
    pub resolved_commit: Option<String>,
    pub skills: Vec<String>,
    pub hook_scripts: Vec<String>,
    pub total_bytes: u64,
    /// Whether the source is one of `CURATED_SKILL_SOURCES` (see
    /// `is_curated_source`) — surfaced so the caller can distinguish "this
    /// prompt exists because the source is arbitrary/unvetted" from a
    /// curated source that still stops here for some other reason.
    pub curated: bool,
}

/// Outcome of `begin_install`: curated sources install immediately (an
/// explicit `ryuzi skill install <curated>` call is itself the trust
/// decision); arbitrary sources stop at a confirmation prompt instead of
/// touching the live install dir.
pub enum BeginInstall {
    Completed(InstalledSkillPack),
    NeedsConfirmation(TrustPrompt),
}

/// Phase 1 of the two-phase tiered trust gate. Clones `source` into a temp
/// dir, classifies its trust tier, and either installs it immediately
/// (curated) or stages the clone and returns a `TrustPrompt` for the caller
/// to show the user before `confirm_install` can proceed (arbitrary).
pub async fn begin_install(source: &str, store: &crate::store::Store) -> Result<BeginInstall> {
    let roots = InstallRoots::for_user()?;
    let cloner = GitRepoCloner;
    begin_install_with(source, &roots, &cloner, store).await
}

async fn begin_install_with(
    source: &str,
    roots: &InstallRoots,
    cloner: &impl RepoCloner,
    store: &crate::store::Store,
) -> Result<BeginInstall> {
    roots.ensure_exists()?;
    let parsed = parse_skill_source(source)?;

    let temp = tempfile::tempdir()?;
    let repo_dir = temp.path().join("repo");
    let commit = cloner.clone_repo(&parsed, &repo_dir).await?;
    let discovered = discover_install_target(&repo_dir, &parsed)?;

    // Curated → frictionless, install immediately. This mirrors
    // `install_skill_source_with_recorded`'s curated branch, just reusing the
    // clone/discovery already done above instead of re-cloning the repo a
    // second time.
    if is_curated_source(&parsed.repo) {
        let pack = match discovered {
            Discovery::Single(skill) => install_single_skill(roots, &parsed, skill)?,
            Discovery::Pack(pack) => install_plugin_pack(roots, &parsed, *pack)?,
        };
        let fingerprint = pack_fingerprint(roots, &pack)?;
        let now = crate::paths::now_ms();
        store
            .upsert_plugin_install(&crate::store::PluginInstallRecord {
                plugin_id: pack.id.clone(),
                kind: if pack.plugin_id.is_some() {
                    "plugin_pack".into()
                } else {
                    "single_skill".into()
                },
                source_spec: source.to_string(),
                resolved_commit: commit,
                fingerprint,
                installed_at: now,
                updated_at: now,
                pinned: false,
                pin_reason: None,
                trust_tier: "curated".into(),
                trust_ack_at: None,
                trust_ack_summary: None,
            })
            .await?;
        return Ok(BeginInstall::Completed(pack));
    }

    // Arbitrary source → stage into a temp dir, build the prompt, hold for
    // confirm. `stage_for_trust_prompt` re-derives `Discovery` from the same
    // on-disk clone (cheap relative to the network clone above) so it can
    // also set `TrustPrompt::skills` from it.
    let prompt = stage_for_trust_prompt(source, parsed, roots, temp, repo_dir, commit, None)?;
    Ok(BeginInstall::NeedsConfirmation(prompt))
}

/// Gate for the raw, single-call `install_skill` entry point — distinct from
/// the two-phase `begin_install`/`confirm_install` flow the Cockpit trust
/// wizard drives. `install_skill` has no confirmation step of its own (it
/// never hands back a token a caller could later pass to `confirm_install`),
/// so it must never complete an install that `begin_install`'s
/// curated-immediate branch wouldn't also complete immediately: reuses
/// `begin_install_with`'s classification and only ever returns `Ok` for the
/// same "curated" condition. Anything else — an arbitrary source — is
/// refused with an error naming the two-phase flow instead; nothing is
/// installed and no ledger row is written.
pub async fn install_skill_source_gated(
    source: &str,
    store: &crate::store::Store,
) -> Result<InstalledSkillPack> {
    let roots = InstallRoots::for_user()?;
    let cloner = GitRepoCloner;
    install_skill_source_gated_with(source, &roots, &cloner, store).await
}

async fn install_skill_source_gated_with(
    source: &str,
    roots: &InstallRoots,
    cloner: &impl RepoCloner,
    store: &crate::store::Store,
) -> Result<InstalledSkillPack> {
    match begin_install_with(source, roots, cloner, store).await? {
        BeginInstall::Completed(pack) => Ok(pack),
        BeginInstall::NeedsConfirmation(prompt) => {
            // This entry point never returns `prompt.token` to its caller,
            // so nothing could ever reach `confirm_install` with it — drop
            // the staged clone right away instead of leaving it in
            // `staging_map()` for the full TTL (see `StagedInstall`'s doc
            // comment on why an abandoned entry otherwise just sits there).
            discard_staged_install(&prompt.token);
            bail!(
                "source `{source}` needs review before it can install — it is not a curated \
                 pack. Use the two-phase begin_install/confirm_install flow \
                 (`begin_skill_install`/`confirm_skill_install`) to review and confirm it first."
            );
        }
    }
}

/// Remove a staged install (or update) from `staging_map()` before it was
/// ever confirmed — used by `install_skill_source_gated_with` when it
/// refuses a `NeedsConfirmation` outcome outright. Frees the staged clone's
/// tempdir immediately rather than waiting out `STAGED_INSTALL_TTL_MS`.
fn discard_staged_install(token: &str) {
    staging_map().lock().unwrap().remove(token);
}

/// Phase 2: complete a staged install (or update) after the user has
/// acknowledged its `TrustPrompt`. Single-use — the token is removed from
/// `staging_map()` up front, so a stale or already-consumed token can never
/// be replayed. Uses the roots captured in the staged state (see
/// `StagedInstall` doc comment), not a freshly re-resolved
/// `InstallRoots::for_user()`, so this always installs into the same
/// directory `begin_install`/`update_installed_pack` staged the clone from.
pub async fn confirm_install(
    token: &str,
    store: &crate::store::Store,
) -> Result<InstalledSkillPack> {
    let staged = staging_map()
        .lock()
        .unwrap()
        .remove(token)
        .ok_or_else(|| anyhow!("install session expired — start the install again"))?;
    if crate::paths::now_ms() - staged.created_ms > STAGED_INSTALL_TTL_MS {
        bail!("install session expired — start the install again");
    }
    let roots = &staged.roots;
    // A reack-triggered update's staged state carries the id of the record
    // being updated (`prior_id`); capture its on-disk pack now, before the
    // install below can touch anything, so an identity change (the confirmed
    // pack's id differs from `prior_id`) can still clean up the old pack's
    // artifacts/ledger row afterward — see `StagedInstall::prior_id`'s doc
    // comment. `None`/not-found both mean "nothing prior to reconcile".
    let prior_installed = staged
        .prior_id
        .as_deref()
        .and_then(|old| read_installed_pack(roots, old).ok());
    let discovered = discover_install_target(&staged.repo_dir, &staged.parsed)?;
    let pack = match discovered {
        Discovery::Single(skill) => install_single_skill(roots, &staged.parsed, skill)?,
        Discovery::Pack(p) => install_plugin_pack(roots, &staged.parsed, *p)?,
    };
    if let Some(old) = staged.prior_id.as_deref() {
        if old != pack.id {
            if let Some(old_installed) = &prior_installed {
                remove_stale_refresh_artifacts(roots, old_installed, &pack)?;
            }
            store.delete_plugin_install(old).await?;
        }
    }
    let fingerprint = pack_fingerprint(roots, &pack)?;
    let now = crate::paths::now_ms();
    store
        .upsert_plugin_install(&crate::store::PluginInstallRecord {
            plugin_id: pack.id.clone(),
            kind: if pack.plugin_id.is_some() {
                "plugin_pack".into()
            } else {
                "single_skill".into()
            },
            source_spec: staged.source_spec.clone(),
            resolved_commit: staged.commit.clone(),
            fingerprint,
            installed_at: now,
            updated_at: now,
            pinned: false,
            pin_reason: None,
            trust_tier: "acknowledged".into(),
            trust_ack_at: Some(now),
            trust_ack_summary: Some(staged.ack_summary.clone()),
        })
        .await?;
    Ok(pack)
}

/// Discover a freshly cloned repo's skills/hook scripts/size, build the
/// ack-summary JSON that will later be persisted verbatim as
/// `trust_ack_summary`, and stage it into `staging_map()` under a fresh
/// token. Shared by `begin_install_with`'s arbitrary-source branch and
/// `update_installed_pack_with`'s re-ack-on-hook branch — both need the same
/// "hold a clone, prompt the user, wait for `confirm_install`" behavior.
/// `prior_id` is `None` from the fresh-install branch, `Some(rec.plugin_id)`
/// from the re-ack-on-hook branch — see `StagedInstall::prior_id`.
fn stage_for_trust_prompt(
    source_spec: &str,
    parsed: ParsedSkillSource,
    roots: &InstallRoots,
    temp: tempfile::TempDir,
    repo_dir: PathBuf,
    commit: Option<String>,
    prior_id: Option<String>,
) -> Result<TrustPrompt> {
    let discovered = discover_install_target(&repo_dir, &parsed)?;
    let skills = discovered_skill_names(&discovered);
    let curated = is_curated_source(&parsed.repo);
    let hook_scripts = list_pack_hook_scripts(&repo_dir);
    let total_bytes = dir_size(&repo_dir);
    let owner_repo = parsed
        .repo
        .trim_start_matches("https://github.com/")
        .to_string();
    let ack_summary = serde_json::json!({
        "sourceSpec": source_spec,
        "ownerRepo": owner_repo,
        "resolvedCommit": commit,
        "skills": skills,
        "hookScripts": hook_scripts,
        "totalBytes": total_bytes,
    })
    .to_string();
    let token = crate::paths::new_id();
    staging_map().lock().unwrap().insert(
        token.clone(),
        StagedInstall {
            parsed,
            source_spec: source_spec.to_string(),
            roots: roots.clone(),
            _temp: temp,
            repo_dir,
            commit: commit.clone(),
            ack_summary,
            created_ms: crate::paths::now_ms(),
            prior_id,
        },
    );
    Ok(TrustPrompt {
        token,
        source_spec: source_spec.to_string(),
        owner_repo,
        resolved_commit: commit,
        skills,
        hook_scripts,
        total_bytes,
        curated,
    })
}

fn discovered_skill_names(d: &Discovery) -> Vec<String> {
    match d {
        Discovery::Single(s) => vec![s.display_name.clone()],
        Discovery::Pack(p) => p.skills.iter().map(|s| s.display_name.clone()).collect(),
    }
}

/// List hook scripts bundled in a pack, relative to its `.ryuzi/hooks` dir
/// (`<event>/<script>`), mirroring the worktree hook layout scanned by
/// `crate::harness::native::hooks::hook_scripts`. Used both to populate
/// `TrustPrompt::hook_scripts` and to detect newly introduced hooks on
/// update (re-ack-on-hook, see `update_installed_pack_with`).
fn list_pack_hook_scripts(repo_dir: &Path) -> Vec<String> {
    let hooks_root = repo_dir.join(".ryuzi/hooks");
    let mut out = Vec::new();
    if let Ok(events) = std::fs::read_dir(&hooks_root) {
        for event in events.filter_map(std::result::Result::ok) {
            if !event.path().is_dir() {
                continue;
            }
            let event_name = event.file_name().to_string_lossy().to_string();
            if let Ok(scripts) = std::fs::read_dir(event.path()) {
                for s in scripts.filter_map(std::result::Result::ok) {
                    if s.path().is_file() {
                        out.push(format!("{event_name}/{}", s.file_name().to_string_lossy()));
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// Total size, in bytes, of every file under `dir` — populates
/// `TrustPrompt::total_bytes`.
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut files = Vec::new();
    let _ = collect_files_rel(dir, dir, &mut files);
    for (_, path) in files {
        if let Ok(meta) = std::fs::metadata(&path) {
            total += meta.len();
        }
    }
    total
}

/// Parse a stored `trust_ack_summary`'s `hookScripts` JSON array (if any)
/// into the set of hook-script paths the user has already acknowledged.
/// `None` (never acknowledged, e.g. a curated or backfilled row) or an
/// unparsable summary both mean "nothing acknowledged" — an empty set, so
/// any hook script found in a later update counts as new.
fn acked_hook_scripts(summary: Option<&str>) -> HashSet<String> {
    summary
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.get("hookScripts").cloned())
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

/// One-time backfill: create a `plugin_installs` ledger row for every
/// on-disk installed pack that lacks one (installs made before the ledger
/// existed). Idempotent — packs that already have a row are skipped, so
/// repeated calls (e.g. every daemon startup) after the first are no-ops.
/// Backfilled rows have no `resolved_commit` (the original clone is long
/// gone) and default to a fresh `installed_at`/`updated_at` timestamp.
pub async fn backfill_install_records(store: &crate::store::Store) -> Result<usize> {
    let roots = InstallRoots::for_user()?;
    backfill_install_records_in(&roots, store).await
}

async fn backfill_install_records_in(
    roots: &InstallRoots,
    store: &crate::store::Store,
) -> Result<usize> {
    let packs = collect_installed_packs(roots)?;
    let mut backfilled = 0usize;
    for pack in packs {
        if store.get_plugin_install(&pack.id).await?.is_some() {
            continue;
        }
        let fingerprint =
            pack_fingerprint(roots, &pack).unwrap_or_else(|_| "sha256:unknown".into());
        let now = crate::paths::now_ms();
        let trust_tier = if is_curated_source(&pack.source) {
            "curated"
        } else {
            "acknowledged"
        };
        store
            .upsert_plugin_install(&crate::store::PluginInstallRecord {
                plugin_id: pack.id.clone(),
                kind: if pack.plugin_id.is_some() {
                    "plugin_pack".into()
                } else {
                    "single_skill".into()
                },
                source_spec: pack.source.clone(),
                resolved_commit: None,
                fingerprint,
                installed_at: now,
                updated_at: now,
                pinned: false,
                pin_reason: None,
                trust_tier: trust_tier.into(),
                trust_ack_at: None,
                trust_ack_summary: None,
            })
            .await?;
        backfilled += 1;
    }
    Ok(backfilled)
}

/// Prefixes used for staging/backup leftovers by `replace_dir_from`'s
/// `.tmp-` staging dir and `DirSwap`'s `.stage-`/`.backup-` dirs. A crash
/// between staging and the final rename (or between a commit's backup-rename
/// and its cleanup) can leave one of these behind under `skills_root` or
/// `plugins_root`.
const STALE_INSTALL_LEFTOVER_PREFIXES: &[&str] = &[".stage-", ".backup-", ".tmp-"];

/// Best-effort sweep of crash leftovers from an interrupted install/update:
/// staging (`.stage-`, `.tmp-`) and backup (`.backup-`) directories that a
/// prior process never got to clean up (see `replace_dir_from`/`DirSwap`).
/// Left behind, these sit alongside real installed packs under
/// `skills_root`/`plugins_root` and — if they happen to carry a stray
/// `.ryuzi-skill.json` copied from the pack being staged — can be misread by
/// `collect_installed_packs` as a phantom installed skill. Idempotent: a
/// clean install tree has nothing matching these prefixes, so repeated calls
/// (e.g. every daemon startup) after the first are no-ops.
pub fn sweep_stale_install_leftovers() -> Result<usize> {
    let roots = InstallRoots::for_user()?;
    sweep_stale_install_leftovers_in(&roots)
}

fn sweep_stale_install_leftovers_in(roots: &InstallRoots) -> Result<usize> {
    let mut removed = 0usize;
    for root in [&roots.skills_root, &roots.plugins_root] {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if STALE_INSTALL_LEFTOVER_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
            {
                std::fs::remove_dir_all(entry.path())?;
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Result of attempting to bring an installed pack up to date with its
/// recorded source. `Failed` carries a human-readable reason (rather than
/// propagating an `Err`) so `update_all_packs` can report a per-pack outcome
/// without one bad pack aborting the whole batch. `NeedsReack` routes back
/// through the two-phase trust gate when the update introduces a hook script
/// the user hasn't already acknowledged (see `update_installed_pack_with`);
/// its `TrustPrompt` carries the same `token` semantics as `BeginInstall::
/// NeedsConfirmation` — pass it to `confirm_install` to complete the update.
/// `#[serde(tag/content)]` keeps this a clean discriminated union for the
/// daemon/Tauri layers that consume it later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "detail")]
pub enum UpdateOutcome {
    Updated,
    AlreadyCurrent,
    SkippedPinned,
    LocalEdits,
    Failed(String),
    NeedsReack(TrustPrompt),
}

/// Update one installed pack to its latest upstream commit, guarding against
/// clobbering local edits and pinned packs. See `update_installed_pack_with`
/// for the full decision order.
pub async fn update_installed_pack(
    id: &str,
    force: bool,
    store: &crate::store::Store,
) -> Result<UpdateOutcome> {
    let roots = InstallRoots::for_user()?;
    let cloner = GitRepoCloner;
    update_installed_pack_with(id, force, &roots, &cloner, store).await
}

/// Decision order: missing ledger record → `Failed`; pinned → `SkippedPinned`
/// (pinning is an explicit, unconditional user choice — `force` does not
/// override it); on-disk fingerprint drifted from the recorded one →
/// `LocalEdits` (unless `force`); re-clone resolves to the same commit
/// already recorded → `AlreadyCurrent` (unless `force`); the re-clone
/// contains a hook script not already covered by the recorded
/// `trust_ack_summary` → `NeedsReack` (stages the clone into `staging_map()`
/// and routes back through `confirm_install` — checked regardless of
/// `force`, since a hook script executes code and re-acknowledging that
/// isn't something `force` should be able to skip); otherwise reinstall
/// (staged), clean up stale refresh artifacts, and rewrite the ledger row
/// with the new commit/fingerprint/`updated_at`, preserving
/// `installed_at`/pin/trust fields from the old row.
async fn update_installed_pack_with(
    id: &str,
    force: bool,
    roots: &InstallRoots,
    cloner: &impl RepoCloner,
    store: &crate::store::Store,
) -> Result<UpdateOutcome> {
    let Some(rec) = store.get_plugin_install(id).await? else {
        return Ok(UpdateOutcome::Failed(format!("no install record for {id}")));
    };
    if rec.pinned {
        return Ok(UpdateOutcome::SkippedPinned);
    }

    // Local-edit guard: the current on-disk fingerprint must match the one
    // recorded at the last install/update, or an update would silently
    // overwrite whatever the user changed by hand.
    let installed = read_installed_pack(roots, id)?;
    if !force {
        let current_fp = pack_fingerprint(roots, &installed).unwrap_or_default();
        if current_fp != rec.fingerprint {
            return Ok(UpdateOutcome::LocalEdits);
        }
    }

    // Re-resolve from the recorded source spec into a temp clone and detect
    // a no-op update by commit equality BEFORE touching the live install.
    let parsed = parse_skill_source(&rec.source_spec)?;
    let temp = tempfile::tempdir()?;
    let repo_dir = temp.path().join("repo");
    let new_commit = cloner.clone_repo(&parsed, &repo_dir).await?;
    if !force && new_commit.is_some() && new_commit == rec.resolved_commit {
        return Ok(UpdateOutcome::AlreadyCurrent);
    }

    // Re-ack-on-hook: a pack that introduces hook scripts the user hasn't
    // acknowledged yet must route back through the trust gate instead of
    // silently swapping in code that runs on every tool call. A backfilled
    // or curated record's `trust_ack_summary` is `None` (nothing
    // acknowledged), so ANY hook script in the update trips this check.
    let hook_scripts_in_update = list_pack_hook_scripts(&repo_dir);
    let acked = acked_hook_scripts(rec.trust_ack_summary.as_deref());
    let new_hook_script = hook_scripts_in_update.iter().any(|h| !acked.contains(h));

    let discovered = discover_install_target(&repo_dir, &parsed)?;

    if new_hook_script {
        let prompt = stage_for_trust_prompt(
            &rec.source_spec,
            parsed,
            roots,
            temp,
            repo_dir,
            new_commit,
            Some(rec.plugin_id.clone()),
        )?;
        return Ok(UpdateOutcome::NeedsReack(prompt));
    }

    // Perform the reinstall (staged, atomic) + stale-artifact cleanup, then
    // rewrite the ledger row to reflect the new install.
    let refreshed = match discovered {
        Discovery::Single(skill) => install_single_skill(roots, &parsed, skill)?,
        Discovery::Pack(pack) => install_plugin_pack(roots, &parsed, *pack)?,
    };
    remove_stale_refresh_artifacts(roots, &installed, &refreshed)?;

    let fingerprint = pack_fingerprint(roots, &refreshed)?;
    let now = crate::paths::now_ms();
    let updated = crate::store::PluginInstallRecord {
        plugin_id: refreshed.id.clone(),
        kind: if refreshed.plugin_id.is_some() {
            "plugin_pack".into()
        } else {
            "single_skill".into()
        },
        source_spec: rec.source_spec.clone(),
        resolved_commit: new_commit,
        fingerprint,
        installed_at: rec.installed_at,
        updated_at: now,
        pinned: rec.pinned,
        pin_reason: rec.pin_reason.clone(),
        trust_tier: rec.trust_tier.clone(),
        trust_ack_at: rec.trust_ack_at,
        trust_ack_summary: rec.trust_ack_summary.clone(),
    };
    // The pack's identity (id) can change across an update (e.g. an upstream
    // rename of the plugin id). When it does, drop the old row instead of
    // leaving a stale duplicate behind.
    if refreshed.id != rec.plugin_id {
        store.delete_plugin_install(&rec.plugin_id).await?;
    }
    store.upsert_plugin_install(&updated).await?;
    Ok(UpdateOutcome::Updated)
}

/// Update every installed pack, skipping pinned ones. Never fails as a whole:
/// a single pack's error becomes `UpdateOutcome::Failed` for that pack so the
/// rest of the batch still runs.
pub async fn update_all_packs(store: &crate::store::Store) -> Result<Vec<(String, UpdateOutcome)>> {
    let roots = InstallRoots::for_user()?;
    let cloner = GitRepoCloner;
    update_all_packs_with(&roots, &cloner, store).await
}

async fn update_all_packs_with(
    roots: &InstallRoots,
    cloner: &impl RepoCloner,
    store: &crate::store::Store,
) -> Result<Vec<(String, UpdateOutcome)>> {
    let mut out = Vec::new();
    for rec in store.list_plugin_installs().await? {
        let outcome =
            match update_installed_pack_with(&rec.plugin_id, false, roots, cloner, store).await {
                Ok(o) => o,
                Err(e) => UpdateOutcome::Failed(e.to_string()),
            };
        out.push((rec.plugin_id, outcome));
    }
    Ok(out)
}

/// Pin (or unpin) an installed pack against future updates. A thin
/// passthrough to the store — the ledger row is the single source of truth
/// for pin state, checked by `update_installed_pack_with` above.
pub async fn set_pack_pin(
    id: &str,
    pinned: bool,
    reason: Option<&str>,
    store: &crate::store::Store,
) -> Result<()> {
    store.set_plugin_install_pin(id, pinned, reason).await
}

async fn refresh_installed_skill_with(
    id: &str,
    roots: &InstallRoots,
    cloner: &impl RepoCloner,
) -> Result<InstalledSkillPack> {
    let installed = read_installed_pack(roots, id)?;
    let refreshed = install_skill_source_with(&installed.source, roots, cloner).await?;
    remove_stale_refresh_artifacts(roots, &installed, &refreshed)?;
    Ok(refreshed)
}

fn remove_stale_refresh_artifacts(
    roots: &InstallRoots,
    installed: &InstalledSkillPack,
    refreshed: &InstalledSkillPack,
) -> Result<()> {
    match (
        installed.plugin_id.as_deref(),
        refreshed.plugin_id.as_deref(),
    ) {
        (Some(previous_plugin_id), Some(refreshed_plugin_id)) => {
            if previous_plugin_id != refreshed_plugin_id {
                remove_all_artifacts_for_identity(roots, previous_plugin_id)?;
            }
        }
        (Some(previous_plugin_id), None) => {
            if previous_plugin_id == refreshed.id {
                remove_checked_dir(&roots.plugins_root, previous_plugin_id)?;
                for skill_id in materialized_skill_ids_for_plugin(roots, previous_plugin_id)? {
                    remove_checked_dir(&roots.skills_root, &skill_id)?;
                }
            } else {
                remove_all_artifacts_for_identity(roots, previous_plugin_id)?;
            }
        }
        (None, Some(refreshed_plugin_id)) => {
            if installed.id != refreshed_plugin_id {
                remove_all_artifacts_for_identity(roots, &installed.id)?;
            } else {
                remove_checked_dir(&roots.skills_root, &installed.id)?;
            }
        }
        (None, None) => {
            if refreshed.id != installed.id {
                remove_all_artifacts_for_identity(roots, &installed.id)?;
            }
        }
    }

    Ok(())
}

fn list_installed_skills_in(roots: &InstallRoots) -> Result<Vec<InstalledSkillInfo>> {
    let mut infos = collect_installed_packs(roots)?
        .into_iter()
        .map(|pack| InstalledSkillInfo {
            id: pack.id,
            name: pack.name,
            source: pack.source,
            plugin_id: pack.plugin_id,
            installed_at: pack.installed_at,
            skill_count: pack.skills.len(),
        })
        .collect::<Vec<_>>();
    infos.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(infos)
}

fn list_installed_skill_catalog_rows_in(
    roots: &InstallRoots,
) -> Result<Vec<InstalledSkillCatalogRow>> {
    let mut rows = Vec::new();
    for pack in collect_installed_packs(roots)? {
        let pack_label = pack.plugin_id.is_some().then(|| pack.name.clone());
        for skill in &pack.skills {
            rows.push(InstalledSkillCatalogRow {
                skill_name: skill.name.clone(),
                label: skill.name.clone(),
                pack: pack_label.clone(),
            });
        }
    }
    rows.sort_by(|a, b| a.pack.cmp(&b.pack).then(a.skill_name.cmp(&b.skill_name)));
    Ok(rows)
}

fn expand_skill_bindings_in(roots: &InstallRoots, bound: &[String]) -> Vec<String> {
    let packs = collect_installed_packs(roots).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for id in bound {
        match packs.iter().find(|p| &p.id == id) {
            Some(pack) => out.extend(pack.skills.iter().map(|s| s.name.clone())),
            None => out.push(id.clone()),
        }
    }
    out.dedup();
    out
}

fn remove_installed_skill_in(roots: &InstallRoots, id: &str) -> Result<()> {
    read_installed_pack(roots, id)?;
    remove_all_artifacts_for_identity(roots, id)
}

fn install_single_skill(
    roots: &InstallRoots,
    source: &ParsedSkillSource,
    skill: SkillDescriptor,
) -> Result<InstalledSkillPack> {
    let installed_at = now_rfc3339();
    let target = checked_child(&roots.skills_root, &skill.normalized_name)?;
    replace_dir_from(&skill.source_dir, &target)?;
    write_provenance(
        &target.join(PROVENANCE_FILE),
        &SkillInstallProvenance {
            source: source.repo.clone(),
            plugin_id: None,
            installed_at: installed_at.clone(),
        },
    )?;
    remove_stale_plugin_pack_artifacts_for_single_install(roots, &skill.normalized_name)?;
    Ok(installed_single_skill_pack(
        &skill,
        &source.repo,
        installed_at,
    ))
}

/// Materializes every skill discovered in a "plugin pack" repo into
/// `~/.config/ryuzi/skills/{plugin_id}--{skill}` — the multi-skill analogue
/// of `install_single_skill`. Unlike the pre-v2 behavior, this no longer
/// writes anything under `plugins_root`: there is no `PluginManifest` to
/// synthesize or preserve (the v2 SDK manifest dropped `skills` entirely),
/// and the loader that used to read a `ryuzi-plugin.toml`/provenance stamp
/// back out of `plugins_root/<plugin_id>` to register a `CorePlugin` has
/// been removed — full plugin-folder installs are a later task. The pack's
/// member skills are still grouped together for listing/removal purposes via
/// their shared `plugin_id` in each skill's own provenance stamp (see
/// `collect_installed_packs`).
fn install_plugin_pack(
    roots: &InstallRoots,
    source: &ParsedSkillSource,
    pack: PackDescriptor,
) -> Result<InstalledSkillPack> {
    let existing = materialized_skill_ids_for_plugin(roots, &pack.plugin_id)?;
    let desired = pack
        .skills
        .iter()
        .map(|skill| format!("{}--{}", pack.plugin_id, skill.normalized_name))
        .collect::<HashSet<_>>();
    let installed_at = now_rfc3339();

    // Stamp each materialized skill's own copy (still inside the temp clone)
    // and stage it into the skills root.
    let mut swap = DirSwap::new();
    for skill in &pack.skills {
        write_provenance(
            &skill.source_dir.join(PROVENANCE_FILE),
            &SkillInstallProvenance {
                source: source.repo.clone(),
                plugin_id: Some(pack.plugin_id.clone()),
                installed_at: installed_at.clone(),
            },
        )?;
        let target_id = format!("{}--{}", pack.plugin_id, skill.normalized_name);
        let target = checked_child(&roots.skills_root, &target_id)?;
        swap.stage(&skill.source_dir, &target)?;
    }

    // Commit all staged dirs as one atomic swap: either all land, or a
    // failure restores every pre-existing target this call already moved
    // aside.
    swap.commit()?;

    // Stale-artifact removal only runs after a successful commit, so a
    // failed install never deletes still-valid pre-existing artifacts.
    for stale in existing {
        if !desired.contains(&stale) {
            remove_checked_dir(&roots.skills_root, &stale)?;
        }
    }

    remove_stale_single_skill_artifact_for_plugin_install(roots, &pack.plugin_id)?;

    Ok(installed_plugin_pack(&pack, &source.repo, installed_at))
}

fn installed_single_skill_pack(
    skill: &SkillDescriptor,
    source: &str,
    installed_at: String,
) -> InstalledSkillPack {
    InstalledSkillPack {
        id: skill.normalized_name.clone(),
        name: skill.display_name.clone(),
        source: source.to_string(),
        plugin_id: None,
        installed_at,
        skills: vec![InstalledSkillEntry {
            id: skill.normalized_name.clone(),
            name: skill.display_name.clone(),
        }],
    }
}

fn installed_plugin_pack(
    pack: &PackDescriptor,
    source: &str,
    installed_at: String,
) -> InstalledSkillPack {
    let mut skills = pack
        .skills
        .iter()
        .map(|skill| InstalledSkillEntry {
            id: format!("{}--{}", pack.plugin_id, skill.normalized_name),
            name: skill.display_name.clone(),
        })
        .collect::<Vec<_>>();
    skills.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    InstalledSkillPack {
        id: pack.plugin_id.clone(),
        name: pack.name.clone(),
        source: source.to_string(),
        plugin_id: Some(pack.plugin_id.clone()),
        installed_at,
        skills,
    }
}

fn remove_stale_plugin_pack_artifacts_for_single_install(
    roots: &InstallRoots,
    skill_id: &str,
) -> Result<()> {
    remove_checked_dir(&roots.plugins_root, skill_id)?;
    for materialized_skill_id in materialized_skill_ids_for_plugin(roots, skill_id)? {
        remove_checked_dir(&roots.skills_root, &materialized_skill_id)?;
    }
    Ok(())
}

fn remove_stale_single_skill_artifact_for_plugin_install(
    roots: &InstallRoots,
    plugin_id: &str,
) -> Result<()> {
    remove_checked_dir(&roots.skills_root, plugin_id)
}

fn parse_skill_source(source: &str) -> Result<ParsedSkillSource> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        bail!("unsupported skill source: empty input");
    }

    let lower = trimmed.to_ascii_lowercase();
    if let Some((_, repo)) = CURATED_SKILL_SOURCES
        .iter()
        .find(|(alias, _)| *alias == lower)
    {
        return parsed_source_from_repo(repo);
    }

    if trimmed.starts_with("https://github.com/") {
        return parsed_source_from_repo(trimmed);
    }

    let parts = trimmed.split('/').collect::<Vec<_>>();
    if parts.len() == 2
        && parts
            .iter()
            .all(|part| !part.trim().is_empty() && !part.contains(char::is_whitespace))
    {
        return parsed_source_from_repo(&format!("https://github.com/{trimmed}"));
    }

    bail!("unsupported skill source: {trimmed}")
}

fn parsed_source_from_repo(repo: &str) -> Result<ParsedSkillSource> {
    let canonical = canonical_github_repo(repo)?;
    let repo_name = canonical
        .rsplit('/')
        .next()
        .ok_or_else(|| anyhow!("could not determine repository name for {canonical}"))?
        .to_string();
    Ok(ParsedSkillSource {
        repo: canonical,
        repo_name,
    })
}

fn canonical_github_repo(repo: &str) -> Result<String> {
    let raw = repo.trim().trim_end_matches('/');
    let Some(rest) = raw.strip_prefix("https://github.com/") else {
        bail!("unsupported skill source: {repo}");
    };
    let rest = rest.trim_end_matches(".git");
    let parts = rest
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() != 2 || parts.iter().any(|part| part.contains(char::is_whitespace)) {
        bail!("unsupported skill source: {repo}");
    }
    Ok(format!("https://github.com/{}/{}", parts[0], parts[1]))
}

fn discover_install_target(repo_dir: &Path, source: &ParsedSkillSource) -> Result<Discovery> {
    let plugin_json_path = repo_dir.join(".codex-plugin/plugin.json");
    if plugin_json_path.is_file() {
        return Ok(Discovery::Pack(Box::new(
            discover_plugin_pack_from_plugin_json(repo_dir, source, &plugin_json_path)?,
        )));
    }

    if repo_dir.join("SKILL.md").is_file() {
        return Ok(Discovery::Single(read_skill_descriptor(repo_dir)?));
    }

    let skills = scan_skill_root(&repo_dir.join("skills"))?;
    if !skills.is_empty() {
        return Ok(Discovery::Pack(Box::new(discover_bare_plugin_pack(
            source, skills,
        )?)));
    }

    bail!("no installable skill found in repo — checked .codex-plugin/plugin.json, SKILL.md, and skills/*/SKILL.md")
}

/// Discover a "plugin pack" repo advertised via `.codex-plugin/plugin.json`
/// (the Codex/Claude-style plugin manifest, unrelated to `ryuzi-plugin.toml`)
/// — resolves the skills it declares (`plugin_json.skills`, defaulting to
/// `./skills/`) and derives a plugin id/display name from its metadata. No
/// `PluginManifest` is read or constructed here: the v2 SDK manifest dropped
/// `skills` entirely, so even a repo that happens to also ship a
/// `ryuzi-plugin.toml` is out of scope for this installer — a real
/// plugin-folder install is a later task.
fn discover_plugin_pack_from_plugin_json(
    repo_dir: &Path,
    source: &ParsedSkillSource,
    plugin_json_path: &Path,
) -> Result<PackDescriptor> {
    let plugin_json: CodexPluginJson =
        serde_json::from_str(&std::fs::read_to_string(plugin_json_path)?)
            .with_context(|| format!("invalid plugin.json at {}", plugin_json_path.display()))?;

    let default_skill_root = plugin_json.skills.as_deref().unwrap_or("./skills/");
    let skills = discover_skill_descriptors(repo_dir, default_skill_root)?;

    let plugin_id = normalize_name(
        plugin_json
            .name
            .as_deref()
            .unwrap_or(source.repo_name.as_str()),
    );
    let name = plugin_json
        .interface
        .as_ref()
        .and_then(|value| value.display_name.clone())
        .or_else(|| plugin_json.name.clone())
        .unwrap_or_else(|| source.repo_name.clone());
    Ok(PackDescriptor {
        plugin_id,
        name,
        skills,
    })
}

/// Discover a "bare" plugin pack: a repo with no `.codex-plugin/plugin.json`
/// and no top-level `SKILL.md`, but multiple skills under `skills/*/SKILL.md`
/// — the plugin id/name are derived purely from the repo name.
fn discover_bare_plugin_pack(
    source: &ParsedSkillSource,
    skills: Vec<SkillDescriptor>,
) -> Result<PackDescriptor> {
    let plugin_id = normalize_name(&source.repo_name);
    Ok(PackDescriptor {
        plugin_id,
        name: source.repo_name.clone(),
        skills,
    })
}

fn discover_skill_descriptors(repo_dir: &Path, rel: &str) -> Result<Vec<SkillDescriptor>> {
    let root = resolve_within(repo_dir, rel)?;
    if root.join("SKILL.md").is_file() {
        return Ok(vec![read_skill_descriptor(&root)?]);
    }
    let skills = scan_skill_root(&root)?;
    if skills.is_empty() {
        bail!("no installable skills found under {}", root.display());
    }
    Ok(skills)
}

fn scan_skill_root(root: &Path) -> Result<Vec<SkillDescriptor>> {
    let mut dirs = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(vec![]);
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if path.join("SKILL.md").is_file() {
            dirs.push(read_skill_descriptor(&path)?);
        }
    }
    dirs.sort_by(|a, b| a.normalized_name.cmp(&b.normalized_name));
    Ok(dirs)
}

fn read_skill_descriptor(skill_dir: &Path) -> Result<SkillDescriptor> {
    let text = std::fs::read_to_string(skill_dir.join("SKILL.md"))
        .with_context(|| format!("missing SKILL.md in {}", skill_dir.display()))?;
    let (frontmatter, _) = crate::harness::native::agents::split_frontmatter_pub(&text);
    let fallback = skill_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill")
        .to_string();
    let display_name = frontmatter
        .into_iter()
        .find_map(|(key, value)| (key == "name").then_some(value))
        .unwrap_or(fallback);
    Ok(SkillDescriptor {
        normalized_name: normalize_name(&display_name),
        display_name,
        source_dir: skill_dir.to_path_buf(),
    })
}

fn collect_installed_packs(roots: &InstallRoots) -> Result<Vec<InstalledSkillPack>> {
    let Ok(entries) = std::fs::read_dir(&roots.skills_root) else {
        return Ok(vec![]);
    };

    let mut grouped: HashMap<String, InstalledSkillPack> = HashMap::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let skill_dir = entry.path();
        let provenance_path = skill_dir.join(PROVENANCE_FILE);
        if !provenance_path.is_file() {
            continue;
        }

        let provenance: SkillInstallProvenance = serde_json::from_str(&std::fs::read_to_string(
            &provenance_path,
        )?)
        .with_context(|| format!("invalid provenance file at {}", provenance_path.display()))?;
        let skill_id = entry.file_name().to_string_lossy().to_string();
        let skill_name = read_skill_descriptor(&skill_dir)?.display_name;
        let group_id = provenance
            .plugin_id
            .clone()
            .unwrap_or_else(|| skill_id.clone());
        let pack_name = match &provenance.plugin_id {
            Some(plugin_id) => {
                plugin_display_name(roots, plugin_id).unwrap_or_else(|| plugin_id.clone())
            }
            None => skill_name.clone(),
        };
        let skill = InstalledSkillEntry {
            id: skill_id,
            name: skill_name,
        };
        let pack = grouped
            .entry(group_id.clone())
            .or_insert_with(|| InstalledSkillPack {
                id: group_id.clone(),
                name: pack_name.clone(),
                source: provenance.source.clone(),
                plugin_id: provenance.plugin_id.clone(),
                installed_at: provenance.installed_at.clone(),
                skills: Vec::new(),
            });
        let is_newer_generation = provenance.installed_at > pack.installed_at;
        let is_same_generation = provenance.installed_at == pack.installed_at;
        if is_newer_generation {
            pack.name = pack_name;
            pack.source = provenance.source;
            pack.plugin_id = provenance.plugin_id;
            pack.installed_at = provenance.installed_at;
            pack.skills.clear();
        }
        if is_same_generation || is_newer_generation {
            pack.skills.push(skill);
        }
    }

    let mut packs = grouped.into_values().collect::<Vec<_>>();
    for pack in &mut packs {
        pack.skills
            .sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    }
    packs.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(packs)
}

fn read_installed_pack(roots: &InstallRoots, id: &str) -> Result<InstalledSkillPack> {
    collect_installed_packs(roots)?
        .into_iter()
        .find(|pack| pack.id == id)
        .ok_or_else(|| anyhow!("unknown installed skill: {id}"))
}

fn materialized_skill_ids_for_plugin(roots: &InstallRoots, plugin_id: &str) -> Result<Vec<String>> {
    let Ok(entries) = std::fs::read_dir(&roots.skills_root) else {
        return Ok(vec![]);
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let provenance_path = entry.path().join(PROVENANCE_FILE);
        if !provenance_path.is_file() {
            continue;
        }
        let provenance: SkillInstallProvenance = serde_json::from_str(&std::fs::read_to_string(
            &provenance_path,
        )?)
        .with_context(|| format!("invalid provenance file at {}", provenance_path.display()))?;
        if provenance.plugin_id.as_deref() == Some(plugin_id) {
            ids.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    Ok(ids)
}

fn remove_all_artifacts_for_identity(roots: &InstallRoots, id: &str) -> Result<()> {
    remove_checked_dir(&roots.plugins_root, id)?;
    remove_checked_dir(&roots.skills_root, id)?;
    for materialized_skill_id in materialized_skill_ids_for_plugin(roots, id)? {
        remove_checked_dir(&roots.skills_root, &materialized_skill_id)?;
    }
    Ok(())
}

fn plugin_display_name(roots: &InstallRoots, plugin_id: &str) -> Option<String> {
    let manifest_path = roots.plugins_root.join(plugin_id).join("ryuzi-plugin.toml");
    let text = std::fs::read_to_string(manifest_path).ok()?;
    ryuzi_plugin_sdk::PluginManifest::from_toml(&text)
        .ok()
        .map(|manifest| manifest.name)
}

fn resolve_within(base_dir: &Path, rel: &str) -> Result<PathBuf> {
    let base_dir = base_dir
        .canonicalize()
        .with_context(|| format!("missing base directory {}", base_dir.display()))?;
    let target = base_dir.join(rel);
    let target = target
        .canonicalize()
        .with_context(|| format!("missing path {} under {}", rel, base_dir.display()))?;
    if !target.starts_with(&base_dir) {
        bail!("path escapes install root: {rel}");
    }
    Ok(target)
}

fn replace_dir_from(source: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("install target has no parent: {}", target.display()))?;
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".tmp-{}", crate::paths::new_id()));
    copy_dir_recursive(source, &staging)?;
    if target.exists() {
        if target.is_dir() {
            std::fs::remove_dir_all(target)?;
        } else {
            std::fs::remove_file(target)?;
        }
    }
    std::fs::rename(&staging, target)?;
    Ok(())
}

/// Multi-directory atomic swap. `stage` copies each source into a sibling
/// staging dir under the target's parent (never touching the live target);
/// `commit` moves every existing target aside to a backup, renames each
/// staging dir into place, and — on ANY failure — restores every backup it
/// already moved and removes staging dirs. Generalizes `replace_dir_from` so a
/// plugin-pack install that writes several directories is all-or-nothing.
struct DirSwap {
    staged: Vec<(PathBuf, PathBuf)>, // (staging_dir, final_target)
}

impl DirSwap {
    fn new() -> Self {
        Self { staged: Vec::new() }
    }

    fn stage(&mut self, source: &Path, target: &Path) -> Result<()> {
        let parent = target
            .parent()
            .ok_or_else(|| anyhow!("install target has no parent: {}", target.display()))?;
        std::fs::create_dir_all(parent)?;
        let staging = parent.join(format!(".stage-{}", crate::paths::new_id()));
        copy_dir_recursive(source, &staging)?;
        self.staged.push((staging, target.to_path_buf()));
        Ok(())
    }

    fn commit(self) -> Result<()> {
        let mut backups: Vec<(PathBuf, PathBuf)> = Vec::new(); // (backup, original_target)
        let mut moved_in: Vec<PathBuf> = Vec::new(); // targets we renamed staging INTO

        let result = (|| -> Result<()> {
            for (staging, target) in &self.staged {
                if target.exists() {
                    let backup =
                        target.with_file_name(format!(".backup-{}", crate::paths::new_id()));
                    std::fs::rename(target, &backup)?;
                    backups.push((backup, target.clone()));
                }
                std::fs::rename(staging, target)?;
                moved_in.push(target.clone());
            }
            Ok(())
        })();

        if result.is_err() {
            // Undo swapped-in targets, then restore backups.
            for target in moved_in.iter().rev() {
                let _ = std::fs::remove_dir_all(target);
            }
            for (backup, target) in backups.iter().rev() {
                let _ = std::fs::rename(backup, target);
            }
            // Best-effort clean of any remaining staging dirs.
            for (staging, _) in &self.staged {
                let _ = std::fs::remove_dir_all(staging);
            }
            return result;
        }

        // Success: delete backups.
        for (backup, _) in backups {
            let _ = std::fs::remove_dir_all(backup);
        }
        Ok(())
    }
}

fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    crate::install_common::copy_dir_recursive(source, dest)
}

/// Stable content hash of an installed tree. Walks files in sorted order,
/// hashing each file's relative path and bytes, so the same content always
/// yields the same digest regardless of install location. Excludes `.git`
/// (stripped by `copy_dir_recursive` anyway) and `PROVENANCE_FILE` (written
/// AFTER fingerprinting — including it would make every live-dir comparison a
/// false mismatch). Symlinks/special files are ignored, matching
/// `copy_dir_recursive`.
///
/// Wired into the install ledger by `install_skill_source_with_recorded` and
/// `backfill_install_records_in` for local-edit detection; also exercised
/// directly by `fingerprint_is_stable_and_excludes_git_and_stamp` below.
pub(crate) fn fingerprint_dir(dir: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files_rel(dir, dir, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (rel, path) in files {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("failed to read {} for fingerprint", path.display()))?;
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update([0u8]);
        hasher.update(&bytes);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn collect_files_rel(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for entry in std::fs::read_dir(dir)?.filter_map(std::result::Result::ok) {
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" || name == std::ffi::OsStr::new(PROVENANCE_FILE) {
            continue;
        }
        if path.is_dir() {
            collect_files_rel(base, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
    Ok(())
}

fn checked_child(root: &Path, id: &str) -> Result<PathBuf> {
    if !is_safe_id(id) {
        bail!("unsafe install id: {id}");
    }
    Ok(root.join(id))
}

fn remove_checked_dir(root: &Path, id: &str) -> Result<()> {
    let target = checked_child(root, id)?;
    if !target.exists() {
        return Ok(());
    }
    if target.is_dir() {
        std::fs::remove_dir_all(target)?;
    } else {
        std::fs::remove_file(target)?;
    }
    Ok(())
}

fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn write_provenance(path: &Path, provenance: &SkillInstallProvenance) -> Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(provenance)?)?;
    Ok(())
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn normalize_name(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "skill".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct FakeRepoCloner {
        repos: BTreeMap<String, PathBuf>,
        commit: Option<String>,
    }

    #[async_trait::async_trait]
    impl RepoCloner for FakeRepoCloner {
        async fn clone_repo(
            &self,
            source: &ParsedSkillSource,
            dest: &Path,
        ) -> Result<Option<String>> {
            let repo = self
                .repos
                .get(&source.repo)
                .ok_or_else(|| anyhow!("missing fake repo for {}", source.repo))?;
            copy_dir_recursive(repo, dest)?;
            Ok(self.commit.clone())
        }
    }

    fn write_skill(dir: &std::path::Path, name: &str, description: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
        )
        .unwrap();
    }

    fn write_installed_skill(
        roots: &InstallRoots,
        id: &str,
        name: &str,
        body: &str,
        provenance: SkillInstallProvenance,
    ) {
        let dir = roots.skills_root.join(id);
        write_skill(&dir, name, "Installed skill", body);
        write_provenance(&dir.join(PROVENANCE_FILE), &provenance).unwrap();
    }

    /// Fixture for the per-skill catalog tests: a 2-skill plugin pack whose
    /// plugin id is "superpack" and display name is "Superpack", with member
    /// skills "alpha" and "beta". Materializes both the skills-root
    /// provenance (`write_installed_skill`, same as a real
    /// `install_plugin_pack` output) AND a real `ryuzi-plugin.toml` under
    /// `plugins_root` so `plugin_display_name` resolves "Superpack" the same
    /// way it would for a genuine install — a bare provenance stamp with no
    /// manifest would fall back to the plugin id itself (see
    /// `collect_installed_packs`'s `pack_name` resolution).
    fn install_fixture_pack_superpack(roots: &InstallRoots) {
        std::fs::create_dir_all(roots.plugins_root.join("superpack")).unwrap();
        std::fs::write(
            roots
                .plugins_root
                .join("superpack")
                .join("ryuzi-plugin.toml"),
            "contract = 2\nid = \"superpack\"\nname = \"Superpack\"\n",
        )
        .unwrap();
        let provenance = SkillInstallProvenance {
            source: "https://example.test/superpack".to_string(),
            plugin_id: Some("superpack".to_string()),
            installed_at: "2026-01-01T00:00:00.000Z".to_string(),
        };
        write_installed_skill(
            roots,
            "superpack--alpha",
            "alpha",
            "Alpha body.",
            provenance.clone(),
        );
        write_installed_skill(roots, "superpack--beta", "beta", "Beta body.", provenance);
    }

    #[test]
    fn parse_skill_source_resolves_curated_and_github_inputs() {
        assert_eq!(
            parse_skill_source("obra/superpowers").unwrap().repo,
            "https://github.com/obra/superpowers"
        );
        assert_eq!(
            parse_skill_source("https://github.com/obra/superpowers")
                .unwrap()
                .repo,
            "https://github.com/obra/superpowers"
        );
        assert_eq!(
            parse_skill_source("superpowers").unwrap().repo,
            "https://github.com/obra/superpowers"
        );
    }

    #[test]
    fn curated_skill_packs_are_deduped_and_resolvable() {
        let packs = curated_skill_packs();
        assert_eq!(packs.len(), 1, "one unique curated repo today");
        let sp = &packs[0];
        assert_eq!(sp.id, "superpowers");
        assert_eq!(sp.name, "Superpowers");
        assert_eq!(sp.repo, "https://github.com/obra/superpowers");
        // Every curated pack id must resolve through the normal source parser.
        assert_eq!(parse_skill_source(sp.id).unwrap().repo, sp.repo);
    }

    #[tokio::test]
    async fn install_single_skill_repo_copies_skill_and_records_provenance() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(
            repo.path(),
            "My Skill",
            "Does one thing well",
            "Use this for the thing.",
        );
        std::fs::write(repo.path().join("notes.txt"), "hello").unwrap();

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/acme/my-skill".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let pack = install_skill_source_with("https://github.com/acme/my-skill", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(pack.id, "my-skill");
        assert_eq!(pack.skills.len(), 1);
        assert_eq!(pack.skills[0].id, "my-skill");

        let installed = roots.skills_root.join("my-skill");
        assert!(installed.join("SKILL.md").is_file());
        assert!(installed.join("notes.txt").is_file());

        let provenance: SkillInstallProvenance = serde_json::from_str(
            &std::fs::read_to_string(installed.join(".ryuzi-skill.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(provenance.source, "https://github.com/acme/my-skill");
        assert_eq!(provenance.plugin_id, None);

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "my-skill");
        assert_eq!(listed[0].skill_count, 1);
    }

    /// `.codex-plugin/plugin.json` discovery materializes each declared
    /// skill under `skills_root` as `{plugin_id}--{skill}`, and writes
    /// nothing under `plugins_root` — no `PluginManifest` is synthesized or
    /// preserved for a skill pack (the v2 SDK manifest dropped `skills`
    /// entirely; a real plugin-folder install is a later task).
    #[tokio::test]
    async fn install_plugin_pack_materializes_skills_without_a_plugin_row() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "superpowers",
                "skills": "./skills/",
                "interface": {
                    "displayName": "Superpowers"
                }
            })
            .to_string(),
        )
        .unwrap();
        write_skill(
            &repo.path().join("skills/brainstorming"),
            "brainstorming",
            "Explore ideas",
            "Ask one question at a time.",
        );
        write_skill(
            &repo.path().join("skills/test-driven-development"),
            "test-driven-development",
            "Write tests first",
            "Red, green, refactor.",
        );

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let pack = install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(pack.id, "superpowers");
        assert_eq!(pack.name, "Superpowers");
        assert_eq!(pack.skills.len(), 2);

        // No plugin row is fabricated: `plugins_root` is never touched by a
        // skill-pack install.
        assert!(!roots.plugins_root.join("superpowers").exists());

        let brainstorming = roots.skills_root.join("superpowers--brainstorming");
        let tdd = roots
            .skills_root
            .join("superpowers--test-driven-development");
        assert!(brainstorming.join("SKILL.md").is_file());
        assert!(tdd.join("SKILL.md").is_file());
        let pack_provenance: SkillInstallProvenance = serde_json::from_str(
            &std::fs::read_to_string(brainstorming.join(PROVENANCE_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            pack_provenance.source,
            "https://github.com/obra/superpowers"
        );
        assert_eq!(pack_provenance.plugin_id.as_deref(), Some("superpowers"));

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "superpowers");
        assert_eq!(listed[0].skill_count, 2);
    }

    #[tokio::test]
    async fn install_single_skill_returns_fresh_shape_when_same_id_pack_artifacts_exist() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "superpowers", "Explore ideas", "Fresh single.");

        let roots = InstallRoots::new(config.path().to_path_buf());
        std::fs::create_dir_all(roots.plugins_root.join("superpowers")).unwrap();
        std::fs::write(
            roots
                .plugins_root
                .join("superpowers")
                .join("ryuzi-plugin.toml"),
            r#"
contract = 1
id = "superpowers"
name = "Superpowers"

[[skills]]
name = "brainstorming"
description = "Explore ideas"
path = "skills/brainstorming"
"#
            .trim_start(),
        )
        .unwrap();
        write_skill(
            &roots.skills_root.join("superpowers--brainstorming"),
            "brainstorming",
            "Explore ideas",
            "Stale pack skill.",
        );
        write_provenance(
            &roots
                .skills_root
                .join("superpowers--brainstorming")
                .join(PROVENANCE_FILE),
            &SkillInstallProvenance {
                source: "https://github.com/obra/superpowers".to_string(),
                plugin_id: Some("superpowers".to_string()),
                installed_at: "9999-12-31T23:59:59.999Z".to_string(),
            },
        )
        .unwrap();

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let pack = install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(pack.id, "superpowers");
        assert_eq!(pack.plugin_id, None);
        assert_eq!(pack.skills.len(), 1);
        assert_eq!(
            pack.skills,
            vec![InstalledSkillEntry {
                id: "superpowers".to_string(),
                name: "superpowers".to_string(),
            }]
        );
        assert!(!roots.plugins_root.join("superpowers").exists());
        assert!(!roots
            .skills_root
            .join("superpowers--brainstorming")
            .exists());
        assert!(roots
            .skills_root
            .join("superpowers")
            .join("SKILL.md")
            .is_file());

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "superpowers");
        assert_eq!(listed[0].plugin_id, None);
        assert_eq!(listed[0].skill_count, 1);
    }

    #[tokio::test]
    async fn install_plugin_pack_returns_fresh_shape_when_same_id_single_artifact_exists() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "superpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Superpowers" }
            })
            .to_string(),
        )
        .unwrap();
        write_skill(
            &repo.path().join("skills/brainstorming"),
            "brainstorming",
            "Explore ideas",
            "Fresh pack skill.",
        );

        let roots = InstallRoots::new(config.path().to_path_buf());
        write_skill(
            &roots.skills_root.join("superpowers"),
            "superpowers",
            "Explore ideas",
            "Stale single skill.",
        );
        write_provenance(
            &roots.skills_root.join("superpowers").join(PROVENANCE_FILE),
            &SkillInstallProvenance {
                source: "https://github.com/obra/superpowers".to_string(),
                plugin_id: None,
                installed_at: "9999-12-31T23:59:59.999Z".to_string(),
            },
        )
        .unwrap();

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let pack = install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(pack.id, "superpowers");
        assert_eq!(pack.plugin_id.as_deref(), Some("superpowers"));
        assert_eq!(pack.name, "Superpowers");
        assert_eq!(
            pack.skills,
            vec![InstalledSkillEntry {
                id: "superpowers--brainstorming".to_string(),
                name: "brainstorming".to_string(),
            }]
        );
        assert!(!roots.skills_root.join("superpowers").exists());
        // No plugin row is fabricated: `plugins_root` is never touched by a
        // skill-pack install.
        assert!(!roots.plugins_root.join("superpowers").exists());
        assert!(roots
            .skills_root
            .join("superpowers--brainstorming")
            .join("SKILL.md")
            .is_file());

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "superpowers");
        assert_eq!(listed[0].plugin_id.as_deref(), Some("superpowers"));
        assert_eq!(listed[0].skill_count, 1);
    }

    #[tokio::test]
    async fn install_bare_plugin_pack_discovers_skills_root_entries() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(
            &repo.path().join("skills/alpha"),
            "alpha",
            "First skill",
            "Alpha body.",
        );
        write_skill(
            &repo.path().join("skills/beta"),
            "beta",
            "Second skill",
            "Beta body.",
        );

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/acme/toolbox".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let pack = install_skill_source_with("acme/toolbox", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(pack.id, "toolbox");
        assert_eq!(pack.skills.len(), 2);
        assert!(!roots.plugins_root.join("toolbox").exists());
        assert!(roots
            .skills_root
            .join("toolbox--alpha")
            .join("SKILL.md")
            .is_file());
        assert!(roots
            .skills_root
            .join("toolbox--beta")
            .join("SKILL.md")
            .is_file());
    }

    #[tokio::test]
    async fn refresh_plugin_pack_replaces_materialized_skill_set() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "superpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Superpowers" }
            })
            .to_string(),
        )
        .unwrap();
        write_skill(
            &repo.path().join("skills/brainstorming"),
            "brainstorming",
            "Explore ideas",
            "v1",
        );

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        std::fs::remove_dir_all(repo.path().join("skills/brainstorming")).unwrap();
        write_skill(
            &repo.path().join("skills/test-driven-development"),
            "test-driven-development",
            "Write tests first",
            "v2",
        );

        let refreshed = refresh_installed_skill_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(refreshed.id, "superpowers");
        assert_eq!(refreshed.skills.len(), 1);
        assert!(!roots
            .skills_root
            .join("superpowers--brainstorming")
            .exists());
        assert!(roots
            .skills_root
            .join("superpowers--test-driven-development")
            .join("SKILL.md")
            .is_file());
    }

    #[tokio::test]
    async fn refresh_plugin_pack_removes_old_pack_when_plugin_id_changes() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "superpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Superpowers" }
            })
            .to_string(),
        )
        .unwrap();
        write_skill(
            &repo.path().join("skills/brainstorming"),
            "brainstorming",
            "Explore ideas",
            "v1",
        );

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let installed = install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();
        assert_eq!(installed.id, "superpowers");
        assert!(roots
            .skills_root
            .join("superpowers--brainstorming")
            .exists());

        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "mindpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Mindpowers" }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::remove_dir_all(repo.path().join("skills/brainstorming")).unwrap();
        write_skill(
            &repo.path().join("skills/focus"),
            "focus",
            "Stay on target",
            "v2",
        );

        let refreshed = refresh_installed_skill_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(refreshed.id, "mindpowers");
        assert_eq!(refreshed.plugin_id.as_deref(), Some("mindpowers"));
        assert!(!roots
            .skills_root
            .join("superpowers--brainstorming")
            .exists());
        // No plugin row is ever fabricated for either identity.
        assert!(!roots.plugins_root.join("superpowers").exists());
        assert!(!roots.plugins_root.join("mindpowers").exists());
        assert!(roots.skills_root.join("mindpowers--focus").exists());

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "mindpowers");
        assert_eq!(listed[0].skill_count, 1);
    }

    #[tokio::test]
    async fn refresh_single_skill_removes_old_id_when_skill_name_changes() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "Brainstorming", "Explore ideas", "v1");

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/acme/skill-pack".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let installed = install_skill_source_with("acme/skill-pack", &roots, &cloner)
            .await
            .unwrap();
        assert_eq!(installed.id, "brainstorming");
        assert!(roots.skills_root.join("brainstorming").exists());

        write_skill(repo.path(), "Fresh Ideas", "Explore ideas", "v2");

        let refreshed = refresh_installed_skill_with("brainstorming", &roots, &cloner)
            .await
            .unwrap();
        assert_eq!(refreshed.id, "fresh-ideas");
        assert!(roots.skills_root.join("fresh-ideas").exists());
        assert!(!roots.skills_root.join("brainstorming").exists());

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "fresh-ideas");
    }

    #[tokio::test]
    async fn refresh_plugin_pack_to_single_skill_removes_old_pack_artifacts() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "superpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Superpowers" }
            })
            .to_string(),
        )
        .unwrap();
        write_skill(
            &repo.path().join("skills/brainstorming"),
            "brainstorming",
            "Explore ideas",
            "v1",
        );

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let installed = install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();
        assert_eq!(installed.id, "superpowers");

        std::fs::remove_file(repo.path().join(".codex-plugin/plugin.json")).unwrap();
        std::fs::remove_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::remove_dir_all(repo.path().join("skills")).unwrap();
        write_skill(repo.path(), "Fresh Ideas", "Explore ideas", "v2");

        let refreshed = refresh_installed_skill_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(refreshed.id, "fresh-ideas");
        assert_eq!(refreshed.plugin_id, None);
        assert!(!roots.plugins_root.join("superpowers").exists());
        assert!(!roots
            .skills_root
            .join("superpowers--brainstorming")
            .exists());
        assert!(roots.skills_root.join("fresh-ideas").exists());

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "fresh-ideas");
    }

    #[tokio::test]
    async fn refresh_plugin_pack_to_same_id_single_skill_removes_old_pack_artifacts() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "superpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Superpowers" }
            })
            .to_string(),
        )
        .unwrap();
        write_skill(
            &repo.path().join("skills/brainstorming"),
            "brainstorming",
            "Explore ideas",
            "v1",
        );

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let installed = install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();
        assert_eq!(installed.id, "superpowers");
        assert_eq!(installed.plugin_id.as_deref(), Some("superpowers"));
        assert!(roots
            .skills_root
            .join("superpowers--brainstorming")
            .exists());

        std::fs::remove_file(repo.path().join(".codex-plugin/plugin.json")).unwrap();
        std::fs::remove_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::remove_dir_all(repo.path().join("skills")).unwrap();
        write_skill(repo.path(), "superpowers", "Explore ideas", "v2");

        let refreshed = refresh_installed_skill_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(refreshed.id, "superpowers");
        assert_eq!(refreshed.plugin_id, None);
        assert!(!roots.plugins_root.join("superpowers").exists());
        assert!(!roots
            .skills_root
            .join("superpowers--brainstorming")
            .exists());
        assert!(roots
            .skills_root
            .join("superpowers")
            .join("SKILL.md")
            .is_file());

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "superpowers");
        assert_eq!(listed[0].plugin_id, None);
        assert_eq!(listed[0].skill_count, 1);
    }

    #[tokio::test]
    async fn refresh_single_skill_to_same_id_plugin_pack_removes_old_single_artifact() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "superpowers", "Explore ideas", "v1");

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let installed = install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();
        assert_eq!(installed.id, "superpowers");
        assert_eq!(installed.plugin_id, None);
        assert!(roots.skills_root.join("superpowers").exists());

        std::fs::create_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "superpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Superpowers" }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::remove_file(repo.path().join("SKILL.md")).unwrap();
        write_skill(
            &repo.path().join("skills/brainstorming"),
            "brainstorming",
            "Explore ideas",
            "v2",
        );

        let refreshed = refresh_installed_skill_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(refreshed.id, "superpowers");
        assert_eq!(refreshed.plugin_id.as_deref(), Some("superpowers"));
        assert!(!roots.skills_root.join("superpowers").exists());
        assert!(!roots.plugins_root.join("superpowers").exists());
        assert!(roots
            .skills_root
            .join("superpowers--brainstorming")
            .join("SKILL.md")
            .is_file());

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "superpowers");
        assert_eq!(listed[0].plugin_id.as_deref(), Some("superpowers"));
        assert_eq!(listed[0].skill_count, 1);
    }

    #[tokio::test]
    async fn remove_installed_skill_deletes_plugin_and_materialized_skills() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "superpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Superpowers" }
            })
            .to_string(),
        )
        .unwrap();
        write_skill(
            &repo.path().join("skills/brainstorming"),
            "brainstorming",
            "Explore ideas",
            "v1",
        );

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();
        remove_installed_skill_in(&roots, "superpowers").unwrap();

        assert!(!roots.plugins_root.join("superpowers").exists());
        assert!(!roots
            .skills_root
            .join("superpowers--brainstorming")
            .exists());
        assert!(list_installed_skills_in(&roots).unwrap().is_empty());
    }

    #[test]
    fn remove_installed_skill_deletes_hidden_mixed_generation_artifacts() {
        let config = tempfile::tempdir().unwrap();
        let roots = InstallRoots::new(config.path().to_path_buf());

        std::fs::create_dir_all(roots.plugins_root.join("superpowers")).unwrap();
        std::fs::write(
            roots
                .plugins_root
                .join("superpowers")
                .join("ryuzi-plugin.toml"),
            r#"
contract = 1
id = "superpowers"
name = "Superpowers"

[[skills]]
name = "focus"
description = "Stay on target"
path = "skills/focus"
"#
            .trim_start(),
        )
        .unwrap();
        write_installed_skill(
            &roots,
            "superpowers",
            "superpowers",
            "Stale single generation.",
            SkillInstallProvenance {
                source: "https://github.com/obra/superpowers".to_string(),
                plugin_id: None,
                installed_at: "2026-01-01T00:00:00.000Z".to_string(),
            },
        );
        write_installed_skill(
            &roots,
            "superpowers--focus",
            "focus",
            "Current pack generation.",
            SkillInstallProvenance {
                source: "https://github.com/obra/superpowers".to_string(),
                plugin_id: Some("superpowers".to_string()),
                installed_at: "2026-02-01T00:00:00.000Z".to_string(),
            },
        );

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "superpowers");
        assert_eq!(listed[0].plugin_id.as_deref(), Some("superpowers"));

        remove_installed_skill_in(&roots, "superpowers").unwrap();

        assert!(!roots.plugins_root.join("superpowers").exists());
        assert!(!roots.skills_root.join("superpowers").exists());
        assert!(!roots.skills_root.join("superpowers--focus").exists());
        assert!(list_installed_skills_in(&roots).unwrap().is_empty());
    }

    #[tokio::test]
    async fn refresh_plugin_pack_plugin_id_change_deletes_hidden_old_identity_artifacts() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "superpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Superpowers" }
            })
            .to_string(),
        )
        .unwrap();
        write_skill(
            &repo.path().join("skills/brainstorming"),
            "brainstorming",
            "Explore ideas",
            "v1",
        );

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        write_installed_skill(
            &roots,
            "superpowers",
            "superpowers",
            "Hidden stale single generation.",
            SkillInstallProvenance {
                source: "https://github.com/obra/superpowers".to_string(),
                plugin_id: None,
                installed_at: "2025-12-31T23:59:59.999Z".to_string(),
            },
        );
        write_installed_skill(
            &roots,
            "superpowers--focus",
            "focus",
            "Hidden stale old-id pack skill.",
            SkillInstallProvenance {
                source: "https://github.com/obra/superpowers".to_string(),
                plugin_id: Some("superpowers".to_string()),
                installed_at: "2025-12-31T23:59:59.999Z".to_string(),
            },
        );

        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "mindpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Mindpowers" }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::remove_dir_all(repo.path().join("skills/brainstorming")).unwrap();
        write_skill(
            &repo.path().join("skills/focus"),
            "focus",
            "Stay on target",
            "v2",
        );

        let refreshed = refresh_installed_skill_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(refreshed.id, "mindpowers");
        assert!(!roots.plugins_root.join("superpowers").exists());
        assert!(!roots.skills_root.join("superpowers").exists());
        assert!(!roots
            .skills_root
            .join("superpowers--brainstorming")
            .exists());
        assert!(!roots.skills_root.join("superpowers--focus").exists());
        // No plugin row is fabricated for the new identity either.
        assert!(!roots.plugins_root.join("mindpowers").exists());
        assert!(roots.skills_root.join("mindpowers--focus").exists());
    }

    #[tokio::test]
    async fn refresh_single_to_plugin_id_change_deletes_hidden_old_pack_artifacts() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "superpowers", "Explore ideas", "v1");

        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };

        let installed = install_skill_source_with("superpowers", &roots, &cloner)
            .await
            .unwrap();
        assert_eq!(installed.id, "superpowers");
        assert_eq!(installed.plugin_id, None);
        assert!(roots.skills_root.join("superpowers").exists());

        std::fs::create_dir_all(roots.plugins_root.join("superpowers")).unwrap();
        std::fs::write(
            roots
                .plugins_root
                .join("superpowers")
                .join("ryuzi-plugin.toml"),
            r#"
contract = 1
id = "superpowers"
name = "Superpowers"

[[skills]]
name = "focus"
description = "Stay on target"
path = "skills/focus"
"#
            .trim_start(),
        )
        .unwrap();
        write_installed_skill(
            &roots,
            "superpowers--focus",
            "focus",
            "Hidden stale old-id pack skill.",
            SkillInstallProvenance {
                source: "https://github.com/obra/superpowers".to_string(),
                plugin_id: Some("superpowers".to_string()),
                installed_at: "2025-12-31T23:59:59.999Z".to_string(),
            },
        );

        std::fs::create_dir_all(repo.path().join(".codex-plugin")).unwrap();
        std::fs::write(
            repo.path().join(".codex-plugin/plugin.json"),
            serde_json::json!({
                "name": "mindpowers",
                "skills": "./skills/",
                "interface": { "displayName": "Mindpowers" }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::remove_file(repo.path().join("SKILL.md")).unwrap();
        write_skill(
            &repo.path().join("skills/focus"),
            "focus",
            "Stay on target",
            "v2",
        );

        let refreshed = refresh_installed_skill_with("superpowers", &roots, &cloner)
            .await
            .unwrap();

        assert_eq!(refreshed.id, "mindpowers");
        assert_eq!(refreshed.plugin_id.as_deref(), Some("mindpowers"));
        assert!(!roots.plugins_root.join("superpowers").exists());
        assert!(!roots.skills_root.join("superpowers").exists());
        assert!(!roots.skills_root.join("superpowers--focus").exists());
        // No plugin row is fabricated for the new identity either.
        assert!(!roots.plugins_root.join("mindpowers").exists());
        assert!(roots
            .skills_root
            .join("mindpowers--focus")
            .join("SKILL.md")
            .is_file());

        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "mindpowers");
        assert_eq!(listed[0].plugin_id.as_deref(), Some("mindpowers"));
        assert_eq!(listed[0].skill_count, 1);
    }

    #[test]
    fn fingerprint_is_stable_and_excludes_git_and_stamp() {
        let a = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(a.path().join("skills/x")).unwrap();
        std::fs::write(a.path().join("skills/x/SKILL.md"), "hello").unwrap();
        let fp1 = fingerprint_dir(a.path()).unwrap();

        // Same content in a different dir → same fingerprint.
        let b = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(b.path().join("skills/x")).unwrap();
        std::fs::write(b.path().join("skills/x/SKILL.md"), "hello").unwrap();
        // Noise that must NOT affect the fingerprint:
        std::fs::create_dir_all(b.path().join(".git")).unwrap();
        std::fs::write(b.path().join(".git/config"), "junk").unwrap();
        std::fs::write(b.path().join(PROVENANCE_FILE), "{\"source\":\"x\"}").unwrap();
        let fp2 = fingerprint_dir(b.path()).unwrap();
        assert_eq!(fp1, fp2);

        // Changed content → different fingerprint.
        std::fs::write(b.path().join("skills/x/SKILL.md"), "changed").unwrap();
        assert_ne!(fp1, fingerprint_dir(b.path()).unwrap());
    }

    #[test]
    fn dir_swap_commits_all_or_restores_on_failure() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        // Pre-existing target with sentinel content.
        std::fs::create_dir_all(base.join("t1")).unwrap();
        std::fs::write(base.join("t1/old.txt"), "old").unwrap();

        // Two source dirs to stage into t1 and t2.
        let src1 = tempfile::tempdir().unwrap();
        std::fs::write(src1.path().join("new.txt"), "new1").unwrap();
        let src2 = tempfile::tempdir().unwrap();
        std::fs::write(src2.path().join("new.txt"), "new2").unwrap();

        // Happy path: both commit.
        let mut swap = DirSwap::new();
        swap.stage(src1.path(), &base.join("t1")).unwrap();
        swap.stage(src2.path(), &base.join("t2")).unwrap();
        swap.commit().unwrap();
        assert_eq!(
            std::fs::read_to_string(base.join("t1/new.txt")).unwrap(),
            "new1"
        );
        assert!(!base.join("t1/old.txt").exists());
        assert_eq!(
            std::fs::read_to_string(base.join("t2/new.txt")).unwrap(),
            "new2"
        );

        // Rollback path: a staged swap whose commit fails must restore t1.
        std::fs::write(base.join("t1/new.txt"), "new1").unwrap(); // t1 currently committed content
        let src3 = tempfile::tempdir().unwrap();
        std::fs::write(src3.path().join("v.txt"), "v3").unwrap();
        std::fs::create_dir_all(base.join("sub")).unwrap();
        let mut swap = DirSwap::new();
        swap.stage(src3.path(), &base.join("t1")).unwrap();
        swap.stage(src3.path(), &base.join("sub/child")).unwrap();
        // Sabotage the second target's parent so its commit-time rename fails.
        std::fs::remove_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("sub"), "now a file").unwrap();
        assert!(swap.commit().is_err());
        // t1 must be restored to its committed content, not left as v3.
        assert_eq!(
            std::fs::read_to_string(base.join("t1/new.txt")).unwrap(),
            "new1"
        );
        assert!(!base.join("t1/v.txt").exists());
    }

    #[tokio::test]
    async fn recorded_install_writes_ledger_row_with_fingerprint_and_trust() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "My Skill", "d", "body");
        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/acme/my-skill".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: Some("deadbeef".into()),
        };

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();

        let pack = install_skill_source_with_recorded(
            "https://github.com/acme/my-skill",
            &roots,
            &cloner,
            &store,
        )
        .await
        .unwrap();

        let rec = store.get_plugin_install(&pack.id).await.unwrap().unwrap();
        assert_eq!(rec.source_spec, "https://github.com/acme/my-skill");
        assert_eq!(rec.resolved_commit.as_deref(), Some("deadbeef"));
        assert!(rec.fingerprint.starts_with("sha256:"));
        assert_eq!(rec.kind, "single_skill");
        // Arbitrary (non-curated) source → acknowledged tier.
        assert_eq!(rec.trust_tier, "acknowledged");
    }

    #[tokio::test]
    async fn recorded_install_of_a_curated_source_gets_curated_tier_without_ack() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "Superpowers", "d", "body");
        let mut repos = BTreeMap::new();
        // The `"superpowers"` alias resolves to this canonical repo, which is
        // in `CURATED_SKILL_SOURCES`.
        repos.insert(
            "https://github.com/obra/superpowers".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: Some("cafef00d".into()),
        };

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();

        let pack = install_skill_source_with_recorded("superpowers", &roots, &cloner, &store)
            .await
            .unwrap();

        let rec = store.get_plugin_install(&pack.id).await.unwrap().unwrap();
        // Curated source → curated tier, and no acknowledgement timestamp
        // (nothing to acknowledge — curated packs are trusted by default).
        assert_eq!(rec.trust_tier, "curated");
        assert!(rec.trust_ack_at.is_none());
        // `source_spec` preserves the caller's literal alias, not the
        // canonicalized repo.
        assert_eq!(rec.source_spec, "superpowers");
    }

    #[tokio::test]
    async fn backfill_records_missing_installs_idempotently() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "S", "d", "b");
        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/acme/s".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };
        // Install WITHOUT recording (legacy install).
        install_skill_source_with("https://github.com/acme/s", &roots, &cloner)
            .await
            .unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        let n = backfill_install_records_in(&roots, &store).await.unwrap();
        assert_eq!(n, 1);
        let rec = store.get_plugin_install("s").await.unwrap().unwrap();
        assert!(rec.resolved_commit.is_none()); // backfilled rows have no commit
                                                // Re-run is a no-op.
        assert_eq!(
            backfill_install_records_in(&roots, &store).await.unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn sweep_removes_stale_leftovers_but_keeps_real_installs() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "S", "d", "b");
        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/acme/s".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };
        install_skill_source_with("https://github.com/acme/s", &roots, &cloner)
            .await
            .unwrap();

        // Fabricate crash leftovers under both roots: a `.stage-`/`.backup-`
        // pair (DirSwap) and a `.tmp-` dir (replace_dir_from), one of which
        // even carries a stray provenance stamp — the exact case that would
        // otherwise be misread as a phantom installed skill.
        roots.ensure_exists().unwrap();
        let leftover_stage = roots.skills_root.join(".stage-abc123");
        std::fs::create_dir_all(&leftover_stage).unwrap();
        std::fs::write(leftover_stage.join(".ryuzi-skill.json"), "{}").unwrap();
        let leftover_backup = roots.plugins_root.join(".backup-def456");
        std::fs::create_dir_all(&leftover_backup).unwrap();
        let leftover_tmp = roots.skills_root.join(".tmp-ghi789");
        std::fs::create_dir_all(&leftover_tmp).unwrap();

        let removed = sweep_stale_install_leftovers_in(&roots).unwrap();
        assert_eq!(removed, 3);
        assert!(!leftover_stage.exists());
        assert!(!leftover_backup.exists());
        assert!(!leftover_tmp.exists());
        // The real install must be untouched.
        assert!(roots.skills_root.join("s").join("SKILL.md").is_file());
        let listed = list_installed_skills_in(&roots).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "s");

        // Idempotent: a clean tree sweeps to zero.
        assert_eq!(sweep_stale_install_leftovers_in(&roots).unwrap(), 0);
    }

    /// The fixture repo directory `recorded_setup` writes the source skill
    /// into. Kept as a fixed sibling of the skills/plugins roots under the
    /// same temp config root so a second `FakeRepoCloner` can be built
    /// against the same fixture content later in a test (simulating a
    /// re-clone of the same upstream repo at a new commit).
    fn roots_repo(roots: &InstallRoots) -> PathBuf {
        roots.config_root.join("_fixture_repo")
    }

    /// A `FakeRepoCloner` that resolves `source` to `repo_path` and reports
    /// `commit` as the resolved HEAD.
    fn fake_cloner(source: &str, repo_path: &Path, commit: &str) -> FakeRepoCloner {
        let parsed = parse_skill_source(source).unwrap();
        let mut repos = BTreeMap::new();
        repos.insert(parsed.repo, repo_path.to_path_buf());
        FakeRepoCloner {
            repos,
            commit: Some(commit.to_string()),
        }
    }

    /// Shared setup for the update tests: an `InstallRoots` over a temp
    /// config root, a `FakeRepoCloner` resolving `source` to a temp fixture
    /// repo (one skill named "P") at `commit`, and a temp `Store`. Tempdirs
    /// are leaked with `.keep()` so they outlive this function — acceptable
    /// for short-lived test processes.
    async fn recorded_setup(
        source: &str,
        commit: &str,
    ) -> (InstallRoots, FakeRepoCloner, crate::store::Store) {
        let config_root = tempfile::tempdir().unwrap().keep();
        let roots = InstallRoots::new(config_root);
        let repo_dir = roots_repo(&roots);
        write_skill(&repo_dir, "P", "d", "body");
        let cloner = fake_cloner(source, &repo_dir, commit);

        let db_path = tempfile::NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        let store = crate::store::Store::open(&db_path).await.unwrap();
        (roots, cloner, store)
    }

    #[tokio::test]
    async fn update_detects_already_current_by_commit() {
        let (roots, cloner, store) = recorded_setup("https://github.com/acme/p", "c1").await;
        install_skill_source_with_recorded("https://github.com/acme/p", &roots, &cloner, &store)
            .await
            .unwrap();
        // Same commit on re-clone → AlreadyCurrent, no swap.
        let outcome = update_installed_pack_with("p", false, &roots, &cloner, &store)
            .await
            .unwrap();
        assert_eq!(outcome, UpdateOutcome::AlreadyCurrent);
    }

    #[tokio::test]
    async fn remove_recorded_deletes_ledger_and_attach_rows_and_artifacts() {
        // The deletion path the Cockpit skill-pack Uninstall button now
        // delegates to: after a recorded uninstall, no ghost `plugin_installs`
        // row survives (which would otherwise make every future
        // `update_all_packs` report `Failed("unknown installed skill: p")`)
        // and no stale `plugin_attach_status` row bleeds into a reappeared
        // Browse card. Driven with injected `roots` because the install seam
        // is crate-private and unreachable from `ryuzi-cockpit`.
        let (roots, cloner, store) = recorded_setup("https://github.com/acme/p", "c1").await;
        let pack = install_skill_source_with_recorded(
            "https://github.com/acme/p",
            &roots,
            &cloner,
            &store,
        )
        .await
        .unwrap();
        // Seed an attach-status row for the same id, as a real attach would.
        store
            .record_plugin_attach(&crate::store::PluginAttachStatus {
                plugin_id: pack.id.clone(),
                last_attach_at: 1,
                outcome: "failed".to_string(),
                reason: Some("p failed to attach".to_string()),
            })
            .await
            .unwrap();
        assert!(store.get_plugin_install("p").await.unwrap().is_some());
        assert!(store.get_plugin_attach("p").await.unwrap().is_some());
        assert!(installed_pack_dir(&roots, &pack).exists());

        remove_installed_skill_recorded_with("p", &roots, &store)
            .await
            .unwrap();

        assert!(
            store.get_plugin_install("p").await.unwrap().is_none(),
            "the plugin_installs ledger row must be gone after a recorded uninstall"
        );
        assert!(
            store.get_plugin_attach("p").await.unwrap().is_none(),
            "the plugin_attach_status row must be gone too"
        );
        assert!(
            !installed_pack_dir(&roots, &pack).exists(),
            "on-disk artifacts must be removed"
        );
    }

    #[tokio::test]
    async fn update_refuses_local_edits_without_force() {
        let (roots, cloner_c1, store) = recorded_setup("https://github.com/acme/p", "c1").await;
        let pack = install_skill_source_with_recorded(
            "https://github.com/acme/p",
            &roots,
            &cloner_c1,
            &store,
        )
        .await
        .unwrap();
        // Simulate a local edit to the installed tree.
        let dir = installed_pack_dir(&roots, &pack);
        std::fs::write(dir.join("SKILL.md"), "locally edited").unwrap();
        let cloner_c2 = fake_cloner("https://github.com/acme/p", &roots_repo(&roots), "c2");
        assert_eq!(
            update_installed_pack_with("p", false, &roots, &cloner_c2, &store)
                .await
                .unwrap(),
            UpdateOutcome::LocalEdits
        );
        // force overrides.
        assert_eq!(
            update_installed_pack_with("p", true, &roots, &cloner_c2, &store)
                .await
                .unwrap(),
            UpdateOutcome::Updated
        );
    }

    #[tokio::test]
    async fn update_all_skips_pinned() {
        let (roots, cloner, store) = recorded_setup("https://github.com/acme/p", "c1").await;
        install_skill_source_with_recorded("https://github.com/acme/p", &roots, &cloner, &store)
            .await
            .unwrap();
        set_pack_pin("p", true, Some("frozen"), &store)
            .await
            .unwrap();
        let outcomes = update_all_packs_with(&roots, &cloner, &store)
            .await
            .unwrap();
        assert_eq!(
            outcomes,
            vec![("p".to_string(), UpdateOutcome::SkippedPinned)]
        );
    }

    #[tokio::test]
    async fn refresh_recorded_updates_ledger_fingerprint_and_preserves_installed_at() {
        let (roots, cloner, store) = recorded_setup("https://github.com/acme/p", "c1").await;
        install_skill_source_with_recorded("https://github.com/acme/p", &roots, &cloner, &store)
            .await
            .unwrap();
        let original = store.get_plugin_install("p").await.unwrap().unwrap();

        // Simulate a stale ledger fingerprint (as if left over from a bug in
        // an earlier code path) WITHOUT touching the on-disk pack, so a bare
        // refresh — which reinstalls the same content — must recompute it
        // back to the real on-disk hash.
        let mut stale = original.clone();
        stale.fingerprint = "sha256:stale".to_string();
        store.upsert_plugin_install(&stale).await.unwrap();

        let refreshed = refresh_installed_skill_recorded_with("p", &roots, &cloner, &store)
            .await
            .unwrap();
        assert_eq!(refreshed.id, "p");

        let rec = store.get_plugin_install("p").await.unwrap().unwrap();
        assert_ne!(rec.fingerprint, "sha256:stale");
        assert_eq!(
            rec.fingerprint, original.fingerprint,
            "refresh must recompute the fingerprint from the refreshed on-disk content"
        );
        assert_eq!(
            rec.installed_at, original.installed_at,
            "installed_at must survive a refresh unchanged"
        );
        assert!(rec.updated_at >= original.updated_at);
        // resolved_commit is left at its prior value — the refresh path
        // doesn't expose the freshly cloned commit — never nulled.
        assert_eq!(rec.resolved_commit, original.resolved_commit);
    }

    #[tokio::test]
    async fn refresh_recorded_backfills_a_ledger_row_when_none_existed() {
        let config = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "P", "d", "body");
        let mut repos = BTreeMap::new();
        repos.insert(
            "https://github.com/acme/p".to_string(),
            repo.path().to_path_buf(),
        );
        let roots = InstallRoots::new(config.path().to_path_buf());
        let cloner = FakeRepoCloner {
            repos,
            commit: None,
        };
        // Install WITHOUT recording (legacy install, no ledger row).
        install_skill_source_with("acme/p", &roots, &cloner)
            .await
            .unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        assert!(store.get_plugin_install("p").await.unwrap().is_none());

        refresh_installed_skill_recorded_with("p", &roots, &cloner, &store)
            .await
            .unwrap();

        let rec = store.get_plugin_install("p").await.unwrap().unwrap();
        assert_eq!(rec.source_spec, "https://github.com/acme/p");
        assert!(rec.resolved_commit.is_none());
    }

    #[tokio::test]
    async fn begin_curated_installs_immediately() {
        // "superpowers" is curated; map its canonical repo to a fixture.
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "S", "d", "b");
        let roots = InstallRoots::new(tempfile::tempdir().unwrap().keep());
        let cloner = FakeRepoCloner {
            repos: BTreeMap::from([(
                "https://github.com/obra/superpowers".into(),
                repo.path().to_path_buf(),
            )]),
            commit: Some("c1".into()),
        };
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        match begin_install_with("superpowers", &roots, &cloner, &store)
            .await
            .unwrap()
        {
            BeginInstall::Completed(p) => {
                assert_eq!(
                    store
                        .get_plugin_install(&p.id)
                        .await
                        .unwrap()
                        .unwrap()
                        .trust_tier,
                    "curated"
                );
            }
            BeginInstall::NeedsConfirmation(_) => panic!("curated must not prompt"),
        }
    }

    #[tokio::test]
    async fn begin_arbitrary_prompts_then_confirm_installs_with_ack() {
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "S", "d", "b");
        std::fs::create_dir_all(repo.path().join(".ryuzi/hooks/tool.before")).unwrap();
        std::fs::write(
            repo.path().join(".ryuzi/hooks/tool.before/guard.sh"),
            "#!/bin/sh\n",
        )
        .unwrap();
        let roots = InstallRoots::new(tempfile::tempdir().unwrap().keep());
        let cloner = FakeRepoCloner {
            repos: BTreeMap::from([(
                "https://github.com/acme/p".into(),
                repo.path().to_path_buf(),
            )]),
            commit: Some("c1".into()),
        };
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        let prompt = match begin_install_with("acme/p", &roots, &cloner, &store)
            .await
            .unwrap()
        {
            BeginInstall::NeedsConfirmation(p) => p,
            BeginInstall::Completed(_) => panic!("arbitrary source must prompt"),
        };
        assert_eq!(prompt.owner_repo, "acme/p");
        assert_eq!(
            prompt.hook_scripts,
            vec!["tool.before/guard.sh".to_string()]
        );
        assert!(store.get_plugin_install("s").await.unwrap().is_none()); // not installed yet

        let pack = confirm_install(&prompt.token, &store).await.unwrap();
        let rec = store.get_plugin_install(&pack.id).await.unwrap().unwrap();
        assert_eq!(rec.trust_tier, "acknowledged");
        let summary = rec.trust_ack_summary.expect("ack summary persisted");
        // The persisted snapshot must be a complete record of what was shown
        // in the trust prompt, including the size the user saw — not just
        // the identity/skills/hooks fields.
        let summary: serde_json::Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(summary["totalBytes"], serde_json::json!(prompt.total_bytes));
    }

    #[tokio::test]
    async fn confirm_install_rejects_unknown_or_expired_token() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        let err = confirm_install("no-such-token", &store)
            .await
            .expect_err("unknown token must be rejected");
        assert!(err.to_string().contains("expired"));
    }

    #[tokio::test]
    async fn confirm_install_rejects_a_staged_token_past_the_ttl() {
        // `confirm_install_rejects_unknown_or_expired_token` only covers the
        // unknown-token branch; this exercises the actual TTL-expiry branch
        // (`now_ms() - staged.created_ms > STAGED_INSTALL_TTL_MS`). No public
        // seam exists to backdate a staged install, so this reaches into
        // `staging_map()`/`StagedInstall` directly — both are private, but
        // this `tests` module is a descendant of `skills_install` and so has
        // the same visibility a same-module caller would.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();
        let roots = InstallRoots::new(tempfile::tempdir().unwrap().keep());
        let temp = tempfile::tempdir().unwrap();
        let repo_dir = temp.path().join("repo");
        std::fs::create_dir_all(&repo_dir).unwrap();
        let token = crate::paths::new_id();
        staging_map().lock().unwrap().insert(
            token.clone(),
            StagedInstall {
                parsed: parse_skill_source("acme/p").unwrap(),
                source_spec: "acme/p".to_string(),
                roots,
                _temp: temp,
                repo_dir,
                commit: None,
                ack_summary: "{}".to_string(),
                created_ms: crate::paths::now_ms() - STAGED_INSTALL_TTL_MS - 1,
                prior_id: None,
            },
        );

        let err = confirm_install(&token, &store)
            .await
            .expect_err("a staged install past the TTL must be rejected");
        assert!(err.to_string().contains("expired"));

        // The token is removed up front regardless of outcome (single-use),
        // so a replay hits the same "expired" message via the unknown-token
        // branch instead of silently completing the install.
        let err = confirm_install(&token, &store)
            .await
            .expect_err("an expired token must not be replayable");
        assert!(err.to_string().contains("expired"));
    }

    #[tokio::test]
    async fn update_needs_reack_when_pack_introduces_a_hook_script() {
        let (roots, cloner_c1, store) = recorded_setup("https://github.com/acme/p", "c1").await;
        install_skill_source_with_recorded("https://github.com/acme/p", &roots, &cloner_c1, &store)
            .await
            .unwrap();

        // Upstream adds a hook script before the next update.
        let repo_dir = roots_repo(&roots);
        std::fs::create_dir_all(repo_dir.join(".ryuzi/hooks/tool.before")).unwrap();
        std::fs::write(
            repo_dir.join(".ryuzi/hooks/tool.before/guard.sh"),
            "#!/bin/sh\n",
        )
        .unwrap();
        let cloner_c2 = fake_cloner("https://github.com/acme/p", &repo_dir, "c2");

        let outcome = update_installed_pack_with("p", false, &roots, &cloner_c2, &store)
            .await
            .unwrap();
        let prompt = match outcome {
            UpdateOutcome::NeedsReack(p) => p,
            other => panic!("expected NeedsReack, got {other:?}"),
        };
        assert_eq!(
            prompt.hook_scripts,
            vec!["tool.before/guard.sh".to_string()]
        );

        // The live install must be untouched — no swap happened yet.
        let rec = store.get_plugin_install("p").await.unwrap().unwrap();
        assert_eq!(rec.resolved_commit.as_deref(), Some("c1"));

        // Confirming completes the update and records the acknowledgment.
        let pack = confirm_install(&prompt.token, &store).await.unwrap();
        let rec = store.get_plugin_install(&pack.id).await.unwrap().unwrap();
        assert_eq!(rec.trust_tier, "acknowledged");
        assert!(rec
            .trust_ack_summary
            .as_deref()
            .unwrap()
            .contains("guard.sh"));
    }

    #[tokio::test]
    async fn confirm_install_reack_identity_change_cleans_up_old_artifacts_and_ledger_row() {
        let (roots, cloner_c1, store) = recorded_setup("https://github.com/acme/p", "c1").await;
        install_skill_source_with_recorded("https://github.com/acme/p", &roots, &cloner_c1, &store)
            .await
            .unwrap();
        assert!(roots.skills_root.join("p").exists());

        // Upstream renames the skill (changing its resolved id from "p" to
        // "q") AND introduces a hook script before the next update, so the
        // update routes through the re-ack trust gate instead of reinstalling
        // directly.
        let repo_dir = roots_repo(&roots);
        write_skill(&repo_dir, "Q", "d", "body-v2");
        std::fs::create_dir_all(repo_dir.join(".ryuzi/hooks/tool.before")).unwrap();
        std::fs::write(
            repo_dir.join(".ryuzi/hooks/tool.before/guard.sh"),
            "#!/bin/sh\n",
        )
        .unwrap();
        let cloner_c2 = fake_cloner("https://github.com/acme/p", &repo_dir, "c2");

        let outcome = update_installed_pack_with("p", false, &roots, &cloner_c2, &store)
            .await
            .unwrap();
        let prompt = match outcome {
            UpdateOutcome::NeedsReack(p) => p,
            other => panic!("expected NeedsReack, got {other:?}"),
        };

        let pack = confirm_install(&prompt.token, &store).await.unwrap();
        assert_eq!(pack.id, "q");

        // The old identity's on-disk artifacts must be gone...
        assert!(!roots.skills_root.join("p").exists());
        assert!(roots.skills_root.join("q").exists());
        // ...and so must its ledger row — no stale duplicate left behind.
        assert!(store.get_plugin_install("p").await.unwrap().is_none());
        let rec = store.get_plugin_install("q").await.unwrap().unwrap();
        assert_eq!(rec.trust_tier, "acknowledged");
    }

    #[tokio::test]
    async fn update_direct_id_change_drops_old_ledger_row_and_records_the_new_id() {
        // Covers `update_installed_pack_with`'s OWN id-change cleanup (`if
        // refreshed.id != rec.plugin_id { store.delete_plugin_install(...) }`)
        // — distinct from the reack-triggered id-change path already covered
        // by `confirm_install_reack_identity_change_cleans_up_old_artifacts_and_ledger_row`.
        // An update with no new hook scripts reinstalls directly (never
        // routes through `NeedsReack`/`confirm_install`), so this exercises
        // the ledger swap that happens inline in `update_installed_pack_with`.
        let (roots, cloner_c1, store) = recorded_setup("https://github.com/acme/p", "c1").await;
        install_skill_source_with_recorded("https://github.com/acme/p", &roots, &cloner_c1, &store)
            .await
            .unwrap();
        assert!(store.get_plugin_install("p").await.unwrap().is_some());

        // Upstream renames the skill (id "p" -> "q") without introducing any
        // hook scripts, so the update reinstalls directly instead of routing
        // through the re-ack trust gate.
        let repo_dir = roots_repo(&roots);
        write_skill(&repo_dir, "Q", "d", "body-v2");
        let cloner_c2 = fake_cloner("https://github.com/acme/p", &repo_dir, "c2");

        let outcome = update_installed_pack_with("p", false, &roots, &cloner_c2, &store)
            .await
            .unwrap();
        assert_eq!(outcome, UpdateOutcome::Updated);

        assert!(
            store.get_plugin_install("p").await.unwrap().is_none(),
            "the old id's ledger row must be gone after a direct (non-reack) id change"
        );
        let rec = store.get_plugin_install("q").await.unwrap().unwrap();
        assert_eq!(rec.resolved_commit.as_deref(), Some("c2"));
    }

    #[tokio::test]
    async fn update_skips_reack_when_hook_already_acknowledged() {
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "S", "d", "b");
        std::fs::create_dir_all(repo.path().join(".ryuzi/hooks/tool.before")).unwrap();
        std::fs::write(
            repo.path().join(".ryuzi/hooks/tool.before/guard.sh"),
            "#!/bin/sh\n",
        )
        .unwrap();
        let roots = InstallRoots::new(tempfile::tempdir().unwrap().keep());
        let cloner_c1 = FakeRepoCloner {
            repos: BTreeMap::from([(
                "https://github.com/acme/p".into(),
                repo.path().to_path_buf(),
            )]),
            commit: Some("c1".into()),
        };
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();

        // Begin + confirm so the hook script is already acknowledged.
        let prompt = match begin_install_with("acme/p", &roots, &cloner_c1, &store)
            .await
            .unwrap()
        {
            BeginInstall::NeedsConfirmation(p) => p,
            BeginInstall::Completed(_) => panic!("arbitrary source must prompt"),
        };
        confirm_install(&prompt.token, &store).await.unwrap();

        // Same hook script, new commit — must update normally, not re-prompt.
        let cloner_c2 = FakeRepoCloner {
            repos: BTreeMap::from([(
                "https://github.com/acme/p".into(),
                repo.path().to_path_buf(),
            )]),
            commit: Some("c2".into()),
        };
        let outcome = update_installed_pack_with("s", false, &roots, &cloner_c2, &store)
            .await
            .unwrap();
        assert_eq!(outcome, UpdateOutcome::Updated);
    }

    // --- DT7 fix-wave: close the two trust-gate bypasses (update path +
    // raw `install_skill`) — see task-dt7-report.md's "Fix wave" section. ---

    #[tokio::test]
    async fn update_returns_updated_when_no_hooks_or_extensions_are_involved() {
        let (roots, cloner_c1, store) = recorded_setup("https://github.com/acme/p", "c1").await;
        install_skill_source_with_recorded("https://github.com/acme/p", &roots, &cloner_c1, &store)
            .await
            .unwrap();

        // Upstream just changes the skill body — no hooks, no manifest, no
        // extensions anywhere in this update. Must update normally.
        let repo_dir = roots_repo(&roots);
        write_skill(&repo_dir, "P", "d", "body-v2");
        let cloner_c2 = fake_cloner("https://github.com/acme/p", &repo_dir, "c2");

        let outcome = update_installed_pack_with("p", false, &roots, &cloner_c2, &store)
            .await
            .unwrap();
        assert_eq!(outcome, UpdateOutcome::Updated);
        let rec = store.get_plugin_install("p").await.unwrap().unwrap();
        assert_eq!(rec.resolved_commit.as_deref(), Some("c2"));
    }

    #[tokio::test]
    async fn install_skill_source_gated_installs_curated_non_code_source() {
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "S", "d", "b");
        let roots = InstallRoots::new(tempfile::tempdir().unwrap().keep());
        let cloner = FakeRepoCloner {
            repos: BTreeMap::from([(
                "https://github.com/obra/superpowers".into(),
                repo.path().to_path_buf(),
            )]),
            commit: Some("c1".into()),
        };
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();

        let pack = install_skill_source_gated_with("superpowers", &roots, &cloner, &store)
            .await
            .unwrap();
        assert!(roots.skills_root.join(&pack.id).exists());
        let rec = store.get_plugin_install(&pack.id).await.unwrap().unwrap();
        assert_eq!(rec.trust_tier, "curated");
    }

    #[tokio::test]
    async fn install_skill_source_gated_refuses_an_arbitrary_source() {
        let repo = tempfile::tempdir().unwrap();
        write_skill(repo.path(), "S", "d", "b");
        let roots = InstallRoots::new(tempfile::tempdir().unwrap().keep());
        let cloner = FakeRepoCloner {
            repos: BTreeMap::from([(
                "https://github.com/acme/p".into(),
                repo.path().to_path_buf(),
            )]),
            commit: Some("c1".into()),
        };
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = crate::store::Store::open(tmp.path()).await.unwrap();

        let err = install_skill_source_gated_with("acme/p", &roots, &cloner, &store)
            .await
            .expect_err("an arbitrary source must be refused, not silently installed");
        assert!(err.to_string().contains("begin_install"));
        assert!(list_installed_skills_in(&roots).unwrap().is_empty());
        assert!(store.get_plugin_install("p").await.unwrap().is_none());
    }

    /// `install_skill_source_gated_with` discards a refused
    /// `NeedsConfirmation` outcome's staged entry right away instead of
    /// leaving it in `staging_map()` for the full TTL (see
    /// `discard_staged_install`). Checked directly against two tokens this
    /// test owns — not against the map's global emptiness, which would be
    /// racy under `cargo test`'s default parallel execution (other tests may
    /// have their own entries staged concurrently).
    #[test]
    fn discard_staged_install_removes_only_the_given_token() {
        fn fake_staged_install() -> StagedInstall {
            let roots = InstallRoots::new(tempfile::tempdir().unwrap().keep());
            let temp = tempfile::tempdir().unwrap();
            let repo_dir = temp.path().join("repo");
            std::fs::create_dir_all(&repo_dir).unwrap();
            StagedInstall {
                parsed: parse_skill_source("acme/p").unwrap(),
                source_spec: "acme/p".to_string(),
                roots,
                _temp: temp,
                repo_dir,
                commit: None,
                ack_summary: "{}".to_string(),
                created_ms: crate::paths::now_ms(),
                prior_id: None,
            }
        }
        let keep_token = crate::paths::new_id();
        let drop_token = crate::paths::new_id();
        staging_map()
            .lock()
            .unwrap()
            .insert(keep_token.clone(), fake_staged_install());
        staging_map()
            .lock()
            .unwrap()
            .insert(drop_token.clone(), fake_staged_install());

        discard_staged_install(&drop_token);

        {
            let map = staging_map().lock().unwrap();
            assert!(!map.contains_key(&drop_token));
            assert!(map.contains_key(&keep_token));
        }
        // Don't leak this test's fixture into other parallel tests' view of
        // the (process-global) staging map.
        staging_map().lock().unwrap().remove(&keep_token);
    }

    // PR-2 fix D: a multi-skill pack yields one catalog row PER SKILL (keyed
    // by SKILL.md frontmatter name), grouped under the pack's display name —
    // not one collapsed row named after the pack.
    #[test]
    fn catalog_rows_list_each_skill_of_a_pack() {
        let config = tempfile::tempdir().unwrap();
        let roots = InstallRoots::new(config.path().to_path_buf());
        roots.ensure_exists().unwrap();
        install_fixture_pack_superpack(&roots);

        let rows = list_installed_skill_catalog_rows_in(&roots).unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.skill_name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        assert!(rows.iter().all(|r| r.pack.as_deref() == Some("Superpack")));
        assert!(
            !names.contains(&"superpack"),
            "the pack id itself is not a row"
        );
    }

    // Regression guard for the standalone-skill half of PR-2 fix D (not one
    // of the brief's two directive tests, added to replace the coverage lost
    // by deleting `catalog.rs`'s `standalone_installed_skill_carries_no_pack_name`,
    // which unit-tested the now-deleted `skill_catalog_entry` helper): a
    // standalone (non-pack) installed skill still gets its own catalog row,
    // with `pack: None`.
    #[test]
    fn catalog_rows_standalone_skill_carries_no_pack_name() {
        let config = tempfile::tempdir().unwrap();
        let roots = InstallRoots::new(config.path().to_path_buf());
        roots.ensure_exists().unwrap();
        write_installed_skill(
            &roots,
            "requesting-code-review",
            "requesting-code-review",
            "Body.",
            SkillInstallProvenance {
                source: "https://example.test/requesting-code-review".to_string(),
                plugin_id: None,
                installed_at: "2026-01-01T00:00:00.000Z".to_string(),
            },
        );

        let rows = list_installed_skill_catalog_rows_in(&roots).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].skill_name, "requesting-code-review");
        assert_eq!(rows[0].pack, None);
    }

    // PR-2 fix E shim: a legacy binding that stored the PACK id expands to
    // its member skill names; unknown ids pass through untouched.
    #[test]
    fn expand_skill_bindings_expands_pack_ids() {
        let config = tempfile::tempdir().unwrap();
        let roots = InstallRoots::new(config.path().to_path_buf());
        roots.ensure_exists().unwrap();
        install_fixture_pack_superpack(&roots);

        let out = expand_skill_bindings_in(&roots, &["superpack".into(), "gamma".into()]);
        assert!(out.contains(&"alpha".to_string()));
        assert!(out.contains(&"beta".to_string()));
        assert!(out.contains(&"gamma".to_string()));
        assert!(!out.contains(&"superpack".to_string()));
    }
}
