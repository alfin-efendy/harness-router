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
//! the row is fully usable once a user clicks it. The connector's resolved
//! STATIC headers (a manifest `Authorization` from a `${setting:}`/`${env:}`
//! value, if any) ARE persisted regardless — `mcp::set_server_headers`,
//! called right after the row upsert below — so
//! `crate::mcp::servers_for_session` can hand them to a native HTTP session
//! even though this sync path never itself opens an HTTP connection (Task 13;
//! before it, the header was resolved here and then silently dropped, since
//! `servers_for_session` hardcoded an empty header list). A header whose
//! value is a plugin's live OAuth BEARER is the one thing deliberately not
//! persisted — it is re-resolved per session instead; see
//! [`persistable_headers`].

use crate::connector::ConnectorCtx;
use crate::domain::McpTransport;
use crate::mcp::{self, McpServerRow};
use crate::plugins::host::CorePlugin;
use crate::settings::SettingsStore;
use crate::store::{PluginAttachStatus, Store};
use ryuzi_plugin_sdk::AuthKind;

/// Upsert one Apps row per `plugin.manifest.mcp` entry (resolving every
/// `${auth}`/`${setting:KEY}`/`${env:VAR}` placeholder through the SAME
/// connector + resolver `declarative_plugin` already builds — reused via the
/// `Connector` trait rather than reimplemented), probe stdio rows for real,
/// and refresh their tool lists (preserving any user-set per-tool perm — see
/// module doc). Never errors on a not-yet-configured or unreachable plugin:
///
/// - a manifest with no `[[mcp]]` entries — silent no-op (after pruning any
///   row a prior manifest declared);
/// - a plugin declaring `[[mcp]]` but registered with NO connector — a
///   registration bug, LOUDLY logged and skipped, see the body;
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
    // A plugin that declares `[[mcp]]` but was registered with no connector
    // cannot sync: this function needs `Connector::ensure_auth` +
    // `Connector::mcp_servers` to resolve the manifest's placeholders, and
    // there is no way to build one HERE — the registered manifest's
    // `[[settings]]` keys are already QUALIFIED (`plugin.<id>.<key>`), which
    // `PluginManifest::validate` — and so `declarative_plugin` — rejects
    // outright, and which `DeclarativeConnector` would then re-qualify. The
    // connector must therefore be built at the REGISTRATION boundary, off the
    // still-bare manifest (`plugins::declarative_connector_for`, used by both
    // `component_catalog::component_catalog_plugins` and
    // `install_installed_plugins`) — so reaching this branch means a
    // registration site skipped that and this plugin's Apps row will never
    // exist. It used to be a SILENT `return`, which is exactly how
    // `atlassian-rovo` shipped inert; warn loudly instead.
    let Some(connector) = &plugin.connector else {
        tracing::warn!(
            plugin = %id,
            mcp_entries = plugin.manifest.mcp.len(),
            "mcp sync: skipping — this plugin declares [[mcp]] but was registered without a \
             connector, so no Apps row can be created for it (registration bug: see \
             plugins::declarative_connector_for)"
        );
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
        let (transport, command, args, env, url, headers) = match &spec.transport {
            McpTransport::Stdio { command, args, env } => (
                "stdio",
                Some(command.clone()),
                args.clone(),
                env.clone(),
                None,
                Vec::new(),
            ),
            // Task 13: carry the connector's resolved headers (e.g. a
            // manifest `Authorization`) through to the row so
            // `servers_for_session` can read them back instead of the
            // `vec![]` it used to hardcode — see `mcp::set_server_headers`'s
            // doc for the encryption-at-rest discipline, and
            // [`persistable_headers`] for the one header kind that is
            // deliberately NOT persisted.
            McpTransport::Http { url, headers } => (
                "http",
                None,
                Vec::new(),
                Vec::new(),
                Some(url.clone()),
                persistable_headers(plugin, headers),
            ),
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
        // Written on EVERY sync (even an empty slice), same as
        // `upsert_server` above — a header a manifest update drops must not
        // linger from a prior sync (Task 13).
        mcp::set_server_headers(store, &row_id, &headers).await?;

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

/// The subset of a connector's resolved HTTP headers that may be PERSISTED
/// onto the `mcp_servers` row.
///
/// Everything a manifest resolves statically — a `${setting:}` API token, a
/// `${env:}` value, a plain constant — is persisted: it is exactly as durable
/// as the row itself, and re-resolving it would need the connector this
/// function's callers only have at sync time.
///
/// An `Authorization` header on an `[auth] kind = "oauth"` plugin is the one
/// exception, and is dropped. `plugins::declarative`'s `build_spec` sets (or
/// OVERWRITES) that header with the plugin's live OAuth bearer, so its value
/// is a short-lived access token, and persisting it is wrong three ways:
///
/// - `sync_plugin_mcp` runs on plugin enable, install, and OAuth completion
///   only — never at session start — so a session could mint its transport
///   from a bearer captured days earlier;
/// - a persisted `Authorization` reads as manifest auth to
///   `harness::native`'s `connect_mcp_tools`, which then wires no `Store`, so
///   the transport deliberately refuses to refresh it on a 401 and the
///   plugin's own refresh path is never reached — an expired snapshot becomes
///   an unrecoverable hard failure until someone toggles the plugin;
/// - `plugins_api::disconnect_plugin_oauth` deletes `plugin_oauth_tokens` and
///   nothing else, so a user who disconnected their account would keep
///   sending the stale bearer from every new session — a local revocation
///   bypass.
///
/// Dropping it makes all three structurally impossible rather than patching
/// the third in a disconnect handler; `mcp::servers_for_session` re-resolves
/// the bearer from `plugin_oauth_tokens` at session start instead (see
/// `mcp::with_live_plugin_oauth_bearer`). Every other header on such a plugin
/// (`Accept`, a tenant id, …) is still persisted.
fn persistable_headers(plugin: &CorePlugin, headers: &[(String, String)]) -> Vec<(String, String)> {
    let http_oauth = plugin
        .manifest
        .auth
        .as_ref()
        .is_some_and(|auth| auth.kind == AuthKind::Oauth);
    if !http_oauth {
        return headers.to_vec();
    }
    headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
        .cloned()
        .collect()
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
        assert_eq!(
            mcp::get_server_headers(&store, "acme-http-svc")
                .await
                .unwrap(),
            Vec::<(String, String)>::new(),
            "an entry with no [[mcp]] headers must persist an empty header list, not error"
        );
    }

    /// PROPERTY (Task 13, sync-side half of the gap): a manifest's resolved
    /// `Authorization` header — proven to resolve correctly at the connector
    /// layer since Task 11 — must be PERSISTED onto the row, not just
    /// computed and thrown away. Before this task nothing in this function
    /// wrote the connector's resolved headers anywhere, so a plugin like
    /// `atlassian-rovo` would sync a working row with no way for
    /// `mcp::servers_for_session` (and, downstream, Task 8's auth-precedence
    /// rule) to ever see the credential it resolved.
    #[tokio::test]
    async fn sync_persists_the_connectors_resolved_authorization_header() {
        crate::llm_router::secrets::use_test_key_file();
        let (store, settings) = mem_store().await;
        store
            .set_setting_raw("plugin.acme-rovo.basic_credential", "dXNlcjpwYXNz")
            .await
            .unwrap();
        let toml = r#"
contract = 2
id = "acme-rovo"
name = "Acme Rovo"

[[settings]]
key = "basic_credential"
label = "Basic auth credential"
secret = true

[[mcp]]
name = "svc"
transport = "http"
url = "https://mcp.acme.example.com/v1/mcp"
headers = { Authorization = "Basic ${setting:plugin.acme-rovo.basic_credential}" }
"#;
        let manifest = PluginManifest::from_toml(toml).unwrap();
        let plugin = declarative::declarative_plugin(manifest, PluginSource::Builtin).unwrap();

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();

        let headers = mcp::get_server_headers(&store, "acme-rovo-svc")
            .await
            .unwrap();
        assert_eq!(
            headers,
            vec![(
                "Authorization".to_string(),
                "Basic dXNlcjpwYXNz".to_string()
            )],
            "the connector's resolved Authorization header must be persisted onto the row, \
             got {headers:?}"
        );
    }

    /// PROPERTY: an `[auth] kind = "oauth"` plugin's resolved
    /// `Authorization: Bearer …` — a short-lived access token — is NOT written
    /// to `headers_json`, while its static headers still are.
    ///
    /// Non-vacuity is proven inside the test itself: it drives the very same
    /// connector `sync_plugin_mcp` uses and asserts that connector DOES resolve
    /// an `Authorization` header, so the header's absence from the row can only
    /// mean [`persistable_headers`] dropped it — not that resolution failed, or
    /// that the sync bailed before writing anything (the row and the `ok`
    /// attach outcome are asserted too).
    ///
    /// Snapshotting it instead would make the bearer outlive the token: sync
    /// runs on enable/install/OAuth-completion only, `connect_mcp_tools` treats
    /// a persisted `Authorization` as unrefreshable manifest auth, and
    /// `disconnect_plugin_oauth` clears `plugin_oauth_tokens` without touching
    /// this column — see [`persistable_headers`] for the full argument.
    #[tokio::test]
    async fn an_oauth_plugins_live_bearer_is_never_persisted_onto_the_row() {
        crate::llm_router::secrets::use_test_key_file();
        let (store, settings) = mem_store().await;
        store
            .upsert_plugin_oauth_token(&crate::plugins::oauth::PluginOauthToken {
                plugin_id: "acme-oauth".into(),
                access_token: "live-access-token".into(),
                refresh_token: Some("refresh".into()),
                token_type: "Bearer".into(),
                // Outside `oauth::needs_refresh`'s window, so resolution
                // returns the stored token instead of attempting a network
                // refresh.
                expires_at: Some(crate::paths::now_ms() + 3_600_000),
                scopes: vec![],
                reconnect_required: false,
            })
            .await
            .unwrap();
        let toml = r#"
contract = 2
id = "acme-oauth"
name = "Acme OAuth"

[auth]
kind = "oauth"

[[mcp]]
name = "svc"
transport = "http"
url = "https://mcp.acme.example.com/v1/mcp"
headers = { Accept = "application/json" }
"#;
        let manifest = PluginManifest::from_toml(toml).unwrap();
        let plugin = declarative::declarative_plugin(manifest, PluginSource::Builtin).unwrap();

        // Positive control: the connector this sync uses really does inject a
        // bearer into the spec it resolves.
        let resolved = crate::connector::Connector::mcp_servers(
            plugin.connector.as_deref().expect("connector"),
            &ConnectorCtx {
                project_id: "acme-oauth".into(),
                work_dir: std::env::temp_dir(),
                settings: settings.clone(),
            },
        )
        .await
        .unwrap();
        let McpTransport::Http { headers, .. } = &resolved[0].transport else {
            panic!("expected an http spec, got {:?}", resolved[0].transport);
        };
        assert!(
            headers.contains(&(
                "Authorization".to_string(),
                "Bearer live-access-token".into()
            )),
            "precondition: the connector must resolve a bearer for this manifest, got {headers:?}"
        );

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();

        assert!(
            mcp::get_server(&store, "acme-oauth-svc")
                .await
                .unwrap()
                .is_some(),
            "the sync must have written the row (otherwise the header's absence proves nothing)"
        );
        assert_eq!(
            store
                .get_plugin_attach("acme-oauth")
                .await
                .unwrap()
                .expect("attach outcome recorded")
                .outcome,
            "ok"
        );
        let persisted = mcp::get_server_headers(&store, "acme-oauth-svc")
            .await
            .unwrap();
        assert!(
            !persisted
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("authorization")),
            "a live OAuth bearer must never be persisted onto the row, got {persisted:?}"
        );
        assert_eq!(
            persisted,
            vec![("Accept".to_string(), "application/json".to_string())],
            "an oauth plugin's STATIC headers must still be persisted, got {persisted:?}"
        );
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

    // ---------- declarative-only ("component-less") first-party bundles ----

    /// PROPERTY (the merge blocker): the REAL, embedded `atlassian-rovo`
    /// registration — a first-party bundle with no `[component]` at all — must
    /// sync its remote MCP row, with the manifest's `${setting:...}` Basic
    /// credential resolved onto it.
    ///
    /// Drives the actual registered `CorePlugin` from
    /// `component_catalog::component_catalog_plugins()` rather than a synthetic
    /// manifest, so it fails if the bundle is not registered at all (the
    /// original defect: it was absent from `COMPONENT_BUNDLE_MANIFESTS`), if it
    /// is registered manifest-only (this function early-returns on
    /// `connector.is_none()`, writing nothing), or if the settings-key
    /// qualification happens before the connector is built (the connector would
    /// then hunt for `plugin.atlassian-rovo.plugin.atlassian-rovo.basic_credential`,
    /// `ensure_auth` would fail "missing required setting", and no row would be
    /// written either).
    ///
    /// Hermetic: the entry is HTTP transport, so sync marks it `unchecked` and
    /// never opens a connection to `mcp.atlassian.com`.
    #[tokio::test]
    async fn the_embedded_atlassian_rovo_bundle_syncs_its_remote_mcp_row() {
        crate::llm_router::secrets::use_test_key_file();
        let (store, settings) = mem_store().await;
        store
            .set_setting_raw("plugin.atlassian-rovo.basic_credential", "dXNlcjpwYXNz")
            .await
            .unwrap();

        let plugins = crate::plugins::component_catalog::component_catalog_plugins();
        let plugin = plugins
            .iter()
            .find(|p| p.manifest.id == "atlassian-rovo")
            .expect("atlassian-rovo must be registered in the component catalog");

        sync_plugin_mcp(&store, &settings, plugin).await.unwrap();

        let row = mcp::get_server(&store, "atlassian-rovo-atlassian-rovo")
            .await
            .unwrap()
            .expect("a signed first-party declarative bundle must get an Apps row");
        assert_eq!(row.plugin_id.as_deref(), Some("atlassian-rovo"));
        assert_eq!(row.transport, "http");
        assert_eq!(row.url.as_deref(), Some("https://mcp.atlassian.com/v1/mcp"));
        assert_eq!(row.status, "unchecked", "an http row is never probed here");
        assert_eq!(
            mcp::get_server_headers(&store, "atlassian-rovo-atlassian-rovo")
                .await
                .unwrap(),
            vec![(
                "Authorization".to_string(),
                "Basic dXNlcjpwYXNz".to_string()
            )],
            "the manifest's ${{setting:...}} Basic credential must resolve onto the row"
        );
        assert_eq!(
            store
                .get_plugin_attach("atlassian-rovo")
                .await
                .unwrap()
                .expect("attach outcome recorded")
                .outcome,
            "ok"
        );
    }

    /// PROPERTY: a SIGNED install creates the MCP row. A signed-catalog install
    /// never runs `install_sources::confirm_plugin_install`'s transient
    /// post-install syncs (that is the local-folder/git-URL path), so
    /// `plugins::install_installed_plugins`' boot scan is the only registration
    /// it ever gets. This drives that exact registration into `sync_plugin_mcp`
    /// — the same object `toggle_enabled` hands it on enable — and asserts a row
    /// appears. Before this fix the scan registered `connector: None` and this
    /// function returned early, silently.
    #[tokio::test]
    async fn a_signed_installed_declarative_plugin_syncs_from_its_boot_scan_registration() {
        crate::llm_router::secrets::use_test_key_file();
        let (store, settings) = mem_store().await;
        store
            .set_setting_raw("plugin.acme-signed.basic_credential", "c2lnbmVk")
            .await
            .unwrap();

        // The on-disk layout a verified `ComponentBundleInstaller` install
        // leaves behind: `<root>/<id>/<version>/ryuzi-plugin.toml` + a `current`
        // pointer, and NO `install.json` — so `read_install_provenance`
        // defaults to `Catalog`, i.e. trusted by construction.
        let root = tempfile::tempdir().unwrap();
        let version_dir = root.path().join("acme-signed").join("0.1.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(
            version_dir.join("ryuzi-plugin.toml"),
            r#"contract = 2
id = "acme-signed"
name = "Acme Signed"

[[settings]]
key = "basic_credential"
label = "Credential"
secret = true
required = true

[[mcp]]
name = "svc"
transport = "http"
url = "https://mcp.acme.example.com/v1/mcp"
headers = { Authorization = "Basic ${setting:plugin.acme-signed.basic_credential}" }
"#,
        )
        .unwrap();
        std::fs::write(root.path().join("acme-signed").join("current"), "0.1.0").unwrap();

        let mut regs = crate::plugins::Registries::new();
        crate::plugins::install_installed_plugins(&mut regs, root.path());
        let plugin = regs
            .plugins
            .get("acme-signed")
            .expect("the boot scan must register the installed plugin");

        sync_plugin_mcp(&store, &settings, &plugin).await.unwrap();

        let row = mcp::get_server(&store, "acme-signed-svc")
            .await
            .unwrap()
            .expect("a signed install must create its mcp_servers row");
        assert_eq!(row.plugin_id.as_deref(), Some("acme-signed"));
        assert_eq!(
            mcp::get_server_headers(&store, "acme-signed-svc")
                .await
                .unwrap(),
            vec![("Authorization".to_string(), "Basic c2lnbmVk".to_string())]
        );
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
