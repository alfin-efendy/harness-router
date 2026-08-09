//! The plugin manifest: the declarative contract every Ryuzi plugin
//! (built-in, embedded catalog, or user-authored) satisfies. This module
//! owns parsing (TOML) and structural validation only — it has no opinion
//! on how a manifest becomes a running harness, gateway, or connector; that
//! binding lives in `ryuzi-core`'s `PluginHost`.
//!
//! Contract 2 unifies the old declarative manifest (`ryuzi-plugin.toml`,
//! contract 1) and the WASM component bundle (`ryuzi-plugin-bundle.toml`)
//! into ONE schema: every plugin — first-party built-in, embedded catalog,
//! user-authored declarative, or WASM-component-backed — is described by a
//! single [`PluginManifest`]. This is a big-bang migration: `validate()`
//! rejects any manifest that does not declare `contract = 2` outright,
//! there is no compat loader for contract 1.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::categories;

/// The manifest contract version this SDK understands. `validate()` rejects
/// any manifest that does not declare exactly this contract — big-bang
/// migration, no compat loader for older contracts.
pub const CONTRACT_VERSION: u32 = 2;

/// One plugin, one manifest. Rust built-ins construct this in code; catalog
/// and user plugins author it as TOML (`ryuzi-plugin.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub contract: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub slot: Option<String>,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub experimental: bool,
    #[serde(default)]
    pub auth: Option<AuthSpec>,
    #[serde(default)]
    pub settings: Vec<SettingField>,
    /// The WASM component this plugin ships, when any component-backed
    /// surface (`[provider]`, `[[tools]]`, `[gateway]`) is used.
    #[serde(default)]
    pub component: Option<ComponentSpec>,
    #[serde(default)]
    pub permissions: PluginPermissions,
    #[serde(default)]
    pub oauth: Vec<OAuthProfile>,
    /// Surface: provider. `ids` = the llm-router provider ids served
    /// (absorbs v1-bundle `provider-ids`); `format`/`base_url`/`models`
    /// carry the old `ProviderMeta` fields used by builtin catalog rows.
    #[serde(default)]
    pub provider: Option<ProviderSpec>,
    /// Surface: MCP tools backed by the component (statically declared so
    /// Cockpit shows "what you'll get" pre-install).
    #[serde(default)]
    pub tools: Vec<DeclaredTool>,
    /// Surface: MCP tools via external servers.
    #[serde(default)]
    pub mcp: Vec<McpServerDef>,
    /// Surface: declarative automation hooks.
    #[serde(default)]
    pub hooks: Vec<HookDef>,
    /// Automation: scheduled-job presets.
    #[serde(default)]
    pub jobs: Vec<JobDef>,
    /// INTERNAL surface — first-party only, enforced at install/link time
    /// (not here; validation only requires a component). Never documented
    /// in the public standard.
    #[serde(default)]
    pub gateway: bool,
}

/// How a plugin authenticates. `none` needs no credential; `api-key` and
/// `token` read a secret (via `setting` and/or `env` fallback); `oauth`
/// delegates to provider-specific machinery elsewhere (e.g. `llm_router`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthKind {
    #[default]
    None,
    ApiKey,
    Token,
    Oauth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
