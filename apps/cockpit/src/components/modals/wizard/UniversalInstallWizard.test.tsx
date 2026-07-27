import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type {
  CatalogEntry,
  CmdError,
  ComponentReleaseDetail,
  PluginDetail,
  PluginFieldInfo,
  PluginInstallBeginResult,
  Result,
  TrustPromptDto,
} from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";

// The shell talks only to the Tauri IPC boundary (`@/bindings`) plus the
// `usePlugins`/`useNav`/`useConnections` stores — mock bindings, and reset
// the REAL stores around each test (same pattern `PluginDetailView.test.tsx`
// uses) since the step components (Task 14, extended to every kind by Task
// 15) drive real store actions instead of a scaffold placeholder.

function field(key: string, label: string): PluginFieldInfo {
  return { key, label, help: "", secret: false, required: false, valueSet: false, kind: "string", options: [], default: null };
}

// Componented-backed connector with oauth auth + a declared setting, so the
// planned sequence is the full six steps: overview, permissions, install,
// connect, settings, done — enough surface to exercise Back/Skip/Continue
// and the progress segment count together.
function detailFixture(): PluginDetail {
  return {
    info: {
      id: "notion",
      name: "Notion",
      description: "Notion MCP",
      icon: null,
      categories: ["docs"],
      slot: null,
      ownsSlot: false,
      verified: true,
      experimental: false,
      enabled: false,
      configured: false,
      source: "catalog",
      capabilities: ["connector"],
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
      componentBacked: true,
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
      helpUrl: null,
      configured: false,
      oauthConnectAvailable: true,
      oauthConnectError: null,
      oauthTokenStored: false,
      oauthReconnectRequired: false,
    },
    settings: [field("plugin.notion.workspace", "Workspace")],
    mcp: [],
    models: [],
    homepage: null,
    publisher: "Notion",
  };
}

function releaseFixture(): ComponentReleaseDetail {
  return { pluginId: "notion", releases: [], activeVersion: null, activeManifest: null, declaredManifest: null };
}

