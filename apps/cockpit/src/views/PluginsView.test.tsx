import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type {
  AddAppInput,
  AppInfo,
  CatalogStatus,
  InstalledSkillInfo,
  PluginDetail,
  PluginInfo,
  PluginInstallBeginResult,
} from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";

// happy-dom lacks a couple of layout APIs Base UI's Menu popup touches when
// positioning (same stub `combobox.test.tsx` uses for the Combobox popup) —
// stub them before anything renders.
if (typeof Element.prototype.scrollIntoView !== "function") {
  Element.prototype.scrollIntoView = () => {};
}
if (typeof globalThis.ResizeObserver === "undefined") {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;
}

// `verified` defaults to `false` (mirrors `plugin-hub.test.ts`'s `mkPlugin`) —
// `featuredItems` spotlights not-installed+verified rows regardless of the
// active rail filter, so a `verified: true` default would make many
// not-installed fixtures show up TWICE (once in the featured strip, once in
// the row list) and break exact-text queries. Tests that need the "Verified"
// badge opt in explicitly.
function plugin(id: string, categories: string[], over: Partial<PluginInfo> = {}): PluginInfo {
  return {
    id,
    name: id,
    description: "",
    icon: null,
    categories,
    slot: null,
    ownsSlot: false,
    verified: false,
    experimental: false,
    enabled: false,
    source: "catalog",
    capabilities: ["connector"],
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
    status: "ok",
    statusDetail: null,
    authKind: "none",
    toolCount: null,
    skillCount: null,
    ...over,
  };
}

// Distinct, human-readable names per row source so assertions never depend on
// accidental id/name collisions across plugins/apps/skills.
const githubPlugin = plugin("github", ["vcs", "issues"], { name: "GitHub", status: "not-installed" });
const notionPlugin = plugin("notion", ["docs"], { name: "Notion", installed: true, status: "ok", toolCount: 12 });
const anthropicPlugin = plugin("anthropic", ["model-provider"], {
  name: "Anthropic",
  kind: "provider",
  family: "anthropic",
  source: "builtin",
  status: "not-installed",
});
const superpowersPlugin = plugin("superpowers", ["skills"], {
  name: "Superpowers",
  kind: "skill-pack",
  source: "skill-pack",
  status: "not-installed",
});

const slackApp: AppInfo = {
  id: "slack",
  name: "Slack",
  kind: "MCP server",
  initial: "S",
  color: "#4A154B",
  desc: "Slack tools",
  transport: "stdio",
  command: "npx",
  args: ["-y", "@modelcontextprotocol/server-slack"],
  url: null,
  scope: "global",
  scopeGateways: [],
  status: "connected",
  statusDetail: null,
  version: "1.0.0",
  publisher: "Acme",
  authKind: "none",
  authDetail: null,
  tools: [],
  agentAccess: [],
};

const docsSkill: InstalledSkillInfo = {
  id: "docs-helper",
  name: "Docs Helper",
  source: "https://github.com/example/docs-helper",
  pluginId: null,
  installedAt: "2026-07-08T10:00:00Z",
  skillCount: 5,
};

// Mutable fixtures read by the mocks at call time. PluginsView's `hydrate`
// (apps), `load` (plugins), and `refresh` (skills) effects re-fetch on mount,
// so tests set these before rendering instead of seeding store state directly.
let appsFixture: AppInfo[] = [];
let pluginsFixture: PluginInfo[] = [];
let skillsFixture: InstalledSkillInfo[] = [];
let catalogStatusFixture: CatalogStatus = { sequence: 0, lastFetchAt: null, outcome: null, entries: 0, blocked: 0 };
let componentBootstrapStatusFixture: { pending: boolean; message: string | null } = { pending: false, message: null };