pub struct AuthSpec {
    pub kind: AuthKind,
    pub setting: Option<String>,
    pub env: Option<String>,
    #[serde(alias = "help_url")]
    pub help_url: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub resource: Option<String>,
    pub scopes: Vec<String>,
    pub client_id_setting: Option<String>,
    pub client_secret_setting: Option<String>,
    pub dynamic_registration: bool,
    pub extra_authorize_params: BTreeMap<String, String>,
    pub extra_token_params: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingField {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub help: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub kind: FieldKind,
    /// When non-empty, this field is an enum/choice: the persisted value
    /// must be one of these members (enforced by
    /// `ryuzi_core::settings::store::validate_plugin_field`). Expressed as
    /// `kind = "string"` + non-empty `options` — `validate()` rejects any
    /// other `kind` paired with non-empty `options`.
    #[serde(default)]
    pub options: Vec<String>,
    /// Pre-filled/effective value to show when no row is persisted yet. When
    /// `options` is non-empty, `default` (if set) must be one of its
    /// members.
    #[serde(default)]
    pub default: Option<String>,
}

/// The value shape a `SettingField` renders and stores. Defaults to
/// `String` since most settings (tokens, hostnames, ids) are plain text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldKind {
    #[default]
    String,
    Int,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerDef {
    pub name: String,
    pub transport: McpTransportDef,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransportDef {
    Stdio,
    Http,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ComponentSpec {
    pub file: String,
    /// Cargo-style semver RANGE the component targets (e.g. `^0.1.0`).
    pub wit_api: String,
    pub lifecycle: PluginLifecycle,
}

/// How the host instances a bundle's component: one shared instance for
/// the whole process, one instance per session, or a fresh instance per
/// call. Purely declarative here — the instancing policy itself is
/// enforced by the (not-yet-implemented) Wasmtime host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginLifecycle {
    Singleton,
    PerSession,
    PerCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
pub struct ProviderSpec {
    pub ids: Vec<String>,
    pub format: Option<String>,
    pub base_url: Option<String>,
    pub models: Vec<ModelDef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelDef {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub default: bool,
}

/// A bundle's permission contract. Currently just the outbound network
/// allowlist; more permission axes (filesystem, env, secrets) can be added
/// as new fields without breaking existing bundles (`#[serde(default)]`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginPermissions {
    pub network: Vec<NetworkPermission>,
}

/// One outbound-network allowlist entry: a bare lowercase hostname
/// (`api.github.com`) or a `*.`-prefixed wildcard hostname
/// (`*.github.com`). No scheme, path, port, IP literal, bare `*`, or
/// uppercase — see the host-validation logic exercised by
/// [`PluginManifest::validate`] for the exact grammar enforced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NetworkPermission(pub String);

/// One OAuth profile a plugin's component may use to authenticate. A
/// plugin may declare more than one (e.g. a connector that talks to two
/// different OAuth-protected APIs); `id` must be unique within the
/// manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
pub struct OAuthProfile {
    pub id: String,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    /// The RFC 8628 device-authorization endpoint (e.g. GitHub's
    /// `https://github.com/login/device/code`). `OAuthProfile` has no other
    /// place to record it, and the host's `begin_device_flow` needs it
    /// explicitly; a component that supports the device grant declares it here.
    /// `None` for a profile that does not offer device flow.
    pub device_authorization_url: Option<String>,
    pub scopes: Vec<String>,
    /// A first-party PUBLIC OAuth client id baked into the (signed) manifest —
    /// the `gh` CLI model: the component ships its own app's client id so an
    /// end-user connects with zero configuration. Public, not a secret (device
    /// flow uses a public client, no client secret). A user-set
    /// [`Self::client_id_setting`] or a stored per-install client id still wins
    /// over this default (see the host's `resolve_client_id`).
    pub client_id: Option<String>,
    pub client_id_setting: Option<String>,
    pub client_secret_setting: Option<String>,
    pub resource: Option<String>,
    pub dynamic_registration: bool,
    /// Extra query parameters the provider's authorize URL requires beyond
    /// the standard PKCE set (e.g. Atlassian's mandatory
    /// `audience=api.atlassian.com`). Mirrors the declarative
    /// `AuthSpec.extra_authorize_params`. Forwarded verbatim by the host's
    /// `begin_pkce`.
    #[serde(default)]
    pub extra_authorize_params: BTreeMap<String, String>,
}

/// A tool the component exposes to agents, declared statically so Cockpit can
/// show "what you'll get" before the plugin is ever installed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "kebab-case")]
pub struct DeclaredTool {
    pub name: String,
    pub description: String,
    pub writes: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookDef {
    pub name: String,
    /// Canonical or Claude-alias spelling; `validate()` requires a known
    /// spelling, `canonical_trigger()` gives the stored form.
    pub trigger: String,
    /// One of `KNOWN_HOOK_ACTIONS`.
    pub action: String,
    /// Action config, shape-checked by ryuzi-core against the matching
    /// `HookActionInput` variant at sync time (the SDK stays
    /// runtime-independent and does not duplicate that schema).
    #[serde(default = "empty_toml_table")]
    pub config: toml::Value,
}

fn empty_toml_table() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobDef {
    pub name: String,
    /// Natural-language ("every day at 9am") or cron. Parsed by
    /// ryuzi-core's scheduler at sync time.
    pub schedule: String,
    pub prompt: String,
    #[serde(default)]
    pub model_override: Option<String>,
}

pub const KNOWN_HOOK_ACTIONS: &[&str] = &["agent.run", "webhook.outbound"];

/// Errors from parsing or validating a `PluginManifest`.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("invalid plugin manifest toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("manifest declares contract {found}, but this build only supports contract 2")]
    ContractUnsupported { found: u32 },
    #[error("invalid plugin id: {0}")]
    InvalidId(String),
    #[error("plugin name must not be empty")]
    EmptyName,
    #[error("duplicate mcp server name: {0}")]
    DuplicateMcpName(String),
    #[error("mcp server \"{0}\" uses stdio transport but has no command")]
    MissingCommand(String),
    #[error("mcp server \"{0}\" uses http transport but has no url")]
    MissingUrl(String),
    #[error("mcp server \"{0}\" references ${{auth}} but the manifest has no [auth] block")]
    AuthPlaceholderWithoutAuth(String),
    #[error("duplicate settings field key: {0}")]
    DuplicateSettingKey(String),
    #[error("settings field key must not be empty")]
    EmptySettingKey,
    #[error("settings key \"{0}\" is a host-owned control key and cannot be declared by a plugin")]
    SettingKeyReserved(String),
    #[error("settings field \"{0}\" declares non-empty `options` but `kind` is not `string`")]
    SettingOptionsRequireStringKind(String),
    #[error("settings field \"{0}\"'s `default` is not a member of its `options`")]
    SettingDefaultNotInOptions(String),
    #[error("invalid version {0:?}: {1}")]
    InvalidVersion(String, String),
    #[error("invalid wit-api version {0:?}: {1}")]
    InvalidWitApi(String, String),
    #[error("component filename must not be empty")]
    EmptyComponent,
    #[error("invalid network allowlist entry: {0:?}")]
    InvalidNetworkHost(String),
    #[error("oauth profile id must not be empty")]
    EmptyOAuthProfileId,
    #[error("duplicate oauth profile id: {0}")]
    DuplicateOAuthProfile(String),
    #[error("oauth profile {profile:?} field {field:?} must be a non-empty https:// url")]
    InsecureOauthUrl {
        profile: String,
        field: &'static str,
    },
    #[error("invalid provider id: {0:?}")]
    InvalidProviderId(String),
    #[error("tool name must not be empty")]
    EmptyToolName,
    #[error("duplicate tool name: {0}")]
    DuplicateTool(String),
    #[error("settings key must not start with \"plugin.\": {0}")]
    SettingKeyPrefixForbidden(String),
    #[error("{0} requires a [component] block")]
    SurfaceRequiresComponent(&'static str),
    #[error("hook name must not be empty")]
    EmptyHookName,
    #[error("duplicate hook name: {0}")]
    DuplicateHookName(String),
    #[error("hook \"{0}\" has unknown trigger \"{1}\"")]
    UnknownTrigger(String, String),
    #[error("hook \"{0}\" has unknown action \"{1}\"")]
    UnknownAction(String, String),
    #[error("job name must not be empty")]
    EmptyJobName,
    #[error("duplicate job name: {0}")]
    DuplicateJobName(String),
    #[error("job \"{0}\" has an empty schedule")]
    EmptyJobSchedule(String),
    #[error("job \"{0}\" has an empty prompt")]
    EmptyJobPrompt(String),
}

fn is_valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn contains_auth_placeholder(server: &McpServerDef) -> bool {
    const PLACEHOLDER: &str = "${auth}";
    server.env.values().any(|v| v.contains(PLACEHOLDER))
        || server.headers.values().any(|v| v.contains(PLACEHOLDER))
        || server.args.iter().any(|a| a.contains(PLACEHOLDER))
        || server
            .url
            .as_deref()
            .is_some_and(|u| u.contains(PLACEHOLDER))
}

/// `true` if `host` is a bare lowercase hostname (`api.github.com`) or a
/// `*.`-prefixed wildcard hostname (`*.github.com`). Rejects a scheme
/// (`://`), a path or port (`/`, `:`), whitespace, an IP literal, a bare
/// `*`, a wildcard anywhere but the leading `*.`, uppercase characters, and
/// blank input.
fn is_valid_network_host(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    if host.contains("://") {
        return false;
    }

    let body = match host.strip_prefix("*.") {
        Some(rest) if !rest.is_empty() => rest,
        Some(_) => return false, // "*." with nothing after it
        None => host,
    };

    if body.contains('*') || body.contains('/') || body.contains(':') || body.contains(' ') {
        return false;
    }
    if body.chars().any(|c| c.is_ascii_uppercase()) {
        return false;
    }
    if body.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }

    let labels: Vec<&str> = body.split('.').collect();
    labels.iter().all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// The contract-1 WASM *bundle* manifest, retained ONLY as a read-only compat
/// shim so a release feed published before the v2 migration stays installable
/// (see [`PluginManifest::from_toml_detecting_legacy`]). Nothing writes this
/// shape — v2 is the only authoring contract.
///
/// The v1 bundle manifest kept the component's coordinates flat
/// (`component = "x.wasm"`, `wit-api`, `lifecycle`) and named router provider
/// ids in a top-level `provider-ids`; v2 nests the former under `[component]`
/// and the latter under `[provider] ids`. Every other field (`permissions`,
/// `oauth`, `tools`, `settings`) is byte-identical between the two contracts,
/// so it is reused verbatim rather than mirrored here.
#[derive(Debug, Clone, Deserialize)]
struct ContractOneBundleManifest {
    id: String,
    name: String,
    version: String,
    #[serde(rename = "wit-api")]
    wit_api: String,
    lifecycle: PluginLifecycle,
    /// v1's flat component filename — v2's `[component] file`.
    component: String,
    #[serde(default)]
    publisher: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    permissions: PluginPermissions,
    #[serde(default)]
    oauth: Vec<OAuthProfile>,
    #[serde(default, rename = "provider-ids")]
    provider_ids: Vec<String>,
    #[serde(default)]
    tools: Vec<DeclaredTool>,
    #[serde(default)]
    settings: Vec<SettingField>,
}

impl ContractOneBundleManifest {
    /// Project a contract-1 bundle onto the v2 schema. The result is an
    /// ordinary [`PluginManifest`] and is validated by the caller exactly like
    /// a natively-authored v2 one — the shim only reshapes fields, it never
    /// relaxes a rule.
    fn upgrade(self) -> PluginManifest {
        PluginManifest {
            contract: CONTRACT_VERSION,
            id: self.id,
            name: self.name,
            version: self.version,
            publisher: self.publisher,
            description: self.description,
            homepage: None,
            icon: None,
            categories: Vec::new(),
            slot: None,
            verified: false,
            experimental: false,
            auth: None,
            settings: self.settings,
            component: Some(ComponentSpec {
                file: self.component,
                wit_api: self.wit_api,
                lifecycle: self.lifecycle,
            }),
            permissions: self.permissions,
            oauth: self.oauth,
            // v1 had no `[provider]` block at all: a bundle was a provider iff
            // it named router ids. A bundle that named none is not a provider,
            // so it must NOT get an (empty, id-defaulting) ProviderSpec here —
            // `resolved_provider_ids` treats `Some(_)` with no ids as "serves
            // my own id", which would register a transport v1 never registered.
            provider: (!self.provider_ids.is_empty()).then(|| ProviderSpec {
                ids: self.provider_ids,
                ..ProviderSpec::default()
            }),
            tools: self.tools,
            mcp: Vec::new(),
            hooks: Vec::new(),
            jobs: Vec::new(),
            // v1 had no gateway DECLARATION: the host discovered a gateway by
            // compiling the component and checking whether it exported
            // `ryuzi:gateway/gateway`. v2 added `gateway = true` purely as a
            // cheap pre-filter so non-gateway bundles need not be compiled —
            // the exports check remains the authority, and PERMISSION to serve
            // a gateway is derived from the verified first-party signing key,
            // never from this flag (see `HostPolicy::for_installed_bundle`).
            // Declaring `true` for every upgraded v1 bundle therefore restores
            // exactly v1's semantics at the cost of compiling legacy bundles
            // during discovery; declaring `false` would silently strand a v1
            // gateway (Discord) that has no way to say so.
            gateway: true,
        }
    }
}

/// What a document's own `contract` key claims, read before any typed parse.
enum DeclaredContract {
    /// No `contract` key — a contract-1 candidate (v1 predates the key).
    Absent,
    /// `contract = N` for some `N != 2`.
    Unsupported(u32),
    /// `contract = 2`, or a `contract` key too malformed to read as one.
    CurrentOrUnreadable,
}

/// Read a document's `contract` claim without deserializing the whole manifest.
///
/// A declared contract is authoritative over the document's SHAPE: a manifest
/// that says `contract = 1` must be reported as an unsupported contract, not
/// as whatever field-level type error its v1 layout happens to trip first.
fn declared_contract(input: &str) -> DeclaredContract {
    let Ok(document) = toml::from_str::<toml::Value>(input) else {
        return DeclaredContract::CurrentOrUnreadable;
    };
    match document.get("contract") {
        None => DeclaredContract::Absent,
        Some(value) => match value.as_integer() {
            Some(found) if found != i64::from(CONTRACT_VERSION) => {
                DeclaredContract::Unsupported(u32::try_from(found).unwrap_or(u32::MAX))
            }
            _ => DeclaredContract::CurrentOrUnreadable,
        },
    }
}

impl PluginManifest {
    /// Parse TOML into a manifest and validate it in one step.
    pub fn from_toml(input: &str) -> Result<PluginManifest, ManifestError> {
        Self::from_toml_detecting_legacy(input).map(|(manifest, _)| manifest)
    }

    /// [`Self::from_toml`], additionally reporting whether the input was a
    /// contract-1 bundle manifest upgraded by the compat shim (`true`) rather
    /// than a natively-authored contract-2 one (`false`).
    ///
    /// Callers that install from a remote feed use the flag to log that they
    /// accepted a pre-v2 artifact; callers that don't care use
    /// [`Self::from_toml`]. A contract-2 parse is always attempted first, and
    /// its error is what surfaces when the input is neither — so a v2 manifest
    /// with a genuine mistake still reports that mistake, not a confusing
    /// "this isn't a v1 bundle either".
    pub fn from_toml_detecting_legacy(
        input: &str,
    ) -> Result<(PluginManifest, bool), ManifestError> {
        let (manifest, upgraded) = match toml::from_str::<PluginManifest>(input) {
            Ok(manifest) => (manifest, false),
            Err(contract_2_error) => match declared_contract(input) {
                // A deliberate contract claim outranks the document's shape:
                // report the contract, not the first field that fails to fit.
                DeclaredContract::Unsupported(found) => {
                    return Err(ManifestError::ContractUnsupported { found })
                }
                // No `contract` key at all — the one case the compat shim
                // covers. If it isn't a v1 bundle either, the v2 error stands.
                DeclaredContract::Absent => {
                    match toml::from_str::<ContractOneBundleManifest>(input) {
                        Ok(legacy) => (legacy.upgrade(), true),
                        Err(_) => return Err(ManifestError::Toml(contract_2_error)),
                    }
                }
                DeclaredContract::CurrentOrUnreadable => {
                    return Err(ManifestError::Toml(contract_2_error))
                }
            },
        };
        manifest.validate()?;
        Ok((manifest, upgraded))
    }

    /// Structural validation: contract version (exact match — big-bang
    /// migration, no compat loader for contract 1), id shape, required
    /// fields, unique MCP server names, transport-specific requirements,
    /// the `${auth}` placeholder requiring an `[auth]` block, the network
    /// allowlist grammar, OAuth profile shape, declared-tool uniqueness,
    /// bare settings keys (non-empty, unique, not `plugin.`-prefixed, and
    /// not one of the host-owned control keys `enabled`/`trusted` — see
    /// `crate::plugins::capabilities::settings::ScopedSettings::effective_key`
    /// in `ryuzi-core` for the matching guest-write-time guard), component-
    /// backed surface requirements, provider id shape, and hook/job shape.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.contract != CONTRACT_VERSION {
            return Err(ManifestError::ContractUnsupported {
                found: self.contract,
            });
        }
        if !is_valid_id(&self.id) {
            return Err(ManifestError::InvalidId(self.id.clone()));
        }
        if self.name.is_empty() {
            return Err(ManifestError::EmptyName);
        }

        let mut seen_setting_keys: HashSet<&str> = HashSet::new();
        for field in &self.settings {
            if field.key.is_empty() {
                return Err(ManifestError::EmptySettingKey);
            }
            if field.key == "enabled" || field.key == "trusted" {
                return Err(ManifestError::SettingKeyReserved(field.key.clone()));
            }
            if !seen_setting_keys.insert(field.key.as_str()) {
                return Err(ManifestError::DuplicateSettingKey(field.key.clone()));
            }
            if !field.options.is_empty() {
                if field.kind != FieldKind::String {
                    return Err(ManifestError::SettingOptionsRequireStringKind(
                        field.key.clone(),
                    ));
                }
                if let Some(default) = &field.default {
                    if !field.options.iter().any(|o| o == default) {
                        return Err(ManifestError::SettingDefaultNotInOptions(field.key.clone()));
                    }
                }
            }
        }

        let mut seen_mcp_names: HashSet<&str> = HashSet::new();
        for server in &self.mcp {
            if !seen_mcp_names.insert(server.name.as_str()) {
                return Err(ManifestError::DuplicateMcpName(server.name.clone()));
            }
            match server.transport {
                McpTransportDef::Stdio if server.command.is_none() => {
                    return Err(ManifestError::MissingCommand(server.name.clone()));
                }
                McpTransportDef::Http if server.url.is_none() => {
                    return Err(ManifestError::MissingUrl(server.name.clone()));
                }
                McpTransportDef::Stdio | McpTransportDef::Http => {}
            }
            if contains_auth_placeholder(server) && self.auth.is_none() {
                return Err(ManifestError::AuthPlaceholderWithoutAuth(
                    server.name.clone(),
                ));
            }
        }

        // ---- moved in from bundle.rs's validate() ----

        for entry in &self.permissions.network {
            if !is_valid_network_host(&entry.0) {
                return Err(ManifestError::InvalidNetworkHost(entry.0.clone()));
            }
        }

        let mut seen_oauth_ids: HashSet<&str> = HashSet::new();
        for profile in &self.oauth {
            if profile.id.is_empty() {
                return Err(ManifestError::EmptyOAuthProfileId);
            }
            if !seen_oauth_ids.insert(profile.id.as_str()) {
                return Err(ManifestError::DuplicateOAuthProfile(profile.id.clone()));
            }
            for (field, url) in [
                ("authorize-url", &profile.authorize_url),
                ("token-url", &profile.token_url),
                (
                    "device-authorization-url",
                    &profile.device_authorization_url,
                ),
            ] {
                if let Some(url) = url {
                    if !url.starts_with("https://") {
                        return Err(ManifestError::InsecureOauthUrl {
                            profile: profile.id.clone(),
                            field,
                        });
                    }
                }
            }
        }

        let mut seen_tool_names: HashSet<&str> = HashSet::new();
        for tool in &self.tools {
            if tool.name.is_empty() {
                return Err(ManifestError::EmptyToolName);
            }
            if !seen_tool_names.insert(tool.name.as_str()) {
                return Err(ManifestError::DuplicateTool(tool.name.clone()));
            }
        }

        for setting in &self.settings {
            if setting.key.starts_with("plugin.") {
                return Err(ManifestError::SettingKeyPrefixForbidden(
                    setting.key.clone(),
                ));
            }
        }

        // ---- new v2 rules ----

        // Component-backed surfaces require a [component].
        if self.component.is_none() {
            if self.provider.as_ref().is_some_and(|p| !p.ids.is_empty()) {
                return Err(ManifestError::SurfaceRequiresComponent("provider.ids"));
            }
            if !self.tools.is_empty() {
                return Err(ManifestError::SurfaceRequiresComponent("tools"));
            }
            if self.gateway {
                return Err(ManifestError::SurfaceRequiresComponent("gateway"));
            }
        }
        if let Some(component) = &self.component {
            if component.file.is_empty() {
                return Err(ManifestError::EmptyComponent);
            }
            semver::VersionReq::parse(&component.wit_api).map_err(|e| {
                ManifestError::InvalidWitApi(component.wit_api.clone(), e.to_string())
            })?;
            // A component-backed plugin must version itself.
            semver::Version::parse(&self.version)
                .map_err(|e| ManifestError::InvalidVersion(self.version.clone(), e.to_string()))?;
        }
        if let Some(provider) = &self.provider {
            for provider_id in &provider.ids {
                if !is_valid_id(provider_id) {
                    return Err(ManifestError::InvalidProviderId(provider_id.clone()));
                }
            }
        }
        let mut seen_hook_names: HashSet<&str> = HashSet::new();
        for hook in &self.hooks {
            if hook.name.is_empty() {
                return Err(ManifestError::EmptyHookName);
            }
            if !seen_hook_names.insert(hook.name.as_str()) {
                return Err(ManifestError::DuplicateHookName(hook.name.clone()));
            }
            if crate::triggers::canonical_trigger(&hook.trigger).is_none() {
                return Err(ManifestError::UnknownTrigger(
                    hook.name.clone(),
                    hook.trigger.clone(),
                ));
            }
            if !KNOWN_HOOK_ACTIONS.contains(&hook.action.as_str()) {
                return Err(ManifestError::UnknownAction(
                    hook.name.clone(),
                    hook.action.clone(),
                ));
            }
        }
        let mut seen_job_names: HashSet<&str> = HashSet::new();
        for job in &self.jobs {
            if job.name.is_empty() {
                return Err(ManifestError::EmptyJobName);
            }
            if !seen_job_names.insert(job.name.as_str()) {
                return Err(ManifestError::DuplicateJobName(job.name.clone()));
            }
            if job.schedule.is_empty() {
                return Err(ManifestError::EmptyJobSchedule(job.name.clone()));
            }
            if job.prompt.is_empty() {
                return Err(ManifestError::EmptyJobPrompt(job.name.clone()));
            }
        }

        Ok(())
    }

    /// Non-fatal feedback: categories outside the standard vocabulary
    /// (`categories::KNOWN`), plus a claimed `slot` outside
    /// `categories::KNOWN_SLOTS` (if any). Unlike `validate()`, this never
    /// rejects a manifest — new categories/slots should not break the
    /// loader.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = self
            .categories
            .iter()
            .filter(|category| !categories::KNOWN.contains(&category.as_str()))
            .cloned()
            .collect();
        if let Some(slot) = &self.slot {
            if !categories::KNOWN_SLOTS.contains(&slot.as_str()) {
                warnings.push(slot.clone());
            }
        }
        warnings
    }

    /// The llm-router provider id(s) this plugin serves: `provider.ids`
    /// when non-empty, else `[self.id]` when a `[provider]` block exists at
    /// all, else empty (not a provider plugin).
    pub fn resolved_provider_ids(&self) -> Vec<String> {
        match &self.provider {
            None => vec![],
            Some(p) if p.ids.is_empty() => vec![self.id.clone()],
            Some(p) => p.ids.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GITHUB_MANIFEST: &str = r#"
contract = 2
id = "github"
name = "GitHub"
version = "0.1.0"
publisher = "ryuzi"
description = "Repos, issues, PRs, and wiki via the GitHub MCP server."
homepage = "https://github.com"
icon = "github"
categories = ["vcs", "issues"]
verified = true

[auth]
kind = "token"
setting = "plugin.github.token"
env = "GITHUB_PERSONAL_ACCESS_TOKEN"
help_url = "https://github.com/settings/tokens"

[[settings]]
key = "github.host"
label = "GitHub host"
help = "Set for GitHub Enterprise."
required = false

[[mcp]]
name = "github"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "${auth}" }
"#;

    #[test]
    fn round_trips_the_github_example_manifest() {
        let manifest =
            PluginManifest::from_toml(GITHUB_MANIFEST).expect("should parse and validate");

        assert_eq!(manifest.contract, 2);
        assert_eq!(manifest.id, "github");
        assert_eq!(manifest.name, "GitHub");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.publisher, "ryuzi");
        assert_eq!(manifest.homepage.as_deref(), Some("https://github.com"));
        assert_eq!(manifest.icon.as_deref(), Some("github"));
        assert_eq!(manifest.categories, vec!["vcs", "issues"]);
        assert!(manifest.verified);
        assert!(!manifest.experimental);

        let auth = manifest.auth.expect("auth block");
        assert_eq!(auth.kind, AuthKind::Token);
        assert_eq!(auth.setting.as_deref(), Some("plugin.github.token"));
        assert_eq!(auth.env.as_deref(), Some("GITHUB_PERSONAL_ACCESS_TOKEN"));
        assert_eq!(
            auth.help_url.as_deref(),
            Some("https://github.com/settings/tokens")
        );

        assert_eq!(manifest.settings.len(), 1);
        let setting = &manifest.settings[0];
        assert_eq!(setting.key, "github.host");
        assert_eq!(setting.label, "GitHub host");
        assert_eq!(setting.help, "Set for GitHub Enterprise.");
        assert!(!setting.required);
        assert!(!setting.secret);
        assert_eq!(setting.kind, FieldKind::String);
        assert!(setting.options.is_empty());
        assert_eq!(setting.default, None);

        assert_eq!(manifest.mcp.len(), 1);
        let mcp = &manifest.mcp[0];
        assert_eq!(mcp.name, "github");
        assert_eq!(mcp.transport, McpTransportDef::Stdio);
        assert_eq!(mcp.command.as_deref(), Some("npx"));
        assert_eq!(
            mcp.args,
            vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-github".to_string()
            ]
        );
        assert_eq!(
            mcp.env
                .get("GITHUB_PERSONAL_ACCESS_TOKEN")
                .map(String::as_str),
            Some("${auth}")
        );
    }