// Provider-shaped fixture with no permissions gate, no settings, and no
// oauth requirement other than the provider-kind rule itself —
// planWizardSteps collapses this down to overview/install/connect/done (4
// steps), exercising the shell against a plan shorter than the full
// six-step fixture every other test in this file uses.
//
// Finding 1 (final-review fix): `componentBacked: true` and a real
// `COMPONENT_BACKED_PROVIDER_IDS` id ("groq" — see
// crates/core/src/plugins/component_catalog.rs) — faithful to what the
// daemon actually sends for every one of the twelve component-backed
// provider bundles. Before the fix, `wizardKind` checked `componentBacked`
// before `kind`, so this exact shape routed through `InstallComponentStep`/
// the component `ConnectStep` instead of the provider adapter; this fixture
// is what proves the fix (`wizardKind`/`planWizardSteps` now check
// `kind === "provider"` first).
function providerDetailFixture(): PluginDetail {
  return {
    info: {
      id: "groq",
      name: "Groq",
      description: "Groq provider",
      icon: null,
      categories: ["llm"],
      slot: null,
      ownsSlot: false,
      verified: true,
      experimental: false,
      enabled: false,
      configured: false,
      source: "catalog",
      capabilities: ["provider"],
      kind: "provider",
      installed: false,
      family: "groq",
      pinned: false,
      sourceSpec: null,
      resolvedCommit: null,
      installedAt: null,
      updatedAt: null,
      trustTier: null,
      catalogVersion: null,
      componentBacked: true,
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
    publisher: "Groq",
  };
}

// Finding 2 (final-review fix): a component-backed provider whose id is NOT
// its own family head — mirrors the real `anthropic-oauth` entry
// (`llm_router::registry::CATALOG`, family "anthropic"). Proves
// `steps-provider.tsx` reads `ctx.detail?.info.family`, not `ctx.pluginId`,
// when calling `installProvider`/rendering `ConnectionMethodForm` — the
// daemon's `install_provider` RPC bails on any id that isn't a family head.
function familyMemberProviderDetailFixture(): PluginDetail {
  return {
    info: {
      id: "anthropic-oauth",
      name: "Claude (subscription)",
      description: "Anthropic subscription sign-in",
      icon: null,
      categories: ["llm"],
      slot: null,
      ownsSlot: false,
      verified: true,
      experimental: false,
      enabled: false,
      configured: false,
      source: "catalog",
      capabilities: ["provider"],
      kind: "provider",
      installed: false,
      family: "anthropic",
      pinned: false,
      sourceSpec: null,
      resolvedCommit: null,
      installedAt: null,
      updatedAt: null,
      trustTier: null,
      catalogVersion: null,
      componentBacked: true,
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
    publisher: "Anthropic",
  };
}

// Finding 1 (Task 14 review): a non-component, token-auth fixture — mirrors
// `PluginDetailView.test.tsx`'s `githubDetail` (componentBacked false,
// auth.kind "token", auth.setting set). PluginDetailView's checklist connect
// routes EVERY plugin through this wizard, component-backed or not, so this
// shape (previously untested here) is production-reachable. No permissions
// gate, no settings, no oauth profiles -> plan collapses to overview/install/
// connect/done (4 steps), landing ConnectStep on `TokenConnect`.
function tokenAuthDetailFixture(authOverrides: Partial<NonNullable<PluginDetail["auth"]>> = {}): PluginDetail {
  return {
    info: {
      id: "github",
      name: "GitHub",
      description: "Repos, issues, and pull requests.",
      icon: null,
      categories: ["vcs"],
      slot: null,
      ownsSlot: false,
      verified: true,
      experimental: false,
      enabled: true,
      configured: false,
      source: "catalog",
      capabilities: ["connector"],
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
      ...authOverrides,
    },
    settings: [],
    mcp: [],
    models: [],
    homepage: null,
    publisher: "GitHub (official)",
  };
}

// Task 15: a skill-pack fixture (kind "skill-pack", componentBacked false,
// no top-level auth/settings — mirrors the curated `superpowers` pack's
// synthesized `PluginDetail`, see `curated_pack_row`/`curated_pack_detail`).
function skillPackDetailFixture(): PluginDetail {
  return {
    info: {
      id: "superpowers",
      name: "Superpowers",
      description: "Curated workflow and development skills",
      icon: "sparkles",
      categories: ["skills"],
      slot: null,
      ownsSlot: false,
      verified: true,
      experimental: false,
      enabled: false,
      configured: false,
      source: "skill-pack",
      capabilities: [],
      kind: "skill-pack",
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
    publisher: "Superpowers",
  };
}

const ok = <T,>(data: T) => Promise.resolve({ status: "ok" as const, data });
const err = (message: string) => Promise.resolve({ status: "error" as const, error: { message } });

let detailData: PluginDetail = detailFixture();
let releaseData: ComponentReleaseDetail = releaseFixture();
// Task 15: `steps-connector.tsx`'s `InstallConnectorStep`/`ConnectorConnectStep`
// mutable begin-result fixture — mirrors the retired
// `InstallWizardModal.test.tsx`'s own `beginData` pattern (tests swap it
// between the mount call and a later re-begin to simulate DCR/oauth
// progressing).
let beginData: PluginInstallBeginResult = {
  authKind: "token",
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

// Fix (dead-end retry + connector coverage): a `beginPluginInstall` result
// builder — mirrors the retired `InstallWizardModal.test.tsx`'s own
// `beginResult` fixture — for the connector oauth sub-state tests below
// (deadEnd/manualClientId), which each only need to override a couple of
// fields off an oauth-shaped baseline.
function beginResult(overrides: Partial<PluginInstallBeginResult> = {}): PluginInstallBeginResult {
  return {
    authKind: "oauth",
    envVarPresent: false,
    envVarName: null,
    oauthAvailable: false,
    oauthExternal: false,
    needsClientId: false,
    dcrSucceeded: false,
    callbackMode: "auto",
    oauthBegin: null,
    dcrError: null,
    ...overrides,
  };
}
// Task 15: `steps-skillpack.tsx`'s `InstallSkillPackStep` mutable
// begin-result fixture.
let skillBeginData: { completed: boolean; trust: TrustPromptDto | null; plugin: { id: string; name: string } | null } = {
  completed: true,
  trust: null,
  plugin: { id: "superpowers", name: "Superpowers" },
};

const pluginDetail = mock((_runnerId: string | null, _id: string): Promise<Result<PluginDetail, CmdError>> => ok(detailData));
const pluginReleaseDetail = mock(
  (_runnerId: string | null, _id: string): Promise<Result<ComponentReleaseDetail, CmdError>> => ok(releaseData),
);
const toastError = mock((_message: string) => {});
const toastSuccess = mock((_message: string) => {});

// Task 14: the real per-kind step components drive these — `installComponentPlugin`
// resolves to a benign non-null release so the install step's auto-advance
// fires; `pluginTools` backs `DoneStep`'s tools list; `beginPluginOauth`/
// `completePluginOauth` back `ConnectStep`'s plugin-level oauth branch;
// `setPluginSetting` backs `SettingsStep` (and the connect step's token
// branch, unused by any fixture here). Device-flow commands are stubbed for
// completeness even though only the dedicated device-flow test below
// exercises that branch (`OauthProfileConnections` calls them on its own
// Connect click, which none of these tests trigger).
// Pre-existing gap (unrelated to Task 15): explicit return type widens this
// to the same `Result<T, CmdError>` union `pluginDetail`/`pluginReleaseDetail`
// already declare, so a test's `mockImplementationOnce(() => err(...))` (the
// failed-install Retry test below) typechecks against an error variant too.
const installComponentPlugin = mock(
  (_runnerId: string, id: string, _version: string | null): Promise<Result<ComponentReleaseDetail, CmdError>> =>
    ok({ pluginId: id, releases: [], activeVersion: "1.0.0", activeManifest: null, declaredManifest: null }),
);
const beginPluginOauth = mock((_runnerId: string, _pluginId: string) =>
  ok({ stateToken: "state-1", authorizeUrl: "https://notion.example/authorize", redirectUri: "http://127.0.0.1:8976/callback" }),
);
const completePluginOauth = mock((_runnerId: string, _pluginId: string, _code: string, _stateToken: string) => ok(null));
const setPluginSetting = mock((_runnerId: string, _key: string, _value: string) => ok(null));
const pluginTools = mock((_runnerId: string, id: string) => ok({ pluginId: id, live: true, entries: [] as unknown[] }));
const pluginProfileBeginDeviceFlow = mock((_runnerId: string, _pluginId: string, _profileId: string, _url: string) =>
  ok({
    deviceCode: "device-1",
    userCode: "ABCD-1234",
    verificationUri: "https://notion.example/device",
    verificationUriComplete: null,
    intervalSecs: 5,
    expiresAt: Date.now() + 60_000,
  }),
);
const pluginProfilePollDeviceFlow = mock(
  (_runnerId: string, _pluginId: string, _profileId: string, _tokenUrl: string, _deviceCode: string, _expiresAt: number) => ok("pending"),
);
const pluginProfileDisconnect = mock((_runnerId: string, _pluginId: string, _profileId: string) => ok(null));

// Task 15: the connector adapter's `beginPluginInstall`/`cancelPluginInstall`/
// `setPluginOauthClientId` (its own `runBegin`/manual-client-id/oauth-wait
// machinery) and the shared `DoneStep`'s connector enable-commit
// (`setPluginEnabled`) — same shape the retired `InstallWizardModal.test.tsx`
// mocked.
const beginPluginInstall = mock(
  (_runnerId: string, _pluginId: string): Promise<Result<PluginInstallBeginResult, CmdError>> => ok(beginData),
);
const cancelPluginInstall = mock((_runnerId: string, _pluginId: string, _stateToken: string | null) => ok(null));
const setPluginOauthClientId = mock((_runnerId: string, _pluginId: string, _clientId: string) => ok(null));
const setPluginEnabled = mock((_runnerId: string, _id: string, _enabled: boolean) => ok(null));
// Task 15: the skill-pack adapter's two-phase `beginSkillInstall`/`confirmSkillInstall`,
// plus the trio `usePlugins().load` (its own post-install `loadPlugins()`
// call) fetches — `listPlugins`/`pluginsRestartRequired`/`catalogStatus`.
const beginSkillInstall = mock((_runnerId: string, _source: string) => ok(skillBeginData));
const listPlugins = mock(() => ok([] as unknown[]));
const pluginsRestartRequired = mock(() => ok(false));
const catalogStatus = mock(() => ok({ sequence: 0, lastFetchAt: null, outcome: null, entries: 0, blocked: 0 }));
const confirmSkillInstall = mock((_runnerId: string, _token: string) => ok({ id: "superpowers", name: "Superpowers" }));
// Task 15: the provider adapter's `installProvider` (via the REAL
// `useConnections` store) and `ConnectionMethodForm`'s own oauth-authorize-url
// listener (mounted unconditionally by the provider connect step).
const installProvider = mock((_runnerId: string, _family: string) => ok(["groq"]));
const oauthAuthorizeUrlMsgListen = mock(async (_cb: (event: { payload: { provider: string; authorizeUrl: string } }) => void) => () => {});

const pluginOauthCompletedMsgListen = mock(
  async (_cb: (event: { payload: { pluginId: string; ok: boolean; error: string | null } }) => void) => () => {},
);
const openUrl = mock(async (_url: string) => {});

mock.module("@/bindings", () => ({
  commands: {
    pluginDetail,
    pluginReleaseDetail,
    installComponentPlugin,
    beginPluginOauth,
    completePluginOauth,
    setPluginSetting,
    pluginTools,
    pluginProfileBeginDeviceFlow,
    pluginProfilePollDeviceFlow,
    pluginProfileDisconnect,
    beginPluginInstall,
    cancelPluginInstall,
    setPluginOauthClientId,
    setPluginEnabled,
    beginSkillInstall,
    confirmSkillInstall,
    installProvider,
    listPlugins,
    pluginsRestartRequired,
    catalogStatus,
  },
  events: {
    pluginOauthCompletedMsg: { listen: pluginOauthCompletedMsgListen },
    oauthAuthorizeUrlMsg: { listen: oauthAuthorizeUrlMsgListen },
  },
}));
mock.module("sonner", () => ({
  toast: { error: toastError, success: toastSuccess, info: mock(() => {}), warning: mock(() => {}) },
  Toaster: () => null,
}));
mock.module("@tauri-apps/plugin-opener", () => ({ openUrl }));

const { UniversalInstallWizard } = await import("./UniversalInstallWizard");
const { usePlugins } = await import("@/store-plugins");
const { useNav } = await import("@/store-nav");
const { useConnections } = await import("@/store-connections");

const onClose = mock(() => {});

// `pluginId` is the prop `ctx.pluginId` resolves to everywhere (the id every
// install/connect RPC in this file is keyed on) — independent of whatever id
// `detailData`'s own fixture happens to carry (`pluginDetail`'s mock ignores
// the requested id and just returns `detailData` regardless, so the two can
// drift harmlessly UNLESS a test asserts an RPC call by id, in which case it
// must pass the matching `pluginId` here).
async function renderWizard(initialStep?: Parameters<typeof UniversalInstallWizard>[0]["initialStep"], pluginId = "notion") {
  const result = render(<UniversalInstallWizard pluginId={pluginId} onClose={onClose} initialStep={initialStep} />);
  await act(async () => {});
  return result;
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
    toolsById: {},
    toolsLiveById: {},
  });
}

// api-key members so `ConnectionMethodForm` (the provider adapter's
// "connect" step) has something concrete to render — mirrors
// `AddConnectionModal.test.tsx`'s own single-member fixtures. Two families:
// `groqCatalogEntry` backs `providerDetailFixture()` (id == family); the
// `anthropic` entry backs `familyMemberProviderDetailFixture()` (Finding 2 —
// its own id "anthropic-oauth" differs from this entry's family
// "anthropic", which is what `ConnectionMethodForm`'s `family` prop must
// resolve to for its member filter to find it).
const groqCatalogEntry: CatalogEntry = {
  id: "groq",
  name: "Groq",
  family: "groq",
  color: "#F97316",
  initial: "G",
  category: "api_key",
  format: "openai",
  requiresBaseUrl: false,
  models: [],
  freeTier: false,
  riskNotice: false,
  usesDeviceGrant: false,
};

const anthropicCatalogEntry: CatalogEntry = {
  id: "anthropic",
  name: "Anthropic",
  family: "anthropic",
  color: "#D97757",
  initial: "A",
  category: "api_key",
  format: "anthropic",
  requiresBaseUrl: false,
  models: [],
  freeTier: false,
  riskNotice: false,
  usesDeviceGrant: false,
};

function resetConnectionsStore() {
  useConnections.setState({
    catalog: [groqCatalogEntry, anthropicCatalogEntry],
    customProviders: [],
    connections: [],
    installedProviders: [],
    loaded: true,
  });
}

beforeEach(() => {
  detailData = detailFixture();
  releaseData = releaseFixture();
  beginData = {
    authKind: "token",
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
  skillBeginData = { completed: true, trust: null, plugin: { id: "superpowers", name: "Superpowers" } };
  pluginDetail.mockClear();
  pluginReleaseDetail.mockClear();
  installComponentPlugin.mockClear();
  beginPluginOauth.mockClear();
  completePluginOauth.mockClear();
  setPluginSetting.mockClear();
  pluginTools.mockClear();
  pluginProfileBeginDeviceFlow.mockClear();
  pluginProfilePollDeviceFlow.mockClear();
  pluginProfileDisconnect.mockClear();
  beginPluginInstall.mockClear();
  cancelPluginInstall.mockClear();
  setPluginOauthClientId.mockClear();
  setPluginEnabled.mockClear();
  beginSkillInstall.mockClear();
  confirmSkillInstall.mockClear();
  installProvider.mockClear();
  listPlugins.mockClear();
  pluginsRestartRequired.mockClear();
  catalogStatus.mockClear();
  oauthAuthorizeUrlMsgListen.mockClear();
  pluginOauthCompletedMsgListen.mockClear();
  openUrl.mockClear();
  toastError.mockClear();
  toastSuccess.mockClear();
  onClose.mockClear();
  resetPluginsStore();
  resetConnectionsStore();
  useNav.setState({ history: { back: [], current: { kind: "plugins" }, forward: [] } });
});

afterEach(() => {
  cleanup();
  resetPluginsStore();
  resetConnectionsStore();
  useNav.setState({ history: { back: [], current: { kind: "home" }, forward: [] } });
});

// ---------- Shell: fetch, progress, navigation (Task 13, unaffected by the real step content) ----------

test("fetches detail and release on mount and renders the title and step 1 of M", async () => {
  await renderWizard();

  expect(pluginDetail).toHaveBeenCalledWith(LOCAL_RUNNER, "notion");
  expect(pluginReleaseDetail).toHaveBeenCalledWith(LOCAL_RUNNER, "notion");
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  expect(within(dialog).getByText("Step 1 of 6 — Overview")).toBeTruthy();
  // OverviewStep's own content (Task 14) — name/description, not the old
  // scaffold's bare step label.
  expect(within(dialog).getByText("Notion MCP")).toBeTruthy();
});

test("the progress bar has one segment per planned step", async () => {
  await renderWizard();

  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  const segments = dialog.querySelectorAll(".rounded-full.h-1, .h-1.rounded-full");
  expect(segments.length).toBe(6);
});

test("Continue advances to the next step and updates the header", async () => {
  await renderWizard();

  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  expect(within(dialog).getByText("Step 1 of 6 — Overview")).toBeTruthy();

  act(() => {
    within(dialog).getByRole("button", { name: "Continue" }).click();
  });

  expect(within(dialog).getByText("Step 2 of 6 — Permissions")).toBeTruthy();
  // PermissionsStep's fallback row (no release installed yet) carries the
  // label "Permissions" as its own text — happens to double as evidence the
  // real step (not the old scaffold) rendered.
  expect(within(dialog).getByText("Permissions")).toBeTruthy();
});

test("Back returns to the previous step and is disabled on the first step", async () => {
  await renderWizard();

  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  const backButton = within(dialog).getByRole("button", { name: "Back" }) as HTMLButtonElement;
  expect(backButton.disabled).toBe(true);

  act(() => {
    within(dialog).getByRole("button", { name: "Continue" }).click();
  });
  expect(within(dialog).getByText("Step 2 of 6 — Permissions")).toBeTruthy();
  expect((within(dialog).getByRole("button", { name: "Back" }) as HTMLButtonElement).disabled).toBe(false);

  act(() => {
    within(dialog).getByRole("button", { name: "Back" }).click();
  });
  expect(within(dialog).getByText("Step 1 of 6 — Overview")).toBeTruthy();
  // PermissionsStep's cleanup effect must clear the Continue-disabled gate it
  // set on mount (accepted starts false) — otherwise stepping back to
  // Overview would wrongly inherit Permissions' disabled Continue button.
  expect((within(dialog).getByRole("button", { name: "Continue" }) as HTMLButtonElement).disabled).toBe(false);
});

test("Skip only shows up on the connect and settings steps", async () => {
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });

  // overview (1 of 6) — no Skip.
  expect(within(dialog).queryByRole("button", { name: "Skip" })).toBeNull();

  // permissions (2 of 6) — no Skip. Continue starts disabled here (gated on
  // the accept switch) — toggle it before moving on.
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).queryByRole("button", { name: "Skip" })).toBeNull();
  fireEvent.click(within(dialog).getByRole("switch", { name: "Accept permissions" }));

  // install (3 of 6) — no Skip.
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).queryByRole("button", { name: "Skip" })).toBeNull();

  // connect (4 of 6) — Skip appears.
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).getByText("Step 4 of 6 — Connect")).toBeTruthy();
  expect(within(dialog).queryByRole("button", { name: "Skip" })).not.toBeNull();

  // settings (5 of 6) — Skip appears.
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).getByText("Step 5 of 6 — Settings")).toBeTruthy();
  expect(within(dialog).queryByRole("button", { name: "Skip" })).not.toBeNull();

  // done (6 of 6) — no Skip.
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  // DoneStep's own `loadTools` mount fetch — flush it fully so it doesn't
  // land (and set state) after this test has already finished.
  await act(async () => {});
  expect(within(dialog).getByText("Step 6 of 6 — Done")).toBeTruthy();
  expect(within(dialog).queryByRole("button", { name: "Skip" })).toBeNull();
});

