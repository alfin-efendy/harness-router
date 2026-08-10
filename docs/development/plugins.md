# Plugin SDK

Ryuzi's extension points — model providers, connectors (GitHub, Atlassian,
Bitbucket...), automation presets, custom slash commands, and skills — are
all **plugins**: one manifest each, surfaced identically through the
daemon's `list_plugins` RPC and Cockpit's Plugins hub. There is no CLI
surface for plugin management — Cockpit (backed by the daemon's RPCs) is the
only management surface. (Chat-platform gateways are ALSO plugins under the
hood, but that surface is internal and first-party-only — see
[Internal surfaces: gateway](#internal-surfaces-gateway) — not something a
third-party plugin author can build.)

This guide documents **manifest contract 2** — the ONE schema every plugin
(first-party built-in, signed catalog, or a hand-installed local/git source)
satisfies today. It documents what is actually implemented on this branch —
verify any command shown here still matches the daemon's current RPC surface
if you're reading this on a different revision. See
[Removed in v2](#removed-in-v2) for what contract 1 used to offer and no
longer does.

---

## Two layers: manifest vs. `CorePlugin`

Every plugin has a **declarative** half and a **behavioral** half, owned by
two different crates:

- **`crates/plugin-sdk`** (`ryuzi-plugin-sdk`) owns the manifest contract:
  `PluginManifest` and its nested types, parsing (TOML) and structural
  `validate()`, the category vocabulary (`categories::KNOWN`), and the known
  automation trigger vocabulary (`triggers::CANONICAL_TRIGGERS` +
  `CLAUDE_ALIASES`). It depends on nothing but `serde`, `serde_json`, `toml`,
  `semver`, and `thiserror` — no `ryuzi-core` dependency — so it's the small,
  stable contract a plugin author (or another crate) targets.
- **`crates/core/src/plugins/`** owns the binding: `CorePlugin { manifest,
  harness, gateway, connector, source }` pairs a manifest with the runtime
  capability it actually provides, `PluginHost` tracks every installed
  plugin, and `Registries` (`harness`/`gateway`/`connector`/`plugins`) is the
  composition root the daemon builds at startup inside
  `ryuzi_core::daemon::build_daemon` — used by both the runner (`ryuzi
  start`) and Cockpit's `--engine-daemon` mode.

Manifests come from two places, merged in this order (first registration for
a given `id` wins — see `PluginHost::add`/`Registries::add_plugin`):

1. **Rust built-ins, at daemon startup** (`plugins::install_builtins`):
   - `install_providers` — one manifest-only `[provider]` plugin generated
     per entry in the static provider catalog
     (`crates/core/src/llm_router/registry.rs::CATALOG`). Registered FIRST so
     a same-id component bundle below never displaces a builtin's richer
     manifest (seed models, auth spec).
   - `component_catalog::component_catalog_plugins()` — the six first-party
     component bundles whose manifest is `include_str!`-embedded so they are
     enumerable (in `list_plugins`) even before install: `github`,
     `atlassian`, `bitbucket`, `discord`, `mimo`, `opencode`. The twelve
     same-named provider bundles under `plugins/{openai,anthropic,...}` are
     deliberately **not** registered a second time here — their bundle id
     already won as a provider builtin above, and are still reported
     `componentBacked: true` by `list_plugins` via
     `component_catalog::is_component_bundle` (checked against
     `COMPONENT_BACKED_PROVIDER_IDS`) so Cockpit can offer release management
     for whichever manifest actually won the id.
   - `harness::native::native_plugin()` — the in-process agent harness,
     registered unconditionally, always enabled, not toggleable.
2. **On-disk installs**, under `~/.config/ryuzi/plugins/<id>/`, discovered by
   `plugins::bundle::load_active_bundles` (component-backed) and
   `PluginHost`'s own directory scan (declarative-only manifests with no
   `[component]`) — every plugin a user installed through the signed catalog,
   a local folder, or a git URL (see
   [Install sources and trust tiers](#install-sources-and-trust-tiers)).
   `component_catalog`'s embedded entries above are placeholders for
   *discovery*; the actual runtime capability (gateway/connector/provider) of
   an installed component always comes from the on-disk, verified bundle —
   nothing in `component_catalog` ever instantiates a component.

Because plugin registration runs once at daemon startup, installing,
updating, or enabling a plugin sets an in-memory "restart required" flag
(`plugins_restart_required` RPC) rather than hot-loading — there is no
hot-reload.

---

## Manifest reference

One plugin = one manifest, `ryuzi-plugin.toml`. Every field
(`ryuzi_plugin_sdk::manifest::PluginManifest`); "Rules" repeats the exact
`validate()`/`ManifestError` behavior, not just the type:

| Field | Type | Default | Rules |
| --- | --- | --- | --- |
| `contract` | integer | *(required)* | Must be exactly `2` (`CONTRACT_VERSION`) — `validate()` rejects anything else with `ContractUnsupported { found }`, including `1`. Big-bang migration: there is no compat loader for contract 1. |
| `id` | string | *(required)* | Kebab-case: first char a lowercase ASCII letter or digit, then only lowercase letters/digits/`-`. Anything else → `InvalidId`. |
| `name` | string | *(required)* | Must be non-empty (`EmptyName`). |
| `version` | string | `""` | Free-form UNLESS a `[component]` block is present, in which case it must parse as `semver::Version` (`InvalidVersion`) — a component-backed plugin must version itself. |
| `publisher` | string | `""` | |
| `description` | string | `""` | |
| `homepage` | string \| null | `None` | |
| `icon` | string \| null | `None` | A lucide icon name; Cockpit maps a small explicit set and falls back to a generic puzzle icon otherwise (`apps/cockpit/src/lib/plugin-icons.ts`). |
| `categories` | string[] | `[]` | See [Category vocabulary](#category-vocabulary). Unknown labels are a non-fatal `warnings()` entry, never a validation error. |
| `slot` | string \| null | `None` | See [Exclusive capability slots](#exclusive-capability-slots). Unknown slot names also only warn. |
| `verified` | bool | `false` | Drives the `verified`/`experimental`/`community` status label. |
| `experimental` | bool | `false` | Docs-only entry. |
| `auth` | `[auth]` table \| null | `None` | See [`[auth]`](#auth). |
| `settings` | `[[settings]]` array | `[]` | See [`[[settings]]`](#settings). |
| `component` | `[component]` table \| null | `None` | See [`[component]`](#component). Required by `provider.ids`, `tools`, and `gateway` — see their rows. |
| `permissions` | `[permissions]` table | `{ network: [] }` | See [`[permissions]`](#permissions). |
| `oauth` | `[[oauth]]` array | `[]` | See [`[[oauth]]`](#oauth). |
| `provider` | `[provider]` table \| null | `None` | See [`[provider]`](#provider). |
| `tools` | `[[tools]]` array | `[]` | Component-backed MCP tools, statically declared so Cockpit shows "what you'll get" pre-install. Non-empty requires `[component]` (`SurfaceRequiresComponent("tools")`). Empty name → `EmptyToolName`; duplicate `name` → `DuplicateTool`. |
| `mcp` | `[[mcp]]` array | `[]` | External MCP servers — see [MCP server defs](#mcp-server-defs). |
| `hooks` | `[[hooks]]` array | `[]` | Declarative automation hooks — see [Automation sync](#automation-sync-hooks-and-jobs). |
| `jobs` | `[[jobs]]` array | `[]` | Scheduled-job presets — see [Automation sync](#automation-sync-hooks-and-jobs). |
| `gateway` | bool | `false` | **INTERNAL, first-party-only** — see [Internal surfaces: gateway](#internal-surfaces-gateway). `true` requires `[component]` (`SurfaceRequiresComponent("gateway")`). |

### `[auth]`

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `kind` | `"none"` \| `"api-key"` \| `"token"` \| `"oauth"` | `"none"` | |
| `setting` | string \| null | `None` | Settings-store key holding the secret. |
| `env` | string \| null | `None` | Fallback environment variable. |
| `help_url` (or `help-url`) | string \| null | `None` | Both spellings accepted. |
| `authorize-url` / `token-url` | string \| null | `None` | Declarative connector OAuth endpoints. |
| `resource` | string \| null | `None` | |
| `scopes` | string[] | `[]` | |
| `client-id-setting` / `client-secret-setting` | string \| null | `None` | |
| `dynamic-registration` | bool | `false` | RFC 7591 DCR attempt. |
| `extra-authorize-params` / `extra-token-params` | table (string→string) | `{}` | |

An MCP entry's `${auth}` placeholder with no `[auth]` block present is
`AuthPlaceholderWithoutAuth`.

### `[[settings]]`

| Field | Type | Default | Rules |
| --- | --- | --- | --- |
| `key` | string | *(required)* | Must NOT start with `"plugin."` — `SettingKeyPrefixForbidden`. Settings keys are **bare**; the host prefixes `plugin.<id>.` when bridging to the settings store. Duplicate keys within one manifest → `DuplicateSettingKey`. |
| `label` | string | *(required)* | |
| `help` | string | `""` | |
| `secret` | bool | `false` | |
| `required` | bool | `false` | |
| `kind` | `"string"` \| `"int"` \| `"bool"` | `"string"` | |
| `options` | string[] | `[]` | Non-empty makes this an enum. Requires `kind = "string"` — otherwise `SettingOptionsRequireStringKind`. |
| `default` | string \| null | `None` | When `options` is non-empty, `default` (if set) must be a member — otherwise `SettingDefaultNotInOptions`. |

### `[component]`

Declares the WASM component bundle a plugin ships. Presence gates three
other surfaces (`provider.ids`, `tools`, `gateway` — see the top-level
table); absence with any of them set is `SurfaceRequiresComponent(surface)`.

| Field | Type | Rules |
| --- | --- | --- |
| `file` | string | Non-empty — `EmptyComponent`. The wasm filename, resolved relative to the manifest's own directory (or the release's `component_url` for a signed install). |
| `wit-api` | string | A Cargo-style semver **range** (e.g. `">=0.1.0, <0.2.0"`), parsed with `semver::VersionReq` — invalid syntax is `InvalidWitApi`. |
| `lifecycle` | `"singleton"` \| `"per-session"` \| `"per-call"` | How the host instances the component: one shared instance for the whole process, one per session, or a fresh instance per call. |

### `[permissions]`

| Field | Type | Rules |
| --- | --- | --- |
| `network` | string[] | Each entry: a bare lowercase hostname (`api.github.com`) or a `*.`-prefixed wildcard (`*.github.com`). Rejects (all as `InvalidNetworkHost`): a scheme (`://`), a path or port (`/`, `:`), whitespace, an IP literal (v4 or v6), a bare `*`, `*.` with nothing after it, a wildcard anywhere but the leading position, uppercase characters, and blank input. This is the component's outbound allowlist — enforced by the host on every `ryuzi:http`/OAuth-egress request and re-checked on every redirect hop. |

### `[[oauth]]`

One host-managed OAuth profile a component may authenticate through
(`ryuzi:oauth/oauth`'s `authorized-request`/`disconnect`) — the component
itself never drives PKCE/device-flow and never sees a token.

| Field | Type | Rules |
| --- | --- | --- |
| `id` | string | Non-empty (`EmptyOAuthProfileId`); unique within the manifest (`DuplicateOAuthProfile`). Must equal the string the component passes to `authorized-request`. |
| `authorize-url` / `token-url` / `device-authorization-url` | string \| null | Each, if present, must be **non-empty and start with `https://`** — an insecure or empty value is `InsecureOauthUrl { profile, field }`. `device-authorization-url` is the RFC 8628 endpoint for a component that supports the device grant. |
| `scopes` | string[] | |
| `client-id` | string \| null | A first-party PUBLIC OAuth client id baked into the (signed) manifest — the `gh` CLI model, so an end-user connects with zero configuration. Public, not a secret. A `client-id-setting` value or a stored per-install client id still wins over this default. |
| `client-id-setting` / `client-secret-setting` | string \| null | |
| `resource` | string \| null | |
| `dynamic-registration` | bool | |
| `extra-authorize-params` | table (string→string) | Forwarded verbatim by the host's `begin_pkce` (e.g. Atlassian's mandatory `audience=api.atlassian.com`). |

### `[provider]`

| Field | Type | Notes |
| --- | --- | --- |
| `ids` | string[] | The llm-router provider id(s) this plugin serves. **Non-empty `ids` requires `[component]`** — `SurfaceRequiresComponent("provider.ids")`. Each id must be a valid plugin-id-shaped string (`InvalidProviderId`). |
| `format` | string \| null | e.g. `"anthropic"`, `"openai"` — metadata-only, used by generated built-in provider plugins; does NOT require a component. |
| `base_url` | string \| null | |
| `models` | `[{ id, label?, default? }]` | |

`PluginManifest::resolved_provider_ids()`: `provider.ids` when non-empty,
else `[self.id]` when a `[provider]` block exists at all (even metadata-only,
`ids = []`), else empty (not a provider plugin at all).

#### Two provider WIT interfaces: 0.1.0 (flat) and 0.2.0 (tool-carrying)

A component-backed provider (`[component]` + non-empty `provider.ids`) exports
one of two versions of `ryuzi:provider/provider`, declared implicitly by
whichever the component's `wit/world.wit` actually `export`s and covered by
its manifest's `wit-api` range:

- **`ryuzi:provider/provider@0.1.0`** — the original, flat-prompt interface.
  Its `complete` takes a single string `prompt` and returns chunks with no
  tool channel at all; a 0.1.0-only component can never carry tools.
- **`ryuzi:provider/provider@0.2.0`** — the structured, tool-carrying
  interface. Its own `complete` (same function name — the two are
  disambiguated by package version, not a different function name) takes a
  full transcript (`messages`, each with typed content blocks — text,
  tool-use, tool-result) plus a `tools` list and a `tool-choice`, and can
  return tool calls the router turns into `tool_use` events.

The host implements **both simultaneously** — see `HOST_WIT_API_VERSIONS` in
`crates/core/src/plugins/runtime.rs`, which lists `["0.1.0", "0.2.0"]` and
accepts a component whose `wit-api` range matches either. A component may
export only 0.1.0, only 0.2.0, or (in principle) both; the host **prefers
0.2.0** and negotiates per component, at discovery time, by reading which
export(s) the compiled component actually has
(`WasmProviderTransport::exports_provider` /
`exports_provider_v2` in `crates/core/src/plugins/wasm_provider.rs`) — this is
a per-component decision, not a global host setting, so a mixed fleet (some
providers still 0.1.0-only, some migrated to 0.2.0) is fully supported.

Discovery then resolves the component's own `capabilities()` export **once**
and caches it (`WasmProviderTransport::new_resolved`). Two *different*
questions are in play once the router has a routed connection in hand, and
they must never be conflated:

- **Which ABI to call** is decided ENTIRELY by `exports_provider_v2()`
  (exposed to the router as `WasmProviderRuntime::speaks_structured_abi`) —
  never by `capabilities().tools`. A component that exports 0.2.0 always
  goes through the structured `complete_v2`; only a component with no 0.2.0
  export at all falls back to the flat 0.1.0 `complete`.
- **Whether tools may be forwarded** is `capabilities().tools`, consulted
  only after the ABI is already decided. It changes what a 0.2.0 request
  carries, never which function is called: `tools: true` forwards the
  request's bound tools and its `tool_choice` as given; `tools: false`
  still calls `complete_v2`, but with an empty `tools` list and
  `tool_choice: none`.

So there are three cases, not two:

| component exports | `capabilities().tools` | path |
| --- | --- | --- |
| 0.2.0 | `true` | `complete_v2`, tools forwarded |
| 0.2.0 | `false` | `complete_v2`, empty tools list |
| 0.1.0 only | *(no 0.2.0 export to ask)* | `complete`, flat prompt |

The middle row matters in practice, not just in principle: `mimo`'s
free-tier component exports ONLY 0.2.0 and honestly reports `tools: false`
(its live probe found no evidence the upstream accepts a tools array).
Keying the ABI choice on `capabilities().tools` instead of the export would
route every `mimo` turn into the flat `complete` — which a 0.2.0-only
component never exports — failing every turn outright rather than returning
a toolless completion. In practice, a real 0.2.0 component should answer
`capabilities().tools = true` only if it genuinely forwards the bound tool
list to its upstream and can return tool-use content back; a component that
exports 0.2.0 but never forwards tools (or whose upstream doesn't support
tool-calling) must still report `tools: false`, or the router will hand it
tool definitions it silently drops. `capabilities()` fails closed: an error,
trap, or timeout while resolving it — and a transport that has never been
resolved at all — is always treated as `tools: false`, never optimistically
tool-capable.

### `[[tools]]`

See the top-level table above; full field shape:

| Field | Type | Default |
| --- | --- | --- |
| `name` | string | *(required, non-empty, unique)* |
| `description` | string | `""` |
| `writes` | bool | `false` — marks a mutating tool; drives the trust-prompt's per-tool disclosure (see [Install sources](#install-sources-and-trust-tiers)) and is what a component itself is expected to gate an unconfirmed mutation behind (`confirm=true`), not something the host enforces structurally. |

### MCP server defs

One `[[mcp]]` entry (`McpServerDef`) declares an external MCP server the
connector attaches at session start — no `[component]` required:

| Field | Type | Default | Rules |
| --- | --- | --- | --- |
| `name` | string | *(required)* | Unique within the manifest — `DuplicateMcpName`. |
| `transport` | `"stdio"` \| `"http"` | *(required)* | |
| `command` | string \| null | `None` | Required when `transport = "stdio"` — otherwise `MissingCommand`. May contain a `${auth}`/`${setting:KEY}`/`${env:VAR}` placeholder. |
| `args` | string[] | `[]` | Same placeholder grammar as `command`. |
| `env` | table (string→string) | `{}` | Values may use the placeholder grammar. |
| `url` | string \| null | `None` | Required when `transport = "http"` — otherwise `MissingUrl`. |
| `headers` | table (string→string) | `{}` | Values may use the placeholder grammar — e.g. `Authorization = "Bearer ${auth}"`. |

A `${auth}` placeholder anywhere in `command`/`args`/`env`/`url`/`headers`
with no `[auth]` block present on the manifest is
`AuthPlaceholderWithoutAuth`.

### `[[hooks]]`

| Field | Type | Rules |
| --- | --- | --- |
| `name` | string | Non-empty (`EmptyHookName`); unique within the manifest (`DuplicateHookName`). |
| `trigger` | string | Must resolve via `triggers::canonical_trigger` — either a canonical dotted name or a known Claude-alias spelling (below) — else `UnknownTrigger`. |
| `action` | string | One of `KNOWN_HOOK_ACTIONS`: `"agent.run"`, `"webhook.outbound"` — else `UnknownAction`. |
| `config` | TOML table | Action-specific config, shape-checked by `ryuzi-core` at sync time against the matching `HookActionInput` variant (`deny_unknown_fields`) — the SDK itself never validates this shape. |

Known trigger spellings (`ryuzi_plugin_sdk::triggers`):

| Canonical | Claude-alias spellings accepted |
| --- | --- |
| `session.start` | `SessionStart` |
| `tool.before` | `PreToolUse` |
| `tool.after` | `PostToolUse` |
| `session.end` | `Stop`, `SessionEnd` |
| `scheduler.run.success` | — |
| `scheduler.run.failed` | — |
| `gateway.status.changed` | — |
| `webhook.inbound` | — |

`webhook.inbound` hooks only support the `agent.run` action — any other
action on that trigger is rejected at sync time (`ryuzi-core`, not the SDK).

### `[[jobs]]`

| Field | Type | Rules |
| --- | --- | --- |
| `name` | string | Non-empty (`EmptyJobName`); unique within the manifest (`DuplicateJobName`). |
| `schedule` | string | Non-empty (`EmptyJobSchedule`) — natural language (`"every day at 9am"`) or cron; parsed by `ryuzi-core`'s scheduler at sync time, not by the SDK. |
| `prompt` | string | Non-empty (`EmptyJobPrompt`). |
| `model_override` | string \| null | `None` |

### Full annotated example

The real, shipping `plugins/github/ryuzi-plugin.toml` (trimmed to its shape;
see the file itself for the complete tool list):

```toml
contract = 2
id = "github"
name = "GitHub"
version = "0.1.1"
publisher = "Ryuzi"
description = "GitHub connector: auth status, repositories, issues, and pull requests..."

[component]
file = "github.wasm"
wit-api = ">=0.1.0, <0.2.0"
lifecycle = "per-call"

[permissions]
network = ["api.github.com", "github.com"]

[[oauth]]
id = "github"
authorize-url = "https://github.com/login/oauth/authorize"
token-url = "https://github.com/login/oauth/access_token"
device-authorization-url = "https://github.com/login/device/code"
scopes = ["repo", "read:org", "user"]
client-id = "Ov23lijhiwiIgxoH2VcV"
dynamic-registration = false

[[tools]]
name = "auth_status"
description = "Report whether GitHub is connected, and the authenticated login/name when it is."

[[tools]]
name = "issue_create"
description = "Create an issue. Mutating: requires confirm=true."
writes = true
```

There is no CLI surface to validate a manifest (the runner's command surface
is `setup`, `start`, `status`, `service`, `config`, `doctor` — no `plugins`
subcommand). The daemon's `plugin_detail` RPC (`{ id: "github" }`) returns
the manifest's fields as JSON, and Cockpit's plugin detail screen renders
them directly. `status` is `verified` when `verified = true`; otherwise
`experimental` when `experimental = true`; otherwise `community`.

---

## The five public manifest surfaces

A manifest can carry any combination of five surfaces a plugin author can
target (the SDK's own doc comments call these out explicitly on
`PluginManifest`'s fields):

1. **`[provider]`** — a model-provider transport (llm-router divert). Needs
   `[component]` only when `provider.ids` is non-empty; a metadata-only
   `[provider]` (built-in catalog rows) needs no component.
2. **`[[tools]]`** — MCP tools backed by the component's
   `ryuzi:connector/connector` export. Always needs `[component]`.
3. **`[[mcp]]`** — MCP tools via an external server (stdio or http), no
   component required.
4. **`[[hooks]]`** — declarative automation hooks synced into the Automation
   domain.
5. **`[[jobs]]`** — scheduled-job presets synced into the Scheduler domain.

Both (2) and (3) end up as ordinary
[`mcp__<id>__<tool>`](#mcp-tool-naming-and-per-tool-perms) tools in a
session's tool registry, governed by the exact same permission model.

### Directory-convention surfaces: commands and skills

Beyond the five manifest FIELD surfaces above, an installed plugin gets two
more surfaces purely from directory convention — neither is a manifest
field, and nothing in `ryuzi-plugin.toml` turns them on or off.
`PluginTrustPrompt.surfaces.commands`/`.skills` (the trust-prompt summary
`install_sources::begin_plugin_install_from_source` shows before install)
are just counts of what's already sitting in the staged directory
(`count_markdown_files`/`count_skill_dirs`). Both are discovered LIVE from
an ENABLED, installed plugin's directory — never copied into
`~/.config/ryuzi/...` — via the same source:
`crate::control::ControlPlane::enabled_plugin_content_roots`'s union of
every currently-enabled plugin's `commands/`/`skills/` subdirectories,
re-scanned fresh on every session/catalog load. Disabling or uninstalling
the plugin makes its commands/skills vanish immediately, no cleanup step
needed.

- **`commands/*.md`** — one custom slash command per markdown file, same
  format project/global commands use
  (`crates/core/src/harness/native/commands.rs`): a `---`-delimited
  frontmatter block (`description`, `agent`, `model`, `subtask`,
  `surfaces: home,session`, `requires-project: true`) followed by the
  prompt template body (`$ARGUMENTS` / `$1`..`$9` placeholders).
  Loaded via `CommandRegistry::load_with_plugins` /
  `load_without_project_with_plugins`.
  - **Precedence: builtin > project > global > plugin.** A plugin command
    is read FIRST and inserted into the collapse map before every other
    origin, so a same-name project/global/builtin command always wins over
    it — but a displaced plugin command is never silently dropped: it's
    re-registered as `<plugin-id>/<name>` (e.g. `/github/review`), so it
    stays reachable under its namespaced form. The same rule applies when
    two different plugins collide on a name — the loser is namespaced too.
- **`skills/<name>/SKILL.md`** — progressive-disclosure capability docs
  (mirrors opencode/Claude skills, `crates/core/src/harness/native/skills.rs`):
  `name`/`description` frontmatter plus a markdown body fetched on demand
  via the `skill` tool. A plugin may ship either a skills ROOT
  (`skills/<name>/SKILL.md`, several skills) or a single-skill LEAF bundle
  (`skills/SKILL.md` directly) — both shapes auto-detect exactly like a
  project/global skills directory. Loaded via
  `SkillRegistry::load_with_plugin_roots` /
  `load_global_with_plugin_roots`.
  - **Precedence: Project > Global > Plugin, first-wins.** A plugin skill
    never displaces a same-name project or global one — unlike commands,
    there is no namespaced fallback route for a displaced skill. Two
    DIFFERENT plugins colliding on the same skill name: the first one
    registered keeps it (a `tracing::warn!` logs the collision so it's not
    silently invisible); the losing plugin's skill is simply unavailable
    under that name.
  - **Listing:** a plugin skill is attributed `SkillOrigin::Plugin`, which
    behaves exactly like `SkillOrigin::Global` for listing/binding — it
    appears in "/" (or the skill catalog) only once explicitly BOUND to the
    agent, but is always reachable through the `skill` tool's index
    regardless of binding.

### Internal surfaces: gateway

`gateway = true` is a **sixth**, but deliberately **not public**, surface:
it marks a component as a chat-platform gateway (the first-party Discord
component is the only one shipped today). It requires `[component]`
structurally like the other component-backed surfaces, but linking the
gateway world's host capabilities (`ryuzi:websocket`) and the
`allow_self_auth` self-registered-OAuth-app privilege are both gated
**first-party-only** at install/link time (`HostPolicy::for_installed_bundle`
checks the installed bundle's `signing_key_id ==
first_party_key::FIRST_PARTY_KEY_ID` — never true for an unsigned local/git
install, see [Install sources](#install-sources-and-trust-tiers)'s trust
table). A third-party manifest can *declare* `gateway = true` and pass
`validate()`, but the runtime never grants it the capabilities that make a
gateway actually work. There is no native (in-process) gateway plugin —
every gateway ships as a signed WASM component, discovered off-disk and
driven through the generic host gateway bridge
(`crates/core/src/plugins/wasm_gateway_bridge.rs`).

---

## Category vocabulary

`ryuzi_plugin_sdk::categories::KNOWN` — 21 standard labels. Unrecognized
categories are a warning, not a validation error, so the vocabulary can grow
without breaking existing manifests: `model-provider`, `api-key`, `oauth`,
`free`, `runtime`, `chat-gateway`, `vcs`, `issues`, `docs`, `wiki`,
`productivity`, `memory`, `knowledge-graph`, `search`, `design`,
`observability`, `sandbox`, `tunnel`, `deploy`, `communication`, `skills`.

`model-provider`/`api-key`/`oauth`/`free` are provider-only labels generated
by `providers.rs`, not something a connector manifest needs — a connector's
auth tier is described by `[auth].kind`, not by category.

---

## Exclusive capability slots

A category is a free-form, cosmetic tag any number of plugins may share. A
`slot` is stricter: it's a plugin's claim to be *the* provider of one named
capability, e.g. `slot = "memory"`. `ryuzi_plugin_sdk::categories::KNOWN_SLOTS`
names three recognized slots — `memory`, `knowledge-graph`, `search` — but,
like `categories`, an unrecognized slot name only warns, never fails
`validate()`.

Arbitration happens in `PluginHost::add` (`crates/core/src/plugins/host.rs`)
at registration time, using the same first-registration-wins rule the host
already uses for duplicate plugin `id`s: the first plugin registered that
claims a given `slot` becomes its owner (`PluginHost::slot_owner(slot)`);
every later plugin claiming the same slot keeps its other capabilities but
loses the slot claim, recorded in `PluginHost::slot_conflicts()`.
`plugin_doctor` (`crates/core/src/plugins/doctor.rs`) surfaces every recorded
conflict as a `"slot-conflict"` warning finding, naming both the winner and
the loser.

---

## Install sources and trust tiers

Beyond the signed catalog feed (below), a plugin can be installed from an
arbitrary **local folder** or **git URL** — the ecosystem-opening
counterpart to the signed pipeline
(`crates/core/src/plugins/install_sources.rs`). Both are two-phase: `begin_
plugin_install_from_source(source)` stages the source into a temp dir
(cloning a git URL, or copying a local folder — the original is never
mutated), parses+validates its `ryuzi-plugin.toml`, and returns a
`PluginTrustPrompt` the caller must show before `confirm_plugin_install`
touches the live install directory. There is no "curated" fast path for
plugin sources the way skill installs have one — every source stops at the
prompt, since a plugin manifest can declare *any* surface.

`PluginTrustPrompt` carries: `id`/`name`/`publisher`, a `PluginSurfacesSummary`
(counts of commands/skills/hooks/jobs — always active, see below), the full
`[[mcp]]` server list (stdio: the exact `"<command> <args...>"`; http: the
URL — never just "trusts a command exists"), and, when `[component]` is
present, a `ComponentTrustSummary` (the network allowlist + every
`[[tools]]` entry with its `writes` flag). `trust_required` is `true` iff
`mcp` is non-empty or `component` is `Some`.

### Trust tiers

| Surface | Signed catalog | Unsigned (local folder / git URL) |
| --- | --- | --- |
| skills, commands, `[[hooks]]`, `[[jobs]]` | active on install | active after the normal install confirm |
| `[[mcp]]` (stdio = an arbitrary process!) | active on install | requires EXPLICIT trust acceptance |
| `[component]` (WASM, sandboxed) | active on install | requires EXPLICIT trust acceptance |
| `allow_self_auth`, `[gateway]` | first-party only | **never** — structural: `signing_key_id` for an unsigned install is never `first_party_key::FIRST_PARTY_KEY_ID` |

Trust acceptance is recorded as the setting `plugin.<id>.trusted = "true"`,
written ONLY by `confirm_plugin_install` and ONLY when the caller passed
`accept_trust: true`. Declining still completes the install — but the
`mcp`/`component` surfaces stay inert (`plugins::host::
component_surfaces_trusted`/`component_surfaces_trusted_for` are the shared
gate every consuming surface checks).

### On-disk layout

Same versioned-directory + `current`-pointer convention the signed-bundle
installer uses: `~/.config/ryuzi/plugins/<id>/<version>/` plus a sibling
`current` text file naming the active version. A declarative-only manifest
(no `[component]`) still gets this exact layout, just without a component
file / `release.json` / release-ledger row. Provenance is stamped as
`install.json` inside the version dir — `InstallProvenance::Catalog`,
`LocalPath`, or `GitUrl(url)` — defaulting to `Catalog` when absent (every
pre-Task-11 install path).

---

## Automation sync: hooks and jobs

A plugin's `[[hooks]]`/`[[jobs]]` become first-class rows in the same
Automation (`automation_hooks`) and Scheduler (`jobs`) tables a
user-created hook/job lives in — same run history, same toggle/edit
surfaces — attributed back to the owning plugin via a `plugin_id` origin
column. `crate::plugins::automation_sync::sync_plugin_automations` runs on
plugin install/enable and on every plugin update (a fresh release);
`remove_plugin_automations` runs on uninstall and cascades run history too.

**Row identity**: a hook row is named `"{plugin_id}/{name}"` (its real
primary key `id` is minted fresh on first sync, then reused on every
re-sync via a name lookup, so run history never orphans). A job row's `id`
**is** `"{plugin_id}/{name}"` directly — the natural key `upsert_job`
already upserts against.

### First sync vs. re-sync

A plugin cannot know the user's project, so on the **first sync**:
- `webhook.outbound` hooks may ship enabled — nothing about delivering to a
  URL needs a project.
- `agent.run` hooks and EVERY job install **disabled**, with an empty
  `project_id`/`branch`/`gateway_id` target.

On a **re-sync** (a plugin update), the user's choices must survive:
- `enabled` is always preserved from the stored row.
- An `agent.run` hook's `project_id`/`branch`/`gateway_id`/`agent_id` keep
  their stored values whenever non-empty; a still-empty stored value takes
  whatever the fresh sync computed.
- Everything else — `trigger_kind`, the prompt-bearing config fields — is
  OVERWRITTEN from the manifest every sync: that's the plugin's own declared
  behavior, not something a user customizes on the row.
- Jobs get symmetric treatment: `enabled` and `project_id`/`branch`/`gateway`
  are preserved when non-empty; `cron`/`mode`/`natural_text`/`prompt`/
  `model_override` always refresh. `notify_success`/`notify_fail`/
  `pre_check` are pure user preferences a manifest never declares — always
  carried over.

### The enable guard

A plugin CAN change a hook's action kind across a re-sync. Flipping an
enabled `webhook.outbound` hook to `agent.run` (with no project) would
otherwise persist an enabled hook with no target — the exact state
`automation::toggle_hook`'s own guard refuses ("pick a project first") — via
a write path that bypasses that guard. `sync_one_hook` re-applies the rule
explicitly: `enabled = existing.enabled && !is_targetless_agent_run(action)`.
The same "pick a project first" guard also blocks directly enabling a
freshly-synced, still-targetless `agent.run` hook or job through the normal
toggle RPC.

### Errors are per-row, never fatal

An unknown trigger spelling, a hook `config` that fails its action's schema
(`HookActionInput`'s `deny_unknown_fields`), or an invalid job schedule is
recorded into the returned `SyncReport` and the loop continues — one
malformed row never blocks every other hook/job in the same manifest from
syncing. Only a genuine store I/O failure surfaces as `Err`.

---

## MCP tool naming and per-tool perms

Every MCP-sourced tool in a session's tool registry — whether from an
external `[[mcp]]` server or from a component's `[[tools]]` (via
`ComponentMcpServer`, an in-process `McpCaller` that calls straight into the
component's `ryuzi:connector/connector` export, no stdio/JSON-RPC framing) —
is named uniformly: **`mcp__<server-or-plugin-id>__<tool-name>`**
(`harness::native::tools::mcp::McpTool::new`). Both paths are governed by the
exact same permission model; a component connector tool is not a distinct
mechanism from a declarative MCP server's tool as far as the session's tool
registry, approval attribution (`Principal`), or permission gating are
concerned.

**Per-tool perms**: Cockpit's plugin detail screen's Tools tab shows each
tool's full `mcp__<id>__<tool>` wire name and a trailing allow/ask/deny
`Segmented` control wired to the existing `set_app_tool_perm` command — the
same per-tool permission mechanism ordinary MCP apps use, gated on the
plugin being installed, trusted (see
[trust tiers](#install-sources-and-trust-tiers)), and having a synced app
row. A `[[tools]]`/component tool's `writes = true` flag is not itself an
enforced permission gate; it is disclosed in the install trust prompt and is
what the component itself is expected to gate its own mutation behind
(`confirm=true`) — enforcement of the actual tool call still goes through
the ordinary allow/ask/deny permission machinery.

---

## Enabling plugins

`PluginHost::is_enabled` resolves enablement by capability, in priority
order:

1. Unknown `id` → `false`.
2. `native` (the built-in agent harness) → always `true`; not toggleable.
3. Gateway-capable → `plugin.<id>.enabled == "true"` — the same key every
   other capability axis uses (`component_plugin_enabled` for WASM gateway
   component bundles).
4. `experimental = true` → always `false`, even if a stray
   `plugin.<id>.enabled = true` row exists.
5. No harness/gateway/connector capability at all (every model-provider
   plugin) → always `true`.
6. Otherwise (every connector-capable plugin) →
   `plugin.<id>.enabled == "true"`, defaulting to `false`.

There is no CLI surface for toggling — only Cockpit's plugin
switch (`set_plugin_enabled` Tauri command → `ryuzi_core::plugins::
toggle_enabled`) or writing the `plugin.<id>.enabled` settings key directly.

---

## Cockpit Plugins hub

Cockpit's plugin UI lives under the dedicated **Plugins** screen
(`apps/cockpit/src/views/PluginsView.tsx`) plus the per-plugin detail screen
(`PluginDetailView.tsx`). Plugin management — install, update, pin,
uninstall, enable/disable, doctor, per-tool perms — is Cockpit- and
daemon-only; there is no CLI surface for any of it.

The detail screen's tabs (`DetailTab`) gate on what the plugin actually
declares: **Tools** (component `[[tools]]` + synced `[[mcp]]`), **Contents**
(commands + skills, gated on `commands.length + skills.length > 0`),
**Automations** (hooks + jobs, gated on `hooks.length + jobs.length > 0`),
and **Versions** — all independent of whether the plugin is currently
installed. Component-backed plugins (including every one of the twelve
provider bundles and the six embedded component-catalog entries) are
first-class rows in `list_plugins` — `componentBacked`/`toolCount` are
populated for them the same as any other plugin, not a separate,
UI-invisible mechanism.

Any successful install, update, uninstall, or trust-confirm sets an
in-memory "restart required" flag Cockpit polls via `plugins_restart_required`
— a banner appears above the main view until the app is restarted, since
registries are only built once at process startup.

### RPC methods (`POST /rpc/{method}`)

A representative slice of `crate::api::plugins_api::dispatch` — every
method's params object uses Rust snake_case names; the Tauri command name
matches the RPC method 1:1:

| Method | Notes |
| --- | --- |
| `list_plugins` / `plugin_detail` | Every plugin / one plugin's full detail, `componentBacked` + ledger fields included. |
| `set_plugin_enabled` / `set_plugin_setting` | |
| `begin_plugin_oauth` / `complete_plugin_oauth` / `disconnect_plugin_oauth` | Cockpit's native declarative-connector OAuth flow. |
| `plugin_profile_begin_pkce` / `plugin_profile_complete_pkce` / `plugin_profile_disconnect` / `plugin_profile_begin_device_flow` / `plugin_profile_poll_device_flow` | Host-managed component OAuth (`[[oauth]]` profiles) — PKCE and RFC 8628 device flow. |
| `begin_plugin_install` / `cancel_plugin_install` | Signed-catalog install. |
| `begin_plugin_source_install` / `confirm_plugin_source_install` | The local-folder/git-URL two-phase trust flow — see [Install sources](#install-sources-and-trust-tiers). |
| `begin_skill_install` / `confirm_skill_install` | Skill-pack installs (unrelated to `[[skills]]` bundled in a plugin manifest — see [Removed in v2](#removed-in-v2)). |
| `update_plugin` / `update_all_plugins` / `set_plugin_pin` / `uninstall_plugin` | |
| `install_component_plugin` / `rollback_component_plugin` / `plugin_release_detail` / `component_bootstrap_status` | Signed component release management. |
| `plugin_doctor` | Read-only findings (`missing-binary`/`reconnect-required`/`attach-failed`/`slot-conflict`). |
| `plugins_restart_required` | Reads the in-memory restart-required latch. |

---

## Release pipeline

Signed first-party component bundles go through
`scripts/plugins/build-first-party.ts` — 18 bundles today: `mimo`,
`opencode`, the twelve provider bundles (`openai`, `openrouter`, `groq`,
`deepseek`, `mistral`, `xai`, `nvidia`, `huggingface`, `google`, `qwen`,
`anthropic`, `anthropic-oauth`), and the four connectors (`github`,
`discord`, `atlassian`, `bitbucket`) — driven by `COMPONENTS` in that script,
guarded by a `bun test` that fails if any `plugins/<id>/ryuzi-plugin.toml`
is missing from the list (or vice versa).

`readManifest(dir)` is the release pipeline's own manifest reader — it
asserts `contract == 2` and reads the wasm filename from `[component].file`
(the v1 shape had a top-level `component = "<name>.wasm"` string field,
which no longer parses):

```ts
const component = (parsed.component as Record<string, unknown> | undefined)?.file;
const contract = parsed.contract;
if (contract !== 2) throw new Error(`${path}: 'contract' must be 2`);
if (typeof component !== "string" || component.length === 0)
  throw new Error(`${path}: missing '[component].file'`);
```

### The seven artifacts

Per component, per release:

```
<id>.ryuzi-plugin.toml           (the committed manifest, verbatim)
<id>.release.json                (a ryuzi_plugin_sdk::PluginRelease descriptor)
<id>.release.json.sig            (the plugin.sig envelope: {key_id, signature})
<id>.wasm                        (the compiled component)
<id>-<version>.ryuzi-plugin.toml (pinned-stem alias, byte-identical)
<id>-<version>.release.json      (pinned-stem alias, byte-identical)
<id>-<version>.release.json.sig  (pinned-stem alias, byte-identical)
```

The three descriptor files publish under BOTH the unversioned `<id>.*` stem
(what a `latest` fetch resolves) and the pinned `<id>-<version>.*` stem
(what a version-pinned fetch resolves) as **byte-identical** copies of the
same signed bytes — the signature is over `release.json`'s exact bytes, so
aliasing (never re-serializing) is load-bearing. The wasm publishes once;
`component_url` in both stems' `release.json` points at it absolutely.

Verification (`crates/core/src/plugins/artifact_verify.rs::verify_artifacts_dir`,
consumed by the `verify-plugin-artifacts` CI binary) restages each
`<stem>.release.json` set into the client-install layout and runs the exact
same `plugins::bundle::verify_bundle` a real install uses — parsing the
manifest via `PluginManifest::from_toml`, which rejects an artifact declaring
an unsupported `contract` outright (`ContractUnsupported`) before any
hash/signature check even runs. A pre-v2 artifact that declares no `contract`
key at all goes through the contract-1 compatibility shim instead (see below).

### Contract-1 compatibility shim

`PluginManifest::from_toml` accepts one legacy shape: the contract-1 WASM
*bundle* manifest, which predates the `contract` key and kept the component's
coordinates flat (`component = "x.wasm"`, `wit-api`, `lifecycle`) with router
provider ids in a top-level `provider-ids`. It exists for exactly one reason —
a release feed published before the v2 migration must stay installable, or
every component install against it dies at `invalid type: string, expected
struct ComponentSpec`.

Rules, all enforced in `crates/plugin-sdk/src/manifest.rs`:

- **Contract 2 is tried first**, and its error is what surfaces when the input
  is neither shape — a v2 manifest with a real mistake reports that mistake.
- **Only a document with NO `contract` key is a candidate.** An explicit
  `contract = N` (N ≠ 2) is a deliberate claim and always fails with
  `ContractUnsupported`, outranking whatever field-level type error the
  document's shape would otherwise trip first.
- **The shim reshapes, it never relaxes.** The upgraded manifest goes through
  the same `validate()` as a natively-authored v2 one.
- **`gateway` upgrades to `true`.** v1 had no gateway declaration — the host
  compiled the component and read its exports. v2's flag is only a discovery
  pre-filter (`exports_gateway()` is still the authority) and grants no
  permission (`allow_gateway` derives from the verified first-party signing
  key), so `true` restores v1 semantics exactly. `false` would silently strand
  a v1 gateway (Discord) that had no way to declare itself one.
- **`provider-ids` maps to `[provider] ids` only when non-empty**, so a v1
  connector doesn't become a provider serving its own id.

`PluginManifest::from_toml_detecting_legacy` returns whether the shim fired;
`install_component_release` uses it to log a warning naming the feed URL.

### Local dev feed

Installs verify against the compiled-in first-party key, so a locally built
bundle is untrusted by default. A **debug** build additionally trusts a key
named by `RYUZI_DEV_PLUGIN_PUBKEY` (base64, 32 raw bytes) under the separate
key id `dev` (`first_party_key::DEV_KEY_ID`). Release builds ignore the env var
entirely.

```sh
bun scripts/plugins/build-first-party.ts keygen     # prints all three exports
export FIRST_PARTY_PRIVATE_KEY='<seed>'             # signs
export FIRST_PARTY_KEY_ID=dev                       # writes key_id "dev"
export RYUZI_DEV_PLUGIN_PUBKEY='<pubkey base64>'    # the daemon trusts it
export FIRST_PARTY_RELEASE_BASE_URL='http://127.0.0.1:8787'
export FIRST_PARTY_OUT_DIR=dist/plugins
bun scripts/plugins/build-first-party.ts            # build + sign every component
# serve dist/plugins at that base, then set the daemon's
# `component_release_base_url` setting to the same URL.
```

`component_url` must be same-origin with `component_release_base_url`
(`require_same_origin`), so the two must name the same scheme+host+port.

Two deliberate limits: the dev key is **additive under a distinct id**, so it
can neither shadow nor impersonate the first-party signer; and because the
first-party-only grants (`allow_self_auth`, `allow_gateway` — see
`runtime::HostPolicy::for_installed_bundle`) key off the verified id being
exactly `first-party`, a dev-signed bundle installs and runs but does not
receive them. Gateway and self-auth components still need a real first-party
release to exercise those paths.

### Pin + latest hybrid

An unversioned fetch (bootstrap, first install, repair) from a
release-stamped build (`RYUZI_RELEASE_TAG`, compiled in via `BUILD_RELEASE_TAG`)
pins to that release's own immutable GitHub-release tag — the exact builds
shipped and tested with this app version. A versioned fetch (an
update the catalog feed advertises) and every unstamped dev/fork build stay
on the rolling `latest` release.

### Remote catalog feed (schema 1)

The signed `catalog.json`/`catalog.json.sig` feed
(`scripts/catalog/build-feed.ts`, verified by
`crates/core/src/plugins/remote_catalog.rs`) declares `schemaVersion: 1`
(`SCHEMA_VERSION` in the script; `CatalogFeed::schema_version` in Rust).
`schemaVersion` versions the feed ENVELOPE — `{schemaVersion, sequence,
generatedAt, entries[{id, manifestToml}], blocked[]}` — which has not
changed since it was introduced; it does NOT version the manifest contract
embedded in each entry's `manifestToml` string (that is the manifest's own
`contract` key, enforced per-entry by `PluginManifest::from_toml`). The
manifest-v2 rollout (Task 17) therefore did NOT bump `SCHEMA_VERSION`: a
schemaVersion-1 envelope carrying contract-2 manifest bodies is normal and
accepted. `parse_and_check_with` rejects any feed whose `schema_version !=
1` with `UnsupportedSchema`, checked BEFORE the anti-rollback `sequence`
comparison.

This matters for revocation. A client that predates a manifest contract
bump can't use the new entries, but it must still receive the feed's
`blocked[]` list — that's the security-relevant channel. `fetch_and_cache_with`
drops (with a warning) any entry whose `manifest_toml` it can't parse, but
still applies the envelope and the blocked list, even when every entry was
dropped. Rejecting the whole envelope on a manifest contract mismatch (by
bumping `schemaVersion` alongside a contract bump) would sever revocation
for every already-shipped client — anti-rollback already prevents a stale
feed from replaying an old sequence, and per-entry parsing already drops
manifest bodies a client can't use, so the envelope schema check is
reserved for actual envelope shape changes.

---

## Removed in v2

Things a contract-1 manifest or the pre-Task-17 docs described that no
longer exist:

- **Both v1 schemas, unified.** Contract 1's declarative manifest
  (`ryuzi-plugin.toml`) and the separate WASM component bundle manifest
  (`ryuzi-plugin-bundle.toml`) are now ONE schema — manifest contract 2. v2 is
  the only authoring contract; nothing writes v1. The one exception is a
  read-only compat shim for the contract-1 *bundle* manifest, so release feeds
  published before the migration stay installable — see
  [Contract-1 compatibility shim](#contract-1-compatibility-shim). A contract-1
  *declarative* manifest still fails to parse.
- **Track D subprocess extensions** (`[[extension]]`, `ExtensionFactory`,
  the `ext__<extension>__<tool>` tool namespace, the supervised-subprocess
  "code plugin" mechanism) — deleted. A plugin that needs in-process tool
  execution ships a WASM component (`[[tools]]`) instead.
  `crates/core/src/plugins/declarative.rs` keeps only the MCP-server half of
  what used to be `ExtensionFactory`.
- **The WASM hooks export** (`ryuzi:hooks/hooks@0.1.0`, a component
  exporting its own hook-handling code) — the interface name is still
  structurally permitted in `ALLOWED_EXPORTS`
  (`crates/core/src/plugins/runtime.rs`) so an old component doesn't fail to
  link, but no host-side dispatcher consumes it anymore. Declarative
  `[[hooks]]` (`agent.run` / `webhook.outbound` actions, synced into the
  Automation domain — see [Automation sync](#automation-sync-hooks-and-jobs))
  is the v2 replacement.
- **Skill packs as a plugin-source mechanism**
  (`PluginSource::SkillPack`, hand-authored manifests loaded from
  `~/.config/ryuzi/plugins/<id>/ryuzi-plugin.toml` gated on a
  `.ryuzi-skill.json` stamp) — `PluginSource` now has exactly two variants,
  `Builtin` and `Installed { dir, provenance }` (`InstallProvenance::Catalog
  | LocalPath | GitUrl`). The Cockpit **Skills** tab's installer still exists
  as its own thing (`begin_skill_install`/`confirm_skill_install`,
  `crates/core/src/skills_install.rs`) for pure skill/command repos with no
  `ryuzi-plugin.toml` — that is unrelated to a plugin's own `[[skills]]`
  bundling field.
- **`enabled_gateways`** — a legacy CSV settings value listing which gateway
  ids were enabled. Retired entirely; every plugin's enablement (gateways
  included) lives at the one per-plugin `plugin.<id>.enabled` key. A
  first-upgrade migration (`crates/core/src/plugins/migrate_v2.rs`) carries
  an existing `enabled_gateways` CSV forward into that key once, then
  deletes the old setting.
- **The embedded `crates/core/plugins/catalog/` directory** (24 baked-in TOML
  connector manifests, `crates/core/src/plugins/catalog.rs`) — deleted along
  with the component-catalog migration. The signed remote catalog feed
  (`scripts/catalog/build-feed.ts`'s `DEFAULT_CATALOG_DIR`) now defaults to
  that same removed path and treats a missing directory as an empty catalog
  (`readCatalogEntries` catches `ENOENT`) rather than an error — every
  first-party connector today ships as a signed WASM component bundle
  instead (`plugins/github`, `plugins/atlassian`, `plugins/bitbucket`).
- **`WasmToolSet`** — the old bespoke `wasm__<id>__<tool>` in-process tool
  bridge. Replaced by `ComponentMcpServer`, which surfaces the exact same
  tools as ordinary `mcp__<id>__<tool>` MCP tools (see
  [MCP tool naming](#mcp-tool-naming-and-per-tool-perms)) — one tool
  namespace, one permission model, instead of a parallel mechanism.
- **A hardcoded `FIRST_PARTY_BUNDLE_IDS` list in Cockpit** — component
  bundles are no longer special-cased by a frontend id allowlist;
  `list_plugins`' `componentBacked` field (backed by
  `component_catalog::is_component_bundle`) is the one source of truth for
  "is this plugin a component" across every first-party AND
  installed-from-source component.
- **"Components never appear in `list_plugins`."** They always did appear as
  a distinct, Cockpit-invisible mechanism status in the old doc — this was
  already stale before this project started. Every component-backed plugin
  (built-in or installed) is an ordinary `list_plugins` row today, with
  `componentBacked`/`toolCount` populated.
