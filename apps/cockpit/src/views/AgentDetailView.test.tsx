import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type {
  AgentDetailInfo,
  AgentLearningInfo,
  AgentModelInfo,
  AgentMutationInfo,
  AgentRegistryInfo,
  AgentConfigurationCatalogInfo,
  AgentStatsInfo,
  AgentSummaryInfo,
  CmdError,
  Result,
  SelectableModelInfo,
  Session,
} from "@/bindings";
import { __resetBundledPetsCacheForTests } from "@/lib/bundled-pets";
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
function statsInfo(overrides: Partial<AgentStatsInfo> = {}): AgentStatsInfo {
  return {
    sessionCount: 5,
    lastActive: 1_700_000_000_000,
    costUsd7d: 3.4,
    tokens7d: 12_345,
    runsTotal30d: 20,
    runsFailed30d: 2,
    topTools: [
      { tool: "read", count: 40, lastUsed: 1_700_000_000_000 },
      { tool: "bash", count: 12, lastUsed: 1_699_999_000_000 },
    ],
    ...overrides,
  };
}
const getAgentStats = mock(async (_runner: string | null, _id: string) => ({ status: "ok" as const, data: statsInfo() }));
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
    getAgentStats,
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
      avatarPet: null,
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
    statsByAgent: {},
    statsDetail: {},
  });
  useNav.setState({ history: { back: [], current: { kind: "agentDetail", agentId: "reviewer" }, forward: [] } });
}