test("Continue on the last step closes the wizard", async () => {
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });

  // overview -> permissions
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  // permissions (accept, else Continue stays disabled) -> install
  fireEvent.click(within(dialog).getByRole("switch", { name: "Accept permissions" }));
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  // install -> connect -> settings -> done
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Step 6 of 6 — Done")).toBeTruthy();

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(onClose).toHaveBeenCalled();
});

test("initialStep resumes at that step's position in the plan", async () => {
  await renderWizard("settings");

  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  expect(within(dialog).getByText("Step 5 of 6 — Settings")).toBeTruthy();
});

test("a pluginDetail error toasts and still renders the shell", async () => {
  pluginDetail.mockImplementationOnce(() => Promise.resolve({ status: "error" as const, error: { message: "manifest read failed" } }));
  await renderWizard();

  expect(toastError).toHaveBeenCalledWith("manifest read failed");
  expect(screen.getByRole("dialog")).toBeTruthy();
});

test("a pluginReleaseDetail error toasts and still renders the shell", async () => {
  pluginReleaseDetail.mockImplementationOnce(() =>
    Promise.resolve({ status: "error" as const, error: { message: "release lookup failed" } }),
  );
  await renderWizard();

  expect(toastError).toHaveBeenCalledWith("release lookup failed");
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  expect(within(dialog).getByText("Step 1 of 6 — Overview")).toBeTruthy();
});

