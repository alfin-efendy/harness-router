//! Sync a plugin manifest's `[[mcp]]` entries into the Apps domain
//! (`crate::mcp`'s `mcp_servers` table), replacing the old transient
//! per-session attach (`ControlPlane::attach_plugin_mcp_servers`, deleted by
//! this task). A synced row gets everything a user-added Apps card gets —
//! real per-tool perms, per-agent access, a probe, and hub presence —
//! instead of silently attaching for the lifetime of one session and
//! forgetting every perm the user set.
//!
//! # Call sites
//! - Plugin enable (`toggle_enabled`'s connector-only true-branch) and OAuth
//!   install completion (`complete_plugin_oauth`, once a fresh credential
//!   makes a previously-unresolvable connector resolvable) call
//!   [`sync_plugin_mcp`].
//! - Plugin disable does **not** call anything here: a disabled plugin's
//!   rows stay in `mcp_servers` (so a user's per-tool perms survive a
//!   disable/enable cycle) and are excluded from new sessions by
//!   `crate::mcp::servers_for_session`'s own enabled-plugin gate.
//! - Uninstall calls [`remove_plugin_mcp`].
//!
//! # Row id / identity
//! Each `[[mcp]]` entry becomes a row with id `"{plugin_id}-{def.name}"` —
//! stable across re-syncs and collision-proof against user-added servers
//! (which never carry a `plugin_id` and are free to pick any id). Re-syncing
//! an existing row is a plain [`crate::mcp::upsert_server`] followed by
//! [`crate::mcp::replace_tools`], which PRESERVES any user-set per-tool perm
//! for a tool name that survives rediscovery — sync never resets perms.
//!
//! # Principal attribution
//! The old transient path resolved `McpServerSpec.name` → owning-`Plugin`
//! attribution once, in the connector-scan loop itself, and threaded it
//! through `SessionCtx.mcp_principals`. That loop is gone; attribution now
//! flows from the row's own `plugin_id` column, re-derived at session-start
//! time by `ControlPlane::mcp_principals_for` (see `control/lifecycle.rs`)
//! from a fresh `crate::mcp::list_servers` read — the column this module
//! writes is the only thing that needs to carry the fact forward.
//!
//! # HTTP transport
//! The native stdio JSON-RPC probe (`crate::mcp::probe_stdio`) can only talk
//! to a stdio-transport server — `crates/core/src/harness/native/mcp_client.rs`
//! bails on HTTP outright, and fixing that native HTTP client is explicitly
//! out of scope here. An HTTP-transport `[[mcp]]` entry's row is synced with
//! `status = "unchecked"` instead of being probed or marked `"error"` — the
//! existing Apps screen "Probe" button (`api::apps_api::probe_and_persist`)
//! already runs a real HTTP reachability check independent of this sync, so
//! the row is fully usable once a user clicks it.

use crate::connector::ConnectorCtx;
use crate::domain::McpTransport;
use crate::mcp::{self, McpServerRow};
use crate::plugins::host::CorePlugin;
use crate::settings::SettingsStore;
use crate::store::{PluginAttachStatus, Store};

