import { afterEach, beforeEach, expect, mock, spyOn, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { AppInfo, AutomationHookInfo, DoctorFinding, JobInfo, PluginDetail, PluginToolEntry } from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";

// The view fetches straight from `commands.pluginDetail` (bypassing the
// `usePlugins` list store, which only carries the flattened `PluginInfo`)
// and only touches the store for `setEnabled`/`load`, so mocking the Tauri
// IPC boundary is enough to drive every section.

const githubDetail: PluginDetail = {
  info: {
    id: "github",
    name: "GitHub",
    description: "Repos, issues, and pull requests via GitHub's official remote MCP server.",
    icon: "github",
    categories: ["vcs", "issues"],
    slot: null,
    ownsSlot: false,
    verified: true,
    experimental: false,
    // Enabled but not yet configured — a real "needs-setup" state (matches
    // `installed_flag`'s `configured || enabled` formula for integrations),
    // so `installed` is true and the Settings tab is reachable to enter the
    // credential (Task 9: pre-install now hides Settings behind the hero's
    // Install action instead).
    enabled: true,
    source: "catalog",
    capabilities: ["connector"],
    configured: false,
    kind: "integration",
    installed: true,
    family: null,
    pinned: false,
    sourceSpec: null,
    resolvedCommit: null,
    installedAt: null,
    updatedAt: null,
    trustTier: null,
    catalogVersion: null,
    componentBacked: false,
    blockedReason: null,
    status: "not-installed",
    statusDetail: null,
    authKind: "token",
    toolCount: null,
    skillCount: null,
    surfaces: [],
    provenance: null,
    trusted: true,
  },
  auth: {
    kind: "token",
    setting: "plugin.github.token",
    env: "GITHUB_PERSONAL_ACCESS_TOKEN",
    helpUrl: "https://github.com/settings/tokens",
    configured: false,
    oauthConnectAvailable: false,
    oauthConnectError: null,
    oauthTokenStored: false,
    oauthReconnectRequired: false,
  },
  settings: [],
  mcp: [{ name: "github", transport: "http", commandOrUrl: "https://api.githubcopilot.com/mcp/" }],
  models: [],
  homepage: "https://github.com/github/github-mcp-server",
  publisher: "GitHub (official)",
  commands: [],
  skills: [],
  hooks: [],
  jobs: [],
};

const ollamaDetail: PluginDetail = {
  info: {
    id: "ollama",
    name: "Ollama",
    description: "Local models via Ollama.",
    icon: "cpu",
    categories: ["model-provider"],
    slot: null,
    ownsSlot: false,
    verified: true,
    experimental: false,
    enabled: true,
    source: "builtin",
    capabilities: ["provider"],
    configured: false,
    kind: "integration",
    // enabled: true above → installed_flag's `configured || enabled` is true.
    installed: true,
    family: null,
    pinned: false,
    sourceSpec: null,
    resolvedCommit: null,
    installedAt: null,
    updatedAt: null,
    trustTier: null,
    catalogVersion: null,
    componentBacked: false,
    blockedReason: null,
    status: "not-installed",
    statusDetail: null,
    authKind: "none",
    toolCount: null,
    skillCount: null,
    surfaces: [],
    provenance: null,
    trusted: true,
  },
  auth: null,
  settings: [
    {
      key: "plugin.ollama.base_url",
      label: "Base URL",
      help: "Defaults to http://localhost:11434",
      secret: false,
      required: false,
      valueSet: false,
      kind: "string",
      options: [],
      default: null,
    },
  ],
  mcp: [],
  models: ["llama3", "mistral"],
  homepage: null,
  publisher: "Ollama (local)",
  commands: [],
  skills: [],
  hooks: [],
  jobs: [],
};

const sandboxDetail: PluginDetail = {
  info: {
    id: "vercel-sandbox",
    name: "Vercel Sandbox",
    description: "Docs-only entry — no MCP surface.",
    icon: "box",
    categories: ["sandbox"],
    slot: null,
    ownsSlot: false,
    verified: false,
    experimental: true,
    enabled: false,
    source: "catalog",
    capabilities: [],
    configured: false,
    kind: "integration",
    installed: false,
    family: null,
    pinned: false,
    sourceSpec: null,
    resolvedCommit: null,
    installedAt: null,
    updatedAt: null,
    trustTier: null,
    catalogVersion: null,
    componentBacked: false,
    blockedReason: null,
    status: "not-installed",
    statusDetail: null,
    authKind: "none",
    toolCount: null,
    skillCount: null,
    surfaces: [],
    provenance: null,
    trusted: true,
  },
  auth: null,
  settings: [],
  mcp: [],
  models: [],
  homepage: "https://vercel.com/docs/vercel-sandbox",
  publisher: "Vercel (no MCP surface)",
  commands: [],
  skills: [],
  hooks: [],
  jobs: [],
};

// A catalog connector genuinely never touched — not enabled, not configured,
// not experimental, not component-backed — the true "pre-install" case (Task
// 9): the hero shows Install instead of the Enabled switch, and there is no
// Settings tab to fall into since nothing is configured/enabled yet.
const freshDetail: PluginDetail = {
  info: {
    id: "acme-fresh",
    name: "Acme Fresh",
    description: "Never installed, configured, or enabled.",
    icon: "sparkles",
    categories: [],
    slot: null,
    ownsSlot: false,
    verified: false,
    experimental: false,
    enabled: false,
    source: "catalog",
    capabilities: [],
    configured: false,
    kind: "integration",
    installed: false,
    family: null,
    pinned: false,
    sourceSpec: null,
    resolvedCommit: null,
    installedAt: null,
    updatedAt: null,
    trustTier: null,
    catalogVersion: null,
    componentBacked: false,
    blockedReason: null,
    status: "not-installed",
    statusDetail: null,
    authKind: "none",
    toolCount: null,
    skillCount: null,
    surfaces: [],
    provenance: null,
    trusted: true,
  },
  auth: null,
  settings: [],
  mcp: [],
  models: [],
  homepage: null,
  publisher: "Acme",
  commands: [],
  skills: [],
  hooks: [],
  jobs: [],
};

// Installed via the tracked git-clone path — carries a full
// `plugin_installs` ledger row, exercising the Provenance block (source,
// short commit, installed/updated timestamps) and the real (persisted)
// `pinned` flag the Pin/Unpin action reads and writes.
const SKILL_PACK_INSTALLED_AT = 1_751_500_800_000; // 2025-07-03T00:00:00.000Z
const SKILL_PACK_UPDATED_AT = 1_751_587_200_000; // 2025-07-04T00:00:00.000Z

const skillPackDetail: PluginDetail = {
  info: {
    id: "acme-pack",
    name: "Acme Pack",
    description: "A skill pack installed from a git source.",
    icon: "sparkles",
    categories: ["skills"],
    slot: null,
    ownsSlot: false,
    verified: false,
    experimental: false,
    enabled: true,
    source: "skill-pack",
    capabilities: [],
    configured: false,
    kind: "skill-pack",
    installed: true,
    family: null,
    pinned: false,
    sourceSpec: "https://github.com/acme/pack",
    resolvedCommit: "deadbeefcafe1234",
    installedAt: SKILL_PACK_INSTALLED_AT,
    updatedAt: SKILL_PACK_UPDATED_AT,
    trustTier: "acknowledged",
    catalogVersion: null,
    componentBacked: false,
    blockedReason: null,
    status: "ok",
    statusDetail: null,
    authKind: "none",
    toolCount: null,
    skillCount: null,
    surfaces: [],
    provenance: null,
    trusted: true,
  },
  auth: null,
  settings: [],
  mcp: [],
  models: [],
  homepage: null,
  publisher: "acme/pack",
  commands: [],
  skills: [],
  hooks: [],
  jobs: [],
};

const oauthDetail: PluginDetail = {
  info: {
    id: "acme-oauth",
    name: "Acme OAuth",
    description: "HTTP MCP plugin authenticated through OAuth.",
    icon: "shield",
    categories: ["issues"],
    slot: null,
    ownsSlot: false,
    verified: true,
    experimental: false,
    enabled: true,
    source: "catalog",
    capabilities: ["connector"],
    configured: false,
    kind: "integration",
    // enabled: true above → installed_flag's `configured || enabled` is true.
    installed: true,
    family: null,
    pinned: false,
    sourceSpec: null,
    resolvedCommit: null,
    installedAt: null,
    updatedAt: null,
    trustTier: null,
    catalogVersion: null,
    componentBacked: false,
    blockedReason: null,
    status: "not-installed",
    statusDetail: null,
    authKind: "oauth",
    toolCount: null,
    skillCount: null,
    surfaces: [],
    provenance: null,
    trusted: true,
  },
  auth: {
    kind: "oauth",
    setting: null,
    env: null,
    helpUrl: "https://acme.example.com/help",
    configured: false,
    oauthConnectAvailable: true,
    oauthConnectError: null,
    oauthTokenStored: false,
    oauthReconnectRequired: false,
  },
  settings: [],
  mcp: [{ name: "acme", transport: "http", commandOrUrl: "https://api.acme.example.com/mcp" }],
  models: [],
  homepage: "https://acme.example.com",
  publisher: "Acme",
  commands: [],
  skills: [],
  hooks: [],
  jobs: [],
};

// Task 11: the SAME oauth plugin as `oauthDetail`, but already connected —
// used to prove the setup checklist disappears once nothing is left undone
// (as opposed to `oauthDetail`, its mid-setup counterpart).
const oauthConnectedDetail: PluginDetail = {
  ...oauthDetail,
  info: { ...oauthDetail.info, id: "acme-oauth-connected", name: "Acme OAuth Connected" },
  auth: { ...(oauthDetail.auth as NonNullable<PluginDetail["auth"]>), configured: true, oauthTokenStored: true },
};

// A plugin exercising every `SettingField.kind` shape (Feature C3):
// `verbose` is a Bool (renders a Switch), `tier` is an enum (`options`
// non-empty, renders a Combobox), `retries` is a plain Int (renders a
// numeric Input).
const richFieldsDetail: PluginDetail = {
  info: {
    id: "acme-rich",
    name: "Acme Rich",
    description: "Exercises every settings field kind.",
    icon: "sparkles",
    categories: [],
    slot: null,
    ownsSlot: false,
    verified: false,
    experimental: false,
    enabled: true,
    source: "catalog",
    capabilities: [],
    configured: false,
    kind: "integration",
    // enabled: true above → installed_flag's `configured || enabled` is true.
    installed: true,
    family: null,
    pinned: false,
    sourceSpec: null,
    resolvedCommit: null,
    installedAt: null,
    updatedAt: null,
    trustTier: null,
    catalogVersion: null,
    componentBacked: false,
    blockedReason: null,
    status: "not-installed",
    statusDetail: null,
    authKind: "none",
    toolCount: null,
    skillCount: null,
    surfaces: [],
    provenance: null,
    trusted: true,
  },
  auth: null,
  settings: [
    {
      key: "plugin.acme-rich.verbose",
      label: "Verbose logging",
      help: "Log extra diagnostic detail.",
      secret: false,
      required: false,
      valueSet: false,
      kind: "bool",
      options: [],
      default: null,
    },
    {
      key: "plugin.acme-rich.tier",
      label: "Tier",
      help: "Pricing tier to target.",
      secret: false,
      required: false,
      valueSet: false,
      kind: "string",
      options: ["free", "pro", "enterprise"],
      default: "free",
    },
    {
      key: "plugin.acme-rich.retries",
      label: "Retries",
      help: "",
      secret: false,
      required: false,
      valueSet: false,
      kind: "int",
      options: [],
      default: null,
    },
  ],
  mcp: [],
  models: [],
  homepage: null,
  publisher: "Acme",
  commands: [],
  skills: [],
  hooks: [],
  jobs: [],
};

// ---------- Contents + Automations tabs, Tools tab per-tool perms — Task 14 ----------
//
// An installed, trusted plugin whose manifest declares commands/skills
// (Contents) and hooks/jobs (Automations) — one hook needs a target, one
// doesn't, same shape for the job pair, so both the enable-switch and the
// "Set up…" deep-link affordances get real coverage. `hooks`/`jobs` are
// mutable module state (like `acmePackPinned`) so the `toggleAutomationHook`/
// `toggleJob` mocks below can flip a row and the view's post-toggle reload
// (`load()`) sees the change reflected.
const acmeSuiteBaseInfo = { ...githubDetail.info, id: "acme-suite", name: "Acme Suite", trusted: true };
let acmeSuiteHooks: PluginDetail["hooks"] = [
  {
    id: "hook-ready",
    name: "acme-suite/ready",
    trigger: "session.end",
    triggerAlias: "Stop",
    action: "webhook.outbound",
    enabled: true,
    needsTarget: false,
  },
  {
    id: "hook-needs-target",
    name: "acme-suite/onrun",
    trigger: "session.start",
    triggerAlias: null,
    action: "agent.run",
    enabled: false,
    needsTarget: true,
  },
];
let acmeSuiteJobs: PluginDetail["jobs"] = [
  { id: "job-ready", name: "Nightly sweep", schedule: "0 2 * * *", enabled: true, needsTarget: false },
  { id: "job-needs-target", name: "New job", schedule: "every day", enabled: false, needsTarget: true },
];
function acmeSuiteDetail(): PluginDetail {
  return {
    info: { ...acmeSuiteBaseInfo, surfaces: ["tools", "mcp", "skills", "commands", "hooks", "jobs"] },
    auth: null,
    settings: [],
    mcp: [{ name: "acme-suite", transport: "stdio", commandOrUrl: "acme-suite-server" }],
    models: [],
    homepage: null,
    publisher: "Acme",
    commands: ["review", "deploy"],
    skills: ["release-notes"],
    hooks: acmeSuiteHooks,
    jobs: acmeSuiteJobs,
  };
}

const ok = <T,>(data: T) => Promise.resolve({ status: "ok" as const, data });
const err = (message: string) => Promise.resolve({ status: "error" as const, error: { message } });

// Mutable so `setPluginPin` below can flip it and a subsequent
// `pluginDetail("acme-pack")` reload reflects the persisted value —
// the real behavior being tested (pin toggles the ledger; the view rereads
// it, it doesn't just paint a session-only flag).
let acmePackPinned = false;

// A first-party component (WASM bundle) plugin's `plugin_detail`: registered
// manifest-only now (`PluginSource::Component`, `componentBacked: true`), so it
// resolves a real detail rather than 404ing. Its release-management UI (install
// / rollback / permission gate) comes from the `ComponentReleaseCard`, which
// the view renders for any `componentBacked` plugin.
function componentDetail(id: string): PluginDetail {
  return {
    info: {
      ...githubDetail.info,
      id,
      name: id,
      description: "First-party WASM component.",
      icon: null,
      categories: ["component"],
      source: "component",
      capabilities: [],
      kind: "component",
      componentBacked: true,
      // Explicit (not inherited from `githubDetail.info`, which is
      // enabled/installed for its own needs-setup scenario) — these fixtures
      // are the "never installed" component-plugin case its own tests exercise.
      enabled: false,
      installed: false,
    },
    auth: null,
    settings: [],
    mcp: [],
    models: [],
    homepage: null,
    // Left blank so a release fixture's own manifest publisher (asserted via a
    // bare `getByText`) stays unambiguous — the header subtitle falls back to
    // the (distinct) description above, not the plugin id.
    publisher: "",
    commands: [],
    skills: [],
    hooks: [],
    jobs: [],
  };
}

const pluginDetail = mock((_runnerId: string, id: string) => {
  if (id === "github") return ok(githubDetail);
  if (id === "ollama") return ok(ollamaDetail);
  if (id === "acme-oauth") return ok(oauthDetail);
  if (id === "acme-oauth-connected") return ok(oauthConnectedDetail);
  if (id === "acme-rich") return ok(richFieldsDetail);
  if (id === "vercel-sandbox") return ok(sandboxDetail);
  if (id === "acme-fresh") return ok(freshDetail);
  if (id === "acme-pack") return ok({ ...skillPackDetail, info: { ...skillPackDetail.info, pinned: acmePackPinned } });
  if (id === "acme-suite") return ok(acmeSuiteDetail());
  // First-party component (WASM bundle) plugins are registered manifest-only
  // now, so `plugin_detail` resolves them. The release ledger + install gate
  // still come from `pluginReleaseDetail`/`ComponentReleaseCard`.
  if (id === "mimo" || id === "opencode" || id === "atlassian" || id === "bitbucket") return ok(componentDetail(id));
  // A genuinely unknown id still 404s with the generic (no-id) shape the
  // ghost-id test asserts on.
  return err("unknown plugin");
});
const setPluginEnabled = mock((_runnerId: string, _id: string, _enabled: boolean) => ok(null));
const setPluginSetting = mock((_runnerId: string, _key: string, _value: string) => ok(null));
const beginPluginOauth = mock((_runnerId: string, _pluginId: string) =>
  ok({
    stateToken: "state-123",
    authorizeUrl: "https://acme.example.com/oauth/authorize?client_id=acme-client",
    redirectUri: "http://127.0.0.1:8976/plugin-oauth/acme-oauth/callback",
  }),
);
const completePluginOauth = mock((_runnerId: string, _pluginId: string, _code: string, _stateToken: string) => ok(oauthDetail.auth));
const disconnectPluginOauth = mock((_runnerId: string, _pluginId: string) =>
  ok({ ...oauthDetail.auth, configured: false, oauthTokenStored: false }),
);
const listPlugins = mock(() => ok([]));
const pluginsRestartRequired = mock(() => ok(false));
const catalogStatus = mock(() => ok({ sequence: 0, lastFetchAt: null, outcome: null, entries: 0, blocked: 0 }));
let doctorFindingsFixture: DoctorFinding[] = [];
const pluginDoctor = mock(() => ok(doctorFindingsFixture));
const updatePlugin = mock((_runnerId: string, _id: string, _force: boolean) => ok({ kind: "updated" as const }));
const setPluginPin = mock((_runnerId: string, id: string, pinned: boolean, _reason: string | null) => {
  if (id === "acme-pack") acmePackPinned = pinned;
  return ok(null);
});
const uninstallPlugin = mock((_runnerId: string, _id: string) => ok([]));
// Task 10: `plugin_tools` per-id fixtures. Absent from the map ⇒ the same
// "no live data yet" baseline the brief calls for
// (`{ pluginId, live: false, entries: [] }`) — a plugin id this file doesn't
// opt into stays exactly as before Task 10 (an empty Tools tab if it has one
// at all).
let pluginToolsFixtures: Record<string, { live: boolean; entries: PluginToolEntry[] }> = {};
// A real `plugin_tools` call resolves the SAME declared manifest tools a
// pre-install component's `pluginReleaseDetail` fallback would use (branch 2
// of the daemon's `plugin_tools`, see `plugins_api.rs`), so in production the
// fallback is only ever visible for the brief window before that RPC
// resolves. To test the fallback deterministically (rather than racing a
// promise that always wins by the time `findByText` settles), a test can
// register an id here to keep its `plugin_tools` call permanently pending.
const pluginToolsPendingIds = new Set<string>();
const pluginTools = mock((_runnerId: string, id: string) => {
  if (pluginToolsPendingIds.has(id)) return new Promise<never>(() => {});
  return ok({ pluginId: id, live: pluginToolsFixtures[id]?.live ?? false, entries: pluginToolsFixtures[id]?.entries ?? [] });
});
// Task 9/15: the pre-install hero's Install action (and the checklist's
// Connect resume) opens the universal wizard, whose classic-connector
// adapter (`steps-connector.tsx`) resolves via `beginPluginInstall` — these
// are its own mount-time RPCs, mostly mocked here just enough that it mounts
// without throwing (the wizard's own exhaustive behavior is
// `UniversalInstallWizard.test.tsx`'s job, not this view's); `acme-oauth`
// gets its own oauth-available shape since the checklist's "Connect" resume
// test below depends on the connect step actually staying put on it.
const beginPluginInstall = mock((_runnerId: string, pluginId: string) => {
  if (pluginId === "acme-oauth") {
    return ok({
      authKind: "oauth",
      envVarPresent: false,
      envVarName: null,
      oauthAvailable: true,
      oauthExternal: false,
      needsClientId: false,
      dcrSucceeded: true,
      callbackMode: "auto",
      oauthBegin: {
        stateToken: "state-456",
        authorizeUrl: "https://acme.example.com/oauth/authorize?client_id=acme-client",
        redirectUri: "http://127.0.0.1:8976/plugin-oauth/acme-oauth/callback",
      },
      dcrError: null,
    });
  }
  return ok({
    authKind: "none",
    envVarPresent: false,
    envVarName: null,
    oauthAvailable: false,
    oauthExternal: false,
    needsClientId: false,
    dcrSucceeded: false,
    callbackMode: "manual",
    oauthBegin: null,
    dcrError: null,
  });
});
const cancelPluginInstall = mock((_runnerId: string, _pluginId: string, _stateToken: string | null) => ok(null));
const setPluginOauthClientId = mock((_runnerId: string, _pluginId: string, _clientId: string) => ok(null));
const openUrl = mock(async (_url: string) => {});

// Task 12: `PluginDetailView` now also fetches `pluginReleaseDetail` for the
// component-release card. Every fixture here defaults to "no releases" (a
// non-component plugin id), matching the RPC's real behavior for an id with
// no `component_plugin_releases` rows — so pre-existing tests are unaffected
// unless they opt into a component-plugin fixture.
type ReleaseInfoFixture = {
  pluginId: string;
  version: string;
  sourceUrl: string;
  sha256: string;
  signingKeyId: string;
  installedAt: number;
  active: boolean;
  revoked: boolean;
  revocationReason: string | null;
  firstParty: boolean;
};
type ReleaseDetailFixture = {
  pluginId: string;
  releases: ReleaseInfoFixture[];
  activeVersion: string | null;
  activeManifest: {
    publisher: string;
    description: string;
    lifecycle: string;
    domains: string[];
    oauthProfiles: OauthProfileFixture[];
    // Task 10: the pre-install Tools tab fallback source
    // (`ComponentManifestInfo.tools`) — optional here (rather than mirroring
    // the real DTO's required field) so every pre-Task-10 fixture literal in
    // this file stays valid without adding `tools: []` to each one.
    tools?: { name: string; description: string; writes: boolean }[];
  } | null;
  declaredManifest: {
    publisher: string;
    description: string;
    lifecycle: string;
    domains: string[];
    oauthProfiles: OauthProfileFixture[];
    tools?: { name: string; description: string; writes: boolean }[];
  } | null;
};
// Task 9: every field beyond `id`/`scopes` is optional so every pre-Task-9
// fixture literal in this file (which only ever set those two) stays valid —
// only the new PKCE-connections test below needs the full
// `ComponentOauthProfileInfo` shape (`OauthProfileConnections` reads
// `tokenUrl`/`deviceAuthorizationUrl`/`authorizeUrl`/`clientIdConfigured`/
// `connected` to decide what to render).
type OauthProfileFixture = {
  id: string;
  scopes: string[];
  tokenUrl?: string | null;
  deviceAuthorizationUrl?: string | null;
  connected?: boolean;
  authorizeUrl?: string | null;
  clientIdConfigured?: boolean;
};
const emptyReleaseDetail = (id: string): ReleaseDetailFixture => ({
  pluginId: id,
  releases: [],
  activeVersion: null,
  activeManifest: null,
  declaredManifest: null,
});
function releaseInfo(over: Partial<ReleaseInfoFixture> = {}): ReleaseInfoFixture {
  return {
    pluginId: "mimo",
    version: "0.1.0",
    sourceUrl: "https://feed.test/mimo/0.1.0",
    sha256: "0".repeat(64),
    signingKeyId: "first-party",
    installedAt: 1_751_500_800_000,
    active: false,
    revoked: false,
    revocationReason: null,
    firstParty: true,
    ...over,
  };
}
// mimo's fixture is mutable so tests can install a multi-release, active
// manifest scenario for the permission-summary/rollback/one-active-version
// tests, while every other id stays "never a component plugin" by default.
let mimoReleaseFixture: ReleaseDetailFixture = emptyReleaseDetail("mimo");
// Task 15c: a per-id map for the atlassian/bitbucket isolation tests — kept
// separate from `mimoReleaseFixture` so none of the pre-existing mimo-keyed
// tests change behavior. Any id not in this map (and not "mimo") still falls
// back to `emptyReleaseDetail(id)`, same as before.
let componentReleaseFixtures: Record<string, ReleaseDetailFixture> = {};
function releaseDetailFor(id: string): ReleaseDetailFixture {
  if (id === "mimo") return mimoReleaseFixture;
  return componentReleaseFixtures[id] ?? emptyReleaseDetail(id);
}
const pluginReleaseDetail = mock(async (_runnerId: string, id: string) => ({
  status: "ok" as const,
  data: releaseDetailFor(id),
}));
const installComponentPlugin = mock(async (_runnerId: string, id: string, _version: string | null) => ({
  status: "ok" as const,
  data: releaseDetailFor(id),
}));
const rollbackComponentPlugin = mock(async (_runnerId: string, id: string, _fromVersion: string, _toVersion: string) => ({
  status: "ok" as const,
  data: releaseDetailFor(id),
}));
const pluginOauthAuthorizeUrlMsgListen = mock(
  async (_cb: (event: { payload: { pluginId: string; authorizeUrl: string } }) => void) => () => {},
);

type OauthCompletedEvent = { payload: { pluginId: string; ok: boolean; error: string | null } };
let oauthCompletedListener: ((event: OauthCompletedEvent) => void) | null = null;
const pluginOauthCompletedMsgListen = mock(async (cb: (event: OauthCompletedEvent) => void) => {
  oauthCompletedListener = cb;
  return () => {
    oauthCompletedListener = null;
  };
});

// Task 14: the Tools tab per-tool perm select reads the plugin's MCP server
// row (id === plugin id) from the apps list — `listApps` backs
// `useApps().hydrate()`, which this view now calls on mount.
let appsFixture: AppInfo[] = [];
const listApps = mock(async () => ({ status: "ok" as const, data: appsFixture }));
const setAppToolPerm = mock(async (_runnerId: string, id: string, tool: string, perm: string) => {
  appsFixture = appsFixture.map((a) => (a.id === id ? { ...a, tools: a.tools.map((t) => (t.name === tool ? { ...t, perm } : t)) } : a));
  return { status: "ok" as const, data: appsFixture };
});
// Task 14: the Automations tab's enable switches. Mutates the module-level
// `acmeSuiteHooks`/`acmeSuiteJobs` fixtures so the view's post-toggle
// `load()` (re-fetches `pluginDetail`) sees the flipped row.
const toggleAutomationHook = mock(async (_runnerId: string, id: string, enabled: boolean) => {
  acmeSuiteHooks = acmeSuiteHooks.map((h) => (h.id === id ? { ...h, enabled } : h));
  const hook = acmeSuiteHooks.find((h) => h.id === id);
  return {
    status: "ok" as const,
    data: {
      id,
      name: hook?.name ?? id,
      triggerKind: "session.end",
      actionKind: "agent.run",
      enabled,
      inboundPath: null,
      createdAt: 0,
      updatedAt: 0,
      pluginId: "acme-suite",
    } as AutomationHookInfo,
  };
});
const toggleJob = mock(async (_runnerId: string, id: string, enabled: boolean) => {
  acmeSuiteJobs = acmeSuiteJobs.map((j) => (j.id === id ? { ...j, enabled } : j));
  return { status: "ok" as const, data: [] as JobInfo[] };
});

mock.module("@/bindings", () => ({
  events: {
    pluginOauthAuthorizeUrlMsg: {
      listen: pluginOauthAuthorizeUrlMsgListen,
    },
    pluginOauthCompletedMsg: {
      listen: pluginOauthCompletedMsgListen,
    },
  },
  commands: {
    pluginDetail,
    setPluginEnabled,
    setPluginSetting,
    beginPluginOauth,
    completePluginOauth,
    disconnectPluginOauth,
    listPlugins,
    pluginsRestartRequired,
    catalogStatus,
    pluginDoctor,
    updatePlugin,
    setPluginPin,
    uninstallPlugin,
    pluginReleaseDetail,
    installComponentPlugin,
    rollbackComponentPlugin,
    beginPluginInstall,
    cancelPluginInstall,
    setPluginOauthClientId,
    pluginTools,
    listApps,
    setAppToolPerm,
    toggleAutomationHook,
    toggleJob,
  },
}));
mock.module("@tauri-apps/plugin-opener", () => ({ openUrl }));

const { PluginDetailView, visibleTabs } = await import("@/views/PluginDetailView");
const { usePlugins } = await import("@/store-plugins");
const { useNav } = await import("@/store-nav");
const { useApps } = await import("@/store-apps");

beforeEach(() => {
  pluginDetail.mockClear();
  setPluginEnabled.mockClear();
  setPluginSetting.mockClear();
  beginPluginOauth.mockClear();
  completePluginOauth.mockClear();
  disconnectPluginOauth.mockClear();
  pluginOauthAuthorizeUrlMsgListen.mockClear();
  pluginOauthCompletedMsgListen.mockClear();
  oauthCompletedListener = null;
  listPlugins.mockClear();
  pluginsRestartRequired.mockClear();
  catalogStatus.mockClear();
  pluginDoctor.mockClear();
  updatePlugin.mockClear();
  setPluginPin.mockClear();
  uninstallPlugin.mockClear();
  pluginReleaseDetail.mockClear();
  installComponentPlugin.mockClear();
  rollbackComponentPlugin.mockClear();
  beginPluginInstall.mockClear();
  cancelPluginInstall.mockClear();
  setPluginOauthClientId.mockClear();
  pluginTools.mockClear();
  listApps.mockClear();
  setAppToolPerm.mockClear();
  toggleAutomationHook.mockClear();
  toggleJob.mockClear();
  doctorFindingsFixture = [];
  acmePackPinned = false;
  acmeSuiteHooks = [
    {
      id: "hook-ready",
      name: "acme-suite/ready",
      trigger: "session.end",
      triggerAlias: "Stop",
      action: "webhook.outbound",
      enabled: true,
      needsTarget: false,
    },
    {
      id: "hook-needs-target",
      name: "acme-suite/onrun",
      trigger: "session.start",
      triggerAlias: null,
      action: "agent.run",
      enabled: false,
      needsTarget: true,
    },
  ];
  acmeSuiteJobs = [
    { id: "job-ready", name: "Nightly sweep", schedule: "0 2 * * *", enabled: true, needsTarget: false },
    { id: "job-needs-target", name: "New job", schedule: "every day", enabled: false, needsTarget: true },
  ];
  mimoReleaseFixture = emptyReleaseDetail("mimo");
  componentReleaseFixtures = {};
  pluginToolsFixtures = {};
  pluginToolsPendingIds.clear();
  appsFixture = [];
  openUrl.mockClear();
  usePlugins.setState({
    plugins: [],
    loaded: false,
    restartRequired: false,
    doctorFindings: [],
    doctorLoaded: false,
    componentBootstrapStatus: null,
    componentPlugins: [],
    componentPluginsLoaded: false,
    toolsById: {},
    toolsLiveById: {},
  });
  useApps.setState({ apps: [], loaded: false, hydrating: false, probing: null });
});

afterEach(() => {
  cleanup();
  usePlugins.setState({
    plugins: [],
    loaded: false,
    restartRequired: false,
    doctorFindings: [],
    doctorLoaded: false,
    componentBootstrapStatus: null,
    componentPlugins: [],
    componentPluginsLoaded: false,
    toolsById: {},
    toolsLiveById: {},
  });
  useApps.setState({ apps: [], loaded: false, hydrating: false, probing: null });
});

// ---------- visibleTabs (Task 9, extended Task 14) — pure, no mounting ----------

function tabInput(overrides: Partial<Parameters<typeof visibleTabs>[0]> = {}): Parameters<typeof visibleTabs>[0] {
  return {
    installed: false,
    hasTools: false,
    hasContents: false,
    hasAutomations: false,
    hasAuth: false,
    hasSettings: false,
    hasVersions: false,
    hasHealth: false,
    ...overrides,
  };
}

test("visibleTabs: pre-install component row shows overview, tools, and versions", () => {
  expect(visibleTabs(tabInput({ installed: false, hasTools: true, hasVersions: true }))).toEqual(["overview", "tools", "versions"]);
});

test("visibleTabs: installed connector with auth+findings shows all five pre-Task-14 tabs", () => {
  expect(
    visibleTabs(tabInput({ installed: true, hasTools: true, hasAuth: true, hasSettings: true, hasVersions: true, hasHealth: true })),
  ).toEqual(["overview", "tools", "settings", "versions", "health"]);
});

test("visibleTabs: installed, no auth, no settings omits the settings tab even with everything else present", () => {
  expect(visibleTabs(tabInput({ installed: true, hasTools: true, hasVersions: true, hasHealth: true }))).toEqual([
    "overview",
    "tools",
    "versions",
    "health",
  ]);
});

test("visibleTabs: settings needs BOTH installed and (auth or settings) — not-installed hides it despite auth/settings", () => {
  expect(visibleTabs(tabInput({ installed: false, hasAuth: true, hasSettings: true }))).toEqual(["overview"]);
});

test("visibleTabs: health needs BOTH installed and hasHealth — not-installed hides it despite findings", () => {
  expect(visibleTabs(tabInput({ installed: false, hasHealth: true }))).toEqual(["overview"]);
});

test("visibleTabs: versions is independent of installed (a component-backed plugin's install gate lives there)", () => {
  expect(visibleTabs(tabInput({ installed: false, hasVersions: true }))).toEqual(["overview", "versions"]);
});

test("visibleTabs: nothing beyond overview when every input is false", () => {
  expect(visibleTabs(tabInput({ installed: true }))).toEqual(["overview"]);
});

// ---------- Task 14: contents/automations tabs ----------

test("visibleTabs: contents appears when hasContents is true, independent of installed", () => {
  expect(visibleTabs(tabInput({ installed: false, hasContents: true }))).toEqual(["overview", "contents"]);
});

test("visibleTabs: automations appears when hasAutomations is true, independent of installed", () => {
  expect(visibleTabs(tabInput({ installed: false, hasAutomations: true }))).toEqual(["overview", "automations"]);
});

test("visibleTabs: contents and automations both absent when both inputs are false", () => {
  const tabs = visibleTabs(tabInput({ installed: true, hasTools: true, hasSettings: true, hasAuth: true }));
  expect(tabs).not.toContain("contents");
  expect(tabs).not.toContain("automations");
});

test("visibleTabs: full order is overview, tools, contents, automations, settings, versions, health", () => {
  expect(
    visibleTabs(
      tabInput({
        installed: true,
        hasTools: true,
        hasContents: true,
        hasAutomations: true,
        hasAuth: true,
        hasSettings: true,
        hasVersions: true,
        hasHealth: true,
      }),
    ),
  ).toEqual(["overview", "tools", "contents", "automations", "settings", "versions", "health"]);
});

test("renders identity, about, and category/status badges from the manifest detail", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  expect(pluginDetail).toHaveBeenCalledWith(LOCAL_RUNNER, "github");
  // "GitHub (official)" appears as the header subtitle.
  expect(screen.getAllByText("GitHub (official)").length).toBeGreaterThanOrEqual(1);
  expect(screen.getByText(/Repos, issues, and pull requests/)).toBeTruthy();
  expect(screen.getByText("Verified")).toBeTruthy();
  expect(screen.getByText("vcs")).toBeTruthy();
  expect(screen.getByText("issues")).toBeTruthy();
  expect(screen.getByText("https://github.com/github/github-mcp-server")).toBeTruthy();
});