test("initialStep falls back to step 1 when the plan doesn't include that step", async () => {
  pluginDetail.mockImplementationOnce(() => ok(providerDetailFixture()));
  await renderWizard("settings");

  const dialog = screen.getByRole("dialog", { name: "Install Groq" });
  expect(within(dialog).getByText("Step 1 of 4 — Overview")).toBeTruthy();
});

// Finding 1 — before both fetches settle, `plan` used to default to a single
// "overview" step, making isLast true on first paint; a Continue click
// during the round trip closed the wizard outright. A permanently-pending
// pluginDetail mock (same deterministic technique as
// PluginDetailView.test.tsx's `pluginToolsPendingIds`) freezes the shell
// mid-fetch so this is reproducible without racing a real promise.
test("Continue is disabled while the initial fetch is pending and does not close the wizard", async () => {
  pluginDetail.mockImplementationOnce(() => new Promise<never>(() => {}));
  render(<UniversalInstallWizard pluginId="notion" onClose={onClose} />);
  await act(async () => {});

  const dialog = screen.getByRole("dialog");
  expect(within(dialog).getByText("Loading…")).toBeTruthy();
  expect(within(dialog).queryByRole("button", { name: "Back" })).toBeNull();
  expect(within(dialog).queryByRole("button", { name: "Skip" })).toBeNull();
  const continueButton = within(dialog).getByRole("button", { name: "Continue" }) as HTMLButtonElement;
  expect(continueButton.disabled).toBe(true);

  act(() => {
    continueButton.click();
  });
  expect(onClose).not.toHaveBeenCalled();
});

// Finding 2 — every other shell test in this file plans the full six steps;
// this fixture (provider kind, no auth, no settings, component-backed per
// Finding 1's fix, no oauth profiles) collapses the plan to
// overview/install/connect/done so the progress math and step sequencing
// are exercised against a shorter plan.
test("renders a shortened 4-step plan for a provider with no settings or permissions gate", async () => {
  // This test navigates past the install step, whose own success path calls
  // `ctx.refresh()` — a SECOND `pluginDetail` fetch, not just the mount's
  // first one — so the fixture has to sit in `detailData` (read on every
  // call) rather than a one-shot `mockImplementationOnce` (which the mount
  // call alone would consume, leaving the refresh to fall through to
  // whatever the OTHER tests' `detailData` last left behind).
  detailData = providerDetailFixture();
  await renderWizard(undefined, "groq");

  const dialog = screen.getByRole("dialog", { name: "Install Groq" });
  expect(within(dialog).getByText("Step 1 of 4 — Overview")).toBeTruthy();
  const segments = dialog.querySelectorAll(".rounded-full.h-1, .h-1.rounded-full");
  expect(segments.length).toBe(4);
  expect(within(dialog).queryByText("Permissions")).toBeNull();
  expect(within(dialog).queryByText("Settings")).toBeNull();

  // install (position 2) auto-advances to connect once the (default,
  // successful) installProvider mock resolves — nothing in this test needs
  // to observe the transient "Installing…" body, only that the plan still
  // lands correctly on the other side of it (Task 15: the provider adapter's
  // own install step, `steps-provider.tsx`, not `InstallComponentStep`).
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();
  expect(installProvider).toHaveBeenCalledWith(LOCAL_RUNNER, "groq");
  expect(within(dialog).queryByText("Permissions")).toBeNull();
  expect(within(dialog).queryByText("Settings")).toBeNull();

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  expect(within(dialog).getByText("Step 4 of 4 — Done")).toBeTruthy();
  expect(within(dialog).queryByText("Permissions")).toBeNull();
  expect(within(dialog).queryByText("Settings")).toBeNull();
});

// ---------- Task 14: real per-step behavior ----------

