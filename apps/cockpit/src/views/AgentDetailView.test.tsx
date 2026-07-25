import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type {
  AgentDetailInfo,
  AgentLearningInfo,
  AgentModelInfo,
  AgentMutationInfo,
  AgentRegistryInfo,
  AgentConfigurationCatalogInfo,
  CmdError,
  Result,
  SelectableModelInfo,
  Session,
} from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";

const getAgent = mock(async (_runner: string | null, id: string) => ({
  status: "ok" as const,
  data: detail({ summary: { ...detail().summary, id, name: id === "ryuzi" ? "Ryuzi" : "Reviewer", isDefault: id === "ryuzi" } }),
}));
const agentConfigurationCatalog: AgentConfigurationCatalogInfo = {
  skills: [
    {
      id: "requesting-code-review",
      label: "Requesting code review",
      description: "Review guidance",
      available: true,
      commandScoped: false,
      pack: null,
    },
  ],
  nativeTools: [
    { id: "read", label: "Read", description: "Read files", available: true, commandScoped: false, pack: null },
    { id: "grep", label: "Grep", description: "Search files", available: true, commandScoped: false, pack: null },
    { id: "bash", label: "Bash", description: "Run commands", available: true, commandScoped: true, pack: null },
  ],
  pluginTools: [{ id: "github", label: "GitHub", description: "GitHub tools", available: true, commandScoped: false, pack: null }],
  apps: [{ id: "github", label: "GitHub", description: "GitHub MCP", available: true, commandScoped: false, pack: null }],
};

const getAgentConfigurationCatalog = mock(async () => ({ status: "ok" as const, data: agentConfigurationCatalog }));
const listApps = mock(async () => ({ status: "ok" as const, data: [] }));
const updateAgent = mock(async (_runner: string | null, _id: string, input: AgentMutationInfo) => ({
  status: "ok" as const,
  data: detail({ ...input, modelInfo: null }),
}));
const duplicateAgent = mock(async (_runner: string | null, _id: string) => ({
  status: "ok" as const,
  data: detail({ summary: { ...detail().summary, id: "reviewer-copy", name: "Reviewer Copy" } }),
}));
const deleteAgent = mock(
  async (_runner: string | null, _id: string): Promise<Result<AgentRegistryInfo, CmdError>> => ({
    status: "ok",
    data: { ...registry, agents: registry.agents.filter((agent) => agent.id !== "reviewer-copy") },
  }),
);

const listAgentSessions = mock(async (_runner: string | null, _agentId: string, _limit: number) => ({
  status: "ok" as const,
  data: [] as Session[],
}));
const listMessages = mock(async (_runner: string | null, _sessionPk: string) => ({ status: "ok" as const, data: [] }));
const updateSubagentModel = mock(async (_runner: string | null, model: AgentModelInfo) => ({
  status: "ok" as const,
  data: { ...registry, subagentModel: model },
}));

mock.module("@/bindings", () => ({
  commands: {
    deleteAgent,
    duplicateAgent,
    getAgent,
    getAgentConfigurationCatalog,
    listAgentSessions,
    listApps,
    listMessages,
    updateAgent,
    updateSubagentModel,
  },
  events: {},
}));

const { AgentDetailView } = await import("./AgentDetailView");
const { useStore } = await import("@/store");
const { useAgents } = await import("@/store-agents");
const { useApps } = await import("@/store-apps");
const { useAgentConfigurationCatalog } = await import("@/store-agent-catalog");
const { useLearning } = await import("@/store-learning");
const { useNav } = await import("@/store-nav");

const routeInfo: SelectableModelInfo = {
  kind: "namedRoute",
  requestValue: "free",
  displayName: "Smart",
  preferenceKey: null,
  supported: [],
  configuredDefault: null,
  resolvedDefault: null,
  defaultSource: "none",
};
const opusInfo: SelectableModelInfo = {
  kind: "concrete",
  requestValue: "anthropic/claude-opus-4-8",
  displayName: "Claude Opus",
  preferenceKey: null,
  supported: ["low", "medium", "high", "max", "xhigh"].map((value) => ({
    value,
    label: value === "xhigh" ? "XHigh" : value[0].toUpperCase() + value.slice(1),
    description: null,
  })),
  configuredDefault: null,
  resolvedDefault: "high",
  defaultSource: "provider",
};

