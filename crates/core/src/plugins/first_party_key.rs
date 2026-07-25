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

use std::collections::HashMap;

/// The `key_id` first-party `plugin.sig` envelopes name (see
/// `plugins::bundle`'s signature protocol). 11b's signer MUST emit this exact
/// id in every first-party bundle's `plugin.sig`.
pub const FIRST_PARTY_KEY_ID: &str = "first-party";

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
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fail-closed guarantee, stated key-state-agnostically: the all-zero
    // placeholder must NEVER reach verify_bundle (empty trusted set), and a
    // real compiled-in key must be exposed under exactly FIRST_PARTY_KEY_ID —
    // so a silent revert to the placeholder can never make a forged bundle
    // installable, and a live key can never be mislabeled.
    #[test]
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
}