test("shows Not configured for an unset credential, disables Save until typed, and saves through setPluginSetting", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  expect(screen.getByText("Not configured")).toBeTruthy();
  expect(screen.getByText(/Falls back to the GITHUB_PERSONAL_ACCESS_TOKEN environment variable/)).toBeTruthy();

  const save = screen.getByRole("button", { name: "Save" }) as HTMLButtonElement;
  expect(save.disabled).toBe(true);

  const input = screen.getByPlaceholderText("Required — not set") as HTMLInputElement;
  fireEvent.change(input, { target: { value: "ghp_test123" } });
  expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(false);

  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(setPluginSetting).toHaveBeenCalledWith(LOCAL_RUNNER, "plugin.github.token", "ghp_test123"));
  await waitFor(() => expect(pluginDetail).toHaveBeenCalledTimes(2));
});

test("opens the auth help link through the shared openUrl mechanism", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  fireEvent.click(screen.getByRole("button", { name: "Help" }));
  expect(openUrl).toHaveBeenCalledWith("https://github.com/settings/tokens");
});

test("oauth plugins start Cockpit sign-in through beginPluginOauth", async () => {
  render(<PluginDetailView id="acme-oauth" />);
  await screen.findByText("Acme OAuth");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  fireEvent.click(screen.getByRole("button", { name: "Connect" }));
  await waitFor(() => expect(beginPluginOauth).toHaveBeenCalledWith(LOCAL_RUNNER, "acme-oauth"));
});