const listApps = mock(async () => ({ status: "ok" as const, data: appsFixture }));
const addApp = mock(async (_runnerId: string, _input: AddAppInput) => ({ status: "ok" as const, data: appsFixture }));
const listPlugins = mock(async () => ({ status: "ok" as const, data: pluginsFixture }));
const uninstallPlugin = mock(async (_runnerId: string, id: string) => ({
  status: "ok" as const,
  data: pluginsFixture.filter((p) => p.id !== id),
}));
const pluginsRestartRequired = mock(async () => ({ status: "ok" as const, data: false }));
const catalogStatus = mock(async () => ({ status: "ok" as const, data: catalogStatusFixture }));
const refreshCatalog = mock(async () => ({ status: "ok" as const, data: catalogStatusFixture }));
const pluginDoctor = mock(async () => ({ status: "ok" as const, data: [] as unknown[] }));
const updatePlugin = mock(async (_runnerId: string, _id: string, _force: boolean) => ({
  status: "ok" as const,
  data: { kind: "updated" as const },
}));
const updateAllPlugins = mock(async (_runnerId: string) => ({
  status: "ok" as const,
  data: [] as { id: string; outcome: { kind: string } }[],
}));
const setPluginPin = mock(async (_runnerId: string, _id: string, _pinned: boolean, _reason: string | null) => ({
  status: "ok" as const,
  data: null,
}));

const beginSkillInstall = mock(async (_runnerId: string, _source: string) => ({
  status: "ok" as const,
  data: {
    completed: true,
    trust: null,
    plugin: {
      id: "superpowers",
      name: "Superpowers",
      source: "superpowers",
      pluginId: null,
      installedAt: "2026-07-08T10:00:00Z",
      skills: [{ id: "superpowers:brainstorming", name: "brainstorming" }],
    },
  },
}));
const confirmSkillInstall = mock(async (_runnerId: string, _token: string) => ({
  status: "ok" as const,
  data: {
    id: "superpowers",
    name: "Superpowers",
    source: "superpowers",
    pluginId: null,
    installedAt: "2026-07-08T10:00:00Z",
    skills: [] as { id: string; name: string }[],
  },
}));

const listSkills = mock(async () => ({ status: "ok" as const, data: skillsFixture }));
const removeSkill = mock(async (_runnerId: string, _id: string) => ({ status: "ok" as const, data: null }));
const refreshSkill = mock(async (_runnerId: string, _id: string) => ({
  status: "ok" as const,
  data: {
    id: "superpowers",
    name: "Superpowers",
    source: "superpowers",
    pluginId: null,
    installedAt: "2026-07-08T10:00:00Z",
    skills: [{ id: "superpowers:brainstorming", name: "brainstorming" }],
  },
}));

// The wizard mounts inside PluginsView, so its IPC surface needs benign
// defaults here: begin resolves to authKind "none" with no settings, which
// routes the wizard straight to done.
const wizardDetail: PluginDetail = {
  info: githubPlugin,
  auth: null,
  settings: [],
  mcp: [],
  models: [],
  homepage: null,
  publisher: "GitHub",
};
const wizardBegin: PluginInstallBeginResult = {
  authKind: "none",
  envVarPresent: false,
  envVarName: null,
  oauthAvailable: false,
  oauthExternal: false,
  needsClientId: false,
  dcrSucceeded: false,
  callbackMode: "auto",
  oauthBegin: null,
  dcrError: null,
};
const pluginDetail = mock(async (_runnerId: string, _id: string) => ({ status: "ok" as const, data: wizardDetail }));
const beginPluginInstall = mock(async (_runnerId: string, _pluginId: string) => ({ status: "ok" as const, data: wizardBegin }));
const setPluginOauthClientId = mock(async (_runnerId: string, _pluginId: string, _clientId: string) => ({
  status: "ok" as const,
  data: null,
}));
const cancelPluginInstall = mock(async (_runnerId: string, _pluginId: string, _stateToken: string | null) => ({
  status: "ok" as const,
  data: null,
}));
const completePluginOauth = mock(async (_runnerId: string, _pluginId: string, _code: string, _stateToken: string) => ({
  status: "ok" as const,
  data: null,
}));
const setPluginSetting = mock(async (_runnerId: string, _key: string, _value: string) => ({ status: "ok" as const, data: null }));
const setPluginEnabled = mock(async (_runnerId: string, _id: string, _enabled: boolean) => ({ status: "ok" as const, data: null }));

