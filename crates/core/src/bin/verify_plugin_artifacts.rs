//! CI verifier for published plugin-release artifacts: first-party component
//! BUNDLES (dir mode) and the remote CATALOG FEED (`catalog` mode).
//!
//! Usage:
//!   `verify-plugin-artifacts <artifacts-dir> [--expect-base <url>]`
//!   `verify-plugin-artifacts catalog <catalog.json> <catalog.json.sig>`
//!
//! Dir mode verifies every `<stem>.release.json` set in the dir against the
//! COMPILED-IN first-party trusted keys through the production
//! `verify_bundle` path (see `plugins::artifact_verify`). Exits non-zero on
//! any failure, on an empty dir, or while the all-zero placeholder key is
//! compiled in — CI must never report a success it did not prove.
//!
//! `catalog` mode verifies a built `catalog.json` + raw detached
//! `catalog.json.sig` against the COMPILED-IN `CATALOG_FEED_PUBKEY` (see
//! `plugins::remote_catalog::verify_catalog_feed_signature`) — proving the
//! `CATALOG_FEED_PRIVATE_KEY` CI secret actually pairs with the pubkey
//! compiled into this build, since the catalog feed is the revocation
//! channel and a broken one must fail the release, not ship silently
//! unverifiable.
//!
//! Used by `plugins-dry-run.yml` (rehearsal) and `release.yml`'s `plugins`
//! and `catalog-feed` jobs (pre-upload and post-upload gates).

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        print_usage();
        return ExitCode::FAILURE;
    };

    if first == "catalog" {
        return run_catalog_mode(args);
    }

    run_dir_mode(PathBuf::from(first), args)
}

fn print_usage() {
    eprintln!("usage: verify-plugin-artifacts <artifacts-dir> [--expect-base <url>]");
    eprintln!("       verify-plugin-artifacts catalog <catalog.json> <catalog.json.sig>");
}

fn run_catalog_mode(mut args: impl Iterator<Item = String>) -> ExitCode {
    let (Some(json_path), Some(sig_path)) = (args.next(), args.next()) else {
        eprintln!("usage: verify-plugin-artifacts catalog <catalog.json> <catalog.json.sig>");
        return ExitCode::FAILURE;
    };

    let feed_bytes = match std::fs::read(&json_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("FAILED: reading {json_path}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let sig_bytes = match std::fs::read(&sig_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("FAILED: reading {sig_path}: {error}");
            return ExitCode::FAILURE;
        }
    };

    if ryuzi_core::plugins::remote_catalog::verify_catalog_feed_signature(&feed_bytes, &sig_bytes) {
        println!("OK: catalog feed signature verifies against the compiled-in CATALOG_FEED_PUBKEY");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "FAILED: catalog feed signature does NOT verify against the compiled-in \
             CATALOG_FEED_PUBKEY ({json_path} / {sig_path}) — the CATALOG_FEED_PRIVATE_KEY \
             secret does not match the pubkey compiled into crates/core/src/plugins/catalog_feed_key.rs"
        );
        ExitCode::FAILURE
    }
}

fn run_dir_mode(dir: PathBuf, mut args: impl Iterator<Item = String>) -> ExitCode {
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