// The brief's TDD scenario: a component fixture with no settings field (so
// the plan is 5 steps, ending Connect → Skip → Done directly) walking
// Overview → Continue → Permissions (gated) → accept → Continue → install
// (auto-advances) → Connect → Skip → Done.
test("component install flow: permissions gates Continue, install fires once, Skip reaches Done", async () => {
  detailData = { ...detailFixture(), settings: [] };
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });

  expect(within(dialog).getByText("Step 1 of 5 — Overview")).toBeTruthy();
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());

  expect(within(dialog).getByText("Step 2 of 5 — Permissions")).toBeTruthy();
  let continueButton = within(dialog).getByRole("button", { name: "Continue" }) as HTMLButtonElement;
  expect(continueButton.disabled).toBe(true);

  fireEvent.click(within(dialog).getByRole("switch", { name: "Accept permissions" }));
  continueButton = within(dialog).getByRole("button", { name: "Continue" }) as HTMLButtonElement;
  expect(continueButton.disabled).toBe(false);

  act(() => continueButton.click());
  // install — auto-advances to connect once installComponentPlugin resolves.
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Step 4 of 5 — Connect")).toBeTruthy();
  expect(installComponentPlugin).toHaveBeenCalledTimes(1);

  act(() => within(dialog).getByRole("button", { name: "Skip" }).click());
  await act(async () => {});
  expect(within(dialog).getByText("Step 5 of 5 — Done")).toBeTruthy();
  expect(within(dialog).getByRole("button", { name: "Open plugin page" })).toBeTruthy();

  // Re-render/effect churn along the way must not have re-triggered install.
  expect(installComponentPlugin).toHaveBeenCalledTimes(1);
});

test("a failed component install shows Retry, and Retry tries again", async () => {
  detailData = { ...detailFixture(), settings: [] };
  installComponentPlugin.mockImplementationOnce(() => err("network unreachable"));
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  fireEvent.click(within(dialog).getByRole("switch", { name: "Accept permissions" }));
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});

  expect(within(dialog).getByText("Step 3 of 5 — Install")).toBeTruthy();
  expect(within(dialog).getByRole("button", { name: "Retry" })).toBeTruthy();

  fireEvent.click(within(dialog).getByRole("button", { name: "Retry" }));
  await act(async () => {});
  await act(async () => {});

  expect(within(dialog).getByText("Step 4 of 5 — Connect")).toBeTruthy();
  expect(installComponentPlugin).toHaveBeenCalledTimes(2);
});

test("Connect step's plugin OAuth begins on mount, and pasting a code completes and auto-advances", async () => {
  detailData = { ...detailFixture(), settings: [] };
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  fireEvent.click(within(dialog).getByRole("switch", { name: "Accept permissions" }));
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});

  expect(within(dialog).getByText("Step 4 of 5 — Connect")).toBeTruthy();
  expect(beginPluginOauth).toHaveBeenCalledWith(LOCAL_RUNNER, "notion");

  fireEvent.change(within(dialog).getByLabelText("Authorization code"), { target: { value: "code-123" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Finish connect" }));
  await act(async () => {});

  expect(completePluginOauth).toHaveBeenCalledWith(LOCAL_RUNNER, "notion", "code-123", "state-1");
  await act(async () => {});
  expect(within(dialog).getByText("Step 5 of 5 — Done")).toBeTruthy();
});

test("Connect step renders the device-flow OAuth profile connections when the manifest declares a connectable profile", async () => {
  releaseData = {
    pluginId: "notion",
    releases: [],
    activeVersion: "1.0.0",
    activeManifest: {
      publisher: "Notion",
      description: "",
      lifecycle: "singleton",
      domains: [],
      oauthProfiles: [
        {
          id: "workspace",
          scopes: ["read"],
          tokenUrl: "https://notion.example/token",
          deviceAuthorizationUrl: "https://notion.example/device",
          connected: false,
          clientIdConfigured: true,
        },
      ],
      tools: [],
    },
    declaredManifest: null,
  };
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  fireEvent.click(within(dialog).getByRole("switch", { name: "Accept permissions" }));
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});

  expect(within(dialog).getByText("Step 4 of 6 — Connect")).toBeTruthy();
  expect(within(dialog).getByText("Connections (OAuth)")).toBeTruthy();
  // The plugin-level oauth path must NOT have also fired for this profile-driven case.
  expect(beginPluginOauth).not.toHaveBeenCalled();
});

// Finding 1 — TokenConnect (non-oauth `detail.auth.setting`) was previously
// untested here even though it's production-reachable: the checklist connect
// action now routes every plugin, component-backed or not, through this
// wizard, and github's real fixture shape (token auth) hits this branch.
test("Connect step's TokenConnect renders the credential field, disabled Save while empty", async () => {
  detailData = tokenAuthDetailFixture();
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });
  expect(within(dialog).getByText("Step 1 of 4 — Overview")).toBeTruthy();

  // overview -> install (auto-advances) -> connect
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();

  const input = within(dialog).getByLabelText("Credential *") as HTMLInputElement;
  expect(input).toBeTruthy();
  const saveButton = within(dialog).getByRole("button", { name: "Save" }) as HTMLButtonElement;
  expect(saveButton.disabled).toBe(true);

  fireEvent.change(input, { target: { value: "ghp_abc123" } });
  expect((within(dialog).getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(false);
});

test("Connect step's TokenConnect Save calls setPluginSetting, disables while saving, clears the field, and does not auto-advance", async () => {
  detailData = tokenAuthDetailFixture();
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();

  fireEvent.change(within(dialog).getByLabelText("Credential *"), { target: { value: "ghp_abc123" } });

  // Freeze setPluginSetting mid-flight to observe the "while saving" disabled
  // state deterministically (same technique the shell tests use for a
  // permanently-pending fetch).
  let resolveSave: (v: { status: "ok"; data: null }) => void = () => {};
  setPluginSetting.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        resolveSave = resolve;
      }),
  );

  fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
  expect(setPluginSetting).toHaveBeenCalledWith(LOCAL_RUNNER, "plugin.github.token", "ghp_abc123");
  expect((within(dialog).getByRole("button", { name: "Saving…" }) as HTMLButtonElement).disabled).toBe(true);

  // The subsequent `ctx.refresh()` re-fetches pluginDetail — reflect the
  // now-configured credential so the field's `valueSet` (-> "connected") state
  // is observable once the round trip lands.
  detailData = tokenAuthDetailFixture({ configured: true });
  await act(async () => {
    resolveSave({ status: "ok", data: null });
  });
  await act(async () => {});
  await act(async () => {});

  expect(toastSuccess).toHaveBeenCalledWith("Saved");
  // TokenConnect's `save()` does not call `onNext()` — per the implementation
  // this step marks the credential connected (via the refreshed `valueSet`)
  // rather than auto-advancing.
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();
  expect((within(dialog).getByLabelText("Credential *") as HTMLInputElement).value).toBe("");
  expect(within(dialog).getByPlaceholderText("●●●● saved")).toBeTruthy();
});