const miniInfo: SelectableModelInfo = {
  ...opusInfo,
  requestValue: "anthropic/claude-haiku-4-5",
  displayName: "Claude Haiku",
  supported: [{ value: "low", label: "Low", description: null }],
  resolvedDefault: "low",
};

function recentSession(overrides: Partial<Session> = {}): Session {
  return {
    sessionPk: "s1",
    primaryAgentId: "reviewer",
    primaryAgentSnapshot: { id: "reviewer", name: "Reviewer", avatarColor: "violet" },
    projectId: "p1",
    agentSessionId: null,
    worktreePath: null,
    branch: "feature/agent-sessions",
    title: "Preserve immutable ownership",
    status: "idle",
    permMode: "default",
    startedBy: "cockpit",
    createdAt: 1,
    lastActive: 2,
    resumeAttempts: 0,
    branchOwned: false,
    kind: "project",
    speaker: null,
    agent: null,
    parentSessionPk: null,
    ...overrides,
  };
}
function detail(overrides: Partial<AgentDetailInfo> = {}): AgentDetailInfo {
  return {
    summary: {
      id: "reviewer",
      name: "Reviewer",
      description: "Reviews implementation quality.",
      avatarColor: "violet",
      model: { kind: "route", route: "free" },
      builtin: false,
      skillCount: 1,
      toolCount: 3,
      knowledgeCount: 12,
      executable: true,
      validation: [],
      isDefault: false,
    },
    permissionRules: [],
    skills: ["requesting-code-review"],
    nativeTools: [
      { tool: "read", decision: "allow" },
      { tool: "grep", decision: "allow" },
      { tool: "bash", decision: "allow" },
    ],
    pluginTools: [],
    apps: [],
    modelInfo: routeInfo,
    personality: { preset: "helpful", custom: null },
    ...overrides,
  };
}

const registry: AgentRegistryInfo = {
  agents: [detail().summary, { ...detail().summary, id: "ryuzi", name: "Ryuzi", isDefault: true }],
  defaultAgentId: "ryuzi",
  recovery: [],
  subagentModel: { kind: "route", route: "free" },
};

const learningSnapshot: AgentLearningInfo = {
  concepts: [],
  invalid: [],
  journey: [],
  skillUsage: [],
  reviews: [],
  curator: { concept: null, lastEventId: null },
  curatorHistory: [],
};

function seed(value = detail()) {
  useAgents.setState({
    registry,
    detail: value,
    models: [routeInfo, opusInfo, miniInfo],
    loaded: true,
    loading: false,
    saving: false,
    recentSessionsByAgent: {},
  });
  useNav.setState({ history: { back: [], current: { kind: "agentDetail", agentId: "reviewer" }, forward: [] } });
}

beforeEach(() => {
  listAgentSessions.mockClear();
  listAgentSessions.mockResolvedValue({ status: "ok", data: [] });
  getAgentConfigurationCatalog.mockClear();
  useAgentConfigurationCatalog.setState({ catalog: null, loading: false, error: null });
  deleteAgent.mockClear();
  duplicateAgent.mockClear();
  listApps.mockClear();
  updateAgent.mockClear();
  updateSubagentModel.mockClear();
  useApps.setState({ apps: [], loaded: false, hydrating: false, probing: null });
  useLearning.setState({
    byAgent: { reviewer: learningSnapshot },
    loading: {},
    rollingBack: {},
    requestGeneration: {},
  });
  seed();
});
afterEach(cleanup);

