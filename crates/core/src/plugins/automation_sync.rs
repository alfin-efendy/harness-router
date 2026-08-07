//! Sync a plugin manifest's `[[hooks]]` and `[[jobs]]` entries into the
//! Automation and Scheduler domains (`crate::automation`'s `automation_hooks`
//! table and `crate::scheduler`'s `jobs` table). A synced row is a
//! first-class row in each domain — same tables, same run history, same
//! toggle/edit surfaces a user-created hook/job gets — attributed back to
//! its plugin via the `plugin_id` origin column (Task 5's migration).
//!
//! # Call sites
//! - Plugin install/enable and plugin update (a fresh release) call
//!   [`sync_plugin_automations`].
//! - Uninstall calls [`remove_plugin_automations`].
//!
//! # Row identity
//! - Hook rows are named `"{plugin_id}/{def.name}"` — `name` is UNIQUE
//!   NOCASE, so this both fits the constraint and is collision-proof against
//!   a user-created hook. The row's `id` (its real primary key) is
//!   independent of the name: minted fresh on first sync, then looked up by
//!   name ([`automation::find_hook_by_name`]) and reused on every re-sync,
//!   so a hook's run history (keyed by `hook_id`) never gets orphaned by an
//!   update.
//! - Job rows use `id = "{plugin_id}/{def.name}"` directly — jobs' `id` IS
//!   the natural key `scheduler::upsert_job` already upserts against.
//!
//! # Enablement + re-sync (the subtle part)
//! A plugin cannot know the user's project. On the FIRST sync:
//! - `webhook.outbound` hooks may ship enabled (the manifest's own action
//!   choice — nothing about delivering to a URL needs a project).
//! - `agent.run` hooks and EVERY job install DISABLED, with an empty
//!   `project_id`/`branch`/`gateway_id` target: nothing about a fresh
//!   install can guess which project the user wants this to run against.
//!
//! On a RE-sync (a plugin update), the user's choices must survive:
//! - `enabled` is always preserved from the stored row, whatever the plugin
//!   ships this time.
//! - An `agent.run` hook's `project_id`/`branch`/`gateway_id`/`agent_id`
//!   keep their STORED values whenever non-empty (a target the user filled
//!   in survives a re-sync). A still-empty stored value takes whatever the
//!   fresh sync computed (ordinarily also empty, since the manifest itself
//!   never declares a target).
//! - Everything else — `trigger_kind`, the prompt-bearing config fields
//!   (`prompt`, `model_override`, `subtask` for `agent.run`;
//!   `WebhookOutboundAction`'s whole config) — is OVERWRITTEN from the
//!   manifest every sync: that is the plugin's own declared behavior, not
//!   something a user customizes on the row.
//! - Jobs get symmetric treatment: `enabled` and
//!   `project_id`/`branch`/`gateway` are preserved from the stored row
//!   whenever non-empty; `cron`/`mode`/`natural_text`/`prompt`/
//!   `model_override` always refresh from the manifest.
//! - `notify_success`/`notify_fail`/`pre_check` are pure user preferences a
//!   manifest never declares — always carried over from the stored row
//!   (defaulted only when the row is brand new).
//!
//! # Errors are per-row, never fatal
//! An unknown trigger spelling (shouldn't happen — manifest `validate()`
//! already rejects it, but canonicalized defensively anyway), a hook config
//! that fails its `HookActionInput` variant's `deny_unknown_fields` schema,
//! or a job schedule that is neither valid natural-language nor valid cron
//! is recorded into the returned [`SyncReport`] and the loop continues — one
//! malformed row in a manifest must never block every other hook/job from
//! syncing. Only a genuine store I/O failure surfaces as `Err`.

use std::str::FromStr;

use anyhow::Context;

use crate::automation::{self, ActionKind, HookActionInput, HookRow, TriggerKind};
use crate::paths::{new_id, now_ms};
use crate::plugins::host::CorePlugin;
use crate::scheduler::{self, JobRow};
use crate::store::Store;

/// Outcome of one [`sync_plugin_automations`] call: how many hook/job rows
/// were written, and the per-row errors (if any) that were skipped rather
/// than aborting the whole sync.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub hooks_synced: usize,
    pub jobs_synced: usize,
    pub errors: Vec<String>,
}

impl SyncReport {
    fn error(&mut self, message: impl Into<String>) {
        let message = message.into();
        tracing::warn!("plugin automation sync: {message}");
        self.errors.push(message);
    }
}

