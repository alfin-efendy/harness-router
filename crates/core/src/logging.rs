//! Process-wide log wiring for the two daemon entry points.
//!
//! `ryuzi-core` emits ~174 `tracing` events, and `tracing` silently discards
//! every one of them unless a subscriber is installed. [`init_tracing`] is the
//! single place that installs one; it is called once per process from
//! `ryuzi-control`'s `main` and from Cockpit's `--engine-daemon` entry point.
//!
//! Output goes to **stderr**, not stdout: both daemon spawn paths redirect the
//! child's stdout AND stderr into `<state dir>/daemon.log`, so stderr lands in
//! the log file just the same, while leaving stdout to the existing
//! `println!` status lines.
//!
//! [`rotate_if_large`] is the other half. `daemon.log` is opened for append and
//! never truncated, so on a long-lived install it grows without bound (a remote
//! runner reconnect loop writes a line every 30s forever). Rotation happens at
//! the moment the log is opened for a spawn — that is the only safe moment,
//! because once the daemon is running the log IS its inherited stdout/stderr
//! file descriptor, which cannot be swapped from outside the process.

use std::path::{Path, PathBuf};

/// Environment variable that overrides the log filter, e.g.
/// `RYUZI_LOG=debug` or `RYUZI_LOG=ryuzi_core::plugins=trace,info`.
pub const LOG_ENV_VAR: &str = "RYUZI_LOG";

/// Filter used when [`LOG_ENV_VAR`] is unset, empty, or unparseable.
pub const DEFAULT_LOG_DIRECTIVE: &str = "info";

/// Rotate `daemon.log` once it grows past this. 10 MiB keeps a useful amount
/// of history while staying trivially openable in an editor.
pub const MAX_DAEMON_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// Resolve the filter directive from the raw environment value.
///
/// Pure so it can be unit-tested without touching the process environment:
/// [`init_tracing`] reads the variable and passes the value in.
pub fn log_filter_directive(env_value: Option<&str>) -> String {
    match env_value.map(str::trim) {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => DEFAULT_LOG_DIRECTIVE.to_string(),
    }
}

/// Install the process-wide `tracing` subscriber. Idempotent and infallible:
/// a second call (or a call in a process that already installed one) is a
/// no-op, because `try_init` returns `Err` rather than panicking.
///
/// An unparseable [`LOG_ENV_VAR`] falls back to [`DEFAULT_LOG_DIRECTIVE`]
/// instead of failing — a typo in an env var must never stop a daemon from
/// starting.
pub fn init_tracing() {
    let raw = std::env::var(LOG_ENV_VAR).ok();
    let directive = log_filter_directive(raw.as_deref());
    let filter = tracing_subscriber::EnvFilter::try_new(&directive)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_DIRECTIVE));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // The default writer is stdout; daemon status lines already own that.
        .with_writer(std::io::stderr)
        // No colour escapes — this stream is redirected into a log file.
        .with_ansi(false)
        .with_target(true)
        .try_init();
}

/// The rotated-out path for `log_path`: `{log_path}.1`.
fn rotated_path(log_path: &Path) -> PathBuf {
    let mut s = log_path.as_os_str().to_owned();
    s.push(".1");
    PathBuf::from(s)
}

/// If `log_path` is a regular file larger than `max_bytes`, rename it to
/// `{log_path}.1`, replacing any previous rotation. Exactly one generation is
/// kept.
///
/// Best-effort and infallible by design — this runs on the daemon startup
/// path, where a failed rotation must never stop the daemon from starting. In
/// particular, on Windows the rename fails while another process still holds
/// the file open (e.g. a previous daemon that has not exited yet); the log is
/// then simply left alone and rotated on a later spawn.
pub fn rotate_if_large(log_path: &Path, max_bytes: u64) {
    let Ok(meta) = std::fs::metadata(log_path) else {
        return;
    };
    if !meta.is_file() || meta.len() <= max_bytes {
        return;
    }
    let _ = std::fs::rename(log_path, rotated_path(log_path));
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- log_filter_directive ----------

    #[test]
    fn unset_or_blank_env_falls_back_to_info() {
        assert_eq!(log_filter_directive(None), "info");
        assert_eq!(log_filter_directive(Some("")), "info");
        assert_eq!(log_filter_directive(Some("   ")), "info");
    }

    #[test]
    fn a_set_env_value_wins_and_is_trimmed() {
        assert_eq!(log_filter_directive(Some("debug")), "debug");
        assert_eq!(
            log_filter_directive(Some("  ryuzi_core::plugins=trace,warn  ")),
            "ryuzi_core::plugins=trace,warn"
        );
    }

    // ---------- rotated_path ----------

    #[test]
    fn rotated_path_appends_dot_one_to_the_full_path() {
        assert_eq!(
            rotated_path(Path::new("/var/ryuzi/daemon.log")),
            PathBuf::from("/var/ryuzi/daemon.log.1")
        );
    }

    // ---------- rotate_if_large ----------

    #[test]
    fn rotates_when_the_log_is_over_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("daemon.log");
        std::fs::write(&log, vec![b'x'; 100]).unwrap();

        rotate_if_large(&log, 50);

        assert!(!log.exists(), "the oversized log must be moved aside");
        assert_eq!(
            std::fs::read(dir.path().join("daemon.log.1"))
                .unwrap()
                .len(),
            100
        );
    }

    #[test]
    fn leaves_a_log_at_or_under_the_cap_alone() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("daemon.log");
        std::fs::write(&log, vec![b'x'; 50]).unwrap();

        rotate_if_large(&log, 50);

        assert!(log.exists(), "a log exactly at the cap must not rotate");
        assert!(!dir.path().join("daemon.log.1").exists());
    }

    #[test]
    fn a_missing_log_is_a_silent_no_op() {
        let dir = tempfile::tempdir().unwrap();
        // Must not panic and must not create anything.
        rotate_if_large(&dir.path().join("daemon.log"), 1);
        assert!(!dir.path().join("daemon.log.1").exists());
    }

    #[test]
    fn a_second_rotation_replaces_the_previous_generation() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("daemon.log");
        let rotated = dir.path().join("daemon.log.1");
        std::fs::write(&rotated, b"older-generation").unwrap();
        std::fs::write(&log, vec![b'y'; 100]).unwrap();

        rotate_if_large(&log, 50);

        assert_eq!(std::fs::read(&rotated).unwrap(), vec![b'y'; 100]);
    }
}
