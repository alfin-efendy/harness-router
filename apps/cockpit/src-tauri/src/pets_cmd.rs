//! Pet catalog commands: thin proxies to the engine daemon's petdex manifest
//! fetch/download/serve RPCs backing the agent avatar pet picker. Always
//! local-engine — mirrors the per-agent Learning commands in `agent_cmd.rs`
//! (no `runner_id`), since a downloaded pet's cached sprite bytes live on
//! disk under this Cockpit's own local engine `state_dir()`, not a remote
//! runner's.

use crate::engine_manager::EngineManager;
use crate::error::CmdError;
use std::sync::Arc;
use tauri::State;

use ryuzi_core::api::types::PetManifestEntryInfo;

type R<T> = Result<T, CmdError>;
type Engine<'a> = State<'a, Arc<EngineManager>>;

fn slug_params(slug: &str) -> serde_json::Value {
    serde_json::json!({ "slug": slug })
}

fn download_pet_params(slug: &str, spritesheet_url: &str) -> serde_json::Value {
    serde_json::json!({ "slug": slug, "spritesheet_url": spritesheet_url })
}

#[tauri::command]
#[specta::specta]
pub async fn list_pet_manifest(engine: Engine<'_>) -> R<Vec<PetManifestEntryInfo>> {
    engine
        .client("local")?
        .rpc("list_pet_manifest", serde_json::json!({}))
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn download_pet(engine: Engine<'_>, slug: String, spritesheet_url: String) -> R<()> {
    engine
        .client("local")?
        .rpc("download_pet", download_pet_params(&slug, &spritesheet_url))
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn get_pet_sprite(engine: Engine<'_>, slug: String) -> R<Option<String>> {
    engine
        .client("local")?
        .rpc("get_pet_sprite", slug_params(&slug))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_payload_matches_core_rpc_contract() {
        assert_eq!(slug_params("sprout"), serde_json::json!({"slug": "sprout"}));
    }

    #[test]
    fn download_pet_payload_matches_core_rpc_contract() {
        assert_eq!(
            download_pet_params("sprout", "https://assets.petdex.dev/sprout/sprite.webp"),
            serde_json::json!({
                "slug": "sprout",
                "spritesheet_url": "https://assets.petdex.dev/sprout/sprite.webp"
            })
        );
    }
}