// ---------- Settings field render-by-kind (Feature C3) ----------

test("a Bool settings field renders as a Switch and saves immediately on toggle", async () => {
  render(<PluginDetailView id="acme-rich" />);
  await screen.findByText("Acme Rich");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  const sw = screen.getByRole("switch", { name: "Verbose logging" });
  expect(sw.getAttribute("aria-checked")).toBe("false");

  fireEvent.click(sw);
  await waitFor(() => expect(setPluginSetting).toHaveBeenCalledWith(LOCAL_RUNNER, "plugin.acme-rich.verbose", "true"));
  // pluginDetail() never re-persists a value back, so the toggle stays a
  // pending client-side flip rather than reflecting a re-fetched "true" —
  // still, the reload must have happened (mount + post-save reload).
  await waitFor(() => expect(pluginDetail).toHaveBeenCalledTimes(2));
});

test("an enum settings field (non-empty options) renders as a Combobox and saves the picked option", async () => {
  render(<PluginDetailView id="acme-rich" />);
  await screen.findByText("Acme Rich");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  const combo = screen.getByRole("combobox", { name: "Tier" });
  // Shows the manifest-declared default as an affordance when unset.
  expect(combo.textContent).toContain("Default: free");

  fireEvent.click(combo);
  fireEvent.click(await screen.findByRole("option", { name: "pro" }));

  const save = screen.getAllByRole("button", { name: "Save" })[0] as HTMLButtonElement;
  expect(save.disabled).toBe(false);
  fireEvent.click(save);
  await waitFor(() => expect(setPluginSetting).toHaveBeenCalledWith(LOCAL_RUNNER, "plugin.acme-rich.tier", "pro"));
});