/// Upsert one Apps row per `plugin.manifest.mcp` entry (resolving every
/// `${auth}`/`${setting:KEY}`/`${env:VAR}` placeholder through the SAME
/// connector + resolver `declarative_plugin` already builds — reused via the
/// `Connector` trait rather than reimplemented), probe stdio rows for real,
/// and refresh their tool lists (preserving any user-set per-tool perm — see
/// module doc). Never errors on a not-yet-configured or unreachable plugin:
///
/// - a manifest with no `[[mcp]]` entries, or a plugin somehow carrying one
///   with no connector (shouldn't happen — `declarative_plugin` always
///   builds a connector when `manifest.mcp` is non-empty) — silent no-op;
/// - `Connector::ensure_auth` failing (e.g. a missing credential) — logged
///   and skipped, no rows written this call; the next enable/install-complete
///   retries;
/// - `Connector::mcp_servers` failing to resolve (e.g. an unresolvable
///   placeholder) — likewise logged and skipped;
/// - a stdio probe failing (server unreachable/misbehaving) — the row is
///   still written, with `status = "error"` + `status_detail` carrying the
///   probe's error, so the plugin still gets an Apps card to diagnose from.
///
/// Only a genuine store error (a DB write failing) surfaces as `Err`.
///
/// Also records the attach outcome into `plugin_attach_status`
/// (`Store::record_plugin_attach`) — the same table `plugin_doctor` already
/// reads back, previously written by the now-deleted
/// `ControlPlane::attach_plugin_mcp_servers`. `Connector::ensure_auth`'s
/// error text is used as-is: every real implementation (`declarative.rs`)
/// already curates it to be secret-free ("configure {id}: ..." messages
/// naming a setting key or help URL, never a value) — see that module's doc.
pub async fn sync_plugin_mcp(
    store: &Store,
    settings: &SettingsStore,
    plugin: &CorePlugin,
) -> anyhow::Result<()> {
    let id = plugin.manifest.id.clone();
    // F3: the current manifest's own row ids — computed up front so both the
    // early-empty-manifest path and the end-of-successful-sync path below
    // prune against the SAME set, regardless of which one fires.
    let keep_ids: Vec<String> = plugin
        .manifest
        .mcp
        .iter()
        .map(|def| format!("{id}-{}", def.name))
        .collect();

    if plugin.manifest.mcp.is_empty() {
        // A plugin update that dropped every `[[mcp]]` entry: prune all of
        // this plugin's rows now, since nothing below will run to do it.
        mcp::prune_plugin_servers(store, &id, &keep_ids).await?;
        return Ok(());
    }
    let Some(connector) = &plugin.connector else {
        return Ok(());
    };
    // Task 11 tiered trust: an `[[mcp]]` entry is an arbitrary stdio process
    // (or an HTTP endpoint) — for an unsigned (local-folder/git-URL) install,
    // it must not sync into the Apps domain until the user has explicitly
    // accepted the trust prompt (`plugin.<id>.trusted`, written only by
    // `install_sources::confirm_plugin_install`'s confirm step). Signed
    // catalog/first-party installs are trusted by construction and skip this
    // check entirely.
    if !crate::plugins::host::component_surfaces_trusted(settings, plugin).await {
        tracing::info!(
            plugin = %plugin.manifest.id,
            "mcp sync: skipping — unsigned mcp surfaces require explicit trust acceptance"
        );
        return Ok(());
    }
    let ctx = ConnectorCtx {
        project_id: id.clone(),
        work_dir: std::env::temp_dir(),
        settings: settings.clone(),
    };

    if let Err(e) = connector.ensure_auth(&ctx).await {
        tracing::warn!(plugin = %id, "mcp sync: connector not ready: {e}");
        let reason = safe_attach_reason(&id, AttachStage::Auth, &e);
        record_attach(store, &id, "failed", Some(reason)).await;
        return Ok(());
    }
    let specs = match connector.mcp_servers(&ctx).await {
        Ok(specs) => specs,
        Err(e) => {
            tracing::warn!(plugin = %id, "mcp sync: resolving mcp servers failed: {e}");
            let reason = safe_attach_reason(&id, AttachStage::McpServers, &e);
            record_attach(store, &id, "failed", Some(reason)).await;
            return Ok(());
        }
    };

    let multi_entry = plugin.manifest.mcp.len() > 1;
    for (def, spec) in plugin.manifest.mcp.iter().zip(specs.iter()) {
        let row_id = format!("{id}-{}", def.name);
        let name = if multi_entry {
            format!("{} — {}", plugin.manifest.name, def.name)
        } else {
            plugin.manifest.name.clone()
        };
        let (transport, command, args, env, url) = match &spec.transport {
            McpTransport::Stdio { command, args, env } => (
                "stdio",
                Some(command.clone()),
                args.clone(),
                env.clone(),
                None,
            ),
            // No `headers` column exists on `McpServerRow` yet — the same
            // pre-existing gap `servers_for_session` already has for
            // http-transport rows (it hardcodes `headers: vec![]`). Out of
            // scope here; see this module's HTTP-transport doc section.
            McpTransport::Http { url, .. } => {
                ("http", None, Vec::new(), Vec::new(), Some(url.clone()))
            }
        };
        let (auth_kind, auth_detail) = if env.is_empty() {
            ("none".to_string(), None)
        } else {
            (
                "env".to_string(),
                Some(
                    env.iter()
                        .map(|(k, _)| k.clone())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            )
        };

        let (status, status_detail, version, tools) = if transport == "stdio" {
            let cmd = command.clone().unwrap_or_default();
            let result = mcp::probe_stdio(&cmd, &args, &env).await;
            let status = if result.ok { "connected" } else { "error" }.to_string();
            (
                status,
                result.error.clone(),
                result.server_version.clone(),
                result.ok.then(|| result.tools.clone()),
            )
        } else {
            ("unchecked".to_string(), None, None, None)
        };
        // A probe that came back without a version (a fresh http row, or a
        // failed stdio probe) must not clobber a version a PRIOR successful
        // probe recorded.
        let version = match version {
            Some(v) => Some(v),
            None => mcp::get_server(store, &row_id)
                .await?
                .and_then(|row| row.version),
        };

        mcp::upsert_server(
            store,
            McpServerRow {
                id: row_id.clone(),
                name,
                kind: "MCP server".into(),
                color: "#8B8B8B".into(),
                description: plugin.manifest.description.clone(),
                transport: transport.into(),
                command,
                args,
                env,
                url,
                scope: "global".into(),
                scope_gateways: vec![],
                version,
                publisher: (!plugin.manifest.publisher.is_empty())
                    .then(|| plugin.manifest.publisher.clone()),
                status,
                status_detail,
                auth_kind,
                auth_detail,
                plugin_id: Some(id.clone()),
            },
        )
        .await?;

        if let Some(tools) = tools {
            mcp::replace_tools(store, &row_id, tools).await?;
        }
    }

    // F3: every entry the manifest currently declares was just
    // upserted above (this point is only reached when auth + resolution +
    // every upsert succeeded), so anything else this plugin owns is a row a
    // prior manifest declared and this one dropped — prune it and its
    // tools/agent-access rows. Not run on any of the early-return paths
    // above (auth failure, unresolvable, untrusted) — a transient failure
    // must never delete a working row that just failed to refresh this
    // round.
    mcp::prune_plugin_servers(store, &id, &keep_ids).await?;

    record_attach(store, &id, "ok", None).await;
    Ok(())
}

/// Best-effort `plugin_attach_status` write — never surfaces its own
/// failure, mirroring the warn-and-continue discipline the deleted
/// `ControlPlane::attach_plugin_mcp_servers` used for the same table.
async fn record_attach(store: &Store, plugin_id: &str, outcome: &str, reason: Option<String>) {
    let _ = store
        .record_plugin_attach(&PluginAttachStatus {
            plugin_id: plugin_id.to_string(),
            last_attach_at: crate::paths::now_ms(),
            outcome: outcome.to_string(),
            reason,
        })
        .await;
}

/// The stage at which a plugin's connector failed during sync — used only to
/// pick a generic fallback message for [`safe_attach_reason`]. Moved here
/// (from the deleted `ControlPlane::attach_plugin_mcp_servers`, which used to
/// own this same sanitizer) since sync is now the only place a connector's
/// attach failure gets persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachStage {
    /// `Connector::ensure_auth` errored — the one stage whose error text can
    /// carry a raw token-endpoint response body (HTTP-OAuth refresh path).
    Auth,
    /// `Connector::mcp_servers` errored while resolving specs.
    McpServers,
}

