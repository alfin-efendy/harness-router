import { afterEach, beforeEach, expect, mock, spyOn, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { DoctorFinding, ExtensionStatusEntry, PluginDetail } from "@/bindings";
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
  },
  auth: null,
  settings: [],
  mcp: [],
  models: [],
  homepage: "https://vercel.com/docs/vercel-sandbox",
  publisher: "Vercel (no MCP surface)",
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
  },
  auth: null,
  settings: [],
  mcp: [],
  models: [],
  homepage: null,
  publisher: "Acme",
};

// A plugin declaring an `[[extension]]` (Track D "code plugin") capability —
// exercises the Extension status card (DT8), gated on
// `info.capabilities.includes("extension")`.
const extensionDetail: PluginDetail = {
  info: {
    id: "acme-ext",
    name: "Acme Ext",
    description: "Ships a supervised extension subprocess.",
    icon: "sparkles",
    categories: [],
    slot: null,
    ownsSlot: false,
    verified: false,
    experimental: false,
    enabled: true,
    source: "catalog",
    capabilities: ["extension"],
    configured: false,
    kind: "integration",
    // enabled: true above → installed_flag's `configured || enabled` is true
    // (needed so the Health tab, which carries the Extension card, exists).
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
  },
  auth: null,
  settings: [],
  mcp: [],
  models: [],
  homepage: null,
  publisher: "Acme",
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
  },
  auth: null,
  settings: [],
  mcp: [],
  models: [],
  homepage: null,
  publisher: "acme/pack",
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
};

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
  };
}

const pluginDetail = mock((_runnerId: string, id: string) => {
  if (id === "github") return ok(githubDetail);
  if (id === "ollama") return ok(ollamaDetail);
  if (id === "acme-oauth") return ok(oauthDetail);
  if (id === "acme-rich") return ok(richFieldsDetail);
  if (id === "vercel-sandbox") return ok(sandboxDetail);
  if (id === "acme-fresh") return ok(freshDetail);
  if (id === "acme-ext") return ok(extensionDetail);
  if (id === "acme-pack") return ok({ ...skillPackDetail, info: { ...skillPackDetail.info, pinned: acmePackPinned } });
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
let extensionStatusFixture: ExtensionStatusEntry[] = [];
const extensionStatus = mock(() => ok(extensionStatusFixture));
const updatePlugin = mock((_runnerId: string, _id: string, _force: boolean) => ok({ kind: "updated" as const }));
const setPluginPin = mock((_runnerId: string, id: string, pinned: boolean, _reason: string | null) => {
  if (id === "acme-pack") acmePackPinned = pinned;
  return ok(null);
});
const uninstallPlugin = mock((_runnerId: string, _id: string) => ok([]));
// Task 9: the pre-install hero's Install action opens `InstallWizardModal`
// for a non-component plugin — these are its own mount-time RPCs (a full
// happy-path stub, matching `InstallWizardModal.test.tsx`'s own baseline),
// mocked here just enough that it mounts without throwing; the wizard's own
// behavior is that file's job, not this view's.
const beginPluginInstall = mock((_runnerId: string, _pluginId: string) =>
  ok({
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
  }),
);
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
    oauthProfiles: { id: string; scopes: string[] }[];
  } | null;
};
const emptyReleaseDetail = (id: string): ReleaseDetailFixture => ({
  pluginId: id,
  releases: [],
  activeVersion: null,
  activeManifest: null,
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
    extensionStatus,
    pluginReleaseDetail,
    installComponentPlugin,
    rollbackComponentPlugin,
    beginPluginInstall,
    cancelPluginInstall,
    setPluginOauthClientId,
  },
}));
mock.module("@tauri-apps/plugin-opener", () => ({ openUrl }));

const { PluginDetailView, visibleTabs } = await import("@/views/PluginDetailView");
const { usePlugins } = await import("@/store-plugins");
const { useNav } = await import("@/store-nav");

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
  extensionStatus.mockClear();
  pluginReleaseDetail.mockClear();
  installComponentPlugin.mockClear();
  rollbackComponentPlugin.mockClear();
  beginPluginInstall.mockClear();
  cancelPluginInstall.mockClear();
  setPluginOauthClientId.mockClear();
  doctorFindingsFixture = [];
  extensionStatusFixture = [];
  acmePackPinned = false;
  mimoReleaseFixture = emptyReleaseDetail("mimo");
  componentReleaseFixtures = {};
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
  });
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
  });
});