/// Sync every `[[hooks]]`/`[[jobs]]` entry `plugin.manifest` declares into
/// their respective domain tables. See the module doc for the full
/// first-sync-vs-re-sync field matrix and the per-row error discipline.
pub async fn sync_plugin_automations(
    store: &Store,
    plugin: &CorePlugin,
) -> anyhow::Result<SyncReport> {
    let plugin_id = plugin.manifest.id.as_str();
    let mut report = SyncReport::default();

    for def in &plugin.manifest.hooks {
        let synced = sync_one_hook(store, plugin_id, def, &mut report).await?;
        if synced {
            report.hooks_synced += 1;
        }
    }

    for def in &plugin.manifest.jobs {
        let synced = sync_one_job(store, plugin_id, def, &mut report).await?;
        if synced {
            report.jobs_synced += 1;
        }
    }

    Ok(report)
}

/// Delete every hook and job `plugin_id` owns, plus their run history — the
/// uninstall counterpart of [`sync_plugin_automations`]. A no-op for a
/// plugin that never synced any row.
pub async fn remove_plugin_automations(store: &Store, plugin_id: &str) -> anyhow::Result<()> {
    automation::delete_hooks_and_runs_for_plugin(store, plugin_id).await?;
    scheduler::delete_jobs_and_runs_for_plugin(store, plugin_id).await?;
    Ok(())
}

/// Build the wire-shape `HookActionInput` a manifest's `action` (a
/// `KNOWN_HOOK_ACTIONS` string, e.g. `"agent.run"`) + `config` (a free-form
/// TOML table, unvalidated by the SDK) describes. `HookActionInput` is
/// adjacently tagged (`{"kind": ..., "config": ...}`) with `deny_unknown_fields`
/// on every variant, so this is exactly that JSON shape assembled from the
/// manifest's two separate fields, then deserialized normally — any missing
/// or extra config key surfaces as an ordinary deserialization error.
fn build_action(action: &str, config: &toml::Value) -> anyhow::Result<HookActionInput> {
    let config_json =
        serde_json::to_value(config).context("hook config could not be converted to JSON")?;
    let wire = serde_json::json!({ "kind": action, "config": config_json });
    serde_json::from_value(wire).context("hook config does not match its action's schema")
}

/// Preserve a user-filled `agent.run` target across a re-sync: any of
/// `project_id`/`branch`/`gateway_id`/`agent_id` the STORED row has non-empty
/// wins over whatever the fresh manifest-derived action carries (ordinarily
/// also empty, since a manifest cannot declare a target). A no-op for
/// `webhook.outbound` (nothing to preserve) or when either side isn't
/// `agent.run` (mismatched action kind between syncs — the fresh one wins
/// outright, same as any other overwritten field).
fn preserve_agent_run_target(
    mut fresh: HookActionInput,
    existing: &HookActionInput,
) -> HookActionInput {
    if let (HookActionInput::AgentRun(fresh_cfg), HookActionInput::AgentRun(existing_cfg)) =
        (&mut fresh, existing)
    {
        if !existing_cfg.project_id.trim().is_empty() {
            fresh_cfg.project_id.clone_from(&existing_cfg.project_id);
        }
        if !existing_cfg.branch.trim().is_empty() {
            fresh_cfg.branch.clone_from(&existing_cfg.branch);
        }
        if !existing_cfg.gateway_id.trim().is_empty() {
            fresh_cfg.gateway_id.clone_from(&existing_cfg.gateway_id);
        }
        if let Some(agent_id) = existing_cfg
            .agent_id
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            fresh_cfg.agent_id = Some(agent_id.clone());
        }
    }
    fresh
}