test("a plain Int settings field renders as a numeric Input", async () => {
  render(<PluginDetailView id="acme-rich" />);
  await screen.findByText("Acme Rich");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  const retries = screen.getByPlaceholderText("Optional — not set") as HTMLInputElement;
  expect(retries.type).toBe("number");
});

test("lists MCP servers with their transport and endpoint", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  expect(screen.getByText("MCP servers")).toBeTruthy();
  expect(screen.getByText("http")).toBeTruthy();
  expect(screen.getByText("https://api.githubcopilot.com/mcp/")).toBeTruthy();
});

// ---------- Tools & Skills tab — Task 10 ----------
//
// The overview's old "Models" card is gone; a provider's models now render
// as `kind: "model"` `plugin_tools` entries in the Tools tab, alongside any
// tool/skill entries (see `PluginToolsList.test.tsx` for that grouping's own
// coverage — this file only proves the wiring: the RPC is called for the
// mounted id, its entries land in the tab, and the overview card never
// comes back).

test("a provider's models come from plugin_tools and render in the Tools tab, not on Overview", async () => {
  pluginToolsFixtures.ollama = {
    live: true,
    entries: [
      { name: "llama3", description: "", kind: "model", writes: null },
      { name: "mistral", description: "", kind: "model", writes: null },
    ],
  };
  render(<PluginDetailView id="ollama" />);
  await screen.findByText("Ollama");
  await waitFor(() => expect(pluginTools).toHaveBeenCalledWith(LOCAL_RUNNER, "ollama"));

  // The overview Models card is gone entirely — no stray "Models" text and
  // no model names leak onto the default (Overview) tab.
  expect(screen.queryByText("Models")).toBeNull();
  expect(screen.queryByText("llama3")).toBeNull();

  const toolsTabButton = await screen.findByRole("button", { name: /Tools \(2\)/ });
  fireEvent.click(toolsTabButton);
  expect(await screen.findByText("llama3")).toBeTruthy();
  expect(screen.getByText("mistral")).toBeTruthy();

  // The declared settings field still lives on the Settings tab.
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByText("Base URL")).toBeTruthy();
  expect(screen.getByPlaceholderText("Optional — not set")).toBeTruthy();
});

