//! Native runtime introspection: agents, slash commands, and per-session
//! todos exposed to Cockpit. Moved verbatim (per the Move Recipe) from
//! `apps/cockpit/src-tauri/src/native_cmd.rs`; that file keeps its own copy
//! until the proxy rewrite in Tasks 15-16. `AgentRegistry::load` and
//! `CommandRegistry::load` walk the project's workdir on disk — this now
//! runs in the daemon process, which is correct since the daemon is where
//! sessions actually run.

use super::{ok, params, ApiError};
use crate::api::types::*;
use crate::control::ControlPlane;
use crate::harness::native::agents::AgentRegistry;
use crate::harness::native::commands::{
    delete_project_command, list_project_commands, read_project_command, write_project_command,
    CommandFileError, CommandOrigin,
};
use crate::serve::ApiState;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

pub(crate) const HANDLES: &[&str] = &[
    "native_agents",
    "slash_catalog",
    "session_todos",
    "list_project_commands",
    "read_project_command",
    "create_project_command",
    "update_project_command",
    "delete_project_command",
];

#[derive(Deserialize)]
struct ProjectIdP {
    project_id: String,
}
#[derive(Deserialize)]
struct SessionPkP {
    session_pk: String,
}

#[derive(Deserialize)]
struct SlashCatalogP {
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectCommandCreateP {
    project_id: String,
    input: ProjectCommandInputDto,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectCommandReadP {
    project_id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectCommandUpdateP {
    project_id: String,
    name: String,
    revision: String,
    input: ProjectCommandMutationDto,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectCommandDeleteP {
    project_id: String,
    name: String,
    revision: String,
}

pub(crate) async fn dispatch(state: &ApiState, method: &str, p: Value) -> Result<Value, ApiError> {
    let cp = &state.cp;
    match method {
        "native_agents" => {
            let a: ProjectIdP = params(p)?;
            ok(native_agents(cp, &a.project_id).await?)
        }
        "slash_catalog" => {
            let a: SlashCatalogP = params(p)?;
            ok(
                slash_catalog_entries(state, a.project_id.as_deref(), a.agent_id.as_deref())
                    .await?,
            )
        }
        "list_project_commands" => {
            let a: ProjectIdP = params(p)?;
            ok(list_commands(cp, &a.project_id).await?)
        }
        "read_project_command" => {
            let a: ProjectCommandReadP = params(p)?;
            ok(read_command(cp, &a.project_id, &a.name).await?)
        }
        "create_project_command" => {
            let a: ProjectCommandCreateP = params(p)?;
            ok(create_command(cp, &a.project_id, a.input).await?)
        }
        "update_project_command" => {
            let a: ProjectCommandUpdateP = params(p)?;
            ok(update_command(cp, &a.project_id, &a.name, &a.revision, a.input).await?)
        }
        "delete_project_command" => {
            let a: ProjectCommandDeleteP = params(p)?;
            delete_command(cp, &a.project_id, &a.name, &a.revision).await?;
            ok(())
        }
        "session_todos" => {
            let a: SessionPkP = params(p)?;
            let rows = cp.store().list_todos(&a.session_pk).await?;
            ok(rows
                .into_iter()
                .map(|(content, status)| TodoItem { content, status })
                .collect::<Vec<_>>())
        }
        _ => Err(ApiError::not_found(format!("unknown method: {method}"))),
    }
}

async fn project_workdir(cp: &ControlPlane, project_id: &str) -> Result<String, ApiError> {
    let projects = cp.list_projects().await?;
    projects
        .into_iter()
        .find(|p| p.project_id == project_id)
        .map(|p| p.workdir)
        .ok_or_else(|| ApiError::not_found(format!("unknown project {project_id}")))
}

/// The agents available for a project (built-ins plus discovered custom agents).
async fn native_agents(cp: &ControlPlane, project_id: &str) -> Result<Vec<AgentInfo>, ApiError> {
    let workdir = project_workdir(cp, project_id).await?;
    let reg = AgentRegistry::load(Path::new(&workdir));
    Ok(reg
        .all()
        .into_iter()
        .map(|a| AgentInfo {
            name: a.name,
            description: a.description,
            mode: a.mode.as_str().to_string(),
            builtin: a.builtin,
        })
        .collect())
}

async fn list_commands(
    cp: &ControlPlane,
    project_id: &str,
) -> Result<Vec<ProjectCommandInfo>, ApiError> {
    let workdir = project_workdir(cp, project_id).await?;
    list_project_commands(Path::new(&workdir))
        .map(|commands| commands.into_iter().map(Into::into).collect())
        .map_err(command_file_error)
}

async fn read_command(
    cp: &ControlPlane,
    project_id: &str,
    name: &str,
) -> Result<ProjectCommandInfo, ApiError> {
    let workdir = project_workdir(cp, project_id).await?;
    read_project_command(Path::new(&workdir), name)
        .map(Into::into)
        .map_err(command_file_error)
}

async fn create_command(
    cp: &ControlPlane,
    project_id: &str,
    input: ProjectCommandInputDto,
) -> Result<ProjectCommandInfo, ApiError> {
    let workdir = project_workdir(cp, project_id).await?;
    write_project_command(Path::new(&workdir), input.into(), None)
        .map(Into::into)
        .map_err(command_file_error)
}

async fn update_command(
    cp: &ControlPlane,
    project_id: &str,
    name: &str,
    revision: &str,
    input: ProjectCommandMutationDto,
) -> Result<ProjectCommandInfo, ApiError> {
    let workdir = project_workdir(cp, project_id).await?;
    write_project_command(Path::new(&workdir), input.with_name(name), Some(revision))
        .map(Into::into)
        .map_err(command_file_error)
}

async fn delete_command(
    cp: &ControlPlane,
    project_id: &str,
    name: &str,
    revision: &str,
) -> Result<(), ApiError> {
    let workdir = project_workdir(cp, project_id).await?;
    delete_project_command(Path::new(&workdir), name, revision).map_err(command_file_error)
}

fn command_file_error(error: CommandFileError) -> ApiError {
    match error {
        CommandFileError::InvalidName(message) => ApiError::bad_request(message),
        CommandFileError::NotFound(name) => {
            ApiError::not_found(format!("project command not found: {name}"))
        }
        CommandFileError::RevisionConflict => ApiError::conflict(error.to_string()),
        CommandFileError::Io(error) => ApiError {
            status: 500,
            message: error.to_string(),
        },
    }
}
/// The unified "/" autocomplete catalog for a project/agent pairing.
async fn slash_catalog_entries(
    state: &ApiState,
    project_id: Option<&str>,
    agent_id: Option<&str>,
) -> Result<Vec<SlashEntryInfo>, ApiError> {
    let workdir = match project_id {
        Some(id) => Some(project_workdir(&state.cp, id).await?),
        None => None,
    };
    let allowed = agent_allowed_skills(state, agent_id).await;
    let catalog = crate::harness::native::slash_catalog::SlashCatalog::load(
        workdir.as_deref().map(Path::new),
        allowed.as_deref(),
    );
    Ok(catalog
        .entries()
        .into_iter()
        .map(slash_entry_info)
        .collect())
}

/// The durable primary agent's bound skill names (`profile.skills`), mirroring
/// the harness adapter's `allowed_skills` mapping. `None` = no binding.
async fn agent_allowed_skills(state: &ApiState, agent_id: Option<&str>) -> Option<Vec<String>> {
    let agent_id = agent_id?;
    let registry = state.agents.snapshot().await;
    let agent = registry.agents.iter().find(|a| a.profile.id == agent_id)?;
    (!agent.profile.skills.is_empty()).then(|| agent.profile.skills.clone())
}

fn slash_entry_info(entry: crate::harness::native::slash_catalog::SlashEntry) -> SlashEntryInfo {
    use crate::harness::native::slash_catalog::SlashKind;
    SlashEntryInfo {
        name: entry.name,
        description: entry.description,
        kind: match entry.kind {
            SlashKind::Command => SlashKindInfo::Command,
            SlashKind::Skill => SlashKindInfo::Skill,
        },
        origin: match entry.origin {
            CommandOrigin::Builtin => CommandOriginInfo::Builtin,
            CommandOrigin::Global => CommandOriginInfo::Global,
            CommandOrigin::Project => CommandOriginInfo::Project,
        },
        home: entry.surfaces.home,
        session: entry.surfaces.session,
        requires_project: entry.requires_project,
        effective: entry.effective,
        shadows_global: entry.shadows_global,
        agent: entry.agent,
        model: entry.model,
        subtask: entry.subtask,
    }
}

#[cfg(test)]
mod tests {
    use crate::api::{dispatch, tests_support::state};
    use serde_json::json;

    #[tokio::test]
    async fn session_todos_returns_empty_on_fresh_store_via_rpc() {
        let s = state().await;
        let out = dispatch(&s, "session_todos", json!({"session_pk": "nope"}))
            .await
            .unwrap();
        assert_eq!(out, json!([]));
    }

    #[tokio::test]
    async fn slash_catalog_lists_commands_and_project_skills_per_surface() {
        use crate::domain::{PermMode, Project};
        let workdir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workdir.path().join(".agents/skills/pdf")).unwrap();
        std::fs::write(
            workdir.path().join(".agents/skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: PDFs\n---\nbody",
        )
        .unwrap();
        let state = state().await;
        state
            .cp
            .store()
            .insert_project(Project {
                project_id: "p1".into(),
                name: "demo".into(),
                workdir: workdir.path().display().to_string(),
                source: None,
                model: None,
                effort: None,
                perm_mode: PermMode::Default,
                created_at: None,
                is_git: false,
            })
            .await
            .unwrap();
        let out = dispatch(&state, "slash_catalog", json!({"project_id": "p1"}))
            .await
            .unwrap();
        let entries: Vec<crate::api::types::SlashEntryInfo> = serde_json::from_value(out).unwrap();
        let init = entries.iter().find(|e| e.name == "init").unwrap();
        assert!(init.home && init.session && init.requires_project);
        assert!(init.origin == crate::api::types::CommandOriginInfo::Builtin && init.effective);
        let review = entries.iter().find(|e| e.name == "review").unwrap();
        assert!(!review.home && review.session);
        let pdf = entries
            .iter()
            .find(|e| e.name == "pdf" && e.kind == crate::api::types::SlashKindInfo::Skill)
            .unwrap();
        assert_eq!(pdf.origin, crate::api::types::CommandOriginInfo::Project);
    }

    #[test]
    fn slash_entry_info_maps_kind_origin_and_precedence_flags() {
        use crate::api::types::{CommandOriginInfo, SlashKindInfo};
        use crate::harness::native::commands::{CommandOrigin, CommandSurfaces};
        use crate::harness::native::slash_catalog::{SlashEntry, SlashKind};

        // (a) builtin command, effective, not shadowed — also covers the
        // surfaces->home/session and requires_project passthrough.
        let builtin_command = SlashEntry {
            name: "init".into(),
            description: "d".into(),
            kind: SlashKind::Command,
            origin: CommandOrigin::Builtin,
            surfaces: CommandSurfaces {
                home: true,
                session: false,
            },
            requires_project: true,
            effective: true,
            shadows_global: false,
            agent: None,
            model: None,
            subtask: false,
        };
        let info = super::slash_entry_info(builtin_command);
        assert_eq!(info.kind, SlashKindInfo::Command);
        assert_eq!(info.origin, CommandOriginInfo::Builtin);
        assert!(info.effective);
        assert!(!info.shadows_global);
        assert!(info.home && !info.session);
        assert!(info.requires_project);

        // (b) project-origin skill.
        let project_skill = SlashEntry {
            name: "pdf".into(),
            description: "d".into(),
            kind: SlashKind::Skill,
            origin: CommandOrigin::Project,
            surfaces: CommandSurfaces::default(),
            requires_project: false,
            effective: true,
            shadows_global: false,
            agent: None,
            model: None,
            subtask: false,
        };
        let info = super::slash_entry_info(project_skill);
        assert_eq!(info.kind, SlashKindInfo::Skill);
        assert_eq!(info.origin, CommandOriginInfo::Project);

        // (c) global-origin command shadowed by a project command: not
        // effective, shadows_global true.
        let shadowed_global = SlashEntry {
            name: "ship".into(),
            description: "d".into(),
            kind: SlashKind::Command,
            origin: CommandOrigin::Global,
            surfaces: CommandSurfaces::default(),
            requires_project: false,
            effective: false,
            shadows_global: true,
            agent: None,
            model: None,
            subtask: false,
        };
        let info = super::slash_entry_info(shadowed_global);
        assert_eq!(info.kind, SlashKindInfo::Command);
        assert_eq!(info.origin, CommandOriginInfo::Global);
        assert!(!info.effective);
        assert!(info.shadows_global);
    }

    #[tokio::test]
    async fn project_command_rpc_crud_enforces_revision_conflicts() {
        use crate::domain::{PermMode, Project};

        let workdir = tempfile::tempdir().unwrap();
        let state = state().await;
        state
            .cp
            .store()
            .insert_project(Project {
                project_id: "p1".into(),
                name: "demo".into(),
                workdir: workdir.path().display().to_string(),
                source: None,
                model: None,
                effort: None,
                perm_mode: PermMode::Default,
                created_at: None,
                is_git: false,
            })
            .await
            .unwrap();
        let input = json!({
            "name": "review/security",
            "description": "Review security changes",
            "template": "Review $ARGUMENTS",
            "agent": "plan",
            "model": "anthropic/claude-sonnet-4-5",
            "subtask": true
        });

        let created = dispatch(
            &state,
            "create_project_command",
            json!({"project_id": "p1", "input": input}),
        )
        .await
        .unwrap();
        let revision = created["revision"].as_str().unwrap().to_string();
        assert_eq!(created["model"], "anthropic/claude-sonnet-4-5");
        assert_eq!(created["subtask"], true);

        let listed = dispatch(&state, "list_project_commands", json!({"project_id": "p1"}))
            .await
            .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);

        let read = dispatch(
            &state,
            "read_project_command",
            json!({"project_id": "p1", "name": "review/security"}),
        )
        .await
        .unwrap();
        assert_eq!(read["revision"], revision);

        let conflict = dispatch(
            &state,
            "update_project_command",
            json!({
                "project_id": "p1",
                "name": "review/security",
                "revision": "stale",
                "input": {
                    "description": "Updated review",
                    "template": "Updated $ARGUMENTS",
                    "agent": null,
                    "model": null,
                    "subtask": false
                }
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(conflict.status, 409);

        let invalid = dispatch(
            &state,
            "read_project_command",
            json!({"project_id": "p1", "name": "../escape"}),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.status, 400);
    }
}
