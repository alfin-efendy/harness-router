import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { AgentDetailInfo, AgentMutationInfo } from "@/bindings";
import { useAgentConfigurationCatalog } from "@/store-agent-catalog";

const getAgentConfigurationCatalog = mock(async () => ({
  status: "ok" as const,
  data: {
    skills: [],
    nativeTools: [],
    pluginTools: [
      { id: "github.search", label: "Search", description: "Search GitHub", available: true, commandScoped: false, pack: null },
      { id: "github.issues", label: "Issues", description: "GitHub issues", available: true, commandScoped: false, pack: null },
      { id: "notion.pages", label: "Pages", description: "Notion pages", available: true, commandScoped: false, pack: null },
    ],
    apps: [
      { id: "github", label: "GitHub", description: "GitHub MCP", available: true, commandScoped: false, pack: null },
      { id: "notion", label: "Notion", description: "Notion MCP", available: true, commandScoped: false, pack: null },
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
  pluginTools: ["github.search"],
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

test("renders one card per catalog app with nested plugin tool rows grouped by provider", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);

  const github = await screen.findByTestId("app-card-github");
  expect(within(github).getByText("GitHub")).toBeTruthy();
  expect(within(github).getByText("Search")).toBeTruthy();
  expect(within(github).getByText("Issues")).toBeTruthy();

  const notion = screen.getByTestId("app-card-notion");
  expect(within(notion).getByText("Notion")).toBeTruthy();
  expect(within(notion).getByText("Pages")).toBeTruthy();
  expect(within(notion).queryByText("Search")).toBeNull();
});

test("master toggle enabling an app adds the app and unions in all of its catalog plugin tools", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);
  const notion = await screen.findByTestId("app-card-notion");

  fireEvent.click(within(notion).getByRole("switch", { name: "Enable app notion" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith(
      "reviewer",
      expect.objectContaining({
        apps: expect.arrayContaining(["github", "notion"]),
        pluginTools: expect.arrayContaining(["github.search", "notion.pages"]),
      }),
    ),
  );
});

test("master toggle disabling an app removes the app and all of its plugin tool ids", async () => {
  render(<AgentAppsTab detail={{ ...reviewerDetail, pluginTools: ["github.search", "github.issues"] }} />);
  const github = await screen.findByTestId("app-card-github");

  fireEvent.click(within(github).getByRole("switch", { name: "Enable app github" }));

  await waitFor(() => expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ apps: [], pluginTools: [] })));
});

test("nested tool toggle fires update changing only that tool, independent of the app master switch", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);
  const github = await screen.findByTestId("app-card-github");

  fireEvent.click(within(github).getByRole("switch", { name: "Enable plugin tool github.issues" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith(
      "reviewer",
      expect.objectContaining({
        apps: ["github"],
        pluginTools: expect.arrayContaining(["github.search", "github.issues"]),
      }),
    ),
  );
});

test("nested tool toggle off fires update removing only that tool", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);
  const github = await screen.findByTestId("app-card-github");

  fireEvent.click(within(github).getByRole("switch", { name: "Enable plugin tool github.search" }));

  await waitFor(() => expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ pluginTools: [] })));
});

test("unavailable app entries keep the remove affordance, and removing clears the app and its orphaned plugin tools", async () => {
  render(
    <AgentAppsTab detail={{ ...reviewerDetail, apps: ["github", "retired-app"], pluginTools: ["github.search", "retired-app.tool"] }} />,
  );

  const retired = await screen.findByTestId("app-card-retired-app");
  expect(within(retired).getByText("Unavailable")).toBeTruthy();
  expect(within(retired).getByRole("button", { name: "Remove unavailable app retired-app" })).toBeTruthy();
  expect(within(retired).getByRole("button", { name: "Remove unavailable plugin tool retired-app.tool" })).toBeTruthy();

  fireEvent.click(within(retired).getByRole("button", { name: "Remove unavailable app retired-app" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ apps: ["github"], pluginTools: ["github.search"] })),
  );
});

test("no update fires from a master or nested toggle while a save is already in flight", async () => {
  render(<AgentAppsTab detail={reviewerDetail} />);
  const github = await screen.findByTestId("app-card-github");
  useAgents.setState({ saving: true });

  fireEvent.click(within(github).getByRole("switch", { name: "Enable app github" }));
  fireEvent.click(within(github).getByRole("switch", { name: "Enable plugin tool github.issues" }));

  expect(updateAgent).not.toHaveBeenCalled();
});