/// Sync one `[[hooks]]` entry. Returns `Ok(true)` when a row was written,
/// `Ok(false)` when the entry was skipped as a recorded per-hook error (see
/// the module doc), and `Err` only for a genuine store failure.
async fn sync_one_hook(
    store: &Store,
    plugin_id: &str,
    def: &ryuzi_plugin_sdk::HookDef,
    report: &mut SyncReport,
) -> anyhow::Result<bool> {
    let name = format!("{plugin_id}/{}", def.name);

    let Some(canonical) = ryuzi_plugin_sdk::canonical_trigger(&def.trigger) else {
        report.error(format!("{name}: unknown trigger {:?}", def.trigger));
        return Ok(false);
    };
    let trigger_kind = match TriggerKind::from_str(canonical) {
        Ok(kind) => kind,
        Err(error) => {
            report.error(format!("{name}: {error}"));
            return Ok(false);
        }
    };

    let action = match build_action(&def.action, &def.config) {
        Ok(action) => action,
        Err(error) => {
            report.error(format!("{name}: {error}"));
            return Ok(false);
        }
    };

    if trigger_kind == TriggerKind::WebhookInbound && action.kind() != ActionKind::AgentRun {
        report.error(format!(
            "{name}: webhook.inbound hooks only support agent.run"
        ));
        return Ok(false);
    }

    let existing = automation::find_hook_by_name(store, &name).await?;
    let now = now_ms();
    let (id, enabled, action, created_at) = match &existing {
        Some(existing) => {
            let action = preserve_agent_run_target(action, &existing.action);
            (
                existing.id.clone(),
                existing.enabled,
                action,
                existing.created_at,
            )
        }
        None => {
            // First sync: only webhook.outbound may ship enabled — agent.run
            // has no project to run against yet (see module doc).
            let enabled = action.kind() == ActionKind::WebhookOutbound;
            (new_id(), enabled, action, now)
        }
    };

    let row = HookRow {
        id,
        name,
        trigger_kind,
        action_kind: action.kind(),
        enabled,
        // Managed by `put_hook_row` itself (COALESCE against any existing
        // path for a webhook.inbound row); this value is never read for the
        // write.
        inbound_path: None,
        action,
        created_at,
        updated_at: now,
        plugin_id: Some(plugin_id.to_string()),
    };
    automation::put_hook_row(store, row).await?;
    Ok(true)
}

