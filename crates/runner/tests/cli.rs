use assert_cmd::Command;
use predicates::prelude::*;

/// Builds a `ryuzi` command whose state and config trees are redirected into
/// `tmp`. **Every** spawned `ryuzi` in this file must go through here, even
/// the ones that only print `--version` or `--help`: `crates/runner/src/
/// main.rs` resolves `ryuzi_core::paths::db_path()` for every invocation, and
/// it is far too easy for a future subcommand to start touching it.
///
/// `XDG_DATA_HOME`/`HOME` remain because `dirs::data_dir()` honors them on
/// Linux and macOS. `RYUZI_STATE_DIR`/`RYUZI_CONFIG_DIR` are the
/// platform-independent seams (see `ryuzi_core::paths::state_dir`) and are the
/// ONLY thing that works on Windows, where `dirs` resolves
/// `FOLDERID_RoamingAppData` through the known-folder API and ignores the
/// environment entirely — without them this test file ran schema migrations
/// against the developer's live `%APPDATA%\ryuzi\ryuzi.sqlite`.
fn sandboxed(tmp: &tempfile::TempDir) -> Command {
    let mut cmd = Command::cargo_bin("ryuzi").unwrap();
    cmd.env("RYUZI_STATE_DIR", tmp.path().join("state"))
        .env("RYUZI_CONFIG_DIR", tmp.path().join("config"))
        .env("RYUZI_PLUGINS_ROOT", tmp.path().join("plugins"))
        .env("XDG_DATA_HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("xdg-config"))
        .env("HOME", tmp.path());
    cmd
}

#[test]
fn version_flag_prints_bare_semver() {
    let tmp = tempfile::tempdir().unwrap();
    sandboxed(&tmp)
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"^\d+\.\d+\.\d+\n$").unwrap());
    sandboxed(&tmp).arg("-v").assert().success();
}

#[test]
fn unknown_command_exits_1_with_hint() {
    let tmp = tempfile::tempdir().unwrap();
    sandboxed(&tmp)
        .arg("bogus")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "unknown command: bogus - run `ryuzi --help`",
        ));
}

#[test]
// No-args always prints help: the TUI was removed with the CLI product.
fn help_flag_and_bare_help_and_no_args_print_usage() {
    let tmp = tempfile::tempdir().unwrap();
    for args in [vec!["--help"], vec!["-h"], vec!["help"], vec![]] {
        sandboxed(&tmp)
            .args(&args)
            .assert()
            .success()
            .stdout(predicate::str::contains("USAGE").and(predicate::str::contains("doctor")));
    }
}

#[test]
fn doctor_prints_three_report_lines() {
    let tmp = tempfile::tempdir().unwrap();
    // `doctor` is the command in this file that actually opens (and migrates)
    // the database at `paths::db_path()`, so the sandbox above is load-bearing
    // here rather than merely defensive.
    let assert = sandboxed(&tmp).arg("doctor").assert();
    let output = assert.get_output().clone();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "doctor must print exactly 3 lines, got: {stdout}"
    );
    assert!(lines[0].starts_with("git:    "), "line 1: {}", lines[0]);
    assert!(lines[1].starts_with("settings: "), "line 2: {}", lines[1]);
    assert!(lines[2].starts_with("doctor: "), "line 3: {}", lines[2]);
    // Exit code must agree with the verdict line (environment-tolerant:
    // a fresh DB always has missing settings, so FAIL is expected here).
    let code = output.status.code().unwrap();
    if lines[2] == "doctor: PASS" {
        assert_eq!(code, 0);
    } else {
        assert_eq!(code, 1);
    }
}

#[test]
fn help_lists_the_start_command() {
    let tmp = tempfile::tempdir().unwrap();
    sandboxed(&tmp)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("start"));
}