// ---------- visibleTabs (Task 9) — pure, no mounting ----------

test("visibleTabs: pre-install component row shows overview, tools, and versions", () => {
  expect(
    visibleTabs({ installed: false, hasTools: true, hasAuth: false, hasSettings: false, hasVersions: true, hasHealth: false }),
  ).toEqual(["overview", "tools", "versions"]);
});

test("visibleTabs: installed connector with auth+findings shows all five tabs", () => {
  expect(visibleTabs({ installed: true, hasTools: true, hasAuth: true, hasSettings: true, hasVersions: true, hasHealth: true })).toEqual([
    "overview",
    "tools",
    "settings",
    "versions",
    "health",
  ]);
});

test("visibleTabs: installed, no auth, no settings omits the settings tab even with everything else present", () => {
  expect(visibleTabs({ installed: true, hasTools: true, hasAuth: false, hasSettings: false, hasVersions: true, hasHealth: true })).toEqual([
    "overview",
    "tools",
    "versions",
    "health",
  ]);
});

test("visibleTabs: settings needs BOTH installed and (auth or settings) — not-installed hides it despite auth/settings", () => {
  expect(
    visibleTabs({ installed: false, hasTools: false, hasAuth: true, hasSettings: true, hasVersions: false, hasHealth: false }),
  ).toEqual(["overview"]);
});

test("visibleTabs: health needs BOTH installed and hasHealth — not-installed hides it despite findings", () => {
  expect(
    visibleTabs({ installed: false, hasTools: false, hasAuth: false, hasSettings: false, hasVersions: false, hasHealth: true }),
  ).toEqual(["overview"]);
});

test("visibleTabs: versions is independent of installed (a component-backed plugin's install gate lives there)", () => {
  expect(
    visibleTabs({ installed: false, hasTools: false, hasAuth: false, hasSettings: false, hasVersions: true, hasHealth: false }),
  ).toEqual(["overview", "versions"]);
});

