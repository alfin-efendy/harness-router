//! Tauri commands exposing the native runtime's agents, slash commands, and
//! per-session todos to Cockpit — thin proxies to the engine daemon's native
//! RPC family.

use crate::engine_manager::EngineManager;
use crate::error::CmdError;
use std::sync::Arc;
use tauri::State;

pub use ryuzi_core::api::types::{
    AgentInfo, CommandFileInfo, CommandFileInputDto, CommandFileMutationDto, QueuedMessageInfo,
    SlashEntryInfo, TodoItem, WorktreeHookStatus,
};

type R<T> = Result<T, CmdError>;
type Engine<'a> = State<'a, Arc<EngineManager>>;

/// The agents available for a project (built-ins plus discovered custom agents).
#[tauri::command]
#[specta::specta]
pub async fn native_agents(
    engine: Engine<'_>,
    runner_id: Option<String>,
    project_id: String,
) -> R<Vec<AgentInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "native_agents",
            serde_json::json!({ "project_id": project_id }),
        )
        .await
}

/// The `.ryuzi/hooks` scripts present in a project's worktree and whether the
/// user has trusted this exact set. Untrusted scripts are never executed.
#[tauri::command]
#[specta::specta]
pub async fn worktree_hook_status(
    engine: Engine<'_>,
    runner_id: Option<String>,
    project_id: String,
) -> R<WorktreeHookStatus> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "worktree_hook_status",
            serde_json::json!({ "project_id": project_id }),
        )
        .await
}

/// Record the user's explicit acceptance of the hook scripts currently on
/// disk in a project's worktree. Editing any of them revokes the acceptance.
#[tauri::command]
#[specta::specta]
pub async fn trust_worktree_hooks(
    engine: Engine<'_>,
    runner_id: Option<String>,
    project_id: String,
) -> R<WorktreeHookStatus> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "trust_worktree_hooks",
            serde_json::json!({ "project_id": project_id }),
        )
        .await
}

/// The unified "/" autocomplete catalog for a project/agent pairing.
#[tauri::command]
#[specta::specta]
pub async fn slash_catalog(
    engine: Engine<'_>,
    runner_id: Option<String>,
    project_id: Option<String>,
    agent_id: Option<String>,
) -> R<Vec<SlashEntryInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "slash_catalog",
            serde_json::json!({ "project_id": project_id, "agent_id": agent_id }),
        )
        .await
}

/// A session's current native todo list.
#[tauri::command]
#[specta::specta]
pub async fn session_todos(
    engine: Engine<'_>,
    runner_id: Option<String>,
    session_pk: String,
) -> R<Vec<TodoItem>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "session_todos",
            serde_json::json!({ "session_pk": session_pk }),
        )
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn global_command_list(
    engine: Engine<'_>,
    runner_id: Option<String>,
) -> R<Vec<CommandFileInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc("global_command_list", serde_json::json!({}))
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn global_command_read(
    engine: Engine<'_>,
    runner_id: Option<String>,
    name: String,
) -> R<CommandFileInfo> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc("global_command_read", serde_json::json!({ "name": name }))
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn global_command_create(
    engine: Engine<'_>,
    runner_id: Option<String>,
    input: CommandFileInputDto,
) -> R<CommandFileInfo> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "global_command_create",
            serde_json::json!({ "input": input }),
        )
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn global_command_update(
    engine: Engine<'_>,
    runner_id: Option<String>,
    name: String,
    revision: String,
    input: CommandFileMutationDto,
) -> R<CommandFileInfo> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "global_command_update",
            serde_json::json!({
                "name": name,
                "revision": revision,
                "input": input,
            }),
        )
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn global_command_delete(
    engine: Engine<'_>,
    runner_id: Option<String>,
    name: String,
    revision: String,
) -> R<()> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "global_command_delete",
            serde_json::json!({
                "name": name,
                "revision": revision,
            }),
        )
        .await
}

/// A session's durable queued messages.
#[tauri::command]
#[specta::specta]
pub async fn session_queue(
    engine: Engine<'_>,
    runner_id: Option<String>,
    session_pk: String,
) -> R<Vec<QueuedMessageInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "session_queue",
            serde_json::json!({ "session_pk": session_pk }),
        )
        .await
}

/// Queue a durable message for a session.
#[tauri::command]
#[specta::specta]
pub async fn enqueue_session_message(
    engine: Engine<'_>,
    runner_id: Option<String>,
    session_pk: String,
    prompt: String,
    options: Option<ryuzi_core::api::types::ChatRequestOptions>,
) -> R<QueuedMessageInfo> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "enqueue_session_message",
            serde_json::json!({
                "session_pk": session_pk,
                "prompt": prompt,
                "options": options,
            }),
        )
        .await
}

/// Remove a durable queued message from a session.
#[tauri::command]
#[specta::specta]
pub async fn remove_session_message(
    engine: Engine<'_>,
    runner_id: Option<String>,
    session_pk: String,
    id: String,
) -> R<bool> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "remove_session_message",
            serde_json::json!({ "session_pk": session_pk, "id": id }),
        )
        .await
}