/// Sync one `[[jobs]]` entry. Returns `Ok(true)` when a row was written,
/// `Ok(false)` when the entry was skipped as a recorded per-job error, and
/// `Err` only for a genuine store failure.
async fn sync_one_job(
    store: &Store,
    plugin_id: &str,
    def: &ryuzi_plugin_sdk::JobDef,
    report: &mut SyncReport,
) -> anyhow::Result<bool> {
    let id = format!("{plugin_id}/{}", def.name);

    let (mode, cron, natural_text) = match scheduler::natural_to_cron(&def.schedule) {
        Some(cron) => ("natural".to_string(), cron, def.schedule.clone()),
        None => {
            if scheduler::next_run_after(&def.schedule, now_ms()).is_none() {
                report.error(format!("{id}: invalid schedule {:?}", def.schedule));
                return Ok(false);
            }
            ("cron".to_string(), def.schedule.clone(), String::new())
        }
    };

    let existing = scheduler::get_job(store, &id).await?;
    let (enabled, project_id, branch, gateway, notify_success, notify_fail, pre_check) =
        match &existing {
            Some(existing) => (
                existing.enabled,
                existing.project_id.clone(),
                if existing.branch.trim().is_empty() {
                    "main".to_string()
                } else {
                    existing.branch.clone()
                },
                if existing.gateway.trim().is_empty() {
                    "local".to_string()
                } else {
                    existing.gateway.clone()
                },
                existing.notify_success,
                existing.notify_fail,
                existing.pre_check.clone(),
            ),
            // First sync: no target project a plugin could ever guess, so
            // this installs disabled — see module doc.
            None => (
                false,
                String::new(),
                "main".to_string(),
                "local".to_string(),
                false,
                false,
                String::new(),
            ),
        };

    let job = JobRow {
        id,
        name: def.name.clone(),
        cron,
        mode,
        natural_text,
        project_id,
        branch,
        gateway,
        enabled,
        prompt: def.prompt.clone(),
        notify_success,
        notify_fail,
        pre_check,
        model_override: def.model_override.clone(),
        plugin_id: Some(plugin_id.to_string()),
    };
    scheduler::upsert_job(store, job).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::declarative;
    use crate::plugins::host::PluginSource;
    use ryuzi_plugin_sdk::PluginManifest;

    async fn mem_store() -> Store {
        let db = tempfile::NamedTempFile::new().unwrap();
        Store::open(db.path()).await.unwrap()
    }

    /// Seed a real project row so `automation::update_hook`'s target
    /// validation (`unknown project: ...`) accepts it as an `agent.run`
    /// target — mirrors `automation.rs`'s own private `seed_project` test
    /// helper (kept separately here; that one is private to its module).
    async fn seed_project(store: &Store, project_id: &str) {
        let workdir = tempfile::tempdir().unwrap().keep();
        store
            .insert_project(crate::domain::Project {
                project_id: project_id.to_string(),
                name: project_id.to_string(),
                workdir: workdir.display().to_string(),
                source: None,
                model: None,
                effort: None,
                perm_mode: crate::domain::PermMode::Default,
                created_at: None,
                is_git: false,
            })
            .await
            .unwrap();
    }

    /// A plugin declaring one `webhook.outbound` hook (Claude-alias
    /// trigger `PreToolUse`, to prove canonicalization), one `agent.run`
    /// hook, and one job — the exact shape the Step-1 sketch in the task
    /// brief drives.
    fn plugin_with_hooks_and_jobs(id: &str) -> CorePlugin {
        let toml = format!(
            r#"
contract = 2
id = "{id}"
name = "{id}"

[[hooks]]
name = "notify"
trigger = "PreToolUse"
action = "webhook.outbound"

[hooks.config]
url = "https://example.com/notify"
method = "POST"

[[hooks]]
name = "triage"
trigger = "tool.before"
action = "agent.run"

[hooks.config]
projectId = ""
branch = ""
gatewayId = ""
prompt = "Triage this tool call"
subtask = false

[[jobs]]
name = "nightly"
schedule = "every day at 2am"
prompt = "Run the nightly audit"
"#
        );
        let manifest = PluginManifest::from_toml(&toml).expect("valid manifest");
        declarative::declarative_plugin(manifest, PluginSource::Builtin)
            .expect("declarative plugin")
    }

    #[tokio::test]
    async fn agent_run_hooks_and_jobs_install_disabled_and_survive_resync() {
        let store = mem_store().await;
        seed_project(&store, "proj-1").await;
        let plugin = plugin_with_hooks_and_jobs("gh");

        let report = sync_plugin_automations(&store, &plugin).await.unwrap();
        assert_eq!(report.hooks_synced, 2);
        assert_eq!(report.jobs_synced, 1);
        assert!(
            report.errors.is_empty(),
            "unexpected errors: {:?}",
            report.errors
        );

        let hooks = automation::list_hooks(&store).await.unwrap();
        let webhook = hooks.iter().find(|h| h.name == "gh/notify").unwrap();
        assert!(webhook.enabled, "webhook.outbound presets may ship enabled");
        assert_eq!(
            webhook.trigger_kind,
            TriggerKind::ToolBefore,
            "PreToolUse canonicalized"
        );
        assert_eq!(webhook.plugin_id.as_deref(), Some("gh"));

        let agent_run = hooks.iter().find(|h| h.name == "gh/triage").unwrap();
        assert!(!agent_run.enabled, "agent.run installs disabled");
        let config = agent_run.action.agent_run().unwrap();
        assert_eq!(
            config.project_id, "",
            "no plugin can guess the user's project"
        );

        let jobs = scheduler::list_jobs(&store).await.unwrap();
        let job = jobs.iter().find(|j| j.id == "gh/nightly").unwrap();
        assert!(!job.enabled, "jobs install disabled");
        assert_eq!(job.project_id, "");
        assert_eq!(job.plugin_id.as_deref(), Some("gh"));

        // User fills a target + enables the hook.
        automation::update_hook(
            &store,
            &agent_run.id,
            automation::HookInput {
                name: agent_run.name.clone(),
                trigger_kind: agent_run.trigger_kind,
                action: HookActionInput::AgentRun(automation::AgentRunAction {
                    project_id: "proj-1".into(),
                    branch: String::new(),
                    gateway_id: "local".into(),
                    prompt: config.prompt.clone(),
                    agent_id: None,
                    model_override: None,
                    subtask: false,
                }),
                enabled: true,
            },
        )
        .await
        .unwrap();

        // The plugin re-syncs (e.g. an update landed a new release).
        let report = sync_plugin_automations(&store, &plugin).await.unwrap();
        assert_eq!(report.hooks_synced, 2, "re-sync still touches both hooks");

        let hooks_after = automation::list_hooks(&store).await.unwrap();
        assert_eq!(hooks_after.len(), 2, "re-sync must not duplicate rows");
        let agent_run_after = hooks_after.iter().find(|h| h.name == "gh/triage").unwrap();
        assert!(
            agent_run_after.enabled,
            "user enablement preserved across plugin update"
        );
        let config_after = agent_run_after.action.agent_run().unwrap();
        assert_eq!(
            config_after.project_id, "proj-1",
            "user-filled target preserved across plugin update"
        );
        assert_eq!(
            config_after.gateway_id, "local",
            "user-filled gateway preserved across plugin update"
        );
        assert_eq!(
            agent_run_after.id, agent_run.id,
            "row id stable across re-sync"
        );
    }

    #[tokio::test]
    async fn uninstall_cascades_hooks_and_jobs() {
        let store = mem_store().await;
        let owned = plugin_with_hooks_and_jobs("acme");
        let other = plugin_with_hooks_and_jobs("other");
        sync_plugin_automations(&store, &owned).await.unwrap();
        sync_plugin_automations(&store, &other).await.unwrap();

        let webhook = automation::list_hooks(&store)
            .await
            .unwrap()
            .into_iter()
            .find(|h| h.name == "acme/notify")
            .unwrap();
        automation::create_run(
            &store,
            &webhook.id,
            serde_json::json!({ "event": "session.end" }),
        )
        .await
        .unwrap();

        let job = scheduler::get_job(&store, "acme/nightly")
            .await
            .unwrap()
            .unwrap();
        scheduler::insert_run(
            &store,
            scheduler::RunRow {
                id: "r-acme".into(),
                job_id: job.id.clone(),
                status: "success".into(),
                started_at: crate::paths::now_ms(),
                finished_at: None,
                session_pk: None,
                error: None,
                add_lines: None,
                del_lines: None,
                note: None,
                log: None,
            },
        )
        .await
        .unwrap();

        remove_plugin_automations(&store, "acme").await.unwrap();

        let remaining_hooks = automation::list_hooks(&store).await.unwrap();
        assert!(remaining_hooks
            .iter()
            .all(|h| h.plugin_id.as_deref() != Some("acme")));
        assert!(remaining_hooks
            .iter()
            .any(|h| h.plugin_id.as_deref() == Some("other")));
        assert!(automation::list_runs(&store, &webhook.id)
            .await
            .unwrap()
            .is_empty());

        let remaining_jobs = scheduler::list_jobs(&store).await.unwrap();
        assert!(remaining_jobs
            .iter()
            .all(|j| j.plugin_id.as_deref() != Some("acme")));
        assert!(remaining_jobs
            .iter()
            .any(|j| j.plugin_id.as_deref() == Some("other")));
        assert!(scheduler::list_runs(&store, &job.id, 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn enabling_a_targetless_agent_run_hook_is_rejected() {
        let store = mem_store().await;
        let plugin = plugin_with_hooks_and_jobs("gh");
        sync_plugin_automations(&store, &plugin).await.unwrap();

        let agent_run = automation::list_hooks(&store)
            .await
            .unwrap()
            .into_iter()
            .find(|h| h.name == "gh/triage")
            .unwrap();

        let error = automation::toggle_hook(&store, &agent_run.id, true)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("pick a project first"),
            "unexpected error message: {error}"
        );

        // Same guard for the scheduler's job.
        let job = scheduler::get_job(&store, "gh/nightly")
            .await
            .unwrap()
            .unwrap();
        let job_error = scheduler::toggle(&store, &job.id, true).await.unwrap_err();
        assert!(
            job_error.to_string().contains("pick a project first"),
            "unexpected error message: {job_error}"
        );
    }

    #[tokio::test]
    async fn a_config_that_fails_its_actions_schema_is_a_recorded_error_not_a_fatal_one() {
        let store = mem_store().await;
        let toml = r#"
contract = 2
id = "broken"
name = "broken"

[[hooks]]
name = "bad"
trigger = "tool.before"
action = "agent.run"

[hooks.config]
totally = "not the right shape"

[[hooks]]
name = "good"
trigger = "session.end"
action = "webhook.outbound"

[hooks.config]
url = "https://example.com/ok"
method = "POST"
"#;
        let manifest = PluginManifest::from_toml(toml).unwrap();
        let plugin = declarative::declarative_plugin(manifest, PluginSource::Builtin).unwrap();

        let report = sync_plugin_automations(&store, &plugin).await.unwrap();
        assert_eq!(report.hooks_synced, 1, "the good hook still syncs");
        assert_eq!(report.errors.len(), 1, "the bad hook is a recorded error");

        let hooks = automation::list_hooks(&store).await.unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, "broken/good");
    }

    #[tokio::test]
    async fn an_invalid_job_schedule_is_a_recorded_error_not_a_fatal_one() {
        let store = mem_store().await;
        let toml = r#"
contract = 2
id = "badsched"
name = "badsched"

[[jobs]]
name = "nope"
schedule = "not a real schedule at all"
prompt = "do the thing"
"#;
        let manifest = PluginManifest::from_toml(toml).unwrap();
        let plugin = declarative::declarative_plugin(manifest, PluginSource::Builtin).unwrap();

        let report = sync_plugin_automations(&store, &plugin).await.unwrap();
        assert_eq!(report.jobs_synced, 0);
        assert_eq!(report.errors.len(), 1);
        assert!(scheduler::list_jobs(&store).await.unwrap().is_empty());
    }
}