beforeEach(() => {
  listAgentSessions.mockClear();
  listAgentSessions.mockResolvedValue({ status: "ok", data: [] });
  getAgentConfigurationCatalog.mockClear();
  useAgentConfigurationCatalog.setState({ catalog: null, loading: false, error: null });
  getAgentStats.mockClear();
  getAgentStats.mockImplementation(async () => ({ status: "ok" as const, data: statsInfo() }));
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

test("detail has Back, identity, actions, seven tabs, and loaded overview stat cards", async () => {
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

  // Activity / Cost · 7 days / Reliability, once the lazy stats load lands.
  expect(await screen.findByText("5 sessions")).toBeTruthy();
  expect(screen.getByText("$3.40")).toBeTruthy();
  expect(screen.getByText("12.3k tokens")).toBeTruthy();
  expect(screen.getByText("90%")).toBeTruthy();
  expect(screen.getByText("2 of 20 runs")).toBeTruthy();
  // Top-tools strip: "name ×count" for each of top_tools.
  expect(screen.getByText(/read ×40/)).toBeTruthy();
  expect(screen.getByText(/bash ×12/)).toBeTruthy();
  // "grep" is explicitly allowed (see the default `detail()` fixture) but
  // never appears in top_tools → the Consider Off hint names it by its
  // catalog label.
  expect(screen.getByText(/Consider Off/)).toBeTruthy();
  expect(screen.getByText(/Grep/)).toBeTruthy();

  expect(screen.getByText("No owned sessions yet.")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Start chat" })).toBeNull();
});

test("Overview stat cards render em-dash placeholders before stats resolve, never partial or crashed", async () => {
  let resolveStats!: (value: { status: "ok"; data: AgentStatsInfo }) => void;
  getAgentStats.mockReturnValueOnce(
    new Promise((resolve) => {
      resolveStats = resolve;
    }),
  );
  render(<AgentDetailView agentId="reviewer" />);

  // Only the stat-card session count ("N session(s)") is under test here —
  // "Recent sessions" (card title) and "No owned sessions yet." (its empty
  // state) both legitimately contain "sessions" and must not be mistaken
  // for a loaded Activity figure.
  expect(screen.queryByText(/^\d+ sessions?$/)).toBeNull();
  expect(screen.getAllByText("—").length).toBeGreaterThanOrEqual(6); // Activity ×2, Cost ×2, Reliability ×2

  resolveStats({ status: "ok", data: statsInfo() });
  expect(await screen.findByText("5 sessions")).toBeTruthy();
});

test("a fresh agent's all-zeroed stats render sane zero labels and an em-dash reliability, not crashes or NaN", async () => {
  getAgentStats.mockResolvedValueOnce({
    status: "ok",
    data: { sessionCount: 0, lastActive: null, costUsd7d: 0, tokens7d: 0, runsTotal30d: 0, runsFailed30d: 0, topTools: [] },
  });
  render(<AgentDetailView agentId="reviewer" />);

  expect(await screen.findByText("0 sessions")).toBeTruthy();
  expect(screen.getByText("$0.00")).toBeTruthy();
  expect(screen.getByText("0 tokens")).toBeTruthy();
  // Last-active (no activity) and both reliability figures (zero runs) fall
  // back to an em dash — "no data" must never render as "0%".
  expect(screen.getAllByText("—").length).toBeGreaterThanOrEqual(3);
  // No top tools and zero runs in the last 30 days → the whole "Tool usage"
  // card (including the Consider Off hint) is omitted, not shown empty.
  expect(screen.queryByText("Tool usage")).toBeNull();
  expect(screen.queryByText(/Consider Off/)).toBeNull();
});

test("Consider Off hint is suppressed entirely when the agent had zero runs in the trailing 30 days, even with an unused, explicitly-on tool", async () => {
  getAgentStats.mockResolvedValueOnce({
    status: "ok",
    data: statsInfo({ runsTotal30d: 0, runsFailed30d: 0, topTools: [] }),
  });
  render(<AgentDetailView agentId="reviewer" />);

  await screen.findByText("5 sessions"); // stats have landed
  expect(screen.queryByText(/Consider Off/)).toBeNull();
  expect(screen.queryByText(/Grep/)).toBeNull();
  // Reliability (percent + detail) is an em dash for zero runs, even though
  // sessionCount > 0.
  expect(screen.getAllByText("—")).toHaveLength(2);
});

test("the header avatar is a button that opens PetPicker; choosing a bundled pet persists it and preserves the rest of the mutation", async () => {
  const originalFetch = globalThis.fetch;
  __resetBundledPetsCacheForTests();
  globalThis.fetch = mock(() =>
    Promise.resolve({
      ok: true,
      json: () => Promise.resolve([{ slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." }]),
    } as Response),
  ) as unknown as typeof fetch;

  try {
    render(<AgentDetailView agentId="reviewer" />);
    fireEvent.click(screen.getByRole("button", { name: "Change Reviewer's pet" }));

    fireEvent.click(await screen.findByRole("button", { name: /Sprout/i }));

    await waitFor(() =>
      expect(updateAgent).toHaveBeenCalledWith(
        LOCAL_RUNNER,
        "reviewer",
        expect.objectContaining({
          avatarPet: "sprout",
          name: "Reviewer",
          description: "Reviews implementation quality.",
          skills: ["requesting-code-review"],
        }),
      ),
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("the built-in Fresh Agent header shows its pet but is not a clickable picker trigger", async () => {
  const originalFetch = globalThis.fetch;
  __resetBundledPetsCacheForTests();
  globalThis.fetch = mock(() =>
    Promise.resolve({
      ok: true,
      json: () => Promise.resolve([{ slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." }]),
    } as Response),
  ) as unknown as typeof fetch;

  try {
    seed(freshDetail({ summary: { avatarPet: "sprout" } }));
    render(<AgentDetailView agentId="fresh" />);

    await waitFor(() => expect(screen.getByTestId("pet-sprite")).toBeTruthy());
    expect(screen.queryByRole("button", { name: "Change Fresh Agent's pet" })).toBeNull();
  } finally {
    globalThis.fetch = originalFetch;
  }
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
      avatarPet: null,
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

test("reselecting the current model or effort fires no update; a genuine change fires exactly one", async () => {
  const concrete = detail({
    summary: { ...detail().summary, model: { kind: "concrete", name: opusInfo.requestValue, effort: "high" } },
    modelInfo: opusInfo,
  });
  seed(concrete);
  render(<AgentDetailView agentId="reviewer" />);
  fireEvent.click(screen.getByRole("button", { name: "Model" }));

  fireEvent.click(screen.getByRole("combobox", { name: "Agent model" }));
  fireEvent.click(await screen.findByRole("option", { name: opusInfo.requestValue }));

  fireEvent.click(screen.getByRole("combobox", { name: "Agent effort" }));
  fireEvent.click(await screen.findByRole("option", { name: "High" }));

  // The store queues the actual `commands.updateAgent` call on a microtask
  // (store-agents.ts's `enqueueMutation`: `mutationTail.then(operation, ...)`),
  // so asserting `not.toHaveBeenCalled()` synchronously right after the two
  // no-op reselects above can't tell a correctly-guarded no-op from a broken
  // guard whose deferred save just hasn't landed yet — both read as "not
  // called yet". Fire one GENUINE effort change and assert the call COUNT
  // once it lands: if either no-op above had (wrongly) queued a save, the
  // count would be 2 or 3 instead of 1, and the no-ops are proven innocent
  // only by that count staying at exactly 1.
  fireEvent.click(screen.getByRole("combobox", { name: "Agent effort" }));
  fireEvent.click(await screen.findByRole("option", { name: "Max" }));

  await waitFor(() => expect(updateAgent).toHaveBeenCalledTimes(1));
  expect(updateAgent).toHaveBeenCalledWith(
    LOCAL_RUNNER,
    "reviewer",
    expect.objectContaining({ model: { kind: "concrete", name: opusInfo.requestValue, effort: "max" } }),
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

function freshDetail(overrides: Omit<Partial<AgentDetailInfo>, "summary"> & { summary?: Partial<AgentSummaryInfo> } = {}): AgentDetailInfo {
  // `summary` is merged field-by-field (not overwritten wholesale) so a
  // caller can override just e.g. `model` without having to repeat every
  // fresh-specific base field (id/name/builtin/…) itself.
  const { summary: summaryOverrides, ...rest } = overrides;
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
      ...summaryOverrides,
    },
    permissionRules: [],
    skills: [],
    nativeTools: [],
    pluginTools: [],
    apps: [],
    personality: { preset: "helpful", custom: null },
    ...rest,
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

test("reselecting the Fresh Agent's current model or effort fires no updateSubagentModel; a genuine change fires exactly one", async () => {
  seed(
    freshDetail({
      summary: { model: { kind: "concrete", name: opusInfo.requestValue, effort: "high" } },
      modelInfo: opusInfo,
    }),
  );
  render(<AgentDetailView agentId="fresh" />);

  fireEvent.click(screen.getByRole("combobox", { name: "Agent model" }));
  fireEvent.click(await screen.findByRole("option", { name: opusInfo.requestValue }));

  fireEvent.click(screen.getByRole("combobox", { name: "Agent effort" }));
  fireEvent.click(await screen.findByRole("option", { name: "High" }));

  // Same count-hardened pattern as the regular-agent Model tab test above: a
  // synchronous "not called" check right after the no-op reselects can't
  // distinguish a correctly-guarded no-op from a broken guard whose deferred
  // `updateSubagentModel` call just hasn't landed yet (store-agents.ts queues
  // it via the same `enqueueMutation` microtask as `update`). Fire a genuine
  // change and assert the call count once it lands.
  fireEvent.click(screen.getByRole("combobox", { name: "Agent effort" }));
  fireEvent.click(await screen.findByRole("option", { name: "Max" }));

  await waitFor(() => expect(updateSubagentModel).toHaveBeenCalledTimes(1));
  expect(updateSubagentModel).toHaveBeenCalledWith(LOCAL_RUNNER, { kind: "concrete", name: opusInfo.requestValue, effort: "max" });
  expect(updateAgent).not.toHaveBeenCalled();
});