// Finding 5a (final-review fix): TokenConnect lost the retired
// `InstallWizardModal`'s "Get a token" helpUrl affordance in the Task 14/15
// rewrite. `tokenAuthDetailFixture` already carries a `helpUrl` —
// https://github.com/settings/tokens — so this only needed a new assertion,
// not a new fixture.
test("Connect step's TokenConnect renders a Get a token button that opens auth.helpUrl", async () => {
  detailData = tokenAuthDetailFixture();
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();

  fireEvent.click(within(dialog).getByRole("button", { name: "Get a token" }));
  expect(openUrl).toHaveBeenCalledWith("https://github.com/settings/tokens");
});

// Finding 5b (final-review fix): an env-only credential (auth.env set, but
// NO auth.setting) used to still render TokenConnect's `FieldRow` — a dead
// field, since `save()` guards on `auth.setting` and silently no-ops without
// it. Reached via the classic connector adapter's "token" sub-state
// (`ConnectorConnectStep`), which — unlike `steps-component.tsx`'s own
// `ConnectStep` — dispatches to `TokenConnect` purely off `begin.authKind`,
// without checking `auth.setting` first.
test("Connect step's TokenConnect shows the env-var message instead of a dead field when auth.setting is unset", async () => {
  detailData = tokenAuthDetailFixture({ setting: null });
  await renderWizard(undefined, "github");
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();

  expect(within(dialog).queryByLabelText("Credential *")).toBeNull();
  expect(within(dialog).getByText("GITHUB_PERSONAL_ACCESS_TOKEN")).toBeTruthy();
  expect(dialog.textContent).toContain("reads its credential from the");
});

// ---------- Task 15: connector / skill-pack / provider adapters ----------

// Port of the retired `InstallWizardModal.test.tsx`'s token path: the
// classic connector adapter's "install" step resolves via
// `beginPluginInstall` (not `installComponentPlugin`), lands on "connect"'s
// `TokenConnect` sub-state, and the done-commit (`setPluginEnabled` unless
// experimental) fires once Continue reaches "done".
test("classic connector token path: beginPluginInstall resolves, TokenConnect saves, and done enables the plugin", async () => {
  detailData = tokenAuthDetailFixture();
  await renderWizard(undefined, "github");
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });
  expect(within(dialog).getByText("Step 1 of 4 — Overview")).toBeTruthy();

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(beginPluginInstall).toHaveBeenCalledWith(LOCAL_RUNNER, "github");
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();

  fireEvent.change(within(dialog).getByLabelText("Credential *"), { target: { value: "ghp_abc123" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(setPluginSetting).toHaveBeenCalledWith(LOCAL_RUNNER, "plugin.github.token", "ghp_abc123"));

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  expect(within(dialog).getByText("Step 4 of 4 — Done")).toBeTruthy();
  await waitFor(() => expect(setPluginEnabled).toHaveBeenCalledWith(LOCAL_RUNNER, "github", true));
});

// Finding 3 (final-review fix): resuming via `initialStep` must apply
// exactly once. Before the fix, the resume effect's dep array included
// `plan` itself — and `plan`'s identity changes on EVERY `ctx.refresh()`
// (here, a settings save), not just the mount-time load — so the effect
// re-fired and snapped the wizard back to `initialStep`'s position even
// after the user had since navigated forward by hand (the setup checklist's
// "Connect" resume is exactly this scenario: resume at connect, wander
// forward, save something, get yanked back).
test("initialStep applies once — a later refresh-causing save does not snap the wizard back to the resumed step", async () => {
  detailData = { ...tokenAuthDetailFixture(), settings: [field("plugin.github.workspace", "Workspace")] };
  await renderWizard("connect", "github");
  await act(async () => {});
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });
  // Resumed straight to connect — index 2 of overview/install/connect/settings/done (5 steps).
  expect(within(dialog).getByText("Step 3 of 5 — Connect")).toBeTruthy();

  // Advance past connect (skippable) to settings, by hand.
  fireEvent.click(within(dialog).getByRole("button", { name: "Skip" }));
  await act(async () => {});
  expect(within(dialog).getByText("Step 4 of 5 — Settings")).toBeTruthy();

  // A settings save calls `ctx.refresh()`, which re-fetches `pluginDetail` —
  // reassign `detailData` to a genuinely NEW object (not just mutate the
  // existing one) so `setDetail` sees a changed reference and `plan`'s
  // `useMemo` actually recomputes, the same way a real IPC round trip always
  // returns a fresh deserialization. Without that, this test can't
  // distinguish the fix from a no-op (React bails out of a `setState` given
  // the same reference back, so `plan` would never recompute either way).
  fireEvent.change(within(dialog).getByLabelText("Workspace"), { target: { value: "acme" } });
  detailData = { ...tokenAuthDetailFixture(), settings: [{ ...field("plugin.github.workspace", "Workspace"), valueSet: true }] };
  fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
  await act(async () => {});

  expect(setPluginSetting).toHaveBeenCalledWith(LOCAL_RUNNER, "plugin.github.workspace", "acme");
  expect(within(dialog).getByText("Step 4 of 5 — Settings")).toBeTruthy();
});

// Port of the retired `InstallWizardModal.test.tsx`'s oauth-event/cancel
// path: closing the wizard while a classic connector's browser sign-in is
// still pending must cancel it (the backend's loopback listener otherwise
// leaks until the flow's own timeout) — same behavior the old modal's
// `close()` had, now scoped to `wizardKind(detail) === "connector"`.
test("closing during a connector's pending oauth flow cancels the pending install", async () => {
  detailData = tokenAuthDetailFixture({ kind: "oauth" });
  beginData = {
    authKind: "oauth",
    envVarPresent: false,
    envVarName: null,
    oauthAvailable: true,
    oauthExternal: false,
    needsClientId: false,
    dcrSucceeded: true,
    callbackMode: "auto",
    oauthBegin: {
      stateToken: "state-999",
      authorizeUrl: "https://github.example.com/oauth/authorize",
      redirectUri: "http://127.0.0.1:8976/callback",
    },
    dcrError: null,
  };
  await renderWizard(undefined, "github");
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Browser opened — finish signing in there.")).toBeTruthy();

  fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
  await waitFor(() => expect(cancelPluginInstall).toHaveBeenCalledWith(LOCAL_RUNNER, "github", "state-999"));
  expect(onClose).toHaveBeenCalled();
});

