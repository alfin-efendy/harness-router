//! The ed25519 public key that signs first-party component-plugin *bundles*
//! (the `plugin.sig` envelope over `release.json`; see `plugins::bundle`). The
//! matching PRIVATE key is a release/CI secret generated and consumed by unit
//! 11b's signer (`scripts/plugins/build-first-party.ts`) and is never
//! committed.
//!
//! This mirrors `plugins::catalog_feed_key` exactly, one layer up: the catalog
//! *feed* key signs which integrations exist; this key signs each downloadable
//! component release before it is installed.
//!
//! # Live key
//! [`FIRST_PARTY_PUBKEY`] shipped live with the plugin-release activation —
//! 11b's signer now signs every first-party bundle with the matching private
//! key. The fail-closed empty-map guard below still protects zeroed forks and
//! dev builds that have not been given a real key. Rotation is overlap-based
//! via the `key_id` -> pubkey map: release N adds a second entry (e.g.
//! `first-party-2`) alongside this one, release N+1 switches CI signing (the
//! signer's `FIRST_PARTY_KEY_ID` + secret) to the new key, and a later
//! release drops the old entry. See
//! docs/development/plugins.md#release-pipeline--key-rollout for the full
//! rollout and compromise-response playbook.
//!
//! # Fail-closed on an all-zero key
//! The all-zero key is a valid *low-order* Edwards point, so a non-strict
//! verify could be tricked into accepting a forged signature against it.
//! [`first_party_trusted_keys`] therefore NEVER hands an all-zero key to
//! `verify_bundle`: whenever the constant is all-zero — a zeroed fork or a
//! dev build that has not been given a real key — it returns an EMPTY
//! trusted set, so every bundle fails the untrusted-key check and NOTHING
//! installs. (`verify_bundle` itself also uses `verify_strict`, which
//! rejects low-order keys, as a second line of defense — see
//! `plugins::bundle`.) The daemon's first-party bootstrap detects the empty
//! set and does nothing (no network, no retry state), so the engine still
//! ships without first-party bundles in that case.

//! # Local dev feed
//! A DEBUG build additionally honors [`DEV_PUBKEY_ENV`], which names a second
//! trusted key under the distinct id [`DEV_KEY_ID`] so a developer can install
//! from a locally built+signed feed without touching (or shadowing) the live
//! first-party key. See [`first_party_trusted_keys`] for the exact guard rails.

use std::collections::HashMap;

/// The `key_id` first-party `plugin.sig` envelopes name (see
/// `plugins::bundle`'s signature protocol). 11b's signer MUST emit this exact
/// id in every first-party bundle's `plugin.sig`.
pub const FIRST_PARTY_KEY_ID: &str = "first-party";

/// The `key_id` a LOCALLY signed dev bundle names. Deliberately distinct from
/// [`FIRST_PARTY_KEY_ID`]: a dev key must never be able to impersonate the
/// first-party signer, because the elevated grants (`allow_self_auth`,
/// `allow_gateway` — see `runtime::HostPolicy::for_installed_bundle`) are
/// derived from the verified key id being exactly `FIRST_PARTY_KEY_ID`. A
/// dev-signed bundle installs and runs, but does not get those grants.
pub const DEV_KEY_ID: &str = "dev";

/// Env var holding an ADDITIONAL bundle-signing public key (base64, 32 raw
/// bytes — exactly what `bun scripts/plugins/build-first-party.ts keygen`
/// prints as the dev pubkey), trusted under [`DEV_KEY_ID`].
pub const DEV_PUBKEY_ENV: &str = "RYUZI_DEV_PLUGIN_PUBKEY";

/// The first-party bundle-signing public key. Live as of the plugin-release
/// activation (see the module docs for rotation). Not a secret.
pub const FIRST_PARTY_PUBKEY: [u8; 32] = [
    114, 176, 153, 157, 229, 138, 150, 207, 30, 129, 201, 64, 150, 198, 74, 77, 208, 45, 133, 111,
    223, 225, 182, 83, 170, 95, 184, 138, 218, 249, 217, 9,
];

