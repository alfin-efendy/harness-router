use std::path::{Path, PathBuf};

/// Reads `var` as a directory override: `Some(path)` when it is set to a
/// non-blank value, `None` when it is unset, empty, or whitespace-only. A
/// blank value is treated as "unset" so an exported-but-empty variable (a
/// common shape in CI matrices and shell wrappers) can never redirect the
/// whole state tree to the process working directory.
fn env_dir_override(var: &str) -> Option<PathBuf> {
    let raw = std::env::var_os(var)?;
    if raw.to_string_lossy().trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

/// Root of the per-user *state* tree — the SQLite database
/// (`ryuzi.sqlite`), `daemon.json`, `secret.key`, worktrees, chat scratch
/// dirs and pet sprites all hang off this one directory.
///
/// **Test seam:** when `RYUZI_STATE_DIR` is set to a non-blank value, that
/// directory is used verbatim and the OS data-dir lookup is never consulted.
/// Unset (the production default) this has no effect. Same shape and naming
/// as the `RYUZI_PLUGINS_ROOT` seam in [`crate::plugins::bundle::
/// installed_bundle_root`] and the `RYUZI_TEST_CONFIG_ROOT` seam in
/// `skills_install`.
///
/// **Why this is a RUNTIME check and NOT `#[cfg(test)]`-gated — do not
/// "tighten" it:** the tests that depend on it (`crates/runner/tests/cli.rs`,
/// `daemon.rs`, `e2e_journey.rs`) spawn the REAL compiled `ryuzi` binary as a
/// subprocess via `assert_cmd`. That binary is built without `cfg(test)`, so
/// a `cfg(test)` seam in this library is invisible to it — the override has
/// to survive into a normal release-shaped build to reach the child process
/// at all. `RYUZI_PLUGINS_ROOT` exists for exactly this reason and carries
/// the same warning.
///
/// Those tests previously relied on `XDG_DATA_HOME`/`HOME` alone. `dirs::
/// data_dir()` honors both on Linux/macOS but on **Windows** it calls the
/// `FOLDERID_RoamingAppData` known-folder API, which ignores every
/// environment variable — so `cargo test -p ryuzi-runner` on Windows opened
/// the developer's LIVE `%APPDATA%\ryuzi\ryuzi.sqlite` and ran schema
/// migrations against it, leaving a database the installed release build
/// refused to open. This project has now destroyed real user state twice by
/// assuming a sandbox that the platform does not actually honor; this seam is
/// the platform-independent one, and every process-spawning test must set it.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = env_dir_override("RYUZI_STATE_DIR") {
        return dir;
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ryuzi")
}

/// Root of the per-user *config* tree (agent definitions, knowledge dirs).
///
/// **Test seam:** `RYUZI_CONFIG_DIR` overrides it, with the same non-blank
/// rule, the same runtime-not-`cfg(test)` rationale, and the same Windows
/// motivation as [`state_dir`] — `dirs::config_dir()` resolves via
/// `FOLDERID_RoamingAppData` on Windows and ignores `HOME`/`XDG_CONFIG_HOME`
/// there. See [`state_dir`]'s doc before changing either.
///
/// Note this is *not* the plugin bundle root: that has its own seam,
/// [`crate::plugins::bundle::installed_bundle_root`] / `RYUZI_PLUGINS_ROOT`,
/// and a test isolating a spawned daemon generally needs both.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = env_dir_override("RYUZI_CONFIG_DIR") {
        return dir;
    }
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("ryuzi")
}

pub fn agents_dir_in(config_root: &Path) -> PathBuf {
    config_root.join("agents")
}

pub fn agent_dir_in(config_root: &Path, agent_id: &str) -> PathBuf {
    agents_dir_in(config_root).join(agent_id)
}

pub fn agent_knowledge_dir_in(config_root: &Path, agent_id: &str) -> PathBuf {
    agent_dir_in(config_root, agent_id).join("knowledge")
}

pub fn db_path() -> PathBuf {
    state_dir().join("ryuzi.sqlite")
}

/// Base directory a git session's isolated worktree is created under.
/// `base` overrides the default `state_dir()/worktrees` root — pass the
/// resolved `worktree_dir` setting (already `expand_home`-d), or `None` to
/// fall back to the default.
pub fn worktree_path_for(base: Option<&Path>, project_id: &str, session_pk: &str) -> PathBuf {
    let short: String = session_pk.chars().take(8).collect();
    let root = base
        .map(Path::to_path_buf)
        .unwrap_or_else(|| state_dir().join("worktrees"));
    root.join(project_id).join(short)
}

