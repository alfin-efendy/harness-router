import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { AgentDetailInfo, AgentMutationInfo } from "@/bindings";
import { useAgentConfigurationCatalog } from "@/store-agent-catalog";

// Realistic fixture shape: `build_live_catalog` emits ONE pluginTools entry
// per installed plugin, id = the bare plugin manifest id (single-segment,
// never "provider.tool"), and apps (MCP servers) are an independent
// registry — so "lint" below is an enabled plugin with no matching app.
const getAgentConfigurationCatalog = mock(async () => ({
  status: "ok" as const,
  data: {
    skills: [],
    nativeTools: [],
    pluginTools: [
      {
        id: "github",
        label: "GitHub tools",
        description: "GitHub plugin tools",
        available: true,
        commandScoped: false,
        pack: null,
        kind: "integration",
      },
      { id: "lint", label: "Lint", description: "Lint plugin", available: true, commandScoped: false, pack: null, kind: "integration" },
      {
        id: "anthropic",
        label: "Anthropic",
        description: "Model provider",
        available: true,
        commandScoped: false,
        pack: null,
        kind: "provider",
      },
      {
        id: "native",
        label: "Ryuzi",
        description: "Built-in agent runtime",
        available: true,
        commandScoped: false,
        pack: null,
        kind: "runtime",
      },
    ],
    apps: [
      { id: "github", label: "GitHub", description: "GitHub MCP", available: true, commandScoped: false, pack: null, kind: null },
      { id: "notion", label: "Notion", description: "Notion MCP", available: true, commandScoped: false, pack: null, kind: null },
    ],
  },
}));
mock.module("@/bindings", () => ({ commands: { getAgentConfigurationCatalog }, events: {} }));

const { AgentAppsTab } = await import("./AgentAppsTab");
const { useAgents } = await import("@/store-agents");

const updateAgent = mock(async (_agentId: string, _input: AgentMutationInfo) => true);

const reviewerDetail: AgentDetailInfo = {
  summary: {
    id: "reviewer",
    name: "Reviewer",
    description: "Reviews implementation quality.",
    avatarColor: "violet",
    avatarPet: null,
    model: { kind: "route", route: "free" },
    builtin: false,
    skillCount: 0,
    toolCount: 1,
    knowledgeCount: 0,
    executable: true,
    validation: [],
    isDefault: false,
  },
  permissionRules: [],
  skills: [],
  nativeTools: [],
  pluginTools: ["github"],
  apps: ["github"],
  modelInfo: null,
  personality: { preset: "helpful", custom: null },
};

beforeEach(() => {
  updateAgent.mockClear();
  getAgentConfigurationCatalog.mockClear();
  useAgents.setState({ saving: false, update: updateAgent });
  useAgentConfigurationCatalog.setState({ catalog: null, loading: false, error: null });
});
afterEach(() => {
  cleanup();
  useAgentConfigurationCatalog.setState({ catalog: null, loading: false, error: null });
});

test("renders one card per catalog app; plugin tools nest under a matching app card and never leak across cards", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);

  const github = await screen.findByTestId("app-card-github");
  expect(within(github).getByText("GitHub")).toBeTruthy();
  expect(within(github).getByText("GitHub tools")).toBeTruthy();

  const notion = screen.getByTestId("app-card-notion");
  expect(within(notion).getByText("Notion")).toBeTruthy();
  expect(within(notion).queryByText("GitHub tools")).toBeNull();
  expect(within(notion).queryByText("Lint")).toBeNull();
});

test("plugins with no matching app render as flat rows in a Plugins section, not under any app card", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);

  const plugins = await screen.findByTestId("plugins-section");
  expect(within(plugins).getByText("Lint")).toBeTruthy();
  expect(within(screen.getByTestId("app-card-github")).queryByText("Lint")).toBeNull();
  expect(within(screen.getByTestId("app-card-notion")).queryByText("Lint")).toBeNull();
});

test("master toggle enabling an app adds the app and unions in its plugin tools", async () => {
  render(<AgentAppsTab detail={{ ...reviewerDetail, apps: [], pluginTools: [] }} />);
  const github = await screen.findByTestId("app-card-github");

  fireEvent.click(within(github).getByRole("switch", { name: "Enable app github" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ apps: ["github"], pluginTools: ["github"] })),
  );
});

