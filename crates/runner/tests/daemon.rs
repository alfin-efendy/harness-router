//! Process-level test for the daemon entry points (hidden `__daemon` and the
//! user-facing `start` alias, both dispatching to
//! `crates/runner/src/daemon_cmd.rs`): spawns the real compiled `ryuzi`
//! binary, waits for it to reach `daemon.json` state `"running"`, then
//! verifies a clean SIGTERM shutdown. Unix-only (SIGTERM/`libc::kill` via
//! `ryuzi_core::daemon_status::send_sigterm`).

#![cfg(unix)]

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ryuzi_core::daemon_status::{read_status, send_sigterm, DaemonFileState};
use ryuzi_core::settings::SettingsStore;
use ryuzi_core::Store;
use serial_test::serial;

fn lifecycle(entry: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let data_home = tmp.path().join("data");
    let home = tmp.path().to_path_buf();
    // I9 fix: this test spawns the REAL compiled `ryuzi` binary, so
    // `daemon::build_daemon`'s destructive v1->v2 first-upgrade plugin
    // migration runs with `cfg!(test)` FALSE (that guard only sees this
    // crate's own tests) — it always resolves
    // `plugins::bundle::installed_bundle_root()`. Redirecting `HOME` alone
    // is NOT sufficient on a Linux box that already exports
    // `XDG_CONFIG_HOME` in its environment: `dirs::config_dir()` honors that
    // var over `HOME`, so the migration sweep would still land on the
    // developer's real `~/.config/ryuzi/plugins` — exactly the class of bug
    // that already destroyed a real user's installed plugins once. Redirect
    // both explicitly so the child process never has a path to the real
    // config dir regardless of the host environment.
    let config_home = tmp.path().join("config");
    let plugins_root = tmp.path().join("plugins-root");

    // Redirect ryuzi_core::paths::state_dir() (and thus db_path()) into the
    // tempdir on both Linux (XDG_DATA_HOME) and macOS (HOME) — same
    // XDG_DATA_HOME/HOME redirection pattern used throughout this crate's
    // integration tests (see e.g. crates/runner/tests/e2e_journey.rs).
    std::env::set_var("XDG_DATA_HOME", &data_home);
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::set_var("RYUZI_PLUGINS_ROOT", &plugins_root);

    let db_path = ryuzi_core::paths::db_path();
    let data_dir = db_path
        .parent()
        .expect("db_path must have a parent dir")
        .to_path_buf();

    // Seed settings BEFORE spawning: a fresh db already boots zero-gateway
    // (Task 4 retired the `enabled_gateways` CSV — there is no seed left to
    // clear). The Store is opened and dropped here so the child owns the
    // only live handle.
    {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = Store::open(&db_path).await.unwrap();
            let settings = SettingsStore::new(Arc::new(store));
            settings.set("auto_update", "off").await.unwrap();
        });
    }

    let mut child = Command::new(assert_cmd::cargo::cargo_bin("ryuzi"))
        .arg(entry)
        .env("XDG_DATA_HOME", &data_home)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("RYUZI_PLUGINS_ROOT", &plugins_root)
        .stdin(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn `ryuzi {entry}`: {e}"));

    // Poll daemon.json until it reaches state "running" (≤10s).
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut running_status = None;
    while Instant::now() < deadline {
        if let Some(status) = read_status(&data_dir) {
            match status.state {
                DaemonFileState::Running => {
                    running_status = Some(status);
                    break;
                }
                DaemonFileState::Error => {
                    let _ = child.kill();
                    panic!("daemon reported an error status: {status:?}");
                }
                DaemonFileState::Connecting => {}
            }
        }
        if let Some(code) = child.try_wait().unwrap() {
            panic!("daemon process exited early with {code:?} before reaching running state");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = running_status.unwrap_or_else(|| {
        let _ = child.kill();
        panic!(
            "daemon.json never reached state \"running\" within 10s (path: {})",
            data_dir.join("daemon.json").display()
        );
    });

    let pid_ok = status.pid == child.id() as i32;
    let ver_ok = status.version.as_deref() == Some(env!("CARGO_PKG_VERSION"));
    if !pid_ok || !ver_ok {
        let _ = child.kill();
        let _ = child.wait();
        panic!("daemon status mismatch: pid_ok={pid_ok} ver_ok={ver_ok} status={status:?}");
    }

    send_sigterm(child.id() as i32);

    let deadline = Instant::now() + Duration::from_secs(10);
    let exit_status = loop {
        if let Some(exit_status) = child.try_wait().unwrap() {
            break exit_status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("daemon did not exit within 10s of SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    assert_eq!(exit_status.code(), Some(0));
    assert!(
        read_status(&data_dir).is_none(),
        "daemon.json must be removed after a clean SIGTERM shutdown"
    );
}

#[test]
#[serial]
fn daemon_entry_point_reaches_running_then_exits_cleanly_on_sigterm() {
    lifecycle("__daemon");
}

#[test]
#[serial]
fn start_entry_point_reaches_running_then_exits_cleanly_on_sigterm() {
    lifecycle("start");
}