test("management flow inspects, duplicates, starts chat with, and deletes through the generated command store", async () => {
  const { unmount } = render(<AgentDetailView agentId="reviewer" />);
  expect(screen.getByRole("heading", { name: "Reviewer" })).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "Actions for Reviewer" }));
  fireEvent.click(screen.getByRole("button", { name: "Duplicate" }));
  await waitFor(() => expect(duplicateAgent).toHaveBeenCalledWith(LOCAL_RUNNER, "reviewer"));
  await waitFor(() => expect(useNav.getState().history.current).toEqual({ kind: "agentDetail", agentId: "reviewer-copy" }));

  unmount();
  seed(detail({ summary: { ...detail().summary, id: "reviewer-copy", name: "Reviewer Copy" } }));
  render(<AgentDetailView agentId="reviewer-copy" />);
  fireEvent.click(screen.getByRole("button", { name: "Actions for Reviewer Copy" }));
  fireEvent.click(screen.getByRole("button", { name: "Start chat" }));
  expect(useNav.getState().pendingPrimaryAgentId).toBe("reviewer-copy");
  expect(useNav.getState().history.current).toEqual({ kind: "home" });

  fireEvent.click(screen.getByRole("button", { name: "Actions for Reviewer Copy" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete" }));
  await screen.findByRole("dialog", { name: "Delete Reviewer Copy?" });
  fireEvent.click(screen.getByRole("button", { name: "Delete agent" }));
  await waitFor(() => expect(deleteAgent).toHaveBeenCalledWith(LOCAL_RUNNER, "reviewer-copy"));
});

test("detail header delete navigates back only after success", async () => {
  useNav.setState({ history: { back: [{ kind: "agents" }], current: { kind: "agentDetail", agentId: "reviewer" }, forward: [] } });
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Actions for Reviewer" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete" }));
  fireEvent.click(await screen.findByRole("button", { name: "Delete agent" }));

  await waitFor(() => expect(useNav.getState().history.current).toEqual({ kind: "agents" }));
});

test("detail header delete failure keeps the detail and confirmation open", async () => {
  deleteAgent.mockResolvedValueOnce({ status: "error", error: { message: "delete rejected" } });
  useNav.setState({ history: { back: [{ kind: "agents" }], current: { kind: "agentDetail", agentId: "reviewer" }, forward: [] } });
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Actions for Reviewer" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete" }));
  fireEvent.click(await screen.findByRole("button", { name: "Delete agent" }));

  await waitFor(() => expect(deleteAgent).toHaveBeenCalledWith(LOCAL_RUNNER, "reviewer"));
  expect(useNav.getState().history.current).toEqual({ kind: "agentDetail", agentId: "reviewer" });
  expect(screen.getByRole("dialog", { name: "Delete Reviewer?" })).toBeTruthy();
});

test("Advanced delete uses the same success-only detail navigation", async () => {
  useNav.setState({ history: { back: [], current: { kind: "agentDetail", agentId: "reviewer" }, forward: [] } });
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Advanced" }));
  fireEvent.click(screen.getByRole("button", { name: "Delete Reviewer" }));
  fireEvent.click(await screen.findByRole("button", { name: "Delete agent" }));

  await waitFor(() => expect(useNav.getState().history.current).toEqual({ kind: "agents" }));
});