function emptyReleaseDetail(id: string) {
  return { pluginId: id, releases: [] as unknown[], activeVersion: null as string | null, activeManifest: null };
}
const componentBootstrapStatus = mock(async () => ({ status: "ok" as const, data: componentBootstrapStatusFixture }));
const pluginReleaseDetail = mock(async (_runnerId: string, id: string) => ({ status: "ok" as const, data: emptyReleaseDetail(id) }));
const installComponentPlugin = mock(async (_runnerId: string, id: string, _version: string | null) => ({
  status: "ok" as const,
  data: emptyReleaseDetail(id),
}));
const rollbackComponentPlugin = mock(async (_runnerId: string, id: string, _fromVersion: string, _toVersion: string) => ({
  status: "ok" as const,
  data: emptyReleaseDetail(id),
}));
const pluginOauthCompletedMsgListen = mock(
  async (_cb: (event: { payload: { pluginId: string; ok: boolean; error: string | null } }) => void) => () => {},
);
const oauthAuthorizeUrlMsgListen = mock(async (_cb: (event: unknown) => void) => () => {});

mock.module("@/bindings", () => ({
  events: {
    pluginOauthCompletedMsg: { listen: pluginOauthCompletedMsgListen },
    oauthAuthorizeUrlMsg: { listen: oauthAuthorizeUrlMsgListen },
  },
  commands: {
    listApps,
    addApp,
    listPlugins,
    uninstallPlugin,
    pluginsRestartRequired,
    catalogStatus,
    refreshCatalog,
    pluginDoctor,
    updatePlugin,
    updateAllPlugins,
    setPluginPin,
    beginSkillInstall,
    confirmSkillInstall,
    listSkills,
    removeSkill,
    refreshSkill,
    pluginDetail,
    beginPluginInstall,
    setPluginOauthClientId,
    cancelPluginInstall,
    completePluginOauth,
    setPluginSetting,
    setPluginEnabled,
    componentBootstrapStatus,
    pluginReleaseDetail,
    installComponentPlugin,
    rollbackComponentPlugin,
  },
}));
const toastSuccess = mock((_message: string) => {});
const toastWarning = mock((_message: string) => {});
const toastError = mock((_message: string) => {});
mock.module("sonner", () => ({
  toast: { success: toastSuccess, warning: toastWarning, error: toastError, info: mock(() => {}) },
  Toaster: () => null,
}));

const { useSkills } = await import("../store-skills");
const { useApps } = await import("@/store-apps");
const { usePlugins } = await import("@/store-plugins");
const { useNav } = await import("@/store-nav");
const { useConnections } = await import("@/store-connections");
const { PluginsView } = await import("./PluginsView");

// Provider install now flows through the connections store's installed set,
// not the add-account modal. Override just that action with a mock and
// restore the real one on the way out (this store singleton is shared
// across test files in one bun process).
const installProviderMock = mock(async (_family: string) => true);
const defaultInstallProvider = useConnections.getState().installProvider;

// Render and flush the mount-effect fetches (apps via `hydrate`, plugins via
// `load`, skills via `refresh`) inside act so their setState calls do not
// fire mid-assertion.
async function renderView() {
  render(<PluginsView />);
  await act(async () => {});
}

function resetPluginsStore() {
  usePlugins.setState({
    plugins: [],
    loaded: false,
    restartRequired: false,
    doctorFindings: [],
    doctorLoaded: false,
    catalogStatus: null,
    componentBootstrapStatus: null,
    componentPlugins: [],
    componentPluginsLoaded: false,
  });
}