/// Managed scratch working directory for a project-less `chat` session.
/// Lives under the state dir (never `$HOME`), created on first resolve.
pub fn chat_scratch_dir(session_pk: &str) -> PathBuf {
    state_dir().join("chat").join(session_pk)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// RAII guard for the process-global `RYUZI_STATE_DIR`/`RYUZI_CONFIG_DIR`
    /// seams: sets (or clears) the variable and restores whatever was there
    /// before on drop, so a panicking assertion can never leave the rest of
    /// this test binary pointed at a deleted tempdir. Every test that touches
    /// these variables — including [`state_dir_is_under_ryuzi`], which reads
    /// them — must be `#[serial]`, matching this crate's convention for other
    /// global-env test seams (see `plugins::bundle`'s `RYUZI_PLUGINS_ROOT`
    /// tests and `api::tests_support::TestConfigRootGuard`).
    struct EnvGuard {
        var: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(var: &'static str, value: &str) -> Self {
            let guard = Self {
                var,
                previous: std::env::var_os(var),
            };
            std::env::set_var(var, value);
            guard
        }

        fn unset(var: &'static str) -> Self {
            let guard = Self {
                var,
                previous: std::env::var_os(var),
            };
            std::env::remove_var(var);
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.var, value),
                None => std::env::remove_var(self.var),
            }
        }
    }

    #[test]
    #[serial]
    fn state_dir_is_under_ryuzi() {
        let _state = EnvGuard::unset("RYUZI_STATE_DIR");
        assert!(state_dir().ends_with("ryuzi"));
        assert!(db_path().ends_with("ryuzi.sqlite"));
    }

    #[test]
    #[serial]
    fn state_dir_honors_the_env_var_override() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set("RYUZI_STATE_DIR", &dir.path().to_string_lossy());
        assert_eq!(
            state_dir(),
            dir.path(),
            "the env override must win over the real OS data dir"
        );
        assert_eq!(
            db_path(),
            dir.path().join("ryuzi.sqlite"),
            "db_path() must follow the override — this is the path that \
             opened the developer's live database on Windows"
        );
    }

    #[test]
    #[serial]
    fn state_dir_falls_back_to_the_os_data_dir_when_unset() {
        let _guard = EnvGuard::unset("RYUZI_STATE_DIR");
        let dir = state_dir();
        assert!(
            dir.ends_with("ryuzi"),
            "unset must fall back to <data-dir>/ryuzi, got {}",
            dir.display()
        );
    }

    #[test]
    #[serial]
    fn state_dir_ignores_a_blank_env_var() {
        let _guard = EnvGuard::set("RYUZI_STATE_DIR", "   ");
        let dir = state_dir();
        assert!(
            dir.ends_with("ryuzi"),
            "a blank/whitespace-only override must be treated as unset, got {}",
            dir.display()
        );

        let _empty = EnvGuard::set("RYUZI_STATE_DIR", "");
        assert!(
            state_dir().ends_with("ryuzi"),
            "an empty override must be treated as unset too"
        );
    }

    #[test]
    #[serial]
    fn config_dir_honors_the_env_var_override() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set("RYUZI_CONFIG_DIR", &dir.path().to_string_lossy());
        assert_eq!(config_dir(), dir.path());
        assert_eq!(agents_dir_in(&config_dir()), dir.path().join("agents"));
    }

    #[test]
    #[serial]
    fn config_dir_falls_back_when_unset_or_blank() {
        {
            let _guard = EnvGuard::unset("RYUZI_CONFIG_DIR");
            let dir = config_dir();
            assert!(
                dir.ends_with("ryuzi"),
                "unset must fall back to <config-dir>/ryuzi, got {}",
                dir.display()
            );
        }
        {
            let _guard = EnvGuard::set("RYUZI_CONFIG_DIR", "  ");
            let dir = config_dir();
            assert!(
                dir.ends_with("ryuzi"),
                "a blank override must be treated as unset, got {}",
                dir.display()
            );
        }
    }

    #[test]
    fn worktree_path_uses_short_session_id() {
        let p = worktree_path_for(None, "proj1", "abcdef0123456789");
        assert!(p.ends_with("worktrees/proj1/abcdef01"));
    }

    #[test]
    fn worktree_path_honors_custom_base() {
        let base = PathBuf::from("/custom/wt-root");
        let p = worktree_path_for(Some(&base), "proj1", "abcdef0123456789");
        assert_eq!(p, PathBuf::from("/custom/wt-root/proj1/abcdef01"));
    }

    #[test]
    fn agent_paths_use_config_not_state_root() {
        let root = PathBuf::from("config-root");
        assert_eq!(agents_dir_in(&root), root.join("agents"));
        assert_eq!(
            agent_dir_in(&root, "reviewer"),
            root.join("agents/reviewer")
        );
        assert_eq!(
            agent_knowledge_dir_in(&root, "reviewer"),
            root.join("agents/reviewer/knowledge")
        );
    }

    #[test]
    fn new_id_is_unique_and_hyphenated() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
    }
}