    #[test]
    fn round_trips_the_provider_block() {
        let toml_str = r#"
contract = 2
id = "anthropic"
name = "Anthropic"

[provider]
format = "anthropic"
base-url = "https://api.anthropic.com"
models = [ { id = "claude-opus-4-5", label = "Opus 4.5", default = true } ]
"#;
        let manifest = PluginManifest::from_toml(toml_str).expect("should parse and validate");

        let provider = manifest.provider.expect("provider block");
        assert!(provider.ids.is_empty());
        assert_eq!(provider.format.as_deref(), Some("anthropic"));
        assert_eq!(
            provider.base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].id, "claude-opus-4-5");
        assert_eq!(provider.models[0].label.as_deref(), Some("Opus 4.5"));
        assert!(provider.models[0].default);
    }

    #[test]
    fn parses_oauth_auth_metadata() {
        let toml_str = r#"
contract = 2
id = "acme-oauth"
name = "Acme OAuth"

[auth]
kind = "oauth"
setting = "plugin.acme.oauth_setting"
env = "ACME_OAUTH"
help_url = "https://acme.example.com/help"
authorize-url = "https://acme.example.com/oauth/authorize"
token-url = "https://acme.example.com/oauth/token"
resource = "acme://api"
scopes = ["repo", "issues:read"]
client-id-setting = "plugin.acme.client_id"
client-secret-setting = "plugin.acme.client_secret"
dynamic-registration = true
extra-authorize-params = { prompt = "consent", access_type = "offline" }
extra-token-params = { audience = "acme", tenant = "engineering" }
"#;
        let manifest = PluginManifest::from_toml(toml_str).expect("should parse and validate");

        let auth = manifest.auth.expect("auth block");
        assert_eq!(auth.kind, AuthKind::Oauth);
        assert_eq!(auth.setting.as_deref(), Some("plugin.acme.oauth_setting"));
        assert_eq!(auth.env.as_deref(), Some("ACME_OAUTH"));
        assert_eq!(
            auth.help_url.as_deref(),
            Some("https://acme.example.com/help")
        );
        assert_eq!(
            auth.authorize_url.as_deref(),
            Some("https://acme.example.com/oauth/authorize")
        );
        assert_eq!(
            auth.token_url.as_deref(),
            Some("https://acme.example.com/oauth/token")
        );
        assert_eq!(auth.resource.as_deref(), Some("acme://api"));
        assert_eq!(
            auth.scopes,
            vec!["repo".to_string(), "issues:read".to_string()]
        );
        assert_eq!(
            auth.client_id_setting.as_deref(),
            Some("plugin.acme.client_id")
        );
        assert_eq!(
            auth.client_secret_setting.as_deref(),
            Some("plugin.acme.client_secret")
        );
        assert!(auth.dynamic_registration);
        assert_eq!(
            auth.extra_authorize_params
                .get("prompt")
                .map(String::as_str),
            Some("consent")
        );
        assert_eq!(
            auth.extra_authorize_params
                .get("access_type")
                .map(String::as_str),
            Some("offline")
        );
        assert_eq!(
            auth.extra_token_params.get("audience").map(String::as_str),
            Some("acme")
        );
        assert_eq!(
            auth.extra_token_params.get("tenant").map(String::as_str),
            Some("engineering")
        );
    }

    #[test]
    fn parses_canonical_help_url_key() {
        let toml_str = r#"
contract = 2
id = "acme-oauth-help-url"
name = "Acme OAuth Help URL"

[auth]
kind = "oauth"
help-url = "https://acme.example.com/help"
"#;
        let manifest = PluginManifest::from_toml(toml_str).expect("should parse and validate");
        let auth = manifest.auth.expect("auth block");
        assert_eq!(
            auth.help_url.as_deref(),
            Some("https://acme.example.com/help")
        );
    }

    #[test]
    fn parses_oauth_with_only_kind_for_backwards_compatibility() {
        let toml_str = r#"
contract = 2
id = "acme-oauth-legacy"
name = "Acme OAuth Legacy"

[auth]
kind = "oauth"
"#;
        let manifest = PluginManifest::from_toml(toml_str).expect("should parse and validate");

        let auth = manifest.auth.expect("auth block");
        assert_eq!(auth.kind, AuthKind::Oauth);
        assert_eq!(auth.setting, None);
        assert_eq!(auth.env, None);
        assert_eq!(auth.help_url, None);
        assert_eq!(auth.authorize_url, None);
        assert_eq!(auth.token_url, None);
        assert_eq!(auth.resource, None);
        assert_eq!(auth.scopes, Vec::<String>::new());
        assert_eq!(auth.client_id_setting, None);
        assert_eq!(auth.client_secret_setting, None);
        assert!(!auth.dynamic_registration);
        assert_eq!(auth.extra_authorize_params, BTreeMap::new());
        assert_eq!(auth.extra_token_params, BTreeMap::new());
    }

    fn minimal_manifest(extra: &str) -> String {
        format!(
            r#"
contract = 2
id = "acme"
name = "Acme"
{extra}
"#
        )
    }

    fn manifest_with_component(extra: &str) -> String {
        format!(
            r#"
contract = 2
id = "test"
name = "Test"
version = "0.1.0"

[component]
file = "test.wasm"
wit-api = "^0.1.0"
lifecycle = "singleton"
{extra}
"#
        )
    }

    fn manifest_with_network(host: &str) -> String {
        format!(
            r#"
contract = 2
id = "acme"
name = "Acme"

[permissions]
network = ["{host}"]
"#
        )
    }

    #[test]
    fn rejects_missing_id() {
        let toml_str = r#"
contract = 2
name = "Acme"
"#;
        let err = PluginManifest::from_toml(toml_str).expect_err("missing id should fail to parse");
        assert!(matches!(err, ManifestError::Toml(_)));
    }

    #[test]
    fn rejects_uppercase_id() {
        let toml_str = r#"
contract = 2
id = "Acme"
name = "Acme"
"#;
        let err =
            PluginManifest::from_toml(toml_str).expect_err("uppercase id should fail validation");
        assert!(matches!(err, ManifestError::InvalidId(id) if id == "Acme"));
    }

    #[test]
    fn rejects_contract_newer_than_supported() {
        let toml_str = r#"
contract = 3
id = "acme"
name = "Acme"
"#;
        let err =
            PluginManifest::from_toml(toml_str).expect_err("contract 3 should fail validation");
        assert!(matches!(
            err,
            ManifestError::ContractUnsupported { found: 3 }
        ));
    }

    #[test]
    fn rejects_duplicate_mcp_names() {
        let toml_str = minimal_manifest(
            r#"
[[mcp]]
name = "dup"
transport = "stdio"
command = "npx"

[[mcp]]
name = "dup"
transport = "stdio"
command = "npx"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("duplicate mcp names should fail validation");
        assert!(matches!(err, ManifestError::DuplicateMcpName(name) if name == "dup"));
    }

    #[test]
    fn rejects_auth_placeholder_without_auth_block() {
        let toml_str = minimal_manifest(
            r#"
[[mcp]]
name = "svc"
transport = "stdio"
command = "npx"
env = { TOKEN = "${auth}" }
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("${auth} without [auth] should fail validation");
        assert!(matches!(err, ManifestError::AuthPlaceholderWithoutAuth(name) if name == "svc"));
    }

    #[test]
    fn stdio_transport_requires_command() {
        let toml_str = minimal_manifest(
            r#"
[[mcp]]
name = "svc"
transport = "stdio"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("stdio without command should fail validation");
        assert!(matches!(err, ManifestError::MissingCommand(name) if name == "svc"));
    }

    #[test]
    fn http_transport_requires_url() {
        let toml_str = minimal_manifest(
            r#"
[[mcp]]
name = "svc"
transport = "http"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("http without url should fail validation");
        assert!(matches!(err, ManifestError::MissingUrl(name) if name == "svc"));
    }

    #[test]
    fn unknown_category_is_a_warning_not_an_error() {
        let toml_str = minimal_manifest(r#"categories = ["not-a-real-category"]"#);
        let manifest =
            PluginManifest::from_toml(&toml_str).expect("unknown category should still parse");
        assert_eq!(manifest.warnings(), vec!["not-a-real-category".to_string()]);
    }

    #[test]
    fn known_categories_produce_no_warnings() {
        let toml_str = minimal_manifest(r#"categories = ["vcs", "issues"]"#);
        let manifest = PluginManifest::from_toml(&toml_str).expect("known categories should parse");
        assert!(manifest.warnings().is_empty());
    }

    // ---------- SettingField: options/default ----------

    #[test]
    fn settings_field_with_valid_enum_options_and_default_parses() {
        let toml_str = minimal_manifest(
            r#"
[[settings]]
key = "tier"
label = "Tier"
kind = "string"
options = ["free", "pro", "enterprise"]
default = "free"
"#,
        );
        let manifest =
            PluginManifest::from_toml(&toml_str).expect("valid enum settings field should parse");
        let field = &manifest.settings[0];
        assert_eq!(field.kind, FieldKind::String);
        assert_eq!(
            field.options,
            vec![
                "free".to_string(),
                "pro".to_string(),
                "enterprise".to_string()
            ]
        );
        assert_eq!(field.default.as_deref(), Some("free"));
    }

    #[test]
    fn rejects_duplicate_setting_keys() {
        let toml_str = minimal_manifest(
            r#"
[[settings]]
key = "dup"
label = "Dup One"

[[settings]]
key = "dup"
label = "Dup Two"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("duplicate settings field keys should fail validation");
        assert!(matches!(err, ManifestError::DuplicateSettingKey(key) if key == "dup"));
    }

    #[test]
    fn rejects_default_not_a_member_of_options() {
        let toml_str = minimal_manifest(
            r#"
[[settings]]
key = "tier"
label = "Tier"
kind = "string"
options = ["free", "pro"]
default = "enterprise"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("default outside options should fail validation");
        assert!(matches!(err, ManifestError::SettingDefaultNotInOptions(key) if key == "tier"));
    }

    #[test]
    fn rejects_options_paired_with_non_string_kind() {
        let toml_str = minimal_manifest(
            r#"
[[settings]]
key = "retries"
label = "Retries"
kind = "int"
options = ["1", "2", "3"]
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("options with a non-string kind should fail validation");
        assert!(
            matches!(err, ManifestError::SettingOptionsRequireStringKind(key) if key == "retries")
        );
    }

    #[test]
    fn bundle_settings_keys_must_be_bare() {
        // validate() rejects a fully-qualified key — v2 settings are bare;
        // the host prefixes `plugin.<id>.` when bridging to the plugin list.
        let toml_str = minimal_manifest(
            r#"
[[settings]]
key = "plugin.acme.token"
label = "Bot token"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("settings key starting with plugin. should fail validation");
        assert!(
            matches!(err, ManifestError::SettingKeyPrefixForbidden(key) if key == "plugin.acme.token")
        );
    }

    // F8: an empty settings key silently validated during the v1/v2 merge —
    // restore the guard.
    #[test]
    fn rejects_empty_setting_key() {
        let toml_str = minimal_manifest(
            r#"
[[settings]]
key = ""
label = "Nothing"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("an empty settings key should fail validation");
        assert!(matches!(err, ManifestError::EmptySettingKey));
    }

    // F8: `enabled`/`trusted` are host-owned control keys — the settings
    // capability already refuses a WASM guest write to either
    // (`crate::plugins::capabilities::settings::ScopedSettings::effective_key`
    // in `ryuzi-core`); a manifest must not be able to declare either as its
    // own `[[settings]]` key.
    #[test]
    fn rejects_reserved_setting_keys() {
        for reserved in ["enabled", "trusted"] {
            let toml_str = minimal_manifest(&format!(
                r#"
[[settings]]
key = "{reserved}"
label = "Reserved"
"#
            ));
            let err = PluginManifest::from_toml(&toml_str)
                .expect_err("host-owned setting key should fail validation");
            assert!(
                matches!(&err, ManifestError::SettingKeyReserved(key) if key == reserved),
                "{reserved}: unexpected error {err:?}"
            );
        }
    }

    // ---------- slot ----------

    #[test]
    fn slot_defaults_to_none_when_omitted() {
        let toml_str = minimal_manifest("");
        let manifest = PluginManifest::from_toml(&toml_str).expect("should parse");
        assert_eq!(manifest.slot, None);
        assert!(manifest.warnings().is_empty());
    }

    #[test]
    fn known_slot_parses_with_no_warning() {
        let toml_str = minimal_manifest(r#"slot = "memory""#);
        let manifest =
            PluginManifest::from_toml(&toml_str).expect("known slot should parse and validate");
        assert_eq!(manifest.slot.as_deref(), Some("memory"));
        assert!(manifest.warnings().is_empty());
    }

    #[test]
    fn unknown_slot_is_a_warning_not_an_error() {
        let toml_str = minimal_manifest(r#"slot = "not-a-real-slot""#);
        let manifest =
            PluginManifest::from_toml(&toml_str).expect("unknown slot should still parse");
        assert_eq!(manifest.slot.as_deref(), Some("not-a-real-slot"));
        assert_eq!(manifest.warnings(), vec!["not-a-real-slot".to_string()]);
    }

    #[test]
    fn unknown_category_and_unknown_slot_both_surface_as_warnings() {
        let toml_str = minimal_manifest(
            r#"
categories = ["not-a-real-category"]
slot = "not-a-real-slot"
"#,
        );
        let manifest =
            PluginManifest::from_toml(&toml_str).expect("unknown category+slot should still parse");
        assert_eq!(
            manifest.warnings(),
            vec![
                "not-a-real-category".to_string(),
                "not-a-real-slot".to_string()
            ]
        );
    }

    // ---------- network allowlist grammar (moved from bundle.rs) ----------

    #[test]
    fn accepts_a_bare_hostname() {
        let toml_str = manifest_with_network("api.github.com");
        let manifest = PluginManifest::from_toml(&toml_str).expect("bare hostname should validate");
        assert_eq!(manifest.permissions.network[0].0, "api.github.com");
    }

    #[test]
    fn accepts_a_wildcard_hostname() {
        let toml_str = manifest_with_network("*.github.com");
        let manifest =
            PluginManifest::from_toml(&toml_str).expect("wildcard hostname should validate");
        assert_eq!(manifest.permissions.network[0].0, "*.github.com");
    }

    #[test]
    fn rejects_network_host_with_scheme() {
        let toml_str = manifest_with_network("https://api.github.com");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("scheme in network host should fail validation");
        assert!(
            matches!(err, ManifestError::InvalidNetworkHost(h) if h == "https://api.github.com")
        );
    }

    #[test]
    fn rejects_network_host_with_scheme_and_path() {
        let toml_str = manifest_with_network("https://api.github.com/v3");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("scheme + path in network host should fail validation");
        assert!(
            matches!(err, ManifestError::InvalidNetworkHost(h) if h == "https://api.github.com/v3")
        );
    }

    #[test]
    fn rejects_network_host_with_bare_path() {
        let toml_str = manifest_with_network("api.github.com/v3");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("path in network host should fail validation");
        assert!(matches!(err, ManifestError::InvalidNetworkHost(h) if h == "api.github.com/v3"));
    }

    #[test]
    fn rejects_network_host_with_port() {
        let toml_str = manifest_with_network("api.github.com:443");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("port in network host should fail validation");
        assert!(matches!(err, ManifestError::InvalidNetworkHost(h) if h == "api.github.com:443"));
    }

    #[test]
    fn rejects_network_host_that_is_an_ip_literal() {
        let toml_str = manifest_with_network("192.168.1.1");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("IP literal network host should fail validation");
        assert!(matches!(err, ManifestError::InvalidNetworkHost(h) if h == "192.168.1.1"));
    }

    #[test]
    fn rejects_network_host_that_is_an_ipv6_literal() {
        let toml_str = manifest_with_network("::1");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("IPv6 literal network host should fail validation");
        assert!(matches!(err, ManifestError::InvalidNetworkHost(h) if h == "::1"));
    }

    #[test]
    fn rejects_bare_wildcard_network_host() {
        let toml_str = manifest_with_network("*");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("bare wildcard network host should fail validation");
        assert!(matches!(err, ManifestError::InvalidNetworkHost(h) if h == "*"));
    }

    #[test]
    fn rejects_wildcard_with_nothing_after_the_dot() {
        let toml_str = manifest_with_network("*.");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("dangling wildcard suffix should fail validation");
        assert!(matches!(err, ManifestError::InvalidNetworkHost(h) if h == "*."));
    }

    #[test]
    fn rejects_wildcard_not_at_the_leading_position() {
        let toml_str = manifest_with_network("api.*.github.com");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("mid-string wildcard should fail validation");
        assert!(matches!(err, ManifestError::InvalidNetworkHost(h) if h == "api.*.github.com"));
    }

    #[test]
    fn rejects_wildcard_without_a_dot_separator() {
        let toml_str = manifest_with_network("*github.com");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("wildcard without a dot separator should fail validation");
        assert!(matches!(err, ManifestError::InvalidNetworkHost(h) if h == "*github.com"));
    }

    #[test]
    fn rejects_uppercase_network_host() {
        let toml_str = manifest_with_network("API.github.com");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("uppercase network host should fail validation");
        assert!(matches!(err, ManifestError::InvalidNetworkHost(h) if h == "API.github.com"));
    }

    #[test]
    fn rejects_blank_network_host() {
        let toml_str = manifest_with_network("");
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("blank network host should fail validation");
        assert!(matches!(err, ManifestError::InvalidNetworkHost(h) if h.is_empty()));
    }

    // ---------- oauth profiles (moved from bundle.rs) ----------

    #[test]
    fn rejects_non_https_authorize_url() {
        let toml_str = minimal_manifest(
            r#"
[[oauth]]
id = "acme-cloud"
authorize-url = "http://example.com/authorize"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("non-https authorize-url should fail validation");
        assert!(matches!(
            err,
            ManifestError::InsecureOauthUrl { ref profile, field }
                if profile == "acme-cloud" && field == "authorize-url"
        ));
    }

    #[test]
    fn rejects_non_https_token_url() {
        let toml_str = minimal_manifest(
            r#"
[[oauth]]
id = "acme-cloud"
token-url = "http://relay.example.com/token/acme"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("non-https token-url should fail validation");
        assert!(matches!(
            err,
            ManifestError::InsecureOauthUrl { ref profile, field }
                if profile == "acme-cloud" && field == "token-url"
        ));
    }

    #[test]
    fn rejects_non_https_device_authorization_url() {
        let toml_str = minimal_manifest(
            r#"
[[oauth]]
id = "acme-cloud"
device-authorization-url = "http://example.com/device/code"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("non-https device-authorization-url should fail validation");
        assert!(matches!(
            err,
            ManifestError::InsecureOauthUrl { ref profile, field }
                if profile == "acme-cloud" && field == "device-authorization-url"
        ));
    }

    #[test]
    fn rejects_empty_string_token_url() {
        let toml_str = minimal_manifest(
            r#"
[[oauth]]
id = "acme-cloud"
token-url = ""
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("empty token-url should fail validation");
        assert!(matches!(
            err,
            ManifestError::InsecureOauthUrl { ref profile, field }
                if profile == "acme-cloud" && field == "token-url"
        ));
    }

    #[test]
    fn https_oauth_urls_validate() {
        let toml_str = minimal_manifest(
            r#"
[[oauth]]
id = "acme-cloud"
authorize-url = "https://example.com/authorize"
token-url = "https://relay.example.com/token/acme"
device-authorization-url = "https://example.com/device/code"
"#,
        );
        PluginManifest::from_toml(&toml_str).expect("https oauth urls should validate");
    }

    #[test]
    fn oauth_profile_without_any_urls_still_validates() {
        let toml_str = minimal_manifest(
            r#"
[[oauth]]
id = "acme-cloud"
"#,
        );
        PluginManifest::from_toml(&toml_str)
            .expect("an oauth profile declaring no urls should still validate");
    }

    #[test]
    fn rejects_duplicate_oauth_profile_ids() {
        let toml_str = minimal_manifest(
            r#"
[[oauth]]
id = "github"

[[oauth]]
id = "github"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("duplicate oauth profile id should fail validation");
        assert!(matches!(err, ManifestError::DuplicateOAuthProfile(id) if id == "github"));
    }

    #[test]
    fn rejects_empty_oauth_profile_id() {
        let toml_str = minimal_manifest(
            r#"
[[oauth]]
id = ""
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("empty oauth profile id should fail validation");
        assert!(matches!(err, ManifestError::EmptyOAuthProfileId));
    }

    #[test]
    fn oauth_profile_parses_extra_authorize_params() {
        let toml_str = minimal_manifest(
            r#"
[[oauth]]
id = "acme-cloud"
authorize-url = "https://auth.acme.test/authorize"

[oauth.extra-authorize-params]
audience = "api.acme.test"
prompt = "consent"
"#,
        );
        let manifest =
            PluginManifest::from_toml(&toml_str).expect("extra-authorize-params should parse");
        assert_eq!(
            manifest.oauth[0]
                .extra_authorize_params
                .get("audience")
                .map(String::as_str),
            Some("api.acme.test")
        );
        assert_eq!(
            manifest.oauth[0]
                .extra_authorize_params
                .get("prompt")
                .map(String::as_str),
            Some("consent")
        );
    }

    // ---------- declared tools (moved from bundle.rs; now component-gated) ----------

    #[test]
    fn manifest_parses_declared_tools() {
        let toml_str = manifest_with_component(
            r#"
[[tools]]
name = "create_issue"
description = "Open an issue in a repository"
writes = true

[[tools]]
name = "list_issues"
description = "List issues"
"#,
        );
        let m = PluginManifest::from_toml(&toml_str).unwrap();
        assert_eq!(m.tools.len(), 2);
        assert_eq!(m.tools[0].name, "create_issue");
        assert!(m.tools[0].writes);
        assert!(!m.tools[1].writes); // default false
        m.validate().unwrap();
    }

    #[test]
    fn manifest_without_tools_defaults_empty() {
        let toml_str = minimal_manifest("");
        let m = PluginManifest::from_toml(&toml_str).unwrap();
        assert!(m.tools.is_empty());
    }

    #[test]
    fn validate_rejects_duplicate_tool_names() {
        let toml_str = manifest_with_component(
            r#"
[[tools]]
name = "create_issue"
description = "Open an issue in a repository"
writes = true

[[tools]]
name = "create_issue"
description = "Duplicate tool name"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("duplicate tool name should fail validation");
        assert!(matches!(err, ManifestError::DuplicateTool(name) if name == "create_issue"));
    }

    #[test]
    fn rejects_empty_tool_name() {
        let toml_str = manifest_with_component(
            r#"
[[tools]]
name = ""
description = "Nameless tool"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str)
            .expect_err("empty tool name should fail validation");
        assert!(matches!(err, ManifestError::EmptyToolName));
    }

    // ---------- v2: component-backed surfaces ----------

    fn v2_component_fixture() -> String {
        r#"
contract = 2
id = "github"
name = "GitHub"
version = "0.2.0"
publisher = "Ryuzi"

[component]
file = "github.wasm"
wit-api = "^0.1.0"
lifecycle = "per-session"

[permissions]
network = ["api.github.com"]

[[oauth]]
id = "github"
device-authorization-url = "https://github.com/login/device/code"
client-id = "Iv1.public-app-id"

[[tools]]
name = "create_pr"
description = "Open a pull request"
writes = true

[[hooks]]
name = "notify-on-fail"
trigger = "PreToolUse"
action = "webhook.outbound"

[[jobs]]
name = "daily-triage"
schedule = "every day at 9am"
prompt = "Triage new issues."
"#
        .to_string()
    }

    #[test]
    fn parses_the_full_v2_fixture() {
        let m = PluginManifest::from_toml(&v2_component_fixture()).unwrap();
        assert_eq!(m.contract, 2);
        assert_eq!(m.component.as_ref().unwrap().file, "github.wasm");
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.hooks[0].trigger, "PreToolUse");
        assert_eq!(m.jobs[0].schedule, "every day at 9am");
        assert!(!m.gateway);
    }

    #[test]
    fn rejects_contract_1() {
        let toml_str = "contract = 1\nid = \"a\"\nname = \"A\"\n";
        let err = PluginManifest::from_toml(toml_str).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::ContractUnsupported { found: 1 }
        ));
    }

    #[test]
    fn tools_without_component_are_rejected() {
        let toml_str = r#"
contract = 2
id = "a"
name = "A"

[[tools]]
name = "t"
description = "d"
"#;
        let err = PluginManifest::from_toml(toml_str).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::SurfaceRequiresComponent("tools")
        ));
    }

    #[test]
    fn gateway_without_component_is_rejected() {
        let toml_str = "contract = 2\nid = \"a\"\nname = \"A\"\ngateway = true\n";
        let err = PluginManifest::from_toml(toml_str).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::SurfaceRequiresComponent("gateway")
        ));
    }

    #[test]
    fn provider_ids_without_component_are_rejected_but_metadata_only_provider_is_fine() {
        // Builtin catalog rows declare [provider] with format/models and NO ids
        // and NO component — that must stay valid.
        let ok = r#"
contract = 2
id = "openai"
name = "OpenAI"

[provider]
format = "openai"
"#;
        PluginManifest::from_toml(ok).unwrap();

        let bad = r#"
contract = 2
id = "mimo"
name = "MiMo"

[provider]
ids = ["mimo-free"]
"#;
        let err = PluginManifest::from_toml(bad).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::SurfaceRequiresComponent("provider.ids")
        ));
    }

    #[test]
    fn hook_with_unknown_trigger_or_action_is_rejected() {
        let bad_trigger = r#"
contract = 2
id = "a"
name = "A"

[[hooks]]
name = "h"
trigger = "UserPromptSubmit"
action = "webhook.outbound"
"#;
        let err = PluginManifest::from_toml(bad_trigger).unwrap_err();
        assert!(matches!(err, ManifestError::UnknownTrigger(_, _)));

        let bad_action = r#"
contract = 2
id = "a"
name = "A"

[[hooks]]
name = "h"
trigger = "tool.before"
action = "shell.exec"
"#;
        let err = PluginManifest::from_toml(bad_action).unwrap_err();
        assert!(matches!(err, ManifestError::UnknownAction(_, _)));
    }

    #[test]
    fn resolved_provider_ids_semantics() {
        let none = PluginManifest::from_toml("contract = 2\nid = \"a\"\nname = \"A\"\n").unwrap();
        assert!(none.resolved_provider_ids().is_empty());

        let meta_only = PluginManifest::from_toml(
            "contract = 2\nid = \"openai\"\nname = \"OpenAI\"\n\n[provider]\nformat = \"openai\"\n",
        )
        .unwrap();
        assert_eq!(
            meta_only.resolved_provider_ids(),
            vec!["openai".to_string()]
        );
    }

    #[test]
    fn component_requires_semver_version() {
        let toml_str = r#"
contract = 2
id = "a"
name = "A"
version = "not-semver"

[component]
file = "a.wasm"
wit-api = "^0.1.0"
lifecycle = "singleton"
"#;
        let err = PluginManifest::from_toml(toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidVersion(v, _) if v == "not-semver"));
    }

    #[test]
    fn rejects_empty_component_file() {
        let toml_str = r#"
contract = 2
id = "a"
name = "A"
version = "0.1.0"

[component]
file = ""
wit-api = "^0.1.0"
lifecycle = "singleton"
"#;
        let err = PluginManifest::from_toml(toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::EmptyComponent));
    }

    #[test]
    fn rejects_invalid_wit_api_range() {
        let toml_str = r#"
contract = 2
id = "a"
name = "A"
version = "0.1.0"

[component]
file = "a.wasm"
wit-api = "not-a-range"
lifecycle = "singleton"
"#;
        let err = PluginManifest::from_toml(toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidWitApi(v, _) if v == "not-a-range"));
    }

    #[test]
    fn rejects_invalid_provider_id() {
        let toml_str = manifest_with_component(
            r#"
[provider]
ids = ["Mimo Free"]
"#,
        );
        let err = PluginManifest::from_toml(&toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidProviderId(id) if id == "Mimo Free"));
    }

    // ---------- v2: hooks ----------

    #[test]
    fn rejects_empty_hook_name() {
        let toml_str = minimal_manifest(
            r#"
[[hooks]]
name = ""
trigger = "tool.before"
action = "webhook.outbound"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::EmptyHookName));
    }

    #[test]
    fn rejects_duplicate_hook_names() {
        let toml_str = minimal_manifest(
            r#"
[[hooks]]
name = "dup"
trigger = "tool.before"
action = "webhook.outbound"

[[hooks]]
name = "dup"
trigger = "tool.after"
action = "agent.run"
"#,
        );
        let err = PluginManifest::from_toml(&toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::DuplicateHookName(name) if name == "dup"));
    }

    #[test]
    fn hook_config_round_trips_arbitrary_toml_table() {
        let toml_str = minimal_manifest(
            r#"
[[hooks]]
name = "h"
trigger = "tool.before"
action = "agent.run"

[hooks.config]
agent = "triage-bot"
retries = 3
"#,
        );
        let m = PluginManifest::from_toml(&toml_str).expect("hook config table should parse");
        let config = &m.hooks[0].config;
        assert_eq!(
            config.get("agent").and_then(|v| v.as_str()),
            Some("triage-bot")
        );
        assert_eq!(config.get("retries").and_then(|v| v.as_integer()), Some(3));
    }

    #[test]
    fn hook_config_defaults_to_empty_table_when_omitted() {
        let toml_str = minimal_manifest(
            r#"
[[hooks]]
name = "h"
trigger = "tool.before"
action = "agent.run"
"#,
        );
        let m = PluginManifest::from_toml(&toml_str).expect("hook without config should parse");
        assert_eq!(m.hooks[0].config, toml::Value::Table(toml::map::Map::new()));
    }

    // ---------- v2: jobs ----------

    #[test]
    fn rejects_empty_job_name() {
        let toml_str = minimal_manifest(
            r#"
[[jobs]]
name = ""
schedule = "every day at 9am"
prompt = "Triage."
"#,
        );
        let err = PluginManifest::from_toml(&toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::EmptyJobName));
    }

    #[test]
    fn rejects_duplicate_job_names() {
        let toml_str = minimal_manifest(
            r#"
[[jobs]]
name = "dup"
schedule = "every day at 9am"
prompt = "Triage."

[[jobs]]
name = "dup"
schedule = "every day at 5pm"
prompt = "Triage again."
"#,
        );
        let err = PluginManifest::from_toml(&toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::DuplicateJobName(name) if name == "dup"));
    }

    #[test]
    fn rejects_empty_job_schedule() {
        let toml_str = minimal_manifest(
            r#"
[[jobs]]
name = "j"
schedule = ""
prompt = "Triage."
"#,
        );
        let err = PluginManifest::from_toml(&toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::EmptyJobSchedule(name) if name == "j"));
    }

    #[test]
    fn rejects_empty_job_prompt() {
        let toml_str = minimal_manifest(
            r#"
[[jobs]]
name = "j"
schedule = "every day at 9am"
prompt = ""
"#,
        );
        let err = PluginManifest::from_toml(&toml_str).unwrap_err();
        assert!(matches!(err, ManifestError::EmptyJobPrompt(name) if name == "j"));
    }

    // ---------- contract-1 compat shim ----------
    //
    // The two fixtures below are verbatim excerpts of manifests actually
    // published on the pre-v2 release feed (v0.8.0). They are the reason the
    // shim exists: without it every component install from that feed dies at
    // `component = "<name>.wasm"` with `invalid type: string, expected struct
    // ComponentSpec`.

    /// The published v1 `mimo.ryuzi-plugin.toml` — a PROVIDER bundle.
    const CONTRACT_1_PROVIDER: &str = r#"
id = "mimo"
name = "MiMo (free)"
version = "0.1.0"
wit-api = ">=0.1.0, <0.2.0"
lifecycle = "per-call"
component = "mimo.wasm"
publisher = "Ryuzi"
description = "Xiaomi MiMo free-tier chat provider."
provider-ids = ["mimo-free"]

[permissions]
network = ["api.xiaomimimo.com"]
"#;

    /// The published v1 `github.ryuzi-plugin.toml` — a CONNECTOR bundle with
    /// an OAuth profile and declared tools.
    const CONTRACT_1_CONNECTOR: &str = r#"
id = "github"
name = "GitHub"
version = "0.1.1"
wit-api = ">=0.1.0, <0.2.0"
lifecycle = "per-call"
component = "github.wasm"
publisher = "Ryuzi"
description = "GitHub connector."

[permissions]
network = ["api.github.com", "github.com"]

[[oauth]]
id = "github"
authorize-url = "https://github.com/login/oauth/authorize"
device-authorization-url = "https://github.com/login/device/code"
scopes = ["repo", "read:org", "user"]
client-id = "Ov23lijhiwiIgxoH2VcV"
dynamic-registration = false

[[tools]]
name = "repo_list"
description = "List repositories."

[[tools]]
name = "issue_create"
description = "Create an issue."
writes = true
"#;

    /// The published v1 `discord.ryuzi-plugin.toml` — a GATEWAY bundle, which
    /// v1 had no way to declare as one.
    const CONTRACT_1_GATEWAY: &str = r#"
id = "discord"
name = "Discord"
version = "0.1.0"
wit-api = ">=0.1.0, <0.2.0"
lifecycle = "singleton"
component = "discord.wasm"
publisher = "Ryuzi"
description = "Discord gateway."

[permissions]
network = ["gateway.discord.gg", "*.discord.gg", "discord.com"]
"#;

    #[test]
    fn upgrades_a_contract_1_provider_bundle() {
        let (m, upgraded) = PluginManifest::from_toml_detecting_legacy(CONTRACT_1_PROVIDER)
            .expect("a published v1 provider bundle must stay installable");
        assert!(upgraded, "must be reported as a contract-1 upgrade");
        assert_eq!(m.contract, CONTRACT_VERSION);
        assert_eq!(m.id, "mimo");
        assert_eq!(m.name, "MiMo (free)");
        assert_eq!(m.version, "0.1.0");
        assert_eq!(m.publisher, "Ryuzi");

        // The flat v1 component triple lands under [component] verbatim.
        let component = m.component.as_ref().expect("component");
        assert_eq!(component.file, "mimo.wasm");
        assert_eq!(component.wit_api, ">=0.1.0, <0.2.0");
        assert_eq!(component.lifecycle, PluginLifecycle::PerCall);

        // `provider-ids` becomes [provider] ids — the router id a v1 provider
        // bundle served must survive, or the transport registers under the
        // wrong id (`mimo` instead of `mimo-free`).
        assert_eq!(m.resolved_provider_ids(), vec!["mimo-free".to_string()]);
        assert_eq!(m.permissions.network[0].0, "api.xiaomimimo.com");
    }

    #[test]
    fn upgrades_a_contract_1_connector_bundle_with_oauth_and_tools() {
        let (m, upgraded) = PluginManifest::from_toml_detecting_legacy(CONTRACT_1_CONNECTOR)
            .expect("a published v1 connector bundle must stay installable");
        assert!(upgraded);
        assert_eq!(m.component.as_ref().unwrap().file, "github.wasm");

        assert_eq!(m.oauth.len(), 1);
        assert_eq!(m.oauth[0].id, "github");
        assert_eq!(
            m.oauth[0].client_id.as_deref(),
            Some("Ov23lijhiwiIgxoH2VcV")
        );
        assert_eq!(
            m.oauth[0].device_authorization_url.as_deref(),
            Some("https://github.com/login/device/code")
        );

        assert_eq!(m.tools.len(), 2);
        assert_eq!(m.tools[0].name, "repo_list");
        assert!(!m.tools[0].writes);
        assert!(m.tools[1].writes);

        // A connector names no router ids, so it must not become a provider.
        assert!(m.provider.is_none());
        assert!(m.resolved_provider_ids().is_empty());
    }

    // A v1 gateway (Discord) could not declare itself one — the host compiled
    // the component and read its exports. The upgrade must therefore leave the
    // bundle eligible for gateway discovery, or Discord installs cleanly and
    // then silently never attaches.
    #[test]
    fn upgraded_contract_1_bundles_stay_eligible_for_gateway_discovery() {
        let (m, upgraded) = PluginManifest::from_toml_detecting_legacy(CONTRACT_1_GATEWAY).unwrap();
        assert!(upgraded);
        assert!(
            m.gateway,
            "a v1 bundle has no gateway declaration, so discovery must fall back to \
             compiling it and checking `exports_gateway()` exactly as v1 did"
        );
        assert_eq!(
            m.component.as_ref().unwrap().lifecycle,
            PluginLifecycle::Singleton
        );
    }

    // The shim reshapes; it must never relax a rule. A v1 manifest that would
    // fail v2 validation still fails it.
    #[test]
    fn an_upgraded_contract_1_bundle_is_still_validated() {
        let bad_host = r#"
id = "acme"
name = "Acme"
version = "0.1.0"
wit-api = ">=0.1.0, <0.2.0"
lifecycle = "per-call"
component = "acme.wasm"

[permissions]
network = ["https://api.acme.com/v1"]
"#;
        let err = PluginManifest::from_toml(bad_host)
            .expect_err("the v2 network grammar still applies after an upgrade");
        assert!(
            matches!(err, ManifestError::InvalidNetworkHost(h) if h == "https://api.acme.com/v1")
        );
    }

    // An explicit `contract = N` is a deliberate claim: it must keep failing
    // loudly instead of being silently reinterpreted as a v1 bundle.
    #[test]
    fn an_explicit_unsupported_contract_is_never_treated_as_contract_1() {
        let toml_str = r#"
contract = 1
id = "mimo"
name = "MiMo"
version = "0.1.0"
wit-api = ">=0.1.0, <0.2.0"
lifecycle = "per-call"
component = "mimo.wasm"
"#;
        let err = PluginManifest::from_toml(toml_str).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::ContractUnsupported { found: 1 }
        ));
    }

    // A v2-shaped manifest with a real mistake must report ITS OWN error. The
    // shim must not swallow it and complain the input isn't a v1 bundle.
    #[test]
    fn a_broken_contract_2_manifest_reports_the_contract_2_error() {
        let missing_contract = r#"
id = "acme"
name = "Acme"
version = "0.1.0"

[component]
file = "acme.wasm"
wit-api = "^0.1.0"
lifecycle = "singleton"
"#;
        let err = PluginManifest::from_toml(missing_contract).unwrap_err();
        let message = err.to_string();
        assert!(
            matches!(err, ManifestError::Toml(_)) && message.contains("contract"),
            "expected the v2 missing-field error, got: {message}"
        );
    }

    #[test]
    fn a_native_contract_2_manifest_is_not_reported_as_upgraded() {
        let (m, upgraded) =
            PluginManifest::from_toml_detecting_legacy(&v2_component_fixture()).unwrap();
        assert!(!upgraded);
        assert_eq!(m.contract, 2);
        assert!(!m.gateway, "the v1 gateway fallback must not leak into v2");
    }
}