test("a plugin with no plugin_tools entries and no manifest tools gets no Tools tab, and no Models card either", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");
  await waitFor(() => expect(pluginTools).toHaveBeenCalledWith(LOCAL_RUNNER, "github"));

  expect(screen.queryByRole("button", { name: /Tools/ })).toBeNull();
  expect(screen.queryByText("Models")).toBeNull();
});

test("a component's declared manifest tools render as the pre-install Tools tab fallback, with the declared-list hint", async () => {
  // Keeps `plugin_tools` from ever resolving for this id, so the fallback
  // (derived from `pluginReleaseDetail`'s already-loaded manifest) stays the
  // resolved source for the whole test — see `pluginToolsPendingIds`'s doc.
  pluginToolsPendingIds.add("atlassian");
  componentReleaseFixtures.atlassian = {
    pluginId: "atlassian",
    activeVersion: null,
    releases: [],
    activeManifest: null,
    declaredManifest: {
      publisher: "Ryuzi",
      description: "",
      lifecycle: "per-call",
      domains: ["api.atlassian.com"],
      oauthProfiles: [],
      tools: [{ name: "jira_search", description: "Search Jira issues", writes: false }],
    },
  };
  render(<PluginDetailView id="atlassian" />);
  await screen.findByText("atlassian");

  const toolsTabButton = await screen.findByRole("button", { name: /Tools \(1\)/ });
  fireEvent.click(toolsTabButton);
  expect(await screen.findByText("jira_search")).toBeTruthy();
  expect(screen.getByText("Search Jira issues")).toBeTruthy();
  expect(screen.getByText("Declared by the plugin — final list may differ after install.")).toBeTruthy();
});

test("active manifest tools take precedence over declared when both are present", async () => {
  // Active (verified) manifest always wins over declared — this ensures
  // we never regress to showing stale declared tools when an active version exists.
  pluginToolsPendingIds.add("bitbucket");
  componentReleaseFixtures.bitbucket = {
    pluginId: "bitbucket",
    activeVersion: "1.0.0",
    releases: [],
    activeManifest: {
      publisher: "Ryuzi",
      description: "",
      lifecycle: "per-call",
      domains: ["api.bitbucket.org"],
      oauthProfiles: [],
      tools: [{ name: "active_tool", description: "Active tool", writes: false }],
    },
    declaredManifest: {
      publisher: "Ryuzi",
      description: "",
      lifecycle: "per-call",
      domains: ["api.bitbucket.org"],
      oauthProfiles: [],
      tools: [{ name: "declared_tool", description: "Declared tool", writes: false }],
    },
  };
  render(<PluginDetailView id="bitbucket" />);
  await screen.findByText("bitbucket");

  const toolsTabButton = await screen.findByRole("button", { name: /Tools \(1\)/ });
  fireEvent.click(toolsTabButton);
  expect(await screen.findByText("active_tool")).toBeTruthy();
  expect(screen.queryByText("declared_tool")).toBeNull();
});

test("disables the enable switch for experimental plugins", async () => {
  render(<PluginDetailView id="vercel-sandbox" />);
  await screen.findByText("Vercel Sandbox");

  expect(screen.getByText("Experimental")).toBeTruthy();
  const sw = screen.getByRole("switch", { name: "Enabled" });
  expect(sw.getAttribute("aria-checked")).toBe("false");

  fireEvent.click(sw);
  expect(setPluginEnabled).not.toHaveBeenCalled();
  expect(sw.getAttribute("aria-checked")).toBe("false");
});

test("shows a not-found state for an unknown plugin id", async () => {
  render(<PluginDetailView id="ghost" />);
  await waitFor(() => expect(pluginDetail).toHaveBeenCalledWith(LOCAL_RUNNER, "ghost"));
  expect(await screen.findByText("Plugin not found.")).toBeTruthy();
});

test("pluginOauthCompletedMsg auto-completes the pending connect flow", async () => {
  render(<PluginDetailView id="acme-oauth" />);
  await screen.findByText("Acme OAuth");
  await waitFor(() => expect(pluginOauthCompletedMsgListen).toHaveBeenCalled());

  await act(async () => {
    oauthCompletedListener?.({ payload: { pluginId: "acme-oauth", ok: true, error: null } });
  });

  await waitFor(() => expect(pluginDetail).toHaveBeenCalledTimes(2));
  expect(completePluginOauth).not.toHaveBeenCalled();
});

test("pluginOauthCompletedMsg for another plugin is ignored", async () => {
  render(<PluginDetailView id="acme-oauth" />);
  await screen.findByText("Acme OAuth");
  await waitFor(() => expect(pluginOauthCompletedMsgListen).toHaveBeenCalled());

  await act(async () => {
    oauthCompletedListener?.({ payload: { pluginId: "other", ok: true, error: null } });
  });

  expect(pluginDetail).toHaveBeenCalledTimes(1);
});

test("skill-pack plugins show Update and Pin actions that call updatePlugin/setPluginPin", async () => {
  render(<PluginDetailView id="acme-pack" />);
  await screen.findByText("Acme Pack");

  // Task 9: Update/Pin moved from hero buttons into the overflow menu.
  fireEvent.click(screen.getByRole("button", { name: "Actions for Acme Pack" }));
  fireEvent.click(await screen.findByRole("menuitem", { name: "Update" }));
  await waitFor(() => expect(updatePlugin).toHaveBeenCalledWith(LOCAL_RUNNER, "acme-pack", false));

  fireEvent.click(screen.getByRole("button", { name: "Actions for Acme Pack" }));
  fireEvent.click(await screen.findByRole("menuitem", { name: "Pin" }));
  await waitFor(() => expect(setPluginPin).toHaveBeenCalledWith(LOCAL_RUNNER, "acme-pack", true, "Pinned from Cockpit"));

  // Pin toggles the ledger, then this view reloads `pluginDetail` — the
  // pill/menu item reflect the REAL persisted `info.pinned`, not a
  // session-only flag. (Calls so far: mount, post-Update reload, post-Pin
  // reload.)
  await waitFor(() => expect(pluginDetail).toHaveBeenCalledTimes(3));
  expect(await screen.findByText("Pinned")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Actions for Acme Pack" }));
  expect(await screen.findByRole("menuitem", { name: "Unpin" })).toBeTruthy();
});

test("pin survives a reload — a fresh pluginDetail fetch reports the persisted pinned flag without any pin() call", async () => {
  acmePackPinned = true;
  render(<PluginDetailView id="acme-pack" />);
  await screen.findByText("Acme Pack");

  expect(screen.getByText("Pinned")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Actions for Acme Pack" }));
  expect(await screen.findByRole("menuitem", { name: "Unpin" })).toBeTruthy();
  expect(setPluginPin).not.toHaveBeenCalled();
});

test("renders the Provenance block: source spec, short commit, and installed/updated dates", async () => {
  render(<PluginDetailView id="acme-pack" />);
  await screen.findByText("Acme Pack");

  expect(screen.getByText("Provenance")).toBeTruthy();
  expect(screen.getByText("https://github.com/acme/pack")).toBeTruthy();
  // Short commit is the first 8 characters of the ledger's full hash.
  expect(screen.getByText("deadbeef")).toBeTruthy();
  expect(screen.getByText(new Date(SKILL_PACK_INSTALLED_AT).toLocaleDateString())).toBeTruthy();
  expect(screen.getByText(new Date(SKILL_PACK_UPDATED_AT).toLocaleDateString())).toBeTruthy();
});

test("Provenance card is hidden entirely for a plugin with no install ledger row", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  // A plugin never installed via the tracked git-clone path has null
  // sourceSpec/resolvedCommit/installedAt/updatedAt, so the whole Provenance
  // card must not render (matching the Auth/Settings/MCP/Models sibling
  // cards, which all guard the whole Card on their content). Previously the
  // card rendered as an empty shell, and before that its Source row
  // duplicated the DetailHeader subtitle by falling back to `publisher`.
  expect(screen.queryByText("Provenance")).toBeNull();
  expect(screen.queryByText("Source")).toBeNull();
  expect(screen.getAllByText("GitHub (official)").length).toBe(1);
});

test("non-skill-pack plugins render no Update/Pin actions, but the overflow menu still carries Uninstall", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  fireEvent.click(screen.getByRole("button", { name: "Actions for GitHub" }));
  expect(await screen.findByRole("menuitem", { name: "Uninstall" })).toBeTruthy();
  expect(screen.queryByRole("menuitem", { name: "Update" })).toBeNull();
  expect(screen.queryByRole("menuitem", { name: "Pin" })).toBeNull();
});