/// The trusted-key map passed to
/// [`crate::plugins::bundle::verify_bundle`] in production. Keyed by
/// [`FIRST_PARTY_KEY_ID`].
///
/// Empty only when [`FIRST_PARTY_PUBKEY`] is all-zero — a zeroed fork or a dev
/// build that hasn't been given the real key — the fail-closed property
/// described in the module docs: no bundle can be trusted until a real key is
/// compiled in. Most unit tests never call this directly; they inject their
/// own generated verifying key instead. The `verify-plugin-artifacts` CI bin
/// DOES call this directly, against the real compiled-in key, as its
/// production trust root.
pub fn first_party_trusted_keys() -> HashMap<String, [u8; 32]> {
    let mut map = HashMap::new();
    if FIRST_PARTY_PUBKEY != [0u8; 32] {
        map.insert(FIRST_PARTY_KEY_ID.to_string(), FIRST_PARTY_PUBKEY);
    }
    if let Some(dev) = dev_trusted_key() {
        map.insert(DEV_KEY_ID.to_string(), dev);
    }
    map
}

/// The debug-build-only dev signing key from [`DEV_PUBKEY_ENV`], if set and
/// well-formed.
///
/// Guard rails, all load-bearing:
/// - **Debug builds only.** A release build ignores the env var outright, so a
///   shipped binary's trust root is exactly the compiled-in key and no
///   environment can widen it.
/// - **Separate id.** Registered under [`DEV_KEY_ID`], never
///   [`FIRST_PARTY_KEY_ID`] — it is additive, cannot shadow the live key, and
///   cannot inherit the first-party-only capability grants.
/// - **Same fail-closed rule as the live key.** An all-zero key is a valid
///   low-order Edwards point and is refused here, exactly as the module docs
///   describe for [`FIRST_PARTY_PUBKEY`].
/// - **Malformed input is ignored, loudly.** A bad value never silently
///   becomes "no key at all"; it warns, so a typo'd base64 doesn't read as an
///   unexplained untrusted-key rejection later.
fn dev_trusted_key() -> Option<[u8; 32]> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let raw = std::env::var(DEV_PUBKEY_ENV).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    use base64::Engine as _;
    let decoded = match base64::engine::general_purpose::STANDARD.decode(raw) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!("{DEV_PUBKEY_ENV} is not valid base64, ignoring it: {error}");
            return None;
        }
    };
    let Ok(key) = <[u8; 32]>::try_from(decoded.as_slice()) else {
        tracing::warn!(
            "{DEV_PUBKEY_ENV} must decode to exactly 32 bytes, got {}; ignoring it",
            decoded.len()
        );
        return None;
    };
    if key == [0u8; 32] {
        tracing::warn!("{DEV_PUBKEY_ENV} is the all-zero key, which is never trusted; ignoring it");
        return None;
    }

    tracing::warn!(
        "trusting the dev plugin-signing key from {DEV_PUBKEY_ENV} under key id {DEV_KEY_ID:?} \
         (debug build only) — locally signed plugin bundles will install"
    );
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fail-closed guarantee, stated key-state-agnostically: the all-zero
    // placeholder must NEVER reach verify_bundle (empty trusted set), and a
    // real compiled-in key must be exposed under exactly FIRST_PARTY_KEY_ID —
    // so a silent revert to the placeholder can never make a forged bundle
    // installable, and a live key can never be mislabeled.
    //
    // Joins the `dev_plugin_pubkey` serial group: this asserts the EXACT map
    // size, and the dev-key tests below mutate the process-global env var that
    // can add a second entry. Without the guard the two would race.
    #[test]
    #[serial_test::serial(dev_plugin_pubkey)]
    fn trusted_keys_fail_closed_on_placeholder_and_expose_a_real_key() {
        let keys = first_party_trusted_keys();
        if FIRST_PARTY_PUBKEY == [0u8; 32] {
            assert!(
                keys.is_empty(),
                "the all-zero placeholder must never be handed to verify_bundle"
            );
        } else {
            assert_eq!(keys.len(), 1);
            assert_eq!(keys.get(FIRST_PARTY_KEY_ID), Some(&FIRST_PARTY_PUBKEY));
        }
    }

    // The key id the map WOULD use once a real key ships must match the id
    // 11b's signer emits in `plugin.sig`, so a live key resolves rather than
    // being rejected as unknown.
    #[test]
    fn key_id_is_first_party() {
        assert_eq!(FIRST_PARTY_KEY_ID, "first-party");
    }

    // ---------- dev feed key ----------
    //
    // These mutate a process-global env var, so they run under one #[serial]
    // group and always restore it.

    struct DevKeyEnv(Option<String>);

    impl DevKeyEnv {
        fn set(value: &str) -> Self {
            let previous = std::env::var(DEV_PUBKEY_ENV).ok();
            std::env::set_var(DEV_PUBKEY_ENV, value);
            Self(previous)
        }
    }

    impl Drop for DevKeyEnv {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var(DEV_PUBKEY_ENV, previous),
                None => std::env::remove_var(DEV_PUBKEY_ENV),
            }
        }
    }

    /// A non-zero, 32-byte key, base64 as the env var expects.
    fn dev_key_base64() -> (String, [u8; 32]) {
        use base64::Engine as _;
        let key = [7u8; 32];
        (base64::engine::general_purpose::STANDARD.encode(key), key)
    }

    // The whole point of the dev key: a locally signed bundle becomes
    // installable in a debug build WITHOUT displacing the live first-party key.
    #[test]
    #[serial_test::serial(dev_plugin_pubkey)]
    fn a_well_formed_dev_key_is_trusted_additively_in_debug_builds() {
        let (encoded, expected) = dev_key_base64();
        let _guard = DevKeyEnv::set(&encoded);

        let keys = first_party_trusted_keys();
        if cfg!(debug_assertions) {
            assert_eq!(keys.get(DEV_KEY_ID), Some(&expected));
            // Additive, never a shadow: the live key must still be there.
            if FIRST_PARTY_PUBKEY != [0u8; 32] {
                assert_eq!(keys.get(FIRST_PARTY_KEY_ID), Some(&FIRST_PARTY_PUBKEY));
            }
        } else {
            assert!(
                !keys.contains_key(DEV_KEY_ID),
                "a release build must ignore the dev key env var entirely"
            );
        }
    }

    // A dev key must never be able to pose as the first-party signer, because
    // the elevated grants (`allow_self_auth`, `allow_gateway`) key off exactly
    // that id.
    #[test]
    #[serial_test::serial(dev_plugin_pubkey)]
    fn the_dev_key_never_registers_under_the_first_party_id() {
        let (encoded, expected) = dev_key_base64();
        let _guard = DevKeyEnv::set(&encoded);

        let keys = first_party_trusted_keys();
        assert_ne!(
            keys.get(FIRST_PARTY_KEY_ID),
            Some(&expected),
            "the dev key must not occupy the first-party slot"
        );
    }

    // Same fail-closed rule the live key has: the all-zero low-order point is
    // never trusted, however it arrives.
    #[test]
    #[serial_test::serial(dev_plugin_pubkey)]
    fn an_all_zero_dev_key_is_refused() {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        let _guard = DevKeyEnv::set(&encoded);

        assert!(!first_party_trusted_keys().contains_key(DEV_KEY_ID));
    }

    // A malformed value must be ignored, not panic and not half-register.
    #[test]
    #[serial_test::serial(dev_plugin_pubkey)]
    fn a_malformed_dev_key_is_ignored() {
        use base64::Engine as _;
        for bad in [
            "not base64!!".to_string(),
            // Valid base64, wrong length.
            base64::engine::general_purpose::STANDARD.encode([1u8; 16]),
            "   ".to_string(),
        ] {
            let _guard = DevKeyEnv::set(&bad);
            assert!(
                !first_party_trusted_keys().contains_key(DEV_KEY_ID),
                "{bad:?} must not be trusted"
            );
        }
    }
}