/// Map a connector-attach failure to a secret-free reason safe to PERSIST
/// into `plugin_attach_status` (which `plugin_doctor` reads back and later
/// surfaces in the UI). The full error still reaches `tracing::warn!` at the
/// call site — only the persisted reason is sanitized here.
///
/// Only the friendly `"configure {id}: ..."` messages that `ensure_auth`
/// (and the HTTP-OAuth `auth_required` path) raise for a missing/expired
/// credential are known to be secret-free: they name a setting key or a help
/// URL, never a value. Those pass through verbatim. Every other error — in
/// particular `refresh_http_oauth_token`'s `"{id} OAuth token refresh failed
/// with HTTP {status}: {detail}"`, where `detail` is the raw token-endpoint
/// response body and the refresh POST carried the real
/// `refresh_token`/`client_secret` — is collapsed to a generic per-stage
/// message so no connector error body is ever written to the DB.
fn safe_attach_reason(id: &str, stage: AttachStage, err: &anyhow::Error) -> String {
    let msg = err.to_string();
    if msg.starts_with(&format!("configure {id}:")) {
        return msg;
    }
    match stage {
        AttachStage::Auth => format!("{id}: authentication failed"),
        AttachStage::McpServers => format!("{id}: could not resolve MCP servers"),
    }
}