test("renders an attach-failed doctor finding as a banner with a Configure action", async () => {
  doctorFindingsFixture = [
    {
      pluginId: "github",
      severity: "warn",
      kind: "attach-failed",
      message: "github: authentication failed",
      suggestedAction: "Check github's configuration",
    },
  ];
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  expect(await screen.findByText("Attach failed")).toBeTruthy();
  expect(screen.getByText("github: authentication failed")).toBeTruthy();
  expect(screen.getByText("Check github's configuration")).toBeTruthy();

  // Task 9: Configure now switches to the Settings tab instead of scrolling.
  fireEvent.click(screen.getByRole("button", { name: "Configure" }));
  expect(await screen.findByText("Authentication")).toBeTruthy();
  expect(screen.queryByText("Attach failed")).toBeNull();
});

test("omits the attach-failed banner when doctor has no finding for this plugin", async () => {
  doctorFindingsFixture = [
    { pluginId: "other-plugin", severity: "warn", kind: "attach-failed", message: "other failed", suggestedAction: "Check other" },
  ];
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  expect(screen.queryByText("Attach failed")).toBeNull();
});

// ---------- Component-plugin (WASM bundle) release management — Task 12 ----------
//
// mimo/opencode are registered manifest-only `CorePlugin`s now, so
// `pluginDetail("mimo")` resolves a real detail (see `componentDetail`) and the
// view renders the normal detail page PLUS the `ComponentReleaseCard` (release
// ledger + install/permission gate) driven by `pluginReleaseDetail`.

test("a never-installed component plugin opens its management page (not 'Plugin not found')", async () => {
  render(<PluginDetailView id="mimo" />);

  expect(await screen.findByText("mimo")).toBeTruthy();
  // Task 14: the hero's Install action for a component-backed plugin opens
  // the universal wizard (it used to just jump to the Versions tab) — the
  // Overview setup checklist ALSO shows its own "Install" button for this
  // exact (component-backed, never-installed) scenario, so this specifically
  // targets the hero's copy — first in DOM order, since it renders above the
  // tabbed content.
  fireEvent.click(screen.getAllByRole("button", { name: "Install" })[0]);
  expect(await screen.findByRole("dialog", { name: "Install mimo" })).toBeTruthy();
  expect(screen.queryByText("Plugin not found.")).toBeNull();
});

test("an unrelated unknown plugin id still shows Plugin not found", async () => {
  render(<PluginDetailView id="ghost-2" />);
  expect(await screen.findByText("Plugin not found.")).toBeTruthy();
});

// Task 14 duplication cleanup: a never-installed component's Versions-tab
// button no longer runs the inline accept-switch-then-install dance itself
// (the wizard's own Permissions step owns that now) — it just opens the
// wizard. The update/rollback gate (a component WITH an active version)
// keeps the original inline accept-then-install behavior untouched, covered
// by the "install is DISABLED until..." scenarios below it in this file.
test("Versions tab's never-installed component shows 'Install with wizard…', which opens the wizard instead of installing inline", async () => {
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");

  fireEvent.click(screen.getByRole("button", { name: "Versions" }));
  const panel = within(screen.getByTestId("tab-panel-versions"));

  expect(panel.queryByRole("switch", { name: "Accept permissions" })).toBeNull();
  fireEvent.click(panel.getByRole("button", { name: "Install with wizard…" }));

  expect(await screen.findByRole("dialog", { name: "Install mimo" })).toBeTruthy();
  expect(installComponentPlugin).not.toHaveBeenCalled();
});

test("the permission summary shows 'Unknown until…' before any release is installed", async () => {
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");

  expect(screen.getByText(/Unknown until a release is fetched and its signature is verified/)).toBeTruthy();
});

// PR-1: a never-installed component with an embedded (declared) manifest
// previews its real permissions, labeled as declared-not-yet-verified —
// the "Unknown until…" placeholder is reserved for bundles with no
// embedded manifest at all.
test("the permission summary shows declared permissions pre-install, labeled as declared", async () => {
  mimoReleaseFixture = {
    pluginId: "mimo",
    activeVersion: null,
    releases: [],
    activeManifest: null,
    declaredManifest: {
      publisher: "Ryuzi",
      description: "Xiaomi MiMo free-tier chat provider.",
      lifecycle: "per-call",
      domains: ["api.xiaomimimo.com"],
      oauthProfiles: [],
    },
  };
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");

  expect(screen.queryByText(/Unknown until a release is fetched/)).toBeNull();
  expect(screen.getAllByText("api.xiaomimimo.com").length).toBeGreaterThan(0);
  expect(screen.getAllByText("As declared by the bundled manifest — verified against its signature at install.").length).toBeGreaterThan(0);
});

// Versions-tab (ComponentReleaseCard) fallback: declared permissions render
// for a never-installed component, even when there's no active release.
test("the permission summary shows declared permissions in the Versions tab pre-install", async () => {
  mimoReleaseFixture = {
    pluginId: "mimo",
    activeVersion: null,
    releases: [],
    activeManifest: null,
    declaredManifest: {
      publisher: "Ryuzi",
      description: "Xiaomi MiMo free-tier chat provider.",
      lifecycle: "per-call",
      domains: ["api.xiaomimimo.com"],
      oauthProfiles: [],
    },
  };
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");

  fireEvent.click(screen.getByRole("button", { name: "Versions" }));
  const panel = within(screen.getByTestId("tab-panel-versions"));

  expect(panel.queryByText(/Unknown until a release is fetched/)).toBeNull();
  expect(panel.getByText("Ryuzi")).toBeTruthy();
  expect(panel.getByText("api.xiaomimimo.com")).toBeTruthy();
  expect(panel.getByText("As declared by the bundled manifest — verified against its signature at install.")).toBeTruthy();
});

test("the permission summary renders the active release's publisher, lifecycle, domains, and OAuth profiles", async () => {
  mimoReleaseFixture = {
    pluginId: "mimo",
    activeVersion: "0.2.0",
    releases: [releaseInfo({ version: "0.2.0", active: true, installedAt: 1_751_500_800_000 })],
    activeManifest: {
      publisher: "Ryuzi",
      description: "Xiaomi MiMo free-tier chat provider.",
      lifecycle: "per-call",
      domains: ["api.xiaomimimo.com"],
      oauthProfiles: [{ id: "github", scopes: ["repo", "read:user"] }],
    },
    declaredManifest: null,
  };
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");

  // The permission summary lives on Overview (default tab).
  expect(screen.getByText("Ryuzi")).toBeTruthy();
  expect(screen.getByText(/Per call — a fresh instance every call/)).toBeTruthy();
  expect(screen.getByText("api.xiaomimimo.com")).toBeTruthy();
  expect(screen.getByText(/github \(repo, read:user\)/)).toBeTruthy();

  // The release-management card (Update button label flips once a version
  // is active) lives on the Versions tab.
  fireEvent.click(screen.getByRole("button", { name: "Versions" }));
  expect(within(screen.getByTestId("tab-panel-versions")).getByRole("button", { name: "Update to latest" })).toBeTruthy();
});

// Task 14 duplication cleanup: only the never-installed case moved its
// install action to the wizard — a component WITH an active version (the
// update/rollback case) keeps its original inline accept-switch-then-Update
// dispatch untouched.
test("Update to latest is DISABLED until the permission-acceptance switch is toggled, then dispatches installComponentPlugin", async () => {
  mimoReleaseFixture = {
    pluginId: "mimo",
    activeVersion: "0.2.0",
    releases: [releaseInfo({ version: "0.2.0", active: true })],
    activeManifest: {
      publisher: "Ryuzi",
      description: "",
      lifecycle: "per-call",
      domains: ["api.xiaomimimo.com"],
      oauthProfiles: [],
    },
    declaredManifest: null,
  };
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");

  fireEvent.click(screen.getByRole("button", { name: "Versions" }));
  const panel = within(screen.getByTestId("tab-panel-versions"));

  const update = panel.getByRole("button", { name: "Update to latest" }) as HTMLButtonElement;
  expect(update.disabled).toBe(true);
  expect(panel.queryByRole("button", { name: "Install with wizard…" })).toBeNull();

  fireEvent.click(panel.getByRole("switch", { name: "Accept permissions" }));
  expect((panel.getByRole("button", { name: "Update to latest" }) as HTMLButtonElement).disabled).toBe(false);

  fireEvent.click(panel.getByRole("button", { name: "Update to latest" }));
  await waitFor(() => expect(installComponentPlugin).toHaveBeenCalledWith(LOCAL_RUNNER, "mimo", null));
});

test("exactly one release shows the Active badge among several (one-active-version display)", async () => {
  mimoReleaseFixture = {
    pluginId: "mimo",
    activeVersion: "0.3.0",
    releases: [
      releaseInfo({ version: "0.1.0", active: false, revoked: true, revocationReason: "superseded" }),
      releaseInfo({ version: "0.2.0", active: false }),
      releaseInfo({ version: "0.3.0", active: true }),
    ],
    activeManifest: {
      publisher: "Ryuzi",
      description: "",
      lifecycle: "per-call",
      domains: ["api.xiaomimimo.com"],
      oauthProfiles: [],
    },
    declaredManifest: null,
  };
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");
  fireEvent.click(screen.getByRole("button", { name: "Versions" }));

  expect(screen.getAllByText("Active").length).toBe(1);
  expect(screen.getByText("0.1.0")).toBeTruthy();
  expect(screen.getByText("0.2.0")).toBeTruthy();
  expect(screen.getByText("0.3.0")).toBeTruthy();
});

