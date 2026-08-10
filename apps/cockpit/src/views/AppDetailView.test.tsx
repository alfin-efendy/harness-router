import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { AppInfo } from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";

// happy-dom lacks a couple of layout APIs Base UI's Menu popup touches when
// positioning (same stub `PluginsView.test.tsx`/`combobox.test.tsx` use) —
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

// Task 12: `AppDetailView` (the MCP-server detail page) folded into the same
// tabbed template `PluginDetailView` uses (Task 9) — hero + Segmented tabs
// instead of one long stacked-card page. Every existing card's logic
// (Connection, Scope, per-tool permission, Agent access, the error status
// banner) is preserved, just relocated into a tab panel. `useApps` is the
// ONLY store this view touches (no `usePlugins`/`pluginDetail`), so mocking
// the app-shaped RPCs on `@/bindings` is enough to drive every section.

const slackApp: AppInfo = {
  id: "slack",
  name: "Slack",
  kind: "MCP · stdio",
  initial: "S",
  color: "#4A154B",
  desc: "Slack tools and channels via the community MCP server.",
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
  authKind: "env",
  authDetail: "SLACK_BOT_TOKEN",
  oauthTokenStored: false,
  oauthReconnectRequired: false,
  tools: [{ name: "channels_list", desc: "List channels the bot can see", perm: "ask" }],
  agentAccess: [],
  pluginId: null,
};

// Same shape as `slackApp` but never discovered any tools and is in the
// error state — exercises the "no Tools tab" + "Health tab appears" arms of
// `visibleTabs`'s input (`hasTools: app.tools.length > 0`, `hasHealth: app.
// status === "error"`).
const brokenApp: AppInfo = {
  ...slackApp,
  id: "broken",
  name: "Broken",
  status: "error",
  statusDetail: "Auth expired — reconnect required.",
  tools: [],
};

// Task 9: a remote (http) server, not yet connected — exercises the OAuth
// Connect affordance on the Connection card.
const remoteApp: AppInfo = {
  ...slackApp,
  id: "remote",
  name: "Remote MCP",
  transport: "http",
  command: null,
  args: [],
  url: "https://mcp.example.com",
  authKind: "none",
  authDetail: null,
};

let appsFixture: AppInfo[] = [slackApp, brokenApp];

const listApps = mock(async () => ({ status: "ok" as const, data: appsFixture }));
const removeApp = mock(async (_runnerId: string, id: string) => {
  appsFixture = appsFixture.filter((a) => a.id !== id);
  return { status: "ok" as const, data: appsFixture };
});
const probeApp = mock(async (_runnerId: string, _id: string) => ({ status: "ok" as const, data: appsFixture }));
const updateAppScope = mock(async (_runnerId: string, id: string, scope: string, scopeGateways: string[]) => {
  appsFixture = appsFixture.map((a) => (a.id === id ? { ...a, scope, scopeGateways } : a));
  return { status: "ok" as const, data: appsFixture };
});
const setAppToolPerm = mock(async (_runnerId: string, id: string, tool: string, perm: string) => {
  appsFixture = appsFixture.map((a) => (a.id === id ? { ...a, tools: a.tools.map((t) => (t.name === tool ? { ...t, perm } : t)) } : a));
  return { status: "ok" as const, data: appsFixture };
});
const toggleAppAgent = mock(async (_runnerId: string, id: string, agentId: string, allowed: boolean) => {
  appsFixture = appsFixture.map((a) =>
    a.id === id ? { ...a, agentAccess: a.agentAccess.map((x) => (x.agentId === agentId ? { ...x, allowed } : x)) } : a,
  );
  return { status: "ok" as const, data: appsFixture };
});
const beginMcpConnect = mock(async (_runnerId: string, _id: string) => ({
  status: "ok" as const,
  data: { authorizeUrl: "https://auth.example.com/authorize?state=abc", state: "abc", verifier: "verifier-xyz" },
}));
const completeMcpConnect = mock(async (_runnerId: string, _id: string, _code: string, _verifier: string) => ({
  status: "ok" as const,
  data: appsFixture,
}));
const disconnectMcp = mock(async (_runnerId: string, id: string) => {
  appsFixture = appsFixture.map((a) => (a.id === id ? { ...a, oauthTokenStored: false, oauthReconnectRequired: false } : a));
  return { status: "ok" as const, data: appsFixture };
});
const openUrl = mock(async (_u: string) => {});

