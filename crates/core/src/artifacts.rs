pub mod service;
pub mod storage;
pub mod types;

pub use service::{
    ingest_saved_attachments, ArtifactConfig, ArtifactRead, ArtifactService, CreateArtifact,
};
pub use storage::{ArtifactError, ArtifactStorage, ReadRange};
pub use types::{
    ArtifactAccessRow, ArtifactCreator, ArtifactListRow, ArtifactRecord, ArtifactReference,
    ArtifactStatus,
};

/// Settings key holding the operator-configured artifact retention window,
/// in days. Declared in `crate::settings::fields::GLOBAL_FIELDS`.
pub const RETENTION_DAYS_SETTING: &str = "artifact_retention_days";

/// Fallback retention window used only when the stored value cannot be
/// parsed as an integer. Kept in sync with the `artifact_retention_days`
/// `ConfigField` default in `crate::settings::fields`.
pub const DEFAULT_RETENTION_DAYS: i64 = 30;

/// Resolve the retention window every artifact-retention pass must use.
///
/// `SettingsStore::get` already falls back to the schema default when no row
/// is persisted, so this only has to defend against an unparseable value.
/// Both callers — the daemon's hourly pass
/// (`crate::daemon::spawn_artifact_retention`) and the on-demand
/// `run_artifact_retention` RPC (`crate::api::artifacts_api`) — go through
/// here, so a manual cleanup can never disagree with the automatic one.
///
/// A retention of `0` is meaningful ("purge everything already archived") and
/// is deliberately NOT floored to 1, unlike `crate::settings::usize_setting`.
pub async fn configured_retention_days(store: &std::sync::Arc<crate::store::Store>) -> i64 {
    crate::settings::SettingsStore::new(std::sync::Arc::clone(store))
        .get(RETENTION_DAYS_SETTING)
        .await
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

#[cfg(test)]
mod service_tdd_probe {
    //! TDD probe: written before `ArtifactService` exists, to force this
    //! module to fail to compile until the service is implemented. Removed
    //! once real service tests below cover the same ground.
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn create_artifact_persists_metadata_and_payload() {
        let storage_dir = tempfile::tempdir().unwrap();
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(crate::store::Store::open(db_file.path()).await.unwrap());
        let storage = ArtifactStorage::new(storage_dir.path());
        let service = ArtifactService::new(
            store,
            storage,
            ArtifactConfig {
                max_bytes: 1_000,
                session_max_bytes: 10_000,
                read_max_bytes: 1_000,
            },
        );

        let record = service
            .create_artifact(CreateArtifact {
                session_pk: "sess-1".into(),
                source_message_seq: Some(3),
                source_run_id: Some("run-1".into()),
                creator: ArtifactCreator::Agent,
                creator_id: Some("ada".into()),
                name: "report.md".into(),
                description: Some("summary".into()),
                content_type: Some("text/markdown".into()),
                bytes: b"hello".to_vec(),
            })
            .await
            .unwrap();

        assert_eq!(record.name, "report.md");
        assert_eq!(record.size_bytes, 5);
    }
}

#[cfg(test)]
mod retention_setting_tests {
    use super::{configured_retention_days, DEFAULT_RETENTION_DAYS, RETENTION_DAYS_SETTING};
    use std::sync::Arc;

    async fn temp_store() -> (Arc<crate::store::Store>, tempfile::NamedTempFile) {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Arc::new(crate::store::Store::open(tmp.path()).await.unwrap());
        (store, tmp)
    }

    #[tokio::test]
    async fn unset_resolves_to_the_schema_default() {
        let (store, _tmp) = temp_store().await;
        assert_eq!(
            configured_retention_days(&store).await,
            DEFAULT_RETENTION_DAYS
        );
    }

    #[tokio::test]
    async fn a_stored_value_wins_over_the_default_including_zero() {
        let (store, _tmp) = temp_store().await;
        store
            .set_setting_raw(RETENTION_DAYS_SETTING, "7")
            .await
            .unwrap();
        assert_eq!(configured_retention_days(&store).await, 7);

        store
            .set_setting_raw(RETENTION_DAYS_SETTING, "0")
            .await
            .unwrap();
        assert_eq!(configured_retention_days(&store).await, 0);
    }

    #[tokio::test]
    async fn an_unparseable_value_falls_back_to_the_default() {
        let (store, _tmp) = temp_store().await;
        store
            .set_setting_raw(RETENTION_DAYS_SETTING, "soon")
            .await
            .unwrap();
        assert_eq!(
            configured_retention_days(&store).await,
            DEFAULT_RETENTION_DAYS
        );
    }
}