test("rolling back to a prior good version dispatches rollbackComponentPlugin with the active version as `from`", async () => {
  mimoReleaseFixture = {
    pluginId: "mimo",
    activeVersion: "0.2.0",
    releases: [releaseInfo({ version: "0.1.0", active: false }), releaseInfo({ version: "0.2.0", active: true })],
    activeManifest: {
      publisher: "Ryuzi",
      description: "",
      lifecycle: "per-call",
      domains: ["api.xiaomimimo.com"],
      oauthProfiles: [],
    },
    declaredManifest: null,
  };
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");
  fireEvent.click(screen.getByRole("button", { name: "Versions" }));

  fireEvent.click(screen.getByRole("button", { name: "Roll back to 0.1.0" }));
  await waitFor(() => expect(rollbackComponentPlugin).toHaveBeenCalledWith(LOCAL_RUNNER, "mimo", "0.2.0", "0.1.0"));
});

test("a revoked release offers no Roll back action, and the active release offers none either", async () => {
  mimoReleaseFixture = {
    pluginId: "mimo",
    activeVersion: "0.2.0",
    releases: [
      releaseInfo({ version: "0.1.0", active: false, revoked: true, revocationReason: "bad" }),
      releaseInfo({ version: "0.2.0", active: true }),
    ],
    activeManifest: null,
    declaredManifest: null,
  };
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");
  fireEvent.click(screen.getByRole("button", { name: "Versions" }));

  expect(screen.queryByRole("button", { name: "Roll back to 0.1.0" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Roll back to 0.2.0" })).toBeNull();
  expect(screen.getByText(/— bad/)).toBeTruthy();
});

test("a third-party (non-first-party) release is labeled distinctly from a first-party one", async () => {
  mimoReleaseFixture = {
    pluginId: "mimo",
    activeVersion: "0.2.0",
    releases: [releaseInfo({ version: "0.2.0", active: true, firstParty: false, signingKeyId: "some-other-key" })],
    activeManifest: null,
    declaredManifest: null,
  };
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");
  fireEvent.click(screen.getByRole("button", { name: "Versions" }));

  expect(screen.getByText("Third-party (key: some-other-key)")).toBeTruthy();
});

// ---------- Task 15c: Atlassian/Bitbucket render as INDEPENDENT experiences,
// each with its own single, non-shared OAuth profile ----------
//
// Both are first-party connector components (like mimo/opencode above), each
// a separate signed bundle with exactly one `[[oauth]]` profile in its own
// manifest. Nothing in `PluginDetailView` branches on plugin id — this proves
// the GENERIC permission-summary rendering already keeps the two profiles
// (and their scopes) fully separate, never implying a shared token.

test("atlassian's detail page shows only its own atlassian-cloud profile, never bitbucket-cloud", async () => {
  componentReleaseFixtures.atlassian = {
    pluginId: "atlassian",
    activeVersion: "0.1.0",
    releases: [releaseInfo({ pluginId: "atlassian", version: "0.1.0", active: true })],
    activeManifest: {
      publisher: "Ryuzi",
      description: "Jira + Confluence over one host-managed atlassian-cloud 3LO OAuth profile.",
      lifecycle: "per-call",
      domains: ["api.atlassian.com", "auth.atlassian.com"],
      oauthProfiles: [{ id: "atlassian-cloud", scopes: ["read:jira-work", "write:jira-work"] }],
    },
    declaredManifest: null,
  };
  render(<PluginDetailView id="atlassian" />);
  await screen.findByText("atlassian");

  expect(screen.getByText(/atlassian-cloud \(read:jira-work, write:jira-work\)/)).toBeTruthy();
  expect(screen.queryByText(/bitbucket-cloud/)).toBeNull();
});

test("bitbucket's detail page shows only its own bitbucket-cloud profile, never atlassian-cloud", async () => {
  componentReleaseFixtures.bitbucket = {
    pluginId: "bitbucket",
    activeVersion: "0.1.0",
    releases: [releaseInfo({ pluginId: "bitbucket", version: "0.1.0", active: true })],
    activeManifest: {
      publisher: "Ryuzi",
      description: "Bitbucket Cloud over a host-managed bitbucket-cloud OAuth profile, distinct from atlassian-cloud.",
      lifecycle: "per-call",
      domains: ["api.bitbucket.org", "bitbucket.org"],
      oauthProfiles: [{ id: "bitbucket-cloud", scopes: ["account", "repository"] }],
    },
    declaredManifest: null,
  };
  render(<PluginDetailView id="bitbucket" />);
  await screen.findByText("bitbucket");

  expect(screen.getByText(/bitbucket-cloud \(account, repository\)/)).toBeTruthy();
  expect(screen.queryByText(/atlassian-cloud/)).toBeNull();
});

test("rendering both in sequence never leaks one's profile into the other's page (two independent experiences, not a shared one)", async () => {
  componentReleaseFixtures.atlassian = {
    pluginId: "atlassian",
    activeVersion: "0.1.0",
    releases: [releaseInfo({ pluginId: "atlassian", version: "0.1.0", active: true })],
    activeManifest: {
      publisher: "Ryuzi",
      description: "",
      lifecycle: "per-call",
      domains: ["api.atlassian.com"],
      oauthProfiles: [{ id: "atlassian-cloud", scopes: [] }],
    },
    declaredManifest: null,
  };
  componentReleaseFixtures.bitbucket = {
    pluginId: "bitbucket",
    activeVersion: "0.1.0",
    releases: [releaseInfo({ pluginId: "bitbucket", version: "0.1.0", active: true })],
    activeManifest: {
      publisher: "Ryuzi",
      description: "",
      lifecycle: "per-call",
      domains: ["api.bitbucket.org"],
      oauthProfiles: [{ id: "bitbucket-cloud", scopes: [] }],
    },
    declaredManifest: null,
  };

  const first = render(<PluginDetailView id="atlassian" />);
  await screen.findByText(/atlassian-cloud/);
  first.unmount();

  render(<PluginDetailView id="bitbucket" />);
  await screen.findByText(/bitbucket-cloud/);
  expect(screen.queryByText(/atlassian-cloud/)).toBeNull();
});

// Task 9: a PKCE-only profile (no device-authorization endpoint) must still
// surface a Settings/auth tab and its own connections-card row — before this
// task, `hasComponentOauth` only checked the device-flow pair, so a
// PKCE-only component like atlassian had literally no way to reach its own
// Connect action once installed.
test("a PKCE-only profile still gets a settings tab with a disabled Connect + settings hint before a client id is configured", async () => {
  const atlassianDetail = componentDetail("atlassian");
  pluginDetail.mockImplementationOnce((_runnerId: string, _id: string) =>
    ok({ ...atlassianDetail, info: { ...atlassianDetail.info, installed: true } }),
  );
  componentReleaseFixtures.atlassian = {
    pluginId: "atlassian",
    activeVersion: "0.1.0",
    releases: [releaseInfo({ pluginId: "atlassian", version: "0.1.0", active: true })],
    activeManifest: {
      publisher: "Ryuzi",
      description: "",
      lifecycle: "per-call",
      domains: ["api.atlassian.com"],
      oauthProfiles: [
        {
          id: "atlassian-cloud",
          scopes: ["read:jira-work"],
          tokenUrl: "https://auth.atlassian.com/oauth/token",
          deviceAuthorizationUrl: null,
          connected: false,
          authorizeUrl: "https://auth.atlassian.com/authorize",
          clientIdConfigured: false,
        },
      ],
    },
    declaredManifest: null,
  };

  render(<PluginDetailView id="atlassian" />);
  await screen.findByText("atlassian");

  fireEvent.click(await screen.findByRole("button", { name: "Settings" }));
  const panel = within(screen.getByTestId("tab-panel-settings"));

  expect(panel.getByText("Connections (OAuth)")).toBeTruthy();
  expect(panel.getByText("atlassian-cloud")).toBeTruthy();
  expect((panel.getByRole("button", { name: "Connect" }) as HTMLButtonElement).disabled).toBe(true);
  expect(panel.getByText("Enter the OAuth client id in Settings first.")).toBeTruthy();
});

// ---------- Tabbed scaffold: hero actions + deep-link consumption — Task 9 ----------

test("initialTab deep-link is honored when the tab is visible (App.tsx's Fix → tab wiring)", async () => {
  render(<PluginDetailView id="github" initialTab="settings" />);
  await screen.findByText("GitHub");

  // No click needed — the Settings tab's own content (Authentication) is
  // already showing, and the Segmented control reflects the selection.
  expect(screen.getByText("Authentication")).toBeTruthy();
  const settingsTabButton = screen.getByRole("button", { name: "Settings" });
  expect(settingsTabButton.className).toContain("bg-background");
});

test("initialTab snaps back to overview when the requested tab isn't visible for this plugin", async () => {
  // github has no doctor findings, so it has no Health tab at all — a
  // stale/irrelevant deep link must not strand the view on dead tab state.
  render(<PluginDetailView id="github" initialTab="health" />);
  await screen.findByText("GitHub");

  expect(screen.getByText("About")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Health" })).toBeNull();
  const overviewTabButton = screen.getByRole("button", { name: "Overview" });
  expect(overviewTabButton.className).toContain("bg-background");
});

test("the overflow menu's Uninstall calls the store's uninstall then navigates back", async () => {
  const goBackSpy = spyOn(useNav.getState(), "goBack");
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  fireEvent.click(screen.getByRole("button", { name: "Actions for GitHub" }));
  fireEvent.click(await screen.findByRole("menuitem", { name: "Uninstall" }));

  await waitFor(() => expect(uninstallPlugin).toHaveBeenCalledWith(LOCAL_RUNNER, "github"));
  await waitFor(() => expect(goBackSpy).toHaveBeenCalled());
  goBackSpy.mockRestore();
});

test("pre-install (never enabled/configured, non-experimental, non-component) shows Install instead of Enabled, and no Settings tab", async () => {
  render(<PluginDetailView id="acme-fresh" />);
  await screen.findByText("Acme Fresh");

  expect(screen.queryByRole("switch", { name: "Enabled" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Settings" })).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "Install" }));

  // Task 15: the hero's Install action opens the universal wizard (starting
  // on Overview) for every kind now — `beginPluginInstall` is the classic
  // connector adapter's "install" step, one Continue click away.
  expect(await screen.findByRole("dialog", { name: "Install Acme Fresh" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Continue" }));
  await waitFor(() => expect(beginPluginInstall).toHaveBeenCalledWith(LOCAL_RUNNER, "acme-fresh"));
});

// ---------- Setup checklist (Overview) — Task 11 ----------
//
// `oauthDetail` is already the exact mid-setup shape the brief calls for
// (installed, oauth, not connected) — no new fixture needed for the
// "appears" half; `oauthConnectedDetail` (same plugin, fully connected)
// proves the "hides once complete" half.

test("the setup checklist appears on Overview for a mid-setup plugin (installed, oauth, not connected)", async () => {
  render(<PluginDetailView id="acme-oauth" />);
  await screen.findByText("Acme OAuth");

  const panel = within(screen.getByTestId("tab-panel-overview"));
  expect(panel.getByText("Finish setting up")).toBeTruthy();
  // install is already done (installed: true); connect is the first (and
  // only) undone item, so it — not install — carries the action button.
  expect(panel.getByText("Install the plugin").className).toContain("text-muted-foreground");
  expect(panel.getByText("Connect your account").className).not.toContain("text-muted-foreground");
  expect(panel.getByRole("button", { name: "Connect" })).toBeTruthy();
});

test("the setup checklist is hidden once every item is done", async () => {
  render(<PluginDetailView id="acme-oauth-connected" />);
  await screen.findByText("Acme OAuth Connected");

  expect(screen.queryByText("Finish setting up")).toBeNull();
});

// Task 14: the checklist's Connect/Settings actions now open the universal
// wizard resumed at that step, instead of just switching to the Settings
// tab — this works for a non-component connector too (its own plan still
// has a connect step, gated on `authKind !== "none"`).
test("the checklist's Connect action opens the universal wizard resumed at the connect step", async () => {
  render(<PluginDetailView id="acme-oauth" />);
  await screen.findByText("Acme OAuth");

  const panel = within(screen.getByTestId("tab-panel-overview"));
  fireEvent.click(panel.getByRole("button", { name: "Connect" }));

  const dialog = await screen.findByRole("dialog", { name: "Install Acme OAuth" });
  // acme-oauth: not component-backed (no permissions step), no settings —
  // plan is overview/install/connect/done (4 steps); connect is index 2. The
  // dialog itself renders before its own `pluginDetail` fetch resolves (it
  // starts on "overview" while loading), so the resumed position needs its
  // own wait rather than a bare `getByText` right after the dialog appears.
  expect(await within(dialog).findByText("Step 3 of 4 — Connect")).toBeTruthy();
});

test("the checklist's install action reuses the hero's Install handler (component-backed opens the wizard)", async () => {
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");

  // mimo is never-installed + component-backed: the checklist's first
  // undone item is "install", scoped to the Overview panel so this can't
  // accidentally hit the hero's OWN (separate) Install button above it.
  const panel = within(screen.getByTestId("tab-panel-overview"));
  expect(panel.getByText("Finish setting up")).toBeTruthy();
  fireEvent.click(panel.getByRole("button", { name: "Install" }));

  // Same behavior the hero's own Install button exercises elsewhere
  // (component-backed → open the universal wizard) — proves the checklist
  // reused that handler rather than duplicating the branch.
  expect(await screen.findByRole("dialog", { name: "Install mimo" })).toBeTruthy();
});

// ---------- Task 14: Contents tab ----------

test("Contents tab lists commands (as /name, mono) and skills", async () => {
  render(<PluginDetailView id="acme-suite" />);
  await screen.findByText("Acme Suite");

  fireEvent.click(screen.getByRole("button", { name: "Contents" }));
  const panel = within(screen.getByTestId("tab-panel-contents"));

  expect(panel.getByText("/review")).toBeTruthy();
  expect(panel.getByText("/deploy")).toBeTruthy();
  expect(panel.getByText("release-notes")).toBeTruthy();
});

test("Contents tab is absent for a plugin with no commands or skills", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  expect(screen.queryByRole("button", { name: "Contents" })).toBeNull();
});

// ---------- Task 14: Automations tab ----------

test("Automations tab shows a trigger-alias label, toggles a ready hook's enable switch, and reloads the row", async () => {
  render(<PluginDetailView id="acme-suite" />);
  await screen.findByText("Acme Suite");

  fireEvent.click(screen.getByRole("button", { name: "Automations" }));
  const panel = within(screen.getByTestId("tab-panel-automations"));

  expect(panel.getByText("Stop · session.end")).toBeTruthy();
  const sw = panel.getByRole("switch", { name: "Enable acme-suite/ready" });
  expect(sw.getAttribute("aria-checked")).toBe("true");

  fireEvent.click(sw);
  await waitFor(() => expect(toggleAutomationHook).toHaveBeenCalledWith(LOCAL_RUNNER, "hook-ready", false));
  await waitFor(() => expect(panel.getByRole("switch", { name: "Enable acme-suite/ready" }).getAttribute("aria-checked")).toBe("false"));
});

test("a needsTarget hook shows 'Set up…' instead of a switch, and it deep-links to Automations", async () => {
  render(<PluginDetailView id="acme-suite" />);
  await screen.findByText("Acme Suite");
  fireEvent.click(screen.getByRole("button", { name: "Automations" }));
  const panel = within(screen.getByTestId("tab-panel-automations"));

  expect(panel.queryByRole("switch", { name: "Enable acme-suite/onrun" })).toBeNull();
  // Two "Set up…" buttons render (one for the hook, one for the job below) —
  // Hooks is the first card, so index 0 is this one.
  fireEvent.click(panel.getAllByRole("button", { name: "Set up…" })[0]);

  expect(useNav.getState().history.current).toEqual({ kind: "automations", tab: "hooks", targetId: "hook-needs-target" });
  expect(toggleAutomationHook).not.toHaveBeenCalledWith(LOCAL_RUNNER, "hook-needs-target", expect.anything());
});

test("Automations tab toggles a ready job's enable switch", async () => {
  render(<PluginDetailView id="acme-suite" />);
  await screen.findByText("Acme Suite");
  fireEvent.click(screen.getByRole("button", { name: "Automations" }));
  const panel = within(screen.getByTestId("tab-panel-automations"));

  const sw = panel.getByRole("switch", { name: "Enable Nightly sweep" });
  fireEvent.click(sw);
  await waitFor(() => expect(toggleJob).toHaveBeenCalledWith(LOCAL_RUNNER, "job-ready", false));
});

test("a needsTarget job shows 'Set up…' that deep-links to the Scheduler", async () => {
  render(<PluginDetailView id="acme-suite" />);
  await screen.findByText("Acme Suite");
  fireEvent.click(screen.getByRole("button", { name: "Automations" }));
  const panel = within(screen.getByTestId("tab-panel-automations"));

  expect(panel.queryByRole("switch", { name: "Enable New job" })).toBeNull();
  const setupButtons = panel.getAllByRole("button", { name: "Set up…" });
  fireEvent.click(setupButtons[1]);

  expect(useNav.getState().history.current).toEqual({ kind: "automations", tab: "scheduler", targetId: "job-needs-target" });
});

test("Automations tab is absent for a plugin with no hooks or jobs", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  expect(screen.queryByRole("button", { name: "Automations" })).toBeNull();
});

// ---------- Task 14: Tools tab per-tool perm select ----------

test("an installed + trusted plugin with a matching MCP app row shows full mcp__<id>__<tool> names and a perm select", async () => {
  appsFixture = [
    {
      id: "acme-suite",
      name: "Acme Suite",
      kind: "mcp",
      initial: "A",
      color: "#123456",
      desc: "",
      transport: "stdio",
      command: "acme-suite-server",
      args: [],
      url: null,
      scope: "global",
      scopeGateways: [],
      status: "connected",
      statusDetail: null,
      version: null,
      publisher: null,
      authKind: "none",
      authDetail: null,
      tools: [{ name: "create_issue", desc: "Open an issue", perm: "ask" }],
      agentAccess: [],
      pluginId: "acme-suite",
    },
  ];
  pluginToolsFixtures["acme-suite"] = {
    live: true,
    entries: [{ name: "create_issue", description: "Open an issue", kind: "tool", writes: true }],
  };
  render(<PluginDetailView id="acme-suite" />);
  await screen.findByText("Acme Suite");
  await waitFor(() => expect(listApps).toHaveBeenCalled());

  fireEvent.click(await screen.findByRole("button", { name: /^Tools/ }));
  const panel = within(screen.getByTestId("tab-panel-tools"));

  expect(await panel.findByText("mcp__acme-suite__create_issue")).toBeTruthy();
  expect(panel.queryByText("create_issue")).toBeNull();

  fireEvent.click(panel.getByRole("button", { name: "Deny" }));
  await waitFor(() => expect(setAppToolPerm).toHaveBeenCalledWith(LOCAL_RUNNER, "acme-suite", "create_issue", "deny"));
});

test("Tools tab renders short names with no perm select when the plugin has no matching app row (not installed/trusted live)", async () => {
  pluginToolsFixtures.github = {
    live: true,
    entries: [{ name: "create_issue", description: "Open an issue", kind: "tool", writes: true }],
  };
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  fireEvent.click(await screen.findByRole("button", { name: /^Tools/ }));
  const panel = within(screen.getByTestId("tab-panel-tools"));

  expect(await panel.findByText("create_issue")).toBeTruthy();
  expect(panel.queryByText("mcp__github__create_issue")).toBeNull();
  expect(panel.queryByRole("button", { name: "Deny" })).toBeNull();
});