// Fix (Finding 1 — review of Task 15): the connector "connect" step's
// `deadEnd` sub-state (every case `subStateFor` can't otherwise resolve) used
// to render a static message with no way out — the retired catalog install
// modal's equivalent `checkError` step had a Retry button. `beginPluginInstall`
// first resolves to a dead-end shape (oauth, not external, doesn't need a
// client id, no browser flow available), then — after clicking Retry —
// resolves to `oauthAvailable`, landing on the oauth-wait UI. Two
// `beginPluginInstall` calls total: the "install" step's own mount-time call,
// then the dead-end's Retry.
test("connector dead-end shows Retry, and Retry re-resolves to the oauth-wait UI", async () => {
  detailData = tokenAuthDetailFixture({ kind: "oauth" });
  beginData = beginResult();
  await renderWizard(undefined, "github");
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();
  expect(within(dialog).getByText("Couldn't resolve a sign-in method for this plugin.")).toBeTruthy();
  expect(beginPluginInstall).toHaveBeenCalledTimes(1);

  beginData = beginResult({
    oauthAvailable: true,
    dcrSucceeded: true,
    oauthBegin: {
      stateToken: "state-retry",
      authorizeUrl: "https://github.example.com/oauth/authorize",
      redirectUri: "http://127.0.0.1:8976/callback",
    },
  });
  fireEvent.click(within(dialog).getByRole("button", { name: "Retry" }));
  await act(async () => {});
  await act(async () => {});

  expect(within(dialog).getByText("Browser opened — finish signing in there.")).toBeTruthy();
  expect(beginPluginInstall).toHaveBeenCalledTimes(2);
});

// Fix (Finding 1): a dead-end with a `dcrError` shows that message instead of
// the generic fallback — same as `ManualClientId`/`OauthWait`'s own dcrError
// rendering.
test("connector dead-end shows the dcrError message when one is present", async () => {
  detailData = tokenAuthDetailFixture({ kind: "oauth" });
  beginData = beginResult({ dcrError: "vendor discovery endpoint returned 500" });
  await renderWizard(undefined, "github");
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("vendor discovery endpoint returned 500")).toBeTruthy();
  expect(within(dialog).getByRole("button", { name: "Retry" })).toBeTruthy();
});

// Fix (Finding 2 — review of Task 15): `ManualClientId`'s `needsClientId`
// sub-state had zero coverage in this file. Its own submit button is also
// labeled "Continue" (same as the shell's footer button, always rendered) —
// scope queries to the modal-body slot to disambiguate.
test("connector manualClientId (needsClientId) renders the client-id form", async () => {
  detailData = tokenAuthDetailFixture({ kind: "oauth" });
  beginData = beginResult({ needsClientId: true });
  await renderWizard(undefined, "github");
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();
  expect(within(dialog).getByText(/doesn't support automatic app registration/)).toBeTruthy();
  expect(within(dialog).getByPlaceholderText("Paste the client ID from the vendor's console")).toBeTruthy();

  // Finding 5c (final-review fix): the retired catalog install modal's
  // manualClientId step had a "Where do I find this?" helpUrl button —
  // restored here, scoped to the modal-body slot since the shell's own
  // Continue button shares this step's label.
  const body = dialog.querySelector('[data-slot="modal-body"]') as HTMLElement;
  fireEvent.click(within(body).getByRole("button", { name: "Where do I find this?" }));
  expect(openUrl).toHaveBeenCalledWith("https://github.com/settings/tokens");
});

// Fix (Finding 2b): `oauthExternal` also lands on `ManualClientId` (different
// copy — the vendor's own first-run sign-in, not automatic app registration)
// — and saving the client id short-circuits straight past the oauth-wait
// machinery via `onNext()` (steps-connector.tsx submit(), the `begin.oauthExternal`
// branch) rather than re-begining a browser flow.
test("connector manualClientId (oauthExternal) renders the form, and saving advances straight to Done without a second begin call", async () => {
  detailData = tokenAuthDetailFixture({ kind: "oauth" });
  beginData = beginResult({ oauthExternal: true, needsClientId: true });
  await renderWizard(undefined, "github");
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText(/brokers its own sign-in/)).toBeTruthy();

  const body = dialog.querySelector('[data-slot="modal-body"]') as HTMLElement;
  fireEvent.change(within(body).getByPlaceholderText("Paste the client ID from the vendor's console"), {
    target: { value: "google-client" },
  });
  fireEvent.click(within(body).getByRole("button", { name: "Continue" }));

  await waitFor(() => expect(setPluginOauthClientId).toHaveBeenCalledWith(LOCAL_RUNNER, "github", "google-client"));
  await act(async () => {});
  expect(beginPluginInstall).toHaveBeenCalledTimes(1);
  expect(within(dialog).getByText("Step 4 of 4 — Done")).toBeTruthy();
});

// Fix (Finding 2c): non-external `needsClientId` — saving calls
// `setPluginOauthClientId` with the runner/pluginId/clientId, then re-begins
// (a second `beginPluginInstall` call); when that re-begin resolves
// `oauthAvailable`, the connect step lands on the oauth-wait UI.
test("connector manualClientId (needsClientId) save re-begins, landing on oauth-wait when it resolves oauthAvailable", async () => {
  detailData = tokenAuthDetailFixture({ kind: "oauth" });
  beginData = beginResult({ needsClientId: true });
  await renderWizard(undefined, "github");
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});

  const body = dialog.querySelector('[data-slot="modal-body"]') as HTMLElement;
  fireEvent.change(within(body).getByPlaceholderText("Paste the client ID from the vendor's console"), {
    target: { value: "client-abc" },
  });

  beginData = beginResult({
    oauthAvailable: true,
    dcrSucceeded: true,
    oauthBegin: {
      stateToken: "state-cid",
      authorizeUrl: "https://github.example.com/oauth/authorize",
      redirectUri: "http://127.0.0.1:8976/callback",
    },
  });
  fireEvent.click(within(body).getByRole("button", { name: "Continue" }));

  await waitFor(() => expect(setPluginOauthClientId).toHaveBeenCalledWith(LOCAL_RUNNER, "github", "client-abc"));
  await waitFor(() => expect(beginPluginInstall).toHaveBeenCalledTimes(2));
  expect(await within(dialog).findByText("Browser opened — finish signing in there.")).toBeTruthy();
});