test("detail has Back, identity, actions, seven tabs, and overview metrics", () => {
  render(<AgentDetailView agentId="reviewer" />);
  expect(screen.getByRole("button", { name: "Back" })).toBeTruthy();
  expect(screen.getByRole("heading", { name: "Reviewer" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Actions for Reviewer" })).toBeTruthy();
  const tabs = screen.getByTestId("agent-detail-tabs");
  expect(
    within(tabs)
      .getAllByRole("button")
      .map((button) => button.textContent),
  ).toEqual(["Overview", "Model", "Permissions", "Skills", "Apps & MCP", "Learning", "Advanced"]);
  expect(screen.getByText("12 readable concepts")).toBeTruthy();
  expect(screen.getByText("1 enabled skill")).toBeTruthy();
  expect(screen.getByText("3 enabled tools")).toBeTruthy();
  expect(screen.getByText("No owned sessions yet.")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Start chat" })).toBeNull();
});

test("Back uses navigation history", () => {
  useNav.setState({ history: { back: [{ kind: "models" }], current: { kind: "agentDetail", agentId: "reviewer" }, forward: [] } });
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Back" }));
  expect(useNav.getState().history.current).toEqual({ kind: "models" });
});

test("concrete model renders resolver-supported effort values and route has no effort", async () => {
  const concrete = detail({
    summary: { ...detail().summary, model: { kind: "concrete", name: opusInfo.requestValue, effort: "high" } },
    modelInfo: opusInfo,
  });
  seed(concrete);
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Model" }));
  fireEvent.click(screen.getByRole("combobox", { name: "Agent effort" }));
  for (const label of ["Model default", "Low", "Medium", "High", "Max", "XHigh"]) {
    expect(await screen.findByRole("option", { name: label })).toBeTruthy();
  }
  cleanup();
  seed();
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Model" }));
  expect(screen.queryByRole("combobox", { name: "Agent effort" })).toBeNull();
});

test("adding a prefix rule under Bash autosaves it immediately (no Save button)", async () => {
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Permissions" }));
  const bashRow = await screen.findByTestId("tool-row-bash");
  fireEvent.click(within(bashRow).getByRole("button", { name: "Expand Bash prefix rules" }));
  fireEvent.click(screen.getByRole("button", { name: "＋ Add prefix rule" }));
  fireEvent.change(screen.getByRole("textbox", { name: "New prefix rule for Bash" }), { target: { value: "cargo test" } });
  fireEvent.click(screen.getByRole("button", { name: "Confirm new rule" }));
  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith(
      LOCAL_RUNNER,
      "reviewer",
      expect.objectContaining({
        permissionRules: [expect.objectContaining({ tool: "bash", decision: "allow", commandPrefix: "cargo test" })],
      }),
    ),
  );
});

test("permission rules for tools no longer in the catalog are preserved untouched across unrelated autosaves", async () => {
  seed(detail({ permissionRules: [{ id: "custom-rule", tool: "plugin__acme__deploy", decision: "deny", commandPrefix: null }] }));
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Permissions" }));
  const readRow = await screen.findByTestId("tool-row-read");
  fireEvent.click(within(readRow).getByRole("button", { name: "Off" }));
  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith(
      LOCAL_RUNNER,
      "reviewer",
      expect.objectContaining({
        permissionRules: [{ id: "custom-rule", tool: "plugin__acme__deploy", decision: "deny", commandPrefix: null }],
      }),
    ),
  );
});

test("model transitions preserve supported effort, clear unsupported effort, and autosave a complete mutation (no Save button)", async () => {
  const concrete = detail({
    summary: { ...detail().summary, model: { kind: "concrete", name: opusInfo.requestValue, effort: "high" } },
    modelInfo: opusInfo,
  });
  seed(concrete);
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Model" }));
  expect(screen.queryByRole("button", { name: "Save model" })).toBeNull();

  fireEvent.click(screen.getByRole("combobox", { name: "Agent model" }));
  fireEvent.click(await screen.findByRole("option", { name: miniInfo.requestValue }));
  expect((screen.getByRole("combobox", { name: "Agent effort" }) as HTMLButtonElement).textContent).toContain("Model default");

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith(LOCAL_RUNNER, "reviewer", {
      name: "Reviewer",
      description: "Reviews implementation quality.",
      avatarColor: "violet",
      model: { kind: "concrete", name: miniInfo.requestValue, effort: null },
      personality: { preset: "helpful", custom: null },
      permissionRules: [],
      skills: ["requesting-code-review"],
      nativeTools: [
        { tool: "read", decision: "allow" },
        { tool: "grep", decision: "allow" },
        { tool: "bash", decision: "allow" },
      ],
      pluginTools: [],
      apps: [],
    }),
  );
});

test("Overview loads owned sessions and opens a selected session", async () => {
  listAgentSessions.mockResolvedValue({ status: "ok", data: [recentSession()] });
  render(<AgentDetailView agentId="reviewer" />);

  expect(await screen.findByRole("button", { name: /Preserve immutable ownership/ })).toBeTruthy();
  expect(listAgentSessions).toHaveBeenCalledWith(LOCAL_RUNNER, "reviewer", 10);

  fireEvent.click(screen.getByRole("button", { name: /Preserve immutable ownership/ }));
  expect(useStore.getState().focusedSession).toEqual({ runnerId: LOCAL_RUNNER, pk: "s1" });
  expect(useNav.getState().history.current).toEqual({ kind: "session" });
});

test("changing agent resets the local tab to Overview", async () => {
  const { rerender } = render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Permissions" }));
  expect(await screen.findByRole("textbox", { name: "Search tools" })).toBeTruthy();

  const ryuzi = detail({ summary: { ...detail().summary, id: "ryuzi", name: "Ryuzi", isDefault: true } });
  act(() => {
    useAgents.setState({ detail: ryuzi });
    rerender(<AgentDetailView agentId="ryuzi" />);
  });
  await waitFor(() => expect(screen.getByText("Recent sessions")).toBeTruthy());
  expect(screen.queryByRole("textbox", { name: "Search tools" })).toBeNull();
});

test("Skills tab renders the pack-grouped skills settings", async () => {
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Skills" }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Search skills" })).toBeTruthy());
  expect(screen.getByTestId("skill-group-Standalone")).toBeTruthy();
  expect(screen.queryByTestId("app-card-github")).toBeNull();
});

