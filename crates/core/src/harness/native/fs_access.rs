//! Gateway filesystem-access enforcement for native tool calls.
//!
//! The Gateways screen lets a user scope what agents may do on a machine
//! (`full` | `projects` | `read`, persisted on the `gateways` row with id
//! `"local"`). This module is the ONLY consumer of that setting: it turns the
//! stored mode plus the session's working directory into an allow/deny verdict
//! that `runner::execute_tool_call` applies before a tool runs.
//!
//! What each mode really guarantees — the UI copy must not promise more:
//!
//! * `full` — no extra restriction (the historical behavior).
//! * `projects` — tools that MUTATE the machine (`tool_kind` `edit`) or run a
//!   shell (`execute`) are refused unless the session's working directory
//!   resolves inside one of the configured roots. Read/search tools are
//!   unaffected: they are already confined to the worktree by `tools::jail`.
//! * `read` — `edit` and `execute` tools are refused outright.
//!
//! An unknown/absent mode resolves to `Full`: the setting was inert for its
//! whole life before this module existed, so an unrecognized value must never
//! silently start blocking work.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsMode {
    Full,
    Projects,
    ReadOnly,
}

impl FsMode {
    /// Fails OPEN on anything unrecognized — see the module docs.
    pub fn parse(raw: &str) -> FsMode {
        match raw {
            "projects" => FsMode::Projects,
            "read" => FsMode::ReadOnly,
            _ => FsMode::Full,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsVerdict {
    Allow,
    /// `(code, message)` — the code becomes the `ToolError` code, the message
    /// is what the model and the transcript see.
    Deny(&'static str, String),
}

const READ_ONLY_MSG: &str = "Filesystem access for this machine is set to \"Read-only\" on the Gateways screen, so file edits and shell commands are refused. Ask the user to change it to \"Projects only\" or \"Full\" if this work is expected.";

const NO_ROOTS_MSG: &str = "Filesystem access for this machine is set to \"Projects only\" on the Gateways screen, but no project folders are configured, so file edits and shell commands are refused. Ask the user to add a folder there, or to switch the setting to \"Full\".";

const OUTSIDE_MSG: &str = "Filesystem access for this machine is set to \"Projects only\" on the Gateways screen, and this session's working directory is not inside any configured project folder, so file edits and shell commands are refused. Ask the user to add this folder there, or to switch the setting to \"Full\".";

/// Pure verdict. `work_dir` and `roots` MUST already be canonicalized by the
/// caller (see [`decide_for_session`]); this function does no filesystem IO so
/// it can be unit-tested without a temp tree.
pub fn decide(mode: FsMode, roots: &[PathBuf], work_dir: &Path, tool_kind: &str) -> FsVerdict {
    if mode == FsMode::Full {
        return FsVerdict::Allow;
    }
    // Only tools that mutate the machine or run a shell are gated. Read and
    // search tools are already confined to the session worktree by
    // `tools::jail`, so restricting them here would buy nothing.
    if tool_kind != "edit" && tool_kind != "execute" {
        return FsVerdict::Allow;
    }
    match mode {
        FsMode::Full => FsVerdict::Allow,
        FsMode::ReadOnly => FsVerdict::Deny("fs_mode_read_only", READ_ONLY_MSG.to_string()),
        FsMode::Projects => {
            if roots.is_empty() {
                return FsVerdict::Deny("fs_mode_no_project_roots", NO_ROOTS_MSG.to_string());
            }
            if roots.iter().any(|root| work_dir.starts_with(root)) {
                FsVerdict::Allow
            } else {
                FsVerdict::Deny("fs_mode_outside_project_roots", OUTSIDE_MSG.to_string())
            }
        }
    }
}

/// Resolve the live verdict for a session: read the engine's own gateway row
/// (id `"local"`), canonicalize both sides, then delegate to [`decide`].
///
/// Fails OPEN on a missing row or a store error: the column was inert for its
/// entire life before this module existed, so a transient DB failure must not
/// brick every session.
pub async fn decide_for_session(
    store: &crate::store::Store,
    work_dir: &Path,
    tool_kind: &str,
) -> FsVerdict {
    let row = match crate::gateways::get_row(store, "local").await {
        Ok(Some(row)) => row,
        Ok(None) => return FsVerdict::Allow,
        Err(e) => {
            tracing::warn!(error = %e, "gateway fs_mode lookup failed; allowing tool call");
            return FsVerdict::Allow;
        }
    };

    let mode = FsMode::parse(&row.fs_mode);
    // Hot path: the permissive default costs zero syscalls.
    if mode == FsMode::Full {
        return FsVerdict::Allow;
    }

    let Ok(work_canon) = tokio::fs::canonicalize(work_dir).await else {
        // A session whose workdir does not exist cannot do damage through
        // these tools anyway, and the tool itself fails with a better message.
        return FsVerdict::Allow;
    };

    // A configured root that no longer resolves is DROPPED rather than kept
    // literally: dropping makes the check stricter, which is the safe
    // direction for a stale or deleted folder.
    let mut roots: Vec<PathBuf> = Vec::with_capacity(row.paths.len());
    for p in &row.paths {
        if let Ok(canon) = tokio::fs::canonicalize(p).await {
            roots.push(canon);
        }
    }

    decide(mode, &roots, &work_canon, tool_kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateways::{upsert_row, GatewayRow};
    use crate::store::Store;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn full_mode_allows_every_kind() {
        for kind in ["edit", "execute", "read"] {
            assert_eq!(
                decide(FsMode::Full, &[], &p("/anywhere"), kind),
                FsVerdict::Allow,
                "kind {kind}"
            );
        }
    }

    #[test]
    fn non_mutating_kinds_are_never_denied() {
        for kind in ["read", "search", "fetch", "other"] {
            assert_eq!(
                decide(FsMode::ReadOnly, &[], &p("/anywhere"), kind),
                FsVerdict::Allow,
                "read-only kind {kind}"
            );
            assert_eq!(
                decide(FsMode::Projects, &[], &p("/anywhere"), kind),
                FsVerdict::Allow,
                "projects kind {kind}"
            );
        }
    }

    #[test]
    fn read_only_denies_edit_and_execute() {
        for kind in ["edit", "execute"] {
            match decide(FsMode::ReadOnly, &[], &p("/anywhere"), kind) {
                FsVerdict::Deny(code, msg) => {
                    assert_eq!(code, "fs_mode_read_only");
                    assert_eq!(msg, READ_ONLY_MSG);
                }
                other => panic!("expected deny for {kind}, got {other:?}"),
            }
        }
    }

    #[test]
    fn projects_with_no_roots_denies() {
        match decide(FsMode::Projects, &[], &p("/anywhere"), "execute") {
            FsVerdict::Deny(code, msg) => {
                assert_eq!(code, "fs_mode_no_project_roots");
                assert_eq!(msg, NO_ROOTS_MSG);
            }
            other => panic!("expected deny, got {other:?}"),
        }
    }

    #[test]
    fn projects_allows_a_workdir_inside_a_root() {
        let roots = vec![p("/srv/app"), p("/srv/other")];
        assert_eq!(
            decide(FsMode::Projects, &roots, &p("/srv/app/sub/dir"), "edit"),
            FsVerdict::Allow
        );
    }

    #[test]
    fn projects_denies_a_workdir_outside_every_root() {
        let roots = vec![p("/srv/app"), p("/srv/other")];
        match decide(FsMode::Projects, &roots, &p("/tmp/elsewhere"), "execute") {
            FsVerdict::Deny(code, msg) => {
                assert_eq!(code, "fs_mode_outside_project_roots");
                assert_eq!(msg, OUTSIDE_MSG);
            }
            other => panic!("expected deny, got {other:?}"),
        }
    }

    #[test]
    fn projects_allows_a_workdir_equal_to_a_root() {
        // `starts_with` on an identical path is `true` — pinned so a refactor
        // to a strict-prefix check cannot silently lock users out of the very
        // folder they configured.
        let roots = vec![p("/srv/app")];
        assert_eq!(
            decide(FsMode::Projects, &roots, &p("/srv/app"), "execute"),
            FsVerdict::Allow
        );
    }

    #[test]
    fn parse_maps_unknown_values_to_full() {
        assert_eq!(FsMode::parse(""), FsMode::Full);
        assert_eq!(FsMode::parse("nonsense"), FsMode::Full);
        assert_eq!(FsMode::parse("FULL"), FsMode::Full);
        assert_eq!(FsMode::parse("projects"), FsMode::Projects);
        assert_eq!(FsMode::parse("read"), FsMode::ReadOnly);
    }

    /// Open a `Store` on a temp file and install a `"local"` gateway row with
    /// the given mode/roots.
    async fn store_with_local_row(
        tmp: &tempfile::NamedTempFile,
        fs_mode: &str,
        paths: Vec<String>,
    ) -> Store {
        let store = Store::open(tmp.path()).await.unwrap();
        upsert_row(
            &store,
            GatewayRow {
                id: "local".into(),
                name: "Local Machine".into(),
                kind: "local".into(),
                host: None,
                port: None,
                username: None,
                fs_mode: fs_mode.into(),
                paths,
                fingerprint: None,
                device_token: None,
            },
        )
        .await
        .unwrap();
        store
    }

    /// `tempfile::tempdir()` hands back an uncanonicalized path (on macOS
    /// `/var` is a symlink to `/private/var`, on Windows a short 8.3 name), so
    /// every comparison in these tests goes through the canonical form.
    fn canon(dir: &tempfile::TempDir) -> PathBuf {
        std::fs::canonicalize(dir.path()).unwrap()
    }

    #[tokio::test]
    async fn missing_local_gateway_row_allows() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            decide_for_session(&store, &canon(&dir), "execute").await,
            FsVerdict::Allow
        );
    }

    #[tokio::test]
    async fn projects_mode_denies_execute_for_a_workdir_outside_the_roots() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let store =
            store_with_local_row(&tmp, "projects", vec![canon(&root).display().to_string()]).await;

        match decide_for_session(&store, elsewhere.path(), "execute").await {
            FsVerdict::Deny(code, _) => assert_eq!(code, "fs_mode_outside_project_roots"),
            other => panic!("expected deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn projects_mode_allows_execute_for_a_workdir_inside_a_root() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let root = tempfile::tempdir().unwrap();
        let work = canon(&root).join("nested");
        std::fs::create_dir(&work).unwrap();
        let store =
            store_with_local_row(&tmp, "projects", vec![canon(&root).display().to_string()]).await;

        assert_eq!(
            decide_for_session(&store, &work, "execute").await,
            FsVerdict::Allow
        );
    }

    #[tokio::test]
    async fn projects_mode_still_allows_read_tools_outside_the_roots() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let store =
            store_with_local_row(&tmp, "projects", vec![canon(&root).display().to_string()]).await;

        assert_eq!(
            decide_for_session(&store, elsewhere.path(), "read").await,
            FsVerdict::Allow
        );
    }

    #[tokio::test]
    async fn read_only_mode_denies_edit_but_allows_read() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_local_row(&tmp, "read", vec![]).await;

        match decide_for_session(&store, dir.path(), "edit").await {
            FsVerdict::Deny(code, _) => assert_eq!(code, "fs_mode_read_only"),
            other => panic!("expected deny, got {other:?}"),
        }
        assert_eq!(
            decide_for_session(&store, dir.path(), "read").await,
            FsVerdict::Allow
        );
    }

    #[tokio::test]
    async fn full_mode_allows_execute_anywhere() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = store_with_local_row(&tmp, "full", vec![]).await;

        assert_eq!(
            decide_for_session(&store, dir.path(), "execute").await,
            FsVerdict::Allow
        );
    }
}