mock.module("@/bindings", () => ({
  // `useGateways` pulls in the shared `@/store` module (for its post-remove
  // session/transcript pruning), which transitively imports several other
  // stores (scheduler, automations, ...) that import `events` from this same
  // module — none of them touch it outside a function body, so an empty stub
  // is enough to keep the import graph linkable without wiring up every
  // event channel in Cockpit.
  events: {},
  commands: {
    listApps,
    removeApp,
    probeApp,
    updateAppScope,
    setAppToolPerm,
    toggleAppAgent,
    beginMcpConnect,
    completeMcpConnect,
    disconnectMcp,
  },
}));
mock.module("@tauri-apps/plugin-opener", () => ({ openUrl }));

const { AppDetailView } = await import("@/views/AppDetailView");
const { useApps } = await import("@/store-apps");
const { useNav } = await import("@/store-nav");

beforeEach(() => {
  listApps.mockClear();
  removeApp.mockClear();
  probeApp.mockClear();
  updateAppScope.mockClear();
  setAppToolPerm.mockClear();
  toggleAppAgent.mockClear();
  beginMcpConnect.mockClear();
  completeMcpConnect.mockClear();
  disconnectMcp.mockClear();
  openUrl.mockClear();
  appsFixture = [slackApp, brokenApp];
  useApps.setState({ apps: appsFixture, loaded: true, hydrating: false, probing: null });
  useNav.setState({ history: { back: [{ kind: "plugins" }], current: { kind: "appDetail", id: "slack" }, forward: [] } });
});

afterEach(() => {
  cleanup();
  useApps.setState({ apps: [], loaded: false, hydrating: false, probing: null });
  useNav.setState({ history: { back: [], current: { kind: "home" }, forward: [] } });
});