beforeEach(() => {
  appsFixture = [];
  pluginsFixture = [];
  skillsFixture = [];
  catalogStatusFixture = { sequence: 0, lastFetchAt: null, outcome: null, entries: 0, blocked: 0 };
  componentBootstrapStatusFixture = { pending: false, message: null };
  listApps.mockClear();
  addApp.mockClear();
  listPlugins.mockClear();
  uninstallPlugin.mockClear();
  pluginsRestartRequired.mockClear();
  catalogStatus.mockClear();
  refreshCatalog.mockClear();
  pluginDoctor.mockClear();
  updatePlugin.mockClear();
  updateAllPlugins.mockClear();
  setPluginPin.mockClear();
  beginSkillInstall.mockClear();
  confirmSkillInstall.mockClear();
  listSkills.mockClear();
  removeSkill.mockClear();
  refreshSkill.mockClear();
  pluginDetail.mockClear();
  beginPluginInstall.mockClear();
  setPluginOauthClientId.mockClear();
  cancelPluginInstall.mockClear();
  completePluginOauth.mockClear();
  setPluginSetting.mockClear();
  setPluginEnabled.mockClear();
  pluginOauthCompletedMsgListen.mockClear();
  oauthAuthorizeUrlMsgListen.mockClear();
  toastSuccess.mockClear();
  toastWarning.mockClear();
  toastError.mockClear();
  installProviderMock.mockClear();
  componentBootstrapStatus.mockClear();
  pluginReleaseDetail.mockClear();
  installComponentPlugin.mockClear();
  rollbackComponentPlugin.mockClear();
  useConnections.setState({ installProvider: installProviderMock });
  useApps.setState({ apps: [], loaded: false, probing: null });
  resetPluginsStore();
  useNav.setState({ history: { back: [], current: { kind: "plugins" }, forward: [] } });
  useSkills.setState({ skills: [], loading: false, error: null });
});

afterEach(() => {
  cleanup();
  useConnections.setState({ installProvider: defaultInstallProvider });
  useApps.setState({ apps: [], loaded: false, probing: null });
  resetPluginsStore();
  useNav.setState({ history: { back: [], current: { kind: "home" }, forward: [] } });
  useSkills.setState({ skills: [], loading: false, error: null });
});

// ---------- Row rendering across the three sources ----------

test("renders one row per plugin, app, and non-plugin skill source", async () => {
  pluginsFixture = [githubPlugin, notionPlugin];
  appsFixture = [slackApp];
  skillsFixture = [docsSkill];
  await renderView();

  expect(await screen.findByText("GitHub")).toBeTruthy();
  expect(screen.getByText("Notion")).toBeTruthy();
  expect(screen.getByText("Slack")).toBeTruthy();
  expect(screen.getByText("Docs Helper")).toBeTruthy();
});

