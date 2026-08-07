//! `ryuzi-plugin-sdk` — the declarative plugin contract for Ryuzi.
//!
//! This crate defines the ONE manifest schema (contract 2, `manifest`
//! module) every plugin — first-party built-in, embedded catalog, or
//! user-authored, declarative or WASM-component-backed — satisfies:
//! identity and metadata, auth description, settings fields, the
//! component/provider/tools/mcp/hooks/jobs/gateway surfaces, and
//! structural validation. It also owns the placeholder substitution
//! grammar used to inject secrets into MCP server definitions at attach
//! time (`subst` module), the standard category/slot vocabulary
//! (`categories` module), the declarative-hook trigger vocabulary
//! (`triggers` module), and the registry release descriptor served for a
//! published component build (`bundle` module).
//!
//! Deliberately dependency-light (`serde`, `serde_json`, `toml`,
//! `thiserror`, `semver` only): this is the contract external plugin
//! authors target, and it has no opinion on how a manifest becomes a
//! running harness, gateway, connector, or Wasm component. That
//! behavioral binding lives in `ryuzi-core`'s `PluginHost`.

pub mod bundle;
pub mod categories;
pub mod manifest;
pub mod subst;
pub mod triggers;

pub use bundle::{BundleError, PluginRelease};
pub use manifest::{
    AuthKind, AuthSpec, ComponentSpec, DeclaredTool, FieldKind, HookDef, JobDef, ManifestError,
    McpServerDef, McpTransportDef, ModelDef, NetworkPermission, OAuthProfile, PluginLifecycle,
    PluginManifest, PluginPermissions, ProviderSpec, SettingField, CONTRACT_VERSION,
    KNOWN_HOOK_ACTIONS,
};
pub use triggers::{canonical_trigger, CANONICAL_TRIGGERS, CLAUDE_ALIASES};
