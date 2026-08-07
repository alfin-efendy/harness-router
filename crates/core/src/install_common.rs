//! Shared filesystem primitives for the two arbitrary-source install
//! pipelines that stage untrusted content before it's admitted onto disk:
//! `skills_install` (git-backed skill packs) and `plugins::install_sources`
//! (local-folder / git-URL plugin installs, Task 11). Both need "clone a git
//! remote into a temp dir" and "copy a local directory tree into a temp dir
//! (stripping `.git`)" — this module owns those two primitives exactly once
//! so neither caller re-implements them.

use anyhow::{bail, Context, Result};
use std::path::Path;

/// Shallow-clone `url` into `dest` (which must not already exist) and
/// resolve the checked-out commit's SHA. Returns `None` for the commit only
/// if `git rev-parse HEAD` itself fails after a successful clone — tolerated
/// rather than treated as fatal, since the clone succeeding is what matters
/// to every caller.
pub(crate) async fn git_clone_repo(url: &str, dest: &Path) -> Result<Option<String>> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("clone").arg("--depth").arg("1").arg(url).arg(dest);
    crate::process_util::no_window(&mut cmd);
    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to spawn git clone for {url}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            bail!("git clone failed for {url}");
        }
        bail!("git clone failed for {url}: {stderr}");
    }
    // The clone still has `.git` at this point; callers that don't want it
    // (e.g. `copy_dir_recursive`-based staging) strip it themselves. Resolve
    // HEAD now while it's still available.
    let head = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dest)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(head)
}

/// Recursively copy `source` into `dest` (creating `dest` and any missing
/// parents), skipping `.git` entries and any symlink/special file — plain
/// files and directories only. Used to stage a local plugin/skill folder
/// into a temp working copy so the ORIGINAL user directory is never mutated
/// or moved.
pub(crate) fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy() == ".git" {
            continue;
        }
        let source_path = entry.path();
        let dest_path = dest.join(&name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source_path, &dest_path)?;
        }
    }
    Ok(())
}
