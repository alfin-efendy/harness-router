//! End-to-end journey over the compiled `ryuzi` binary: config persistence
//! and the daemon lifecycle share one isolated HOME, the way a real install
//! does. Complements the narrower per-command tests (cli.rs, config.rs,
//! daemon.rs) — this is the cross-command regression net.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use predicates::prelude::*;
use ryuzi_core::daemon_status::{read_status, send_sigterm, DaemonFileState};
use ryuzi_core::settings::SettingsStore;
use ryuzi_core::Store;
use serial_test::serial;

/// Every path the spawned `ryuzi` binary is allowed to write to, derived from
/// one tempdir root.
///
/// `state_dir`/`config_dir` are deliberately spelled as the exact paths
/// `XDG_DATA_HOME`/`XDG_CONFIG_HOME` resolve to, so pointing
/// `RYUZI_STATE_DIR`/`RYUZI_CONFIG_DIR` at them changes nothing on Linux while
/// making the sandbox real on platforms where `dirs` ignores the environment.
struct Sandbox {
    data_home: PathBuf,
    home: PathBuf,
    config_home: PathBuf,
    state_dir: PathBuf,
    config_dir: PathBuf,
    plugins_root: PathBuf,
}

impl Sandbox {
    fn new(root: &Path) -> Self {
        let data_home = root.join("data");
        let config_home = root.join("config");
        Self {
            state_dir: data_home.join("ryuzi"),
            config_dir: config_home.join("ryuzi"),
            data_home,
            home: root.to_path_buf(),
            config_home,
            plugins_root: root.join("plugins-root"),
        }
    }

    /// Redirects this *test* process's own `ryuzi_core::paths` lookups (used
    /// below to compute `db_path()` for the pre-spawn settings seed) at the
    /// sandbox. Process-global, hence the `#[serial]` on the test.
    fn export(&self) {
        for (key, value) in self.vars() {
            std::env::set_var(key, value);
        }
    }

    fn vars(&self) -> [(&'static str, &Path); 6] {
        [
            ("XDG_DATA_HOME", self.data_home.as_path()),
            ("HOME", self.home.as_path()),
            ("XDG_CONFIG_HOME", self.config_home.as_path()),
            // `XDG_*`/`HOME` above only work where `dirs` consults the
            // environment (Linux, and `HOME` on macOS). On Windows
            // `dirs::data_dir()`/`dirs::config_dir()` go straight to the
            // `FOLDERID_RoamingAppData` known-folder API and ignore all of
            // them, which is how this crate's tests came to open and migrate a
            // developer's live `%APPDATA%\ryuzi\ryuzi.sqlite`. These two are
            // honored by `ryuzi_core::paths` itself — a runtime check, not
            // `cfg(test)`, so it survives into the real binary spawned below.
            ("RYUZI_STATE_DIR", self.state_dir.as_path()),
            ("RYUZI_CONFIG_DIR", self.config_dir.as_path()),
            // I9 fix: `__daemon` runs `daemon::build_daemon`'s destructive
            // v1->v2 plugin migration with `cfg!(test)` FALSE (it spawns the
            // real compiled binary), so it always resolves
            // `plugins::bundle::installed_bundle_root()` — which has its own
            // seam. See that function's doc.
            ("RYUZI_PLUGINS_ROOT", self.plugins_root.as_path()),
        ]
    }
}

fn bin(sandbox: &Sandbox) -> Command {
    let mut c = Command::cargo_bin("ryuzi").unwrap();
    for (key, value) in sandbox.vars() {
        c.env(key, value);
    }
    c
}

#[test]
#[serial]
fn config_survives_a_full_daemon_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox = Sandbox::new(tmp.path());

    // 1. config set/get round-trips through the real binary.
    bin(&sandbox)
        .args(["config", "set", "default_effort", "high"])
        .assert()
        .success();
    bin(&sandbox)
        .args(["config", "get", "default_effort"])
        .assert()
        .success()
        .stdout(predicate::str::contains("high"));

    // 2. Seed settings so the daemon never touches the network (same seeding
    //    as crates/runner/tests/daemon.rs). A fresh db already boots
    //    zero-gateway — Task 4 retired the `enabled_gateways` CSV, so there
    //    is no longer a seed to clear.
    sandbox.export();
    let db_path = ryuzi_core::paths::db_path();
    let data_dir = db_path.parent().unwrap().to_path_buf();
    {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = Store::open(&db_path).await.unwrap();
            let settings = SettingsStore::new(Arc::new(store));
            settings.set("auto_update", "off").await.unwrap();
        });
    }

    // 3. Daemon reaches running, then exits cleanly on SIGTERM.
    let mut spawn = std::process::Command::new(assert_cmd::cargo::cargo_bin("ryuzi"));
    spawn.arg("__daemon");
    for (key, value) in sandbox.vars() {
        spawn.env(key, value);
    }
    let mut child = spawn
        .stdin(Stdio::null())
        .spawn()
        .expect("failed to spawn `ryuzi __daemon`");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = read_status(&data_dir) {
            if matches!(status.state, DaemonFileState::Running) {
                break;
            }
        }
        if let Some(code) = child.try_wait().unwrap() {
            panic!("daemon exited early with {code:?}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("daemon never reached state \"running\" within 10s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    send_sigterm(child.id() as i32);
    let deadline = Instant::now() + Duration::from_secs(10);
    let exit = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("daemon did not exit within 10s of SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(exit.code(), Some(0));
    assert!(
        read_status(&data_dir).is_none(),
        "daemon.json must be removed after a clean shutdown"
    );

    // 4. The setting written before the daemon run is still there after it.
    bin(&sandbox)
        .args(["config", "get", "default_effort"])
        .assert()
        .success()
        .stdout(predicate::str::contains("high"));
}