/// Delete every `mcp_servers` row `plugin_id` owns — the uninstall
/// counterpart of [`sync_plugin_mcp`]. Thin wrapper over
/// [`crate::mcp::remove_plugin_servers`]; kept here (not called directly at
/// call sites) so every plugin-mcp lifecycle operation has one obvious home.
pub async fn remove_plugin_mcp(store: &Store, plugin_id: &str) -> anyhow::Result<()> {
    mcp::remove_plugin_servers(store, plugin_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::declarative;
    use crate::plugins::host::PluginSource;
    use ryuzi_plugin_sdk::PluginManifest;
    use std::sync::Arc;

    async fn mem_store() -> (Arc<Store>, SettingsStore) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(Store::open(tmp.path()).await.unwrap());
        let settings = SettingsStore::new(store.clone());
        (store, settings)
    }

    /// Build a connector-capable declarative plugin with one `[[mcp]]` entry
    /// per `(name, transport, command)` tuple — every entry is stdio here,
    /// matching every current caller; a `command` value need not actually
    /// exist on disk (probing it is expected to fail and that failure must
    /// not stop the sync — see `sync_creates_a_row_even_when_the_probe_fails`).
    fn plugin_with_mcp(id: &str, defs: &[(&str, &str, &str)]) -> CorePlugin {
        let mut toml = format!("contract = 2\nid = \"{id}\"\nname = \"{id}\"\n\n");
        for (name, transport, command) in defs {
            toml.push_str(&format!(
                "[[mcp]]\nname = \"{name}\"\ntransport = \"{transport}\"\ncommand = \"{command}\"\n\n"
            ));
        }
        let manifest = PluginManifest::from_toml(&toml).expect("valid manifest");
        declarative::declarative_plugin(manifest, PluginSource::Builtin)
            .expect("declarative plugin")
    }

    #[tokio::test]
    async fn sync_upserts_and_resync_preserves_perms() {
        let (store, settings) = mem_store().await;
        let plugin = plugin_with_mcp("linear", &[("main", "stdio", "linear-mcp")]);

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();
        let rows = mcp::list_servers(&store).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "linear-main");
        assert_eq!(rows[0].plugin_id.as_deref(), Some("linear"));

        // Seed a discovered tool + a user-set perm, the way a prior
        // successful probe (through this same sync path, or a manual Apps
        // "Probe" click) would have left behind.
        mcp::replace_tools(
            &store,
            "linear-main",
            vec![("create_issue".into(), "".into())],
        )
        .await
        .unwrap();
        mcp::set_tool_perm(&store, "linear-main", "create_issue", "deny")
            .await
            .unwrap();

        // Re-sync: `linear-mcp` isn't a real binary in this sandbox, so the
        // probe fails again — sync must still upsert the row's other fields
        // without touching `mcp_tools`, so the user's perm survives, and
        // must never duplicate the row.
        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();

        let rows = mcp::list_servers(&store).await.unwrap();
        assert_eq!(rows.len(), 1, "resync must not duplicate the row");
        let tools = mcp::list_tools(&store, "linear-main").await.unwrap();
        assert_eq!(
            tools
                .iter()
                .find(|t| t.name == "create_issue")
                .unwrap()
                .perm,
            "deny"
        );
    }

    #[tokio::test]
    async fn sync_creates_a_row_even_when_the_probe_fails() {
        let (store, settings) = mem_store().await;
        let plugin = plugin_with_mcp(
            "acme",
            &[("main", "stdio", "definitely-not-a-real-binary-xyz")],
        );

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();
        let row = mcp::get_server(&store, "acme-main").await.unwrap().unwrap();
        assert_eq!(row.status, "error");
        assert!(row.status_detail.is_some());
    }

    #[tokio::test]
    async fn sync_marks_http_transport_rows_unchecked_without_probing() {
        let (store, settings) = mem_store().await;
        let toml = r#"
contract = 2
id = "acme-http"
name = "Acme HTTP"

[[mcp]]
name = "svc"
transport = "http"
url = "https://mcp.acme.example.com"
"#;
        let manifest = PluginManifest::from_toml(toml).unwrap();
        let plugin = declarative::declarative_plugin(manifest, PluginSource::Builtin).unwrap();

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();
        let row = mcp::get_server(&store, "acme-http-svc")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "unchecked");
        assert_eq!(row.transport, "http");
        assert_eq!(row.url.as_deref(), Some("https://mcp.acme.example.com"));
    }

    #[tokio::test]
    async fn sync_resolves_setting_and_auth_placeholders_into_the_row() {
        let (store, settings) = mem_store().await;
        store
            .set_setting_raw("plugin.acme-ph.token", "secret-token")
            .await
            .unwrap();
        store
            .set_setting_raw("plugin.acme-ph.host", "acme.example.com")
            .await
            .unwrap();
        let toml = r#"
contract = 2
id = "acme-ph"
name = "Acme"

[auth]
kind = "token"
setting = "plugin.acme-ph.token"

[[mcp]]
name = "main"
transport = "stdio"
command = "acme-mcp"
args = ["--host", "${setting:plugin.acme-ph.host}"]
env = { TOKEN = "${auth}" }
"#;
        let manifest = PluginManifest::from_toml(toml).unwrap();
        let plugin = declarative::declarative_plugin(manifest, PluginSource::Builtin).unwrap();

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();
        let row = mcp::get_server(&store, "acme-ph-main")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.args,
            vec!["--host".to_string(), "acme.example.com".to_string()]
        );
        assert_eq!(
            row.env,
            vec![("TOKEN".to_string(), "secret-token".to_string())]
        );
        assert_eq!(row.auth_kind, "env");
        assert_eq!(row.auth_detail.as_deref(), Some("TOKEN"));
    }

    #[tokio::test]
    async fn sync_skips_silently_when_a_placeholder_cannot_resolve() {
        let (store, settings) = mem_store().await;
        // `${auth}` has nothing to resolve from: no `[auth]` value is set
        // for `plugin.broken.token`, so `connector.mcp_servers()` errors on
        // the unresolved placeholder.
        let toml = r#"
contract = 2
id = "broken"
name = "Broken"

[auth]
kind = "token"
setting = "plugin.broken.token"

[[mcp]]
name = "main"
transport = "stdio"
command = "broken-mcp"
env = { TOKEN = "${auth}" }
"#;
        let manifest = PluginManifest::from_toml(toml).unwrap();
        let plugin = declarative::declarative_plugin(manifest, PluginSource::Builtin).unwrap();

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();
        assert!(mcp::list_servers(&store).await.unwrap().is_empty());

        let status = store
            .get_plugin_attach("broken")
            .await
            .unwrap()
            .expect("a failed attach must still be recorded for plugin_doctor");
        assert_eq!(status.outcome, "failed");
    }

    #[test]
    fn safe_attach_reason_passes_through_the_friendly_configure_message_verbatim() {
        let err = anyhow::anyhow!("configure acme: see https://acme.test/help");
        assert_eq!(
            safe_attach_reason("acme", AttachStage::Auth, &err),
            "configure acme: see https://acme.test/help"
        );
    }

    #[test]
    fn safe_attach_reason_never_lets_an_oauth_refresh_body_reach_the_persisted_reason() {
        // Simulate the raw HTTP-OAuth token-refresh error whose `detail` is
        // an untruncated response body echoing the refresh POST's form
        // fields — the exact leak the sanitizer must stop.
        let err = anyhow::anyhow!(
            "acme OAuth token refresh failed with HTTP 400: \
             {{\"echo\":\"refresh_token=leaked-secret-token&client_secret=leaked-client-secret\"}}"
        );
        let reason = safe_attach_reason("acme", AttachStage::Auth, &err);
        assert_eq!(reason, "acme: authentication failed");
        assert!(!reason.contains("leaked-secret-token"));
        assert!(!reason.contains("leaked-client-secret"));
        assert!(!reason.contains("refresh_token"));
    }

    #[test]
    fn safe_attach_reason_mcp_stage_error_is_generic_and_drops_raw_text() {
        let err = anyhow::anyhow!("some internal detail with a token=abc123 in it");
        let mcp = safe_attach_reason("acme", AttachStage::McpServers, &err);
        assert!(!mcp.contains("abc123"));
        assert!(mcp.starts_with("acme:"));
    }

    /// Build the same shape [`plugin_with_mcp`] does, but with an
    /// `Installed { .. }` source instead of `Builtin` — the shape
    /// `install_sources::confirm_plugin_install` actually passes.
    fn installed_plugin_with_mcp(
        id: &str,
        defs: &[(&str, &str, &str)],
        provenance: crate::plugins::host::InstallProvenance,
    ) -> CorePlugin {
        let mut toml = format!("contract = 2\nid = \"{id}\"\nname = \"{id}\"\n\n");
        for (name, transport, command) in defs {
            toml.push_str(&format!(
                "[[mcp]]\nname = \"{name}\"\ntransport = \"{transport}\"\ncommand = \"{command}\"\n\n"
            ));
        }
        let manifest = PluginManifest::from_toml(&toml).expect("valid manifest");
        declarative::declarative_plugin(
            manifest,
            PluginSource::Installed {
                dir: std::path::PathBuf::from("/tmp/does-not-matter"),
                provenance,
            },
        )
        .expect("declarative plugin")
    }

    // ---------- Task 11: tiered trust gate ----------

    #[tokio::test]
    async fn untrusted_unsigned_plugin_syncs_no_mcp_rows() {
        let (store, settings) = mem_store().await;
        let plugin = installed_plugin_with_mcp(
            "acme-untrusted",
            &[("main", "stdio", "acme-mcp")],
            crate::plugins::host::InstallProvenance::LocalPath,
        );

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();
        assert!(
            mcp::list_servers(&store).await.unwrap().is_empty(),
            "an unsigned, untrusted plugin must sync no mcp rows"
        );
    }

    #[tokio::test]
    async fn trusted_unsigned_plugin_syncs_its_mcp_rows() {
        let (store, settings) = mem_store().await;
        let plugin = installed_plugin_with_mcp(
            "acme-trusted",
            &[("main", "stdio", "acme-mcp")],
            crate::plugins::host::InstallProvenance::LocalPath,
        );
        store
            .set_setting_raw("plugin.acme-trusted.trusted", "true")
            .await
            .unwrap();

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();
        let rows = mcp::list_servers(&store).await.unwrap();
        assert_eq!(rows.len(), 1, "a trusted plugin's mcp entry must sync");
        assert_eq!(rows[0].plugin_id.as_deref(), Some("acme-trusted"));
    }

    #[tokio::test]
    async fn a_git_url_install_needs_trust_the_same_as_a_local_path_install() {
        let (store, settings) = mem_store().await;
        let plugin = installed_plugin_with_mcp(
            "acme-git",
            &[("main", "stdio", "acme-mcp")],
            crate::plugins::host::InstallProvenance::GitUrl("https://example.com/acme.git".into()),
        );

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();
        assert!(mcp::list_servers(&store).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn catalog_provenance_is_trusted_by_construction() {
        let (store, settings) = mem_store().await;
        let plugin = installed_plugin_with_mcp(
            "acme-catalog",
            &[("main", "stdio", "acme-mcp")],
            crate::plugins::host::InstallProvenance::Catalog,
        );

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();
        assert_eq!(
            mcp::list_servers(&store).await.unwrap().len(),
            1,
            "a signed-catalog install must sync without any trust setting"
        );
    }

    // F3: a plugin update whose manifest drops a previously-synced `[[mcp]]`
    // entry must prune that server row (and its tools) on re-sync, while
    // leaving entries the manifest still declares — and another plugin's
    // rows — untouched.
    #[tokio::test]
    async fn resync_prunes_a_server_the_new_manifest_no_longer_declares() {
        let (store, settings) = mem_store().await;
        let other = plugin_with_mcp("other", &[("main", "stdio", "other-mcp")]);
        sync_plugin_mcp(&store, &settings, &other).await.unwrap();

        let v1 = plugin_with_mcp(
            "acme",
            &[
                ("keep", "stdio", "acme-keep"),
                ("drop", "stdio", "acme-drop"),
            ],
        );
        sync_plugin_mcp(&store, &settings, &v1).await.unwrap();
        assert_eq!(mcp::list_servers(&store).await.unwrap().len(), 3);
        mcp::replace_tools(&store, "acme-drop", vec![("some_tool".into(), "".into())])
            .await
            .unwrap();

        // v2 drops the "drop" entry, keeps "keep".
        let v2 = plugin_with_mcp("acme", &[("keep", "stdio", "acme-keep")]);
        sync_plugin_mcp(&store, &settings, &v2).await.unwrap();

        let rows = mcp::list_servers(&store).await.unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert!(
            !ids.contains(&"acme-drop"),
            "the dropped mcp entry must be pruned, got: {ids:?}"
        );
        assert!(
            ids.contains(&"acme-keep"),
            "an entry the new manifest still declares must survive"
        );
        assert!(
            ids.contains(&"other-main"),
            "another plugin's row must never be pruned"
        );
        assert!(
            mcp::list_tools(&store, "acme-drop")
                .await
                .unwrap()
                .is_empty(),
            "the pruned server's tools must be gone too"
        );
    }

    // F3: a manifest update that drops EVERY `[[mcp]]` entry must prune all
    // of that plugin's server rows, even though the early-return path never
    // reaches the connector/auth machinery.
    #[tokio::test]
    async fn resync_with_no_mcp_entries_prunes_every_row_for_that_plugin() {
        let (store, settings) = mem_store().await;
        let v1 = plugin_with_mcp("acme", &[("main", "stdio", "acme-mcp")]);
        sync_plugin_mcp(&store, &settings, &v1).await.unwrap();
        assert_eq!(mcp::list_servers(&store).await.unwrap().len(), 1);

        let toml = "contract = 2\nid = \"acme\"\nname = \"acme\"\n";
        let manifest = PluginManifest::from_toml(toml).unwrap();
        let v2 = declarative::declarative_plugin(manifest, PluginSource::Builtin).unwrap();
        sync_plugin_mcp(&store, &settings, &v2).await.unwrap();

        assert!(mcp::list_servers(&store).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn remove_plugin_mcp_deletes_only_that_plugins_rows() {
        let (store, settings) = mem_store().await;
        let a = plugin_with_mcp("acme-a", &[("main", "stdio", "acme-a-mcp")]);
        let b = plugin_with_mcp("acme-b", &[("main", "stdio", "acme-b-mcp")]);
        sync_plugin_mcp(&store, &settings, &a).await.unwrap();
        sync_plugin_mcp(&store, &settings, &b).await.unwrap();
        assert_eq!(mcp::list_servers(&store).await.unwrap().len(), 2);

        remove_plugin_mcp(&store, "acme-a").await.unwrap();

        let rows = mcp::list_servers(&store).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "acme-b-main");
    }
}
