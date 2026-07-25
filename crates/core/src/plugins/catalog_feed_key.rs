//! The ed25519 public key that signs the remote catalog feed. The matching
//! PRIVATE key is a release/CI secret (`CATALOG_FEED_PRIVATE_KEY`, consumed
//! by `scripts/catalog/build-feed.ts`) and is never committed.
//!
//! Live as of the plugin-release activation: `build-feed.ts` now signs the
//! published feed with the matching private key, and `verify_with`
//! (`crates/core/src/plugins/remote_catalog.rs`) verifies fetched feeds
//! against this compiled-in public key. The all-zero key is a valid
//! *low-order* Edwards point, so a non-strict verify could be tricked into
//! accepting a forged signature against it; `verify_with` still rejects an
//! all-zero key two ways — an explicit all-zero guard AND `verify_strict`
//! (which rejects low-order keys) — so a zeroed fork or a dev build that
//! hasn't been given a real key stays fail-closed (EVERY feed is rejected,
//! and the engine still ships and enables the embedded catalog either way).
//!
//! Rotation is overlap-based, on the same release-N/release-N+1 cadence as
//! the first-party bundle key. See
//! docs/development/plugins.md#release-pipeline--key-rollout for the full
//! rollout and compromise-response playbook (and
//! docs/development/plugins.md#remote-catalog for the feed pipeline itself).
pub const CATALOG_FEED_PUBKEY: [u8; 32] = [
    95, 16, 205, 235, 121, 10, 169, 175, 134, 221, 148, 40, 59, 168, 223, 92, 124, 87, 250, 169,
    27, 0, 236, 12, 251, 192, 190, 188, 159, 166, 120, 148,
];