test("master toggle disabling an app removes the app and all of its plugin tool ids", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);
  const github = await screen.findByTestId("app-card-github");

  fireEvent.click(within(github).getByRole("switch", { name: "Enable app github" }));

  await waitFor(() => expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ apps: [], pluginTools: [] })));
});

test("nested tool toggle fires update changing only that tool, independent of the app master switch", async () => {
  render(<AgentAppsTab detail={{ ...reviewerDetail, pluginTools: [] }} />);
  const github = await screen.findByTestId("app-card-github");

  fireEvent.click(within(github).getByRole("switch", { name: "Enable plugin tool github" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ apps: ["github"], pluginTools: ["github"] })),
  );
});

test("flat plugin row toggle fires update adding that plugin id, leaving apps untouched", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);
  const plugins = await screen.findByTestId("plugins-section");

  fireEvent.click(within(plugins).getByRole("switch", { name: "Enable plugin tool lint" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ apps: ["github"], pluginTools: ["github", "lint"] })),
  );
});

test("flat plugin row toggle off fires update removing only that plugin id", async () => {
  render(<AgentAppsTab detail={{ ...reviewerDetail, pluginTools: ["github", "lint"] }} />);
  const plugins = await screen.findByTestId("plugins-section");

  fireEvent.click(within(plugins).getByRole("switch", { name: "Enable plugin tool lint" }));

  await waitFor(() => expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ pluginTools: ["github"] })));
});

test("unavailable app entries keep the remove affordance, and removing clears the app and its orphaned plugin tools", async () => {
  render(<AgentAppsTab detail={{ ...reviewerDetail, apps: ["github", "retired-app"], pluginTools: ["github", "retired-app.tool"] }} />);

  const retired = await screen.findByTestId("app-card-retired-app");
  expect(within(retired).getByText("Unavailable")).toBeTruthy();
  expect(within(retired).getByRole("button", { name: "Remove unavailable app retired-app" })).toBeTruthy();
  expect(within(retired).getByRole("button", { name: "Remove unavailable plugin tool retired-app.tool" })).toBeTruthy();

  fireEvent.click(within(retired).getByRole("button", { name: "Remove unavailable app retired-app" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ apps: ["github"], pluginTools: ["github"] })),
  );
});

test("an enabled plugin id missing from the catalog with no matching app stays visible and removable in the Plugins section", async () => {
  render(<AgentAppsTab detail={{ ...reviewerDetail, pluginTools: ["github", "ghost"] }} />);

  const plugins = await screen.findByTestId("plugins-section");
  expect(within(plugins).getByText(/ghost \(unavailable\)/)).toBeTruthy();

  fireEvent.click(within(plugins).getByRole("button", { name: "Remove unavailable plugin tool ghost" }));

  await waitFor(() => expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ pluginTools: ["github"] })));
});

test("no update fires from a master, nested, or flat toggle while a save is already in flight", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);
  const github = await screen.findByTestId("app-card-github");
  const plugins = screen.getByTestId("plugins-section");
  useAgents.setState({ saving: true });

  fireEvent.click(within(github).getByRole("switch", { name: "Enable app github" }));
  fireEvent.click(within(github).getByRole("switch", { name: "Enable plugin tool github" }));
  fireEvent.click(within(plugins).getByRole("switch", { name: "Enable plugin tool lint" }));

  expect(updateAgent).not.toHaveBeenCalled();
});

test("provider and runtime plugins are hidden from the Plugins section", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);
  const plugins = await screen.findByTestId("plugins-section");
  expect(within(plugins).getByText("Lint")).toBeTruthy();
  expect(within(plugins).queryByText("Anthropic")).toBeNull();
  expect(within(plugins).queryByText("Ryuzi")).toBeNull();
});

test("a hidden-kind plugin the profile already enables stays visible and can be toggled off", async () => {
  render(<AgentAppsTab detail={{ ...reviewerDetail, pluginTools: ["github", "anthropic"] }} />);
  const plugins = await screen.findByTestId("plugins-section");
  expect(within(plugins).getByText("Anthropic")).toBeTruthy();

  fireEvent.click(within(plugins).getByRole("switch", { name: "Enable plugin tool anthropic" }));
  await waitFor(() => expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ pluginTools: ["github"] })));
});