test("header shows the Plugins title, search box, and a single + Add menu (no separate top-level buttons)", async () => {
  await renderView();

  expect(screen.getByRole("heading", { name: "Plugins" })).toBeTruthy();
  expect(screen.getByPlaceholderText("Search plugins, tools, skills")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Add" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Add MCP server" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Add skill source" })).toBeNull();
});

test("a skill source backed by an installed plugin id renders once, as the plugin row (not a duplicate)", async () => {
  const installedPack = { ...superpowersPlugin, installed: true, status: "ok" as const };
  pluginsFixture = [installedPack];
  skillsFixture = [{ ...docsSkill, id: "superpowers", name: "Superpowers", pluginId: null }];
  await renderView();

  expect(await screen.findByText("Superpowers")).toBeTruthy();
  expect(screen.getAllByText("Superpowers")).toHaveLength(1);
});

// ---------- Rail state filters ----------

test("clicking the Discover rail entry hides installed rows and keeps not-installed rows", async () => {
  pluginsFixture = [githubPlugin, notionPlugin];
  await renderView();
  await screen.findByText("Notion");

  fireEvent.click(screen.getByText("Discover"));

  expect(screen.queryByText("Notion")).toBeNull();
  expect(screen.getByText("GitHub")).toBeTruthy();
});

test("clicking the Installed rail entry hides not-installed rows", async () => {
  pluginsFixture = [githubPlugin, notionPlugin];
  await renderView();
  await screen.findByText("GitHub");

  fireEvent.click(screen.getByText("Installed"));

  expect(screen.queryByText("GitHub")).toBeNull();
  expect(screen.getByText("Notion")).toBeTruthy();
});

test("clicking the Providers kind rail entry narrows the list to provider rows", async () => {
  pluginsFixture = [githubPlugin, notionPlugin, { ...anthropicPlugin, installed: true, status: "ok" as const }];
  await renderView();
  await screen.findByText("Anthropic");

  fireEvent.click(screen.getByText("Providers"));

  expect(screen.getByText("Anthropic")).toBeTruthy();
  expect(screen.queryByText("GitHub")).toBeNull();
  expect(screen.queryByText("Notion")).toBeNull();
});

// ---------- Search ----------

test("typing in the search box filters the row list live", async () => {
  pluginsFixture = [githubPlugin, notionPlugin];
  await renderView();
  await screen.findByText("Notion");

  fireEvent.change(screen.getByPlaceholderText("Search plugins, tools, skills"), { target: { value: "notion" } });

  expect(screen.queryByText("GitHub")).toBeNull();
  expect(screen.getByText("Notion")).toBeTruthy();
});

// ---------- Row status / action button ----------

test("an attach-failed installed plugin shows a Fix button and the Attach failed status label", async () => {
  const broken = plugin("broken", ["vcs"], { name: "Broken", installed: true, status: "attach-failed", statusDetail: "token rejected" });
  pluginsFixture = [broken];
  await renderView();

  expect(await screen.findByText("Attach failed")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Fix Broken" })).toBeTruthy();
});

test("Fix navigates to the plugin detail page on the tab that resolves the status (health for attach-failed)", async () => {
  const broken = plugin("broken", ["vcs"], { name: "Broken", installed: true, status: "attach-failed" });
  pluginsFixture = [broken];
  await renderView();
  await screen.findByText("Broken");

  fireEvent.click(screen.getByRole("button", { name: "Fix Broken" }));

  expect(useNav.getState().history.current).toEqual({ kind: "pluginDetail", id: "broken", tab: "health" });
});

test("a not-installed plugin row shows the Install button", async () => {
  pluginsFixture = [githubPlugin];
  await renderView();

  expect(await screen.findByRole("button", { name: "Install GitHub" })).toBeTruthy();
});

test("a healthy installed plugin shows a Manage button that navigates with no tab", async () => {
  pluginsFixture = [notionPlugin];
  await renderView();
  await screen.findByText("Notion");

  fireEvent.click(screen.getByRole("button", { name: "Manage Notion" }));

  expect(useNav.getState().history.current).toEqual({ kind: "pluginDetail", id: "notion" });
});

test("clicking the row body (not the action button) also navigates to the detail page", async () => {
  pluginsFixture = [notionPlugin];
  await renderView();

  fireEvent.click(await screen.findByText("Notion"));

  expect(useNav.getState().history.current).toEqual({ kind: "pluginDetail", id: "notion" });
});

test("a blocked plugin shows its blockedReason and no action button", async () => {
  const evil = plugin("evil-plugin", ["vcs"], {
    name: "Evil Plugin",
    blockedReason: "revoked: known-malicious update",
    status: "blocked",
  });
  pluginsFixture = [evil];
  await renderView();

  expect(await screen.findByText("revoked: known-malicious update")).toBeTruthy();
  // No action button (Install/Fix/Manage) for a blocked row — the row stays
  // open-able (the "Open Evil Plugin" row-click affordance is intentionally
  // unrelated to this), so check the specific action labels are absent.
  expect(screen.queryByRole("button", { name: "Install Evil Plugin" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Fix Evil Plugin" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Manage Evil Plugin" })).toBeNull();
});

test("a pinned installed plugin shows the Pinned pill", async () => {
  pluginsFixture = [{ ...notionPlugin, pinned: true }];
  await renderView();

  expect(await screen.findByText("Pinned")).toBeTruthy();
});

// ---------- Install action ----------

test("installing a not-installed integration opens the install wizard", async () => {
  pluginsFixture = [githubPlugin];
  await renderView();
  await screen.findByText("GitHub");

  fireEvent.click(screen.getByRole("button", { name: "Install GitHub" }));

  expect(await screen.findByText("Install GitHub", { selector: "h2" })).toBeTruthy();
  await waitFor(() => expect(beginPluginInstall).toHaveBeenCalledWith(LOCAL_RUNNER, "github"));
});

// Task 14: a component-backed row's Install opens the universal wizard
// instead — the mocked `pluginDetail` otherwise always resolves the fixed
// `wizardDetail` (github-shaped, componentBacked false), so this override
// gives the wizard a componentBacked detail for its own plan/title.
const atlassianComponentDetail: PluginDetail = {
  ...wizardDetail,
  info: { ...wizardDetail.info, id: "atlassian", name: "Atlassian", componentBacked: true },
};

test("installing a not-installed component-backed row opens the universal wizard instead of the classic modal", async () => {
  const componentPlugin = plugin("atlassian", ["issues"], { name: "Atlassian", status: "not-installed", componentBacked: true });
  pluginsFixture = [componentPlugin];
  pluginDetail.mockImplementationOnce(async () => ({ status: "ok" as const, data: atlassianComponentDetail }));
  await renderView();
  await screen.findByText("Atlassian");

  fireEvent.click(screen.getByRole("button", { name: "Install Atlassian" }));

  expect(await screen.findByRole("dialog", { name: "Install Atlassian" })).toBeTruthy();
  expect(beginPluginInstall).not.toHaveBeenCalled();
});

test("installing a not-installed provider adds it to the installed set instead of opening a modal", async () => {
  pluginsFixture = [anthropicPlugin];
  await renderView();
  await screen.findByText("Anthropic");

  fireEvent.click(screen.getByRole("button", { name: "Install Anthropic" }));

  await waitFor(() => expect(installProviderMock).toHaveBeenCalledWith("anthropic"));
  expect(screen.queryByText("Install Anthropic", { selector: "h2" })).toBeNull();
});

test("installing a not-installed skill pack routes through the two-phase trust flow (beginSkillInstall)", async () => {
  pluginsFixture = [superpowersPlugin];
  await renderView();
  await screen.findByText("Superpowers");

  fireEvent.click(screen.getByRole("button", { name: "Install Superpowers" }));

  await waitFor(() => expect(beginSkillInstall).toHaveBeenCalledWith(LOCAL_RUNNER, "superpowers"));
});

// ---------- + Add menu ----------

test("the + Add menu opens and shows Add MCP server and Add skill source", async () => {
  await renderView();

  fireEvent.click(screen.getByRole("button", { name: "Add" }));

  expect(await screen.findByRole("menuitem", { name: "Add MCP server" })).toBeTruthy();
  expect(screen.getByRole("menuitem", { name: "Add skill source" })).toBeTruthy();
});

test("Add MCP server opens AddAppModal", async () => {
  await renderView();

  fireEvent.click(screen.getByRole("button", { name: "Add" }));
  fireEvent.click(await screen.findByRole("menuitem", { name: "Add MCP server" }));

  expect(await screen.findByText(/Point Cockpit at an MCP server/)).toBeTruthy();
});

test("Add skill source opens the manual source-entry step of the trust-gated install flow", async () => {
  await renderView();

  fireEvent.click(screen.getByRole("button", { name: "Add" }));
  fireEvent.click(await screen.findByRole("menuitem", { name: "Add skill source" }));

  expect(await screen.findByLabelText("Skill source")).toBeTruthy();
  expect(beginSkillInstall).not.toHaveBeenCalled();
});

// ---------- Rail footer: catalog status, refresh, doctor, update all ----------

test("the rail footer shows the catalog status line once catalogStatus has loaded", async () => {
  catalogStatusFixture = { sequence: 4, lastFetchAt: 1_700_000_000_000, outcome: "ok", entries: 12, blocked: 1 };
  await renderView();

  expect(await screen.findByText(/Catalog seq 4/)).toBeTruthy();
});

test("the rail's Refresh catalog button calls refreshCatalog and toasts the outcome", async () => {
  catalogStatusFixture = { sequence: 4, lastFetchAt: 1_700_000_000_000, outcome: "ok", entries: 12, blocked: 1 };
  await renderView();
  await screen.findByText(/Catalog seq 4/);

  fireEvent.click(screen.getByRole("button", { name: "Refresh catalog" }));

  await waitFor(() => expect(refreshCatalog).toHaveBeenCalled());
  await waitFor(() => expect(toastSuccess).toHaveBeenCalled());
});

test("the rail's Doctor link opens the doctor panel", async () => {
  await renderView();

  fireEvent.click(screen.getByRole("button", { name: "Doctor" }));

  expect(await screen.findByText("Plugin doctor")).toBeTruthy();
});

test("Update all is disabled with nothing installed, and enabled once an installed skill pack exists", async () => {
  pluginsFixture = [notionPlugin];
  await renderView();

  const disabled = (await screen.findByRole("button", { name: "Update all" })) as HTMLButtonElement;
  expect(disabled.disabled).toBe(true);

  pluginsFixture = [notionPlugin, { ...superpowersPlugin, installed: true, status: "ok" }];
  await act(async () => {
    await usePlugins.getState().load();
  });

  const enabled = screen.getByRole("button", { name: "Update all" }) as HTMLButtonElement;
  expect(enabled.disabled).toBe(false);

  fireEvent.click(enabled);
  await waitFor(() => expect(updateAllPlugins).toHaveBeenCalled());
});

test("Update all is enabled when an update-available row exists, even with no installed skill packs", async () => {
  pluginsFixture = [{ ...notionPlugin, status: "update-available" }];
  await renderView();

  const enabled = (await screen.findByRole("button", { name: "Update all" })) as HTMLButtonElement;
  expect(enabled.disabled).toBe(false);
});

// ---------- Bootstrap banner (Task 12, unchanged behavior) ----------

test("shows the retryable bootstrap banner when component_bootstrap_status reports pending, and hides it otherwise", async () => {
  componentBootstrapStatusFixture = { pending: true, message: "mimo: signature verification failed" };
  await renderView();

  expect(await screen.findByText("Component plugins need attention")).toBeTruthy();
  expect(screen.getByText("mimo: signature verification failed")).toBeTruthy();
});

test("omits the bootstrap banner when nothing is pending", async () => {
  await renderView();

  expect(screen.queryByText("Component plugins need attention")).toBeNull();
});

test("the bootstrap banner's Retry button reinstalls every known first-party id and refreshes the status", async () => {
  pluginsFixture = [
    plugin("mimo", ["component"], { name: "Mimo", source: "component", componentBacked: true, installed: true }),
    plugin("opencode", ["component"], { name: "Opencode", source: "component", componentBacked: true, installed: true }),
  ];
  componentBootstrapStatusFixture = { pending: true, message: "opencode: network unreachable" };
  await renderView();
  await screen.findByText("Component plugins need attention");

  componentBootstrapStatusFixture = { pending: false, message: null };
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));

  await waitFor(() => expect(installComponentPlugin).toHaveBeenCalledWith(LOCAL_RUNNER, "mimo", null));
  await waitFor(() => expect(installComponentPlugin).toHaveBeenCalledWith(LOCAL_RUNNER, "opencode", null));
  await waitFor(() => expect(screen.queryByText("Component plugins need attention")).toBeNull());
  expect(toastSuccess).toHaveBeenCalled();
});

// ---------- Empty state ----------

test("empty state shows when nothing is installed and the catalog is empty", async () => {
  await renderView();

  expect(await screen.findByText(/Nothing here yet/)).toBeTruthy();
});