// Fix (Finding 3 — review of Task 15): `DoneStep`'s classic-connector
// enable-commit (`setPluginEnabled`) must stay gated on `info.experimental` —
// pairs with the existing "classic connector token path" test above, which
// asserts the non-experimental case DOES enable.
test("classic connector experimental: reaching Done does not call setPluginEnabled", async () => {
  const fixture = tokenAuthDetailFixture();
  detailData = { ...fixture, info: { ...fixture.info, experimental: true } };
  await renderWizard(undefined, "github");
  const dialog = screen.getByRole("dialog", { name: "Install GitHub" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();

  fireEvent.change(within(dialog).getByLabelText("Credential *"), { target: { value: "ghp_abc123" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(setPluginSetting).toHaveBeenCalledWith(LOCAL_RUNNER, "plugin.github.token", "ghp_abc123"));

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  expect(within(dialog).getByText("Step 4 of 4 — Done")).toBeTruthy();
  await act(async () => {});
  expect(setPluginEnabled).not.toHaveBeenCalled();
});

// Skill-pack trust path: `beginSkillInstall` returns a trust prompt (an
// arbitrary or curated-but-code-running source) — the plan reactively grows
// a "permissions" step right before "install" the moment `ctx.skillTrust` is
// set (see `WizardCtx`'s doc), accepting it re-enters "install" for the
// `confirmSkillInstall` pass, and the plan shrinks back once that clears
// `skillTrust`, landing on "done" without this step ever calling `onNext()`.
test("skill-pack trust path: the permissions step shows the trust prompt, and accepting confirms the install", async () => {
  detailData = skillPackDetailFixture();
  skillBeginData = {
    completed: false,
    trust: {
      token: "trust-token-1",
      sourceSpec: "https://github.com/acme/pack",
      ownerRepo: "acme/pack",
      resolvedCommit: "deadbeefcafe1234",
      skills: ["triage", "review"],
      hookScripts: [],
      totalBytes: 2048,
      runsCode: false,
      curated: false,
    },
    plugin: null,
  };
  await renderWizard(undefined, "superpowers");
  const dialog = screen.getByRole("dialog", { name: "Install Superpowers" });
  expect(within(dialog).getByText("Step 1 of 3 — Overview")).toBeTruthy();

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(beginSkillInstall).toHaveBeenCalledWith(LOCAL_RUNNER, "superpowers");
  expect(within(dialog).getByText("Step 2 of 4 — Permissions")).toBeTruthy();
  expect(within(dialog).getByText(/isn't a curated pack/)).toBeTruthy();
  expect(within(dialog).getByText("acme/pack")).toBeTruthy();

  let continueButton = within(dialog).getByRole("button", { name: "Continue" }) as HTMLButtonElement;
  expect(continueButton.disabled).toBe(true);
  fireEvent.click(within(dialog).getByRole("switch", { name: "Accept permissions" }));
  continueButton = within(dialog).getByRole("button", { name: "Continue" }) as HTMLButtonElement;
  expect(continueButton.disabled).toBe(false);

  act(() => continueButton.click());
  await act(async () => {});
  await act(async () => {});
  expect(confirmSkillInstall).toHaveBeenCalledWith(LOCAL_RUNNER, "trust-token-1");
  // The plan shrank back to 3 steps once `confirmSkillInstall` cleared
  // `ctx.skillTrust` — "done" is index 2 of 3 here, not 3 of 4.
  expect(within(dialog).getByText("Step 3 of 3 — Done")).toBeTruthy();
});

// A curated pack (no trust prompt at all) never grows a permissions step —
// `beginSkillInstall` resolving `completed: true` advances straight to done.
test("a curated skill pack's install completes immediately with no permissions step", async () => {
  detailData = skillPackDetailFixture();
  skillBeginData = { completed: true, trust: null, plugin: { id: "superpowers", name: "Superpowers" } };
  await renderWizard(undefined, "superpowers");
  const dialog = screen.getByRole("dialog", { name: "Install Superpowers" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(beginSkillInstall).toHaveBeenCalledWith(LOCAL_RUNNER, "superpowers");
  expect(confirmSkillInstall).not.toHaveBeenCalled();
  expect(within(dialog).getByText("Step 3 of 3 — Done")).toBeTruthy();
});

// Provider path: install registers the family into the connections store,
// connect renders the extracted `ConnectionMethodForm` inline, and Skip
// (connect is skippable) reaches Done without adding an account.
test("provider path: install registers the family, connect renders the account form, and Skip reaches Done", async () => {
  detailData = providerDetailFixture();
  await renderWizard(undefined, "groq");
  const dialog = screen.getByRole("dialog", { name: "Install Groq" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(installProvider).toHaveBeenCalledWith(LOCAL_RUNNER, "groq");
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();
  // `ConnectionMethodForm`'s own account-form content — proves the provider
  // adapter's connect step actually rendered it, not a placeholder.
  expect(within(dialog).getByRole("button", { name: "Add account" })).toBeTruthy();

  fireEvent.click(within(dialog).getByRole("button", { name: "Skip" }));
  await act(async () => {});
  expect(within(dialog).getByText("Step 4 of 4 — Done")).toBeTruthy();
});

// Finding 1 (c): the component-backed-provider fixture must walk the
// PROVIDER install/connect steps, not the component ones — proves
// `wizardKind`'s reordered check. `installComponentPlugin` must never fire
// for it, and its connect step must render `ConnectionMethodForm` (not the
// component `ConnectStep`'s "Nothing more to connect here." fallback).
test("a component-backed provider installs via installProvider (not installComponentPlugin) and connects via ConnectionMethodForm", async () => {
  detailData = providerDetailFixture();
  await renderWizard(undefined, "groq");
  const dialog = screen.getByRole("dialog", { name: "Install Groq" });
  expect(within(dialog).getByText("Step 1 of 4 — Overview")).toBeTruthy();
  // No permissions step in the plan at all.
  expect(within(dialog).queryByText("Permissions")).toBeNull();

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(installProvider).toHaveBeenCalledWith(LOCAL_RUNNER, "groq");
  expect(installComponentPlugin).not.toHaveBeenCalled();
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();
  expect(within(dialog).getByRole("button", { name: "Add account" })).toBeTruthy();
  expect(within(dialog).queryByText("Nothing more to connect here.")).toBeNull();
});

// Finding 2: a family member (id != family head) must install/connect via
// the FAMILY, not its own id — the daemon's `install_provider` bails on a
// non-head id, and `ConnectionMethodForm`'s catalog filter only matches the
// head.
test("a provider family member installs and connects via its family, not its own id", async () => {
  detailData = familyMemberProviderDetailFixture();
  await renderWizard(undefined, "anthropic-oauth");
  const dialog = screen.getByRole("dialog", { name: "Install Claude (subscription)" });

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  await act(async () => {});
  await act(async () => {});
  expect(installProvider).toHaveBeenCalledWith(LOCAL_RUNNER, "anthropic");
  expect(installProvider).not.toHaveBeenCalledWith(LOCAL_RUNNER, "anthropic-oauth");
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();
  // `ConnectionMethodForm` found the "anthropic" catalog entry via `family`
  // — if it had been passed "anthropic-oauth" instead, no member would match
  // and "Add account" would not render.
  expect(within(dialog).getByRole("button", { name: "Add account" })).toBeTruthy();
});

test("Settings step renders a FieldRow per declared setting and saves via setPluginSetting", async () => {
  await renderWizard("settings");
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  expect(within(dialog).getByText("Step 5 of 6 — Settings")).toBeTruthy();

  fireEvent.change(within(dialog).getByLabelText("Workspace"), { target: { value: "acme" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
  await act(async () => {});

  expect(setPluginSetting).toHaveBeenCalledWith(LOCAL_RUNNER, "plugin.notion.workspace", "acme");
});

test("Done step's Open plugin page navigates to the plugin detail page and closes the wizard", async () => {
  await renderWizard("done");
  await act(async () => {});
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  expect(within(dialog).getByText("Step 6 of 6 — Done")).toBeTruthy();

  act(() => within(dialog).getByRole("button", { name: "Open plugin page" }).click());

  expect(useNav.getState().history.current).toEqual({ kind: "pluginDetail", id: "notion" });
  expect(onClose).toHaveBeenCalled();
});
