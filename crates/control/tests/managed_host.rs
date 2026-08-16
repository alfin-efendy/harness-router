//! Cross-process proof of the `RYUZI_MANAGED_HOST` contract: Cockpit sets it
//! on the control plane it spawns (`apps/cockpit/src-tauri/src/engine.rs`),
//! and the control plane must answer by skipping its self-updater
//! (`daemon_cmd::run_daemon`, gated by `daemon_cmd::self_update_enabled`).
//!
//! The unit tests next to `self_update_enabled` prove the *decision*. They
//! cannot prove the *wiring*, which is the half that fails silently: if the
//! variable never reaches the child — a typo on either side, an env block the
//! spawn drops — the daemon simply keeps its updater and everything still
//! looks healthy, right up until a Cockpit bundle and the control plane inside
//! it self-update apart from each other. So these tests spawn the real
//! compiled `ryuzi` binary and read what it actually printed.
//!
//! Deliberately NOT in `daemon.rs`, which is `#![cfg(unix)]` because it
//! asserts SIGTERM shutdown semantics. Nothing here is unix-specific, and
//! Windows is where an env-var-crosses-`CreateProcess` bug is least likely to
//! be caught by anything else — Cockpit ships there, and
//! `cargo test -p ryuzi-cockpit` cannot even run on that platform
//! (tauri#13419).

use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ryuzi_core::settings::SettingsStore;
use ryuzi_core::Store;
use serial_test::serial;

/// Verbatim from `daemon_cmd::run_daemon`'s managed-host branch.
const MANAGED_LINE: &str = "daemon: self-update disabled (managed host)";

/// Printed by `run_daemon` *after* it has already taken the managed/unmanaged
/// branch, so once it appears the captured output is complete for both
/// directions of the assertion — which is what makes asserting an *absence*
/// meaningful rather than a race against a slow boot.
const READY_LINE: &str = "daemon: running";

/// Spawn the real compiled `ryuzi start` in a hermetic sandbox, with
/// `RYUZI_MANAGED_HOST=1` iff `managed`, wait until it reports itself running,
/// tear it down, and return everything it wrote to stdout/stderr.
///
/// Every assertion is left to the caller so a failing one cannot leak a live
/// daemon: by the time this returns, the child is dead.
fn boot_then_capture_output(managed: bool) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let data_home = tmp.path().join("data");
    let home = tmp.path().to_path_buf();
    let config_home = tmp.path().join("config");
    let plugins_root = tmp.path().join("plugins-root");
    let state_dir = data_home.join("ryuzi");
    let config_dir = config_home.join("ryuzi");

    // The same six-variable sandbox `daemon.rs` builds — read its comments
    // before touching this. Short version: `XDG_*`/`HOME` are honored by
    // `dirs` on Linux/macOS only, so `RYUZI_STATE_DIR`/`RYUZI_CONFIG_DIR`/
    // `RYUZI_PLUGINS_ROOT` are the seams that make the sandbox real on
    // Windows — which this file, unlike `daemon.rs`, actually runs on. Without
    // them a spawned daemon migrates the developer's live
    // `%APPDATA%\ryuzi\ryuzi.sqlite` and sweeps their installed plugins.
    std::env::set_var("XDG_DATA_HOME", &data_home);
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::set_var("RYUZI_STATE_DIR", &state_dir);
    std::env::set_var("RYUZI_CONFIG_DIR", &config_dir);
    std::env::set_var("RYUZI_PLUGINS_ROOT", &plugins_root);

    let db_path = ryuzi_core::paths::db_path();

    // Seeded before the spawn, and the handle dropped, so the child owns the
    // only live one.
    //
    // `control_port` is deliberately NOT seeded (it is not a writable schema
    // setting anyway): the daemon takes the 4483 default, and `serve` already
    // falls back to an ephemeral port when that one is busy — so a developer's
    // real daemon sitting on 4483 costs this test nothing. The `DaemonLock`
    // that could actually collide is per state dir, and this one is a tempdir.
    {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = Store::open(&db_path).await.unwrap();
            let settings = SettingsStore::new(Arc::new(store));
            // The unmanaged run builds a real `UpdateManager`; it must not
            // then go poll a release feed over the network. What is under
            // test is the missing log line, not an update round trip.
            settings.set("auto_update", "off").await.unwrap();
        });
    }

    // A file rather than a pipe: nothing has to keep draining it, so the poll
    // loop below can read the daemon's output while it is still running
    // without risking a full-pipe deadlock. `println!` writes through a
    // `LineWriter`, so every line is flushed as it is produced.
    let log_path = tmp.path().join("daemon.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let log2 = log.try_clone().unwrap();

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("ryuzi"));
    cmd.arg("start")
        .env("XDG_DATA_HOME", &data_home)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("RYUZI_STATE_DIR", &state_dir)
        .env("RYUZI_CONFIG_DIR", &config_dir)
        .env("RYUZI_PLUGINS_ROOT", &plugins_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log2));
    if managed {
        // Byte-for-byte what `engine::spawn_control_plane` sets on its spawn.
        cmd.env("RYUZI_MANAGED_HOST", "1");
    } else {
        // Never inherit it from whatever ran the test suite: the negative
        // assertion is only worth anything if the variable is genuinely unset.
        cmd.env_remove("RYUZI_MANAGED_HOST");
    }
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn `ryuzi start`: {e}"));

    let deadline = Instant::now() + Duration::from_secs(60);
    let output = loop {
        // A partial read (the child may be mid-line) is not an error worth
        // reporting — the next poll sees the rest.
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        if text.contains(READY_LINE) {
            break text;
        }
        if let Some(code) = child.try_wait().unwrap() {
            panic!("`ryuzi start` exited with {code:?} before reporting ready.\n--- output ---\n{text}");
        }
        if Instant::now() > deadline {
            kill_and_reap(&mut child);
            panic!("`ryuzi start` never reported ready within 60s.\n--- output ---\n{text}");
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    kill_and_reap(&mut child);
    output
}

/// Stop the spawned daemon. SIGTERM where it exists — `daemon.rs` already
/// covers that graceful path end-to-end, so all this has to guarantee is that
/// no test leaves a daemon running against a tempdir.
fn kill_and_reap(child: &mut Child) {
    #[cfg(unix)]
    {
        ryuzi_core::daemon_status::send_sigterm(child.id() as i32);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The payload assertion: `RYUZI_MANAGED_HOST=1` set by the *parent* reaches
/// the *child* and turns the self-updater off there. This is the only
/// automated check that the two sides of that string are wired to each other
/// at all.
#[test]
#[serial]
fn a_managed_child_disables_its_self_updater() {
    let output = boot_then_capture_output(true);
    assert!(
        output.contains(MANAGED_LINE),
        "a daemon spawned with RYUZI_MANAGED_HOST=1 must report {MANAGED_LINE:?}.\n--- output ---\n{output}"
    );
}

/// The other half. Without it, an unconditional `println!` in `run_daemon`
/// would satisfy the test above while the updater kept running — the exact
/// drift `RYUZI_MANAGED_HOST` exists to prevent, passing its own guard.
#[test]
#[serial]
fn an_unmanaged_child_keeps_its_self_updater() {
    let output = boot_then_capture_output(false);
    assert!(
        !output.contains(MANAGED_LINE),
        "a daemon spawned without RYUZI_MANAGED_HOST must NOT report {MANAGED_LINE:?}.\n--- output ---\n{output}"
    );
}