test("visibleTabs: nothing beyond overview when every input is false", () => {
  expect(
    visibleTabs({ installed: true, hasTools: false, hasAuth: false, hasSettings: false, hasVersions: false, hasHealth: false }),
  ).toEqual(["overview"]);
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

test("renders a Models card listing every model for provider-capable plugins", async () => {
  render(<PluginDetailView id="ollama" />);
  await screen.findByText("Ollama");

  // Models stays on Overview (Task 10 moves it into the Tools tab).
  expect(screen.getByText("Models")).toBeTruthy();
  expect(screen.getByText("llama3")).toBeTruthy();
  expect(screen.getByText("mistral")).toBeTruthy();

  // The declared settings field moved into the Settings tab.
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(screen.getByText("Base URL")).toBeTruthy();
  expect(screen.getByPlaceholderText("Optional — not set")).toBeTruthy();
});

test("omits the Models card for non-provider plugins", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  expect(screen.queryByText("Models")).toBeNull();
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

// ---------- Extension (Track D "code plugin") status card — DT8 ----------

test("a non-extension plugin never calls extension_status and renders no Extension card", async () => {
  render(<PluginDetailView id="github" />);
  await screen.findByText("GitHub");

  expect(extensionStatus).not.toHaveBeenCalled();
  expect(screen.queryByText("Extension")).toBeNull();
  expect(screen.queryByText("Runs code")).toBeNull();
});

test("an extension-capable plugin fetches extension_status and shows the Runs code badge", async () => {
  extensionStatusFixture = [];
  render(<PluginDetailView id="acme-ext" />);
  await screen.findByText("Acme Ext");

  // "Runs code" is a hero badge (always visible); the Extension card itself
  // moved into the Health tab.
  expect(screen.getByText("Runs code")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Health" }));
  expect(await screen.findByText("Extension")).toBeTruthy();
  await waitFor(() => expect(extensionStatus).toHaveBeenCalled());
  expect(await screen.findByText("No extension status reported yet.")).toBeTruthy();
});

test("renders a Running extension's status badge", async () => {
  extensionStatusFixture = [
    {
      pluginId: "acme-ext",
      name: "linter",
      status: "running",
      restartCount: 0,
      lastError: null,
      confirmedEvents: ["tool.before"],
      toolCount: 2,
    },
  ];
  render(<PluginDetailView id="acme-ext" />);
  await screen.findByText("Acme Ext");
  fireEvent.click(screen.getByRole("button", { name: "Health" }));

  expect(await screen.findByText("linter")).toBeTruthy();
  expect(screen.getByText("Running")).toBeTruthy();
  expect(screen.queryByText(/restart/)).toBeNull();
});

test("renders a Failed extension's restart count and sanitized last error", async () => {
  extensionStatusFixture = [
    {
      pluginId: "acme-ext",
      name: "linter",
      status: "failed",
      restartCount: 5,
      lastError: "linter: restart-exhausted: 5 restarts within 300s",
      confirmedEvents: [],
      toolCount: 0,
    },
  ];
  render(<PluginDetailView id="acme-ext" />);
  await screen.findByText("Acme Ext");
  fireEvent.click(screen.getByRole("button", { name: "Health" }));

  expect(await screen.findByText("Failed")).toBeTruthy();
  expect(screen.getByText("5 restarts")).toBeTruthy();
  expect(screen.getByText("linter: restart-exhausted: 5 restarts within 300s")).toBeTruthy();
});

test("extension_status entries for a different plugin are filtered out", async () => {
  extensionStatusFixture = [
    { pluginId: "other-plugin", name: "other", status: "running", restartCount: 0, lastError: null, confirmedEvents: [], toolCount: 0 },
  ];
  render(<PluginDetailView id="acme-ext" />);
  await screen.findByText("Acme Ext");
  fireEvent.click(screen.getByRole("button", { name: "Health" }));

  expect(await screen.findByText("No extension status reported yet.")).toBeTruthy();
  expect(screen.queryByText("other")).toBeNull();
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
  // The hero's Install action is reachable even before any release exists —
  // for a component-backed plugin it jumps to the Versions tab, where the
  // real install gate lives.
  fireEvent.click(screen.getByRole("button", { name: "Install" }));
  expect(await screen.findByText("Not installed")).toBeTruthy();
  expect(screen.queryByText("Plugin not found.")).toBeNull();
});

test("an unrelated unknown plugin id still shows Plugin not found", async () => {
  render(<PluginDetailView id="ghost-2" />);
  expect(await screen.findByText("Plugin not found.")).toBeTruthy();
});

test("install is DISABLED until the permission-acceptance switch is toggled, then dispatches installComponentPlugin", async () => {
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");

  // The hero ALSO shows an (always-enabled, navigational) "Install" button
  // while `!info.installed` — scope queries to the Versions tab panel so
  // they target `ComponentReleaseCard`'s own gated Install button instead.
  fireEvent.click(screen.getByRole("button", { name: "Versions" }));
  const panel = within(screen.getByTestId("tab-panel-versions"));

  const install = panel.getByRole("button", { name: "Install" }) as HTMLButtonElement;
  expect(install.disabled).toBe(true);

  fireEvent.click(panel.getByRole("switch", { name: "Accept permissions" }));
  expect((panel.getByRole("button", { name: "Install" }) as HTMLButtonElement).disabled).toBe(false);

  fireEvent.click(panel.getByRole("button", { name: "Install" }));
  await waitFor(() => expect(installComponentPlugin).toHaveBeenCalledWith(LOCAL_RUNNER, "mimo", null));
});

test("the permission summary shows 'Unknown until…' before any release is installed", async () => {
  render(<PluginDetailView id="mimo" />);
  await screen.findByText("mimo");

  expect(screen.getByText(/Unknown until a release is fetched and its signature is verified/)).toBeTruthy();
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
  };

  const first = render(<PluginDetailView id="atlassian" />);
  await screen.findByText(/atlassian-cloud/);
  first.unmount();

  render(<PluginDetailView id="bitbucket" />);
  await screen.findByText(/bitbucket-cloud/);
  expect(screen.queryByText(/atlassian-cloud/)).toBeNull();
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
  // github has no doctor findings and isn't an extension plugin, so it has
  // no Health tab at all — a stale/irrelevant deep link must not strand the
  // view on dead tab state.
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
  await waitFor(() => expect(beginPluginInstall).toHaveBeenCalledWith(LOCAL_RUNNER, "acme-fresh"));
});