test("Apps & MCP tab renders the apps settings", async () => {
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Apps & MCP" }));
  await waitFor(() => expect(screen.getByTestId("app-card-github")).toBeTruthy());
  expect(screen.queryByRole("textbox", { name: "Search skills" })).toBeNull();
});

test("Learning renders the selected agent's Learning tab", () => {
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Learning" }));
  expect(screen.getByRole("button", { name: "Add memory" })).toBeTruthy();
  expect(screen.getByText("No memory concepts yet.")).toBeTruthy();
});

function freshDetail(overrides: Partial<AgentDetailInfo> = {}): AgentDetailInfo {
  return detail({
    summary: {
      ...detail().summary,
      id: "fresh",
      name: "Fresh Agent",
      description: "Ephemeral, memoryless worker dispatched for delegated tasks.",
      avatarColor: "slate",
      builtin: true,
      skillCount: 0,
      toolCount: 0,
      knowledgeCount: 0,
      isDefault: false,
    },
    permissionRules: [],
    skills: [],
    nativeTools: [],
    pluginTools: [],
    apps: [],
    personality: { preset: "helpful", custom: null },
    ...overrides,
  });
}

test("Fresh Agent detail renders only the header and the shared model editor", () => {
  seed(freshDetail());
  render(<AgentDetailView agentId="fresh" />);

  expect(screen.getByRole("heading", { name: "Fresh Agent" })).toBeTruthy();
  expect(screen.getByText("Built-in")).toBeTruthy();
  expect(screen.queryByTestId("agent-detail-tabs")).toBeNull();
  expect(screen.queryByRole("button", { name: "Actions for Fresh Agent" })).toBeNull();
  expect(screen.queryByText("Executable")).toBeNull();
  expect(screen.queryByText("Invalid")).toBeNull();
  expect(screen.queryByRole("combobox", { name: "Personality preset" })).toBeNull();
  expect(screen.queryByRole("textbox", { name: "Search tools" })).toBeNull();
  expect(screen.getByRole("combobox", { name: "Agent model" })).toBeTruthy();
});

test("Fresh Agent model change autosaves via updateSubagentModel, not updateAgent", async () => {
  seed(freshDetail());
  render(<AgentDetailView agentId="fresh" />);

  fireEvent.click(screen.getByRole("combobox", { name: "Agent model" }));
  fireEvent.click(await screen.findByRole("option", { name: miniInfo.requestValue }));

  await waitFor(() =>
    expect(updateSubagentModel).toHaveBeenCalledWith(LOCAL_RUNNER, { kind: "concrete", name: miniInfo.requestValue, effort: null }),
  );
  expect(updateAgent).not.toHaveBeenCalled();
});
