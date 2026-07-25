//! Verify a directory of published (signer-layout) component-release
//! artifacts through the SAME code path a client install uses.
//!
//! The signer (`scripts/plugins/build-first-party.ts`) publishes, per
//! component and stem, `<stem>.ryuzi-plugin.toml`, `<stem>.release.json`,
//! `<stem>.release.json.sig`, plus ONE `<manifest.component>` wasm shared by
//! both stems. This module restages each `<stem>.release.json` set into the
//! `verify_bundle` layout (`ryuzi-plugin.toml` / `release.json` /
//! `plugin.sig` / the component) in a temp dir and runs
//! [`crate::plugins::bundle::verify_bundle`] over it — so CI's post-build and
//! post-upload checks prove exactly what a client would accept, byte for
//! byte. Consumed by the `verify-plugin-artifacts` bin (CI's release
//! rehearsal and post-upload gates).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use ryuzi_plugin_sdk::{PluginBundleManifest, PluginRelease};

/// One verified stem's summary, for the caller to log/assert on.
#[derive(Debug, Clone)]
pub struct VerifiedArtifact {
    pub stem: String,
    pub id: String,
    pub version: String,
    pub signing_key_id: String,
}

/// Restage and verify EVERY `<stem>.release.json` set in `dir`. Hard-fails on
/// the first bad set (missing file, hash/signature mismatch, untrusted key),
/// on an EMPTY `trusted_keys` (the all-zero placeholder — CI must never
/// report a success it did not prove), and on a dir containing no artifact
/// sets at all. When `expect_base` is given, each release's `component_url`
/// must be exactly `<expect_base>/<manifest.component>` — the URL a client
/// derives, so a mis-based release job fails here, not in the field.
pub fn verify_artifacts_dir(
    dir: &Path,
    trusted_keys: &HashMap<String, [u8; 32]>,
    expect_base: Option<&str>,
) -> Result<Vec<VerifiedArtifact>> {
    if trusted_keys.is_empty() {
        bail!(
            "trusted-key set is empty (all-zero FIRST_PARTY_PUBKEY placeholder?) — \
             nothing can verify; refusing to report success"
        );
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading artifacts dir {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    let mut verified = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".release.json") else {
            continue;
        };
        let read = |suffix: &str| -> Result<Vec<u8>> {
            let path = dir.join(format!("{stem}{suffix}"));
            std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
        };
        let manifest_bytes = read(".ryuzi-plugin.toml")?;
        let release_bytes = read(".release.json")?;
        let sig_bytes = read(".release.json.sig")?;

        let manifest = PluginBundleManifest::from_toml(
            std::str::from_utf8(&manifest_bytes)
                .with_context(|| format!("{stem}: manifest is not UTF-8"))?,
        )
        .with_context(|| format!("{stem}: parsing manifest"))?;
        let release = PluginRelease::from_json(&release_bytes)
            .with_context(|| format!("{stem}: parsing release.json"))?;
        if let Some(base) = expect_base {
            let expected = format!("{}/{}", base.trim_end_matches('/'), manifest.component);
            if release.component_url != expected {
                bail!(
                    "{stem}: component_url {:?} != expected {expected:?}",
                    release.component_url
                );
            }
        }

        // Same pre-write guard the install pipeline runs: the component name
        // must stay inside the restage dir before anything is copied there.
        super::remote_catalog::sanitize_staged_component(&manifest.component)
            .with_context(|| format!("{stem}: component filename"))?;
        let staged = tempfile::tempdir().context("creating restage dir")?;
        std::fs::write(staged.path().join("ryuzi-plugin.toml"), &manifest_bytes)?;
        std::fs::write(staged.path().join("release.json"), &release_bytes)?;
        std::fs::write(staged.path().join("plugin.sig"), &sig_bytes)?;
        let wasm_src = dir.join(&manifest.component);
        std::fs::copy(&wasm_src, staged.path().join(&manifest.component))
            .with_context(|| format!("{stem}: copying {}", wasm_src.display()))?;

        let bundle = crate::plugins::bundle::verify_bundle(staged.path(), trusted_keys)
            .with_context(|| format!("{stem}: verify_bundle"))?;
        verified.push(VerifiedArtifact {
            stem: stem.to_string(),
            id: bundle.release.id.clone(),
            version: bundle.release.version.clone(),
            signing_key_id: bundle.signing_key_id.clone(),
        });
    }
    if verified.is_empty() {
        bail!("no *.release.json artifact sets found in {}", dir.display());
    }
    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::Digest;

    const KEY_ID: &str = "first-party";

    fn test_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn trusted(key: &SigningKey) -> HashMap<String, [u8; 32]> {
        let mut map = HashMap::new();
        map.insert(KEY_ID.to_string(), key.verifying_key().to_bytes());
        map
    }

    /// Write one signer-layout artifact set into `dir` under `stem`, signed by
    /// `key`. Mirrors the real signer's output byte-for-byte semantics: the
    /// signature is over release.json's exact bytes.
    fn write_artifact_set(dir: &Path, stem: &str, id: &str, version: &str, key: &SigningKey) {
        let component = format!("{id}.wasm");
        let wasm = b"\0asm test bytes".to_vec();
        let sha = format!("{:x}", sha2::Sha256::digest(&wasm));
        let manifest = format!(
            "id = \"{id}\"\nname = \"Test\"\nversion = \"{version}\"\n\
             wit-api = \">=0.1.0, <0.2.0\"\nlifecycle = \"per-call\"\n\
             component = \"{component}\"\npublisher = \"Ryuzi\"\ndescription = \"test\"\n"
        );
        let release = format!(
            "{{\n  \"id\": \"{id}\",\n  \"version\": \"{version}\",\n  \"wit-api\": \"0.1.0\",\n  \
             \"component_url\": \"https://github.com/alfin-efendy/ryuzi/releases/download/v9.9.9/{component}\",\n  \
             \"component_sha256\": \"{sha}\"\n}}\n"
        );
        let sig = key.sign(release.as_bytes());
        let envelope = format!(
            "{{\"key_id\":\"{KEY_ID}\",\"signature\":\"{}\"}}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes())
        );
        std::fs::write(dir.join(format!("{stem}.ryuzi-plugin.toml")), &manifest).unwrap();
        std::fs::write(dir.join(format!("{stem}.release.json")), &release).unwrap();
        std::fs::write(dir.join(format!("{stem}.release.json.sig")), &envelope).unwrap();
        std::fs::write(dir.join(component), &wasm).unwrap();
    }

    // Both stems of a dual-stem publish verify independently — the pinned
    // aliases are byte-identical, so one shared wasm satisfies both.
    #[test]
    fn verifies_every_stem_in_the_dir() {
        let key = test_key();
        let dir = tempfile::tempdir().unwrap();
        write_artifact_set(dir.path(), "github", "github", "0.1.1", &key);
        write_artifact_set(dir.path(), "github-0.1.1", "github", "0.1.1", &key);
        let verified = verify_artifacts_dir(dir.path(), &trusted(&key), None).unwrap();
        assert_eq!(verified.len(), 2);
        assert!(verified
            .iter()
            .all(|v| v.id == "github" && v.version == "0.1.1"));
        assert!(verified.iter().all(|v| v.signing_key_id == KEY_ID));
    }

    // The placeholder guard: with no trusted key, refuse loudly instead of
    // vacuously succeeding — CI must never green-light unverified artifacts.
    #[test]
    fn empty_trusted_keys_is_a_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = verify_artifacts_dir(dir.path(), &HashMap::new(), None).unwrap_err();
        assert!(err.to_string().contains("trusted-key set is empty"));
    }

    // A tampered wasm must fail the hash check through the production path.
    #[test]
    fn tampered_component_fails() {
        let key = test_key();
        let dir = tempfile::tempdir().unwrap();
        write_artifact_set(dir.path(), "github", "github", "0.1.1", &key);
        std::fs::write(dir.path().join("github.wasm"), b"evil").unwrap();
        assert!(verify_artifacts_dir(dir.path(), &trusted(&key), None).is_err());
    }

    // `--expect-base` pins component_url to the URL a client will derive.
    #[test]
    fn expect_base_mismatch_fails() {
        let key = test_key();
        let dir = tempfile::tempdir().unwrap();
        write_artifact_set(dir.path(), "github", "github", "0.1.1", &key);
        let err = verify_artifacts_dir(
            dir.path(),
            &trusted(&key),
            Some("https://github.com/alfin-efendy/ryuzi/releases/download/v0.0.1"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("component_url"));
    }

    // An empty dir is a failure, not a vacuous pass.
    #[test]
    fn empty_dir_fails() {
        let key = test_key();
        let dir = tempfile::tempdir().unwrap();
        assert!(verify_artifacts_dir(dir.path(), &trusted(&key), None).is_err());
    }
}