test("a connected app shows Overview/Tools/Settings tabs but no Health tab", async () => {
  render(<AppDetailView id="slack" />);
  expect(await screen.findByText("Slack")).toBeTruthy();

  expect(screen.getByRole("button", { name: "Overview" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Tools" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Settings" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Health" })).toBeNull();
});

test("an error-status app with no tools omits Tools but shows Health with the status-detail banner", async () => {
  render(<AppDetailView id="broken" />);
  expect(await screen.findByText("Broken")).toBeTruthy();

  expect(screen.queryByRole("button", { name: "Tools" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Health" }));
  expect(screen.getByText("Auth expired — reconnect required.")).toBeTruthy();
});

test("Overview shows the description and the Connection card (command + env)", async () => {
  render(<AppDetailView id="slack" />);
  await screen.findByText("Slack");

  expect(screen.getByText("Slack tools and channels via the community MCP server.")).toBeTruthy();
  expect(screen.getByText("Connection")).toBeTruthy();
  expect(screen.getByText("npx -y @modelcontextprotocol/server-slack")).toBeTruthy();
  expect(screen.getByText("SLACK_BOT_TOKEN")).toBeTruthy();
});

test("the per-tool permission Segmented still fires setToolPerm through the store", async () => {
  render(<AppDetailView id="slack" />);
  await screen.findByText("Slack");
  fireEvent.click(screen.getByRole("button", { name: "Tools" }));

  expect(await screen.findByText("channels_list")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Allow" }));
  await waitFor(() => expect(setAppToolPerm).toHaveBeenCalledWith(LOCAL_RUNNER, "slack", "channels_list", "allow"));
});

test("Settings tab: the Agent-access Switch still fires toggleAgent", async () => {
  render(<AppDetailView id="slack" />);
  await screen.findByText("Slack");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  const sw = screen.getByRole("switch", { name: "Agent access" });
  // No `agentAccess` row for the native agent ⇒ `agentAllowed` defaults to
  // allowed — toggling should flip it to false.
  fireEvent.click(sw);
  await waitFor(() => expect(toggleAppAgent).toHaveBeenCalledWith(LOCAL_RUNNER, "slack", "native", false));
});

test("Settings tab: the Scope Segmented still fires setScope", async () => {
  render(<AppDetailView id="slack" />);
  await screen.findByText("Slack");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  fireEvent.click(screen.getByRole("button", { name: "Select" }));
  await waitFor(() => expect(updateAppScope).toHaveBeenCalledWith(LOCAL_RUNNER, "slack", "select", []));
});

test("Probe button still fires probe(id)", async () => {
  render(<AppDetailView id="slack" />);
  await screen.findByText("Slack");

  fireEvent.click(screen.getByRole("button", { name: "Probe" }));
  await waitFor(() => expect(probeApp).toHaveBeenCalledWith(LOCAL_RUNNER, "slack"));
});

test("Remove in the overflow menu calls remove(id) then navigates back", async () => {
  render(<AppDetailView id="slack" />);
  await screen.findByText("Slack");

  fireEvent.click(screen.getByRole("button", { name: "Actions for Slack" }));
  fireEvent.click(await screen.findByRole("menuitem", { name: "Remove" }));

  await waitFor(() => expect(removeApp).toHaveBeenCalledWith(LOCAL_RUNNER, "slack"));
  expect(useNav.getState().history.current).toEqual({ kind: "plugins" });
});

// ---------- Task 9: remote MCP server OAuth connect ----------

test("a not-yet-connected remote server shows a Connect button and no OAuth pill states leak in", async () => {
  appsFixture = [remoteApp];
  useApps.setState({ apps: appsFixture, loaded: true, hydrating: false, probing: null });
  useNav.setState({ history: { back: [{ kind: "plugins" }], current: { kind: "appDetail", id: "remote" }, forward: [] } });

  render(<AppDetailView id="remote" />);
  await screen.findByText("Remote MCP");

  expect(screen.getByText("Not connected")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Connect" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Disconnect" })).toBeNull();
  expect(screen.queryByText("Reconnect required")).toBeNull();
});

test("clicking Connect starts the flow, opens the authorize URL, and shows a pending state (not idle/broken)", async () => {
  appsFixture = [remoteApp];
  useApps.setState({ apps: appsFixture, loaded: true, hydrating: false, probing: null });
  useNav.setState({ history: { back: [{ kind: "plugins" }], current: { kind: "appDetail", id: "remote" }, forward: [] } });

  render(<AppDetailView id="remote" />);
  await screen.findByText("Remote MCP");

  fireEvent.click(screen.getByRole("button", { name: "Connect" }));

  await waitFor(() => expect(beginMcpConnect).toHaveBeenCalledWith(LOCAL_RUNNER, "remote"));
  await waitFor(() => expect(openUrl).toHaveBeenCalledWith("https://auth.example.com/authorize?state=abc"));
  // The user leaves for the browser here — the UI must not look idle: a
  // visible waiting message + a way out (Cancel), not a silently-disabled
  // Connect button with no explanation.
  expect(await screen.findByText(/Waiting for you to finish signing in in the browser/)).toBeTruthy();
  expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
});

test(
  "the connect poll picks up a completed connection and reports success",
  async () => {
    appsFixture = [remoteApp];
    useApps.setState({ apps: appsFixture, loaded: true, hydrating: false, probing: null });
    useNav.setState({ history: { back: [{ kind: "plugins" }], current: { kind: "appDetail", id: "remote" }, forward: [] } });
    // PROPERTY: the pending UI must actually resolve once the server reports
    // connected, not spin forever — the first post-Connect `listApps` refresh
    // still reports "not connected" (proving the loop doesn't just declare
    // victory on the very next tick), the second reports connected.
    let listAppsCalls = 0;
    listApps.mockImplementation(async () => {
      listAppsCalls += 1;
      const connected = listAppsCalls >= 2; // 1st poll: still pending; 2nd poll: connected
      return {
        status: "ok" as const,
        data: [{ ...remoteApp, oauthTokenStored: connected, oauthReconnectRequired: false }],
      };
    });

    render(<AppDetailView id="remote" />);
    await screen.findByText("Remote MCP");
    fireEvent.click(screen.getByRole("button", { name: "Connect" }));

    await screen.findByText(/Waiting for you to finish signing in in the browser/);
    await waitFor(() => expect(screen.getByText("OAuth connected")).toBeTruthy(), { timeout: 12_000 });
    expect(screen.queryByText(/Waiting for you to finish/)).toBeNull();
    listApps.mockImplementation(async () => ({ status: "ok" as const, data: appsFixture }));
  },
  15_000,
);

test("a reconnect-required remote server shows the warning pill and a Reconnect label", async () => {
  const tripped: AppInfo = { ...remoteApp, oauthTokenStored: true, oauthReconnectRequired: true };
  appsFixture = [tripped];
  useApps.setState({ apps: appsFixture, loaded: true, hydrating: false, probing: null });
  useNav.setState({ history: { back: [{ kind: "plugins" }], current: { kind: "appDetail", id: "remote" }, forward: [] } });

  render(<AppDetailView id="remote" />);
  await screen.findByText("Remote MCP");

  expect(screen.getByText("Reconnect required")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Reconnect" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Disconnect" })).toBeTruthy();
});

test("a connected remote server's Disconnect button calls disconnectMcp with the server id", async () => {
  const connected: AppInfo = { ...remoteApp, oauthTokenStored: true, oauthReconnectRequired: false };
  appsFixture = [connected];
  useApps.setState({ apps: appsFixture, loaded: true, hydrating: false, probing: null });
  useNav.setState({ history: { back: [{ kind: "plugins" }], current: { kind: "appDetail", id: "remote" }, forward: [] } });

  render(<AppDetailView id="remote" />);
  await screen.findByText("Remote MCP");
  expect(screen.getByText("OAuth connected")).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));
  await waitFor(() => expect(disconnectMcp).toHaveBeenCalledWith(LOCAL_RUNNER, "remote"));
});
