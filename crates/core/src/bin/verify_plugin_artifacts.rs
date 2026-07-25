//! CI verifier for published first-party component-release artifacts.
//!
//! Usage: `verify-plugin-artifacts <artifacts-dir> [--expect-base <url>]`
//!
//! Verifies every `<stem>.release.json` set in the dir against the
//! COMPILED-IN first-party trusted keys through the production
//! `verify_bundle` path (see `plugins::artifact_verify`). Exits non-zero on
//! any failure, on an empty dir, or while the all-zero placeholder key is
//! compiled in — CI must never report a success it did not prove. Used by
//! `plugins-dry-run.yml` (rehearsal) and `release.yml`'s `plugins` job
//! (pre-upload and post-upload gates).

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next().map(PathBuf::from) else {
        eprintln!("usage: verify-plugin-artifacts <artifacts-dir> [--expect-base <url>]");
        return ExitCode::FAILURE;
    };
    let mut expect_base = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--expect-base" => {
                let Some(value) = args.next() else {
                    eprintln!("--expect-base requires a value");
                    return ExitCode::FAILURE;
                };
                expect_base = Some(value);
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let trusted = ryuzi_core::plugins::first_party_key::first_party_trusted_keys();
    match ryuzi_core::plugins::artifact_verify::verify_artifacts_dir(
        &dir,
        &trusted,
        expect_base.as_deref(),
    ) {
        Ok(verified) => {
            for artifact in &verified {
                println!(
                    "verified {} ({} {}) signed by {}",
                    artifact.stem, artifact.id, artifact.version, artifact.signing_key_id
                );
            }
            println!("OK: {} artifact set(s) verified", verified.len());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("FAILED: {error:#}");
            ExitCode::FAILURE
        }
    }
}
