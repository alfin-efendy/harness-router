//! Session artifact commands: thin proxies to the engine artifact RPC family.

use crate::engine_manager::EngineManager;
use crate::error::CmdError;
use std::sync::Arc;
use tauri::State;

pub use ryuzi_core::api::types::{ArtifactFileInfo, ArtifactInfo};

type R<T> = Result<T, CmdError>;
type Engine<'a> = State<'a, Arc<EngineManager>>;

#[tauri::command]
#[specta::specta]
pub async fn list_session_artifacts(
    engine: Engine<'_>,
    runner_id: Option<String>,
    session_pk: String,
) -> R<Vec<ArtifactInfo>> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "list_session_artifacts",
            serde_json::json!({ "session_pk": session_pk }),
        )
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn fetch_artifact(
    engine: Engine<'_>,
    runner_id: Option<String>,
    session_pk: String,
    artifact_id: String,
) -> R<ArtifactFileInfo> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc(
            "fetch_artifact",
            serde_json::json!({ "session_pk": session_pk, "artifact_id": artifact_id }),
        )
        .await
}

/// Run the engine's artifact-retention pass immediately, instead of waiting
/// for the daemon's hourly timer. Sends no `retentionDays`, so the engine
/// resolves the operator's configured `artifact_retention_days`. Returns the
/// number of archived sessions purged.
#[tauri::command]
#[specta::specta]
pub async fn run_artifact_retention(engine: Engine<'_>, runner_id: Option<String>) -> R<u32> {
    let client = engine.client(runner_id.as_deref().unwrap_or("local"))?;
    client
        .rpc("run_artifact_retention", serde_json::json!({}))
        .await
}
