import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type {
  AgentDetailInfo,
  AgentModelInfo,
  AgentMutationInfo,
  AgentRegistryInfo,
  AgentStatsLite,
  AgentSummaryInfo,
  SelectableModelInfo,
} from "@/bindings";
import { __resetBundledPetsCacheForTests } from "@/lib/bundled-pets";
import { LOCAL_RUNNER } from "@/lib/session-key";

const route = (r: string): AgentModelInfo => ({ kind: "route", route: r });

function summary(id: string, name: string, overrides: Partial<AgentSummaryInfo> = {}): AgentSummaryInfo {
  return {
    id,
    name,
    description: "",
    avatarColor: "violet",
    avatarPet: null,
    model: route("free"),
    builtin: false,
    skillCount: 0,
    toolCount: 0,
    knowledgeCount: 0,
    executable: true,
    validation: [],
    isDefault: id === "ryuzi",
    ...overrides,
  };
}

const reviewer = summary("reviewer", "Reviewer", {
  description: "Reviews implementation quality and regressions.",
  skillCount: 1,
  toolCount: 3,
});
const ryuzi = summary("ryuzi", "Ryuzi");
// Synthetic, non-registry row the backend always appends last (see
// fresh_agent_summary in crates/core/src/api/agent_api.rs) — ephemeral,
// memoryless, never invalid, never deletable.
const fresh = summary("fresh", "Fresh Agent", {
  description: "Ephemeral worker for one-off delegated work.",
  builtin: true,
  isDefault: false,
});

function registry(): AgentRegistryInfo {
  return { agents: [ryuzi, reviewer, fresh], defaultAgentId: "ryuzi", recovery: [], subagentModel: route("free") };
}

const selectable: SelectableModelInfo = {
  kind: "concrete",
  requestValue: "anthropic/claude-opus-4",
  displayName: "Claude Opus",
  preferenceKey: null,
  supported: [
    { value: "low", label: "Low", description: null },
    { value: "high", label: "High", description: null },
  ],
  configuredDefault: null,
  resolvedDefault: "high",
  defaultSource: "provider",
};

function detail(input: AgentMutationInfo): AgentDetailInfo {
  return {
    summary: summary(input.name.trim().toLowerCase().replace(/\s+/g, "-"), input.name, {
      description: input.description,
      avatarColor: input.avatarColor,
      model: input.model,
    }),
    permissionRules: input.permissionRules,
    skills: input.skills,
    nativeTools: input.nativeTools,
    pluginTools: input.pluginTools,
    apps: input.apps,
    modelInfo: null,
    personality: { preset: "helpful", custom: null },
  };
}

const createAgent = mock(async (_runnerId: string | null, input: AgentMutationInfo) => ({ status: "ok" as const, data: detail(input) }));
const updateSubagentModel = mock(async (_runnerId: string | null, model: AgentModelInfo) => ({
  status: "ok" as const,
  data: { ...registry(), subagentModel: model },
}));
const getAgentStatsBatch = mock(async (_runnerId: string | null, _agentIds: string[]) => ({
  status: "ok" as const,
  data: {} as Record<string, AgentStatsLite>,
}));

mock.module("@/bindings", () => ({
  commands: { createAgent, updateSubagentModel, getAgentStatsBatch },
  events: {},
}));

const { AgentsView } = await import("./AgentsView");
const { useAgents } = await import("@/store-agents");
const { useNav } = await import("@/store-nav");

const originalFetch = globalThis.fetch;

function seedAgents() {
  useAgents.setState({
    registry: registry(),
    detail: null,
    models: [selectable],
    loaded: true,
    loading: false,
    saving: false,
    statsByAgent: {},
    statsDetail: {},
  });
  useNav.setState({
    history: { back: [], current: { kind: "agents" }, forward: [] },
    pendingPrimaryAgentId: null,
  });
}

beforeEach(() => {
  createAgent.mockClear();
  updateSubagentModel.mockClear();
  getAgentStatsBatch.mockClear();
  getAgentStatsBatch.mockImplementation(async () => ({ status: "ok" as const, data: {} }));
  // The editor modal fetches the bundled-pet roster for its avatar prefill;
  // an EMPTY roster keeps the prefill inert so the strict create-payload
  // assertion below (avatarPet: null) stays deterministic.
  __resetBundledPetsCacheForTests();
  globalThis.fetch = mock(() => Promise.resolve({ ok: true, json: () => Promise.resolve([]) } as Response)) as unknown as typeof fetch;
  seedAgents();
});

afterEach(() => {
  cleanup();
  globalThis.fetch = originalFetch;
});

test("management flow creates through the generated command store and opens detail", async () => {
  useAgents.setState({ registry: { ...registry(), agents: [ryuzi] } });
  render(<AgentsView />);
  fireEvent.click(screen.getByRole("button", { name: "New agent" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Reviewer" } });
  fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "Reviews changes" } });
  fireEvent.click(screen.getByRole("button", { name: "Create" }));

  await waitFor(() =>
    expect(createAgent).toHaveBeenCalledWith(LOCAL_RUNNER, expect.objectContaining({ name: "Reviewer", description: "Reviews changes" })),
  );
  await waitFor(() => expect(useNav.getState().history.current).toEqual({ kind: "agentDetail", agentId: "reviewer" }));
});

test("renders roster metadata for every agent and opens a real agent's dedicated detail", () => {
  render(<AgentsView />);
  expect(screen.getByText("Reviews implementation quality and regressions.")).toBeTruthy();
  expect(screen.getAllByText("free").length).toBeGreaterThan(0);
  expect(screen.getByText("1 skill · 3 tools")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Open Reviewer" }));
  expect(useNav.getState().history.current).toEqual({ kind: "agentDetail", agentId: "reviewer" });
});

test("a row tile renders the agent's pet when it has one, and the plain color tile otherwise", async () => {
  const originalFetch = globalThis.fetch;
  __resetBundledPetsCacheForTests();
  globalThis.fetch = mock(() =>
    Promise.resolve({
      ok: true,
      json: () => Promise.resolve([{ slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." }]),
    } as Response),
  ) as unknown as typeof fetch;

  try {
    useAgents.setState({ registry: { ...registry(), agents: [ryuzi, summary("reviewer", "Reviewer", { avatarPet: "sprout" })] } });
    render(<AgentsView />);

    await waitFor(() => expect(screen.getByTestId("pet-sprite")).toBeTruthy());
    expect(screen.getByTestId("agent-avatar-color-tile")).toBeTruthy(); // ryuzi still has no pet
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("renders a single list — no Main/Sub segmented control and no subagent settings section", () => {
  render(<AgentsView />);
  expect(screen.queryByRole("button", { name: "Main Agent" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Sub Agent" })).toBeNull();
  expect(screen.queryByText(/ephemeral, memoryless runtime workers/)).toBeNull();
  expect(screen.queryByRole("combobox", { name: "Shared subagent model" })).toBeNull();
  // "New agent" is no longer tab-gated — it is always present.
  expect(screen.getByRole("button", { name: "New agent" })).toBeTruthy();
  expect(screen.getByText("Manage the agents available in this workspace.")).toBeTruthy();
});

test("pins the built-in Fresh Agent row last, with a Built-in badge, dashed styling, and no actions menu", () => {
  render(<AgentsView />);
  const openButtons = screen.getAllByRole("button", { name: /^Open / });
  expect(openButtons[openButtons.length - 1]?.getAttribute("aria-label")).toBe("Open Fresh Agent");

  const freshButton = screen.getByRole("button", { name: "Open Fresh Agent" });
  expect(freshButton.closest(".border-dashed")).toBeTruthy();
  expect(screen.getByText("Built-in")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Actions for Fresh Agent" })).toBeNull();
});

test("omits the Invalid badge for the built-in row even when it is marked non-executable", () => {
  useAgents.setState({ registry: { ...registry(), agents: [ryuzi, reviewer, { ...fresh, executable: false }] } });
  render(<AgentsView />);
  expect(screen.queryByText("Invalid")).toBeNull();
  expect(screen.getByText("Built-in")).toBeTruthy();
});

test("clicking the built-in Fresh Agent row navigates to its detail like any other row", () => {
  render(<AgentsView />);
  fireEvent.click(screen.getByRole("button", { name: "Open Fresh Agent" }));
  expect(useNav.getState().history.current).toEqual({ kind: "agentDetail", agentId: "fresh" });
});

test("create modal sends the complete initial mutation and opens the new detail", async () => {
  render(<AgentsView />);
  fireEvent.click(screen.getByRole("button", { name: "New agent" }));
  expect((screen.getByRole("button", { name: "Create" }) as HTMLButtonElement).disabled).toBe(true);

  fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "  Architect  " } });
  fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "  Designs system boundaries.  " } });
  fireEvent.click(screen.getByRole("button", { name: "Create" }));

  await waitFor(() =>
    expect(createAgent).toHaveBeenCalledWith(LOCAL_RUNNER, {
      name: "Architect",
      description: "Designs system boundaries.",
      avatarColor: "violet",
      avatarPet: null,
      model: route("free"),
      personality: { preset: "helpful", custom: null },
      permissionRules: [],
      skills: [],
      nativeTools: [],
      pluginTools: [],
      apps: [],
    }),
  );
  await waitFor(() => expect(useNav.getState().history.current).toEqual({ kind: "agentDetail", agentId: "architect" }));
});

test("row stats fragment renders only once lazily-loaded batch stats resolve, without blocking or reordering the list", async () => {
  getAgentStatsBatch.mockImplementationOnce(async () => ({
    status: "ok" as const,
    data: { reviewer: { sessionCount: 3, lastActive: Date.now() - 5 * 60_000, costUsd7d: 1.5 } },
  }));
  render(<AgentsView />);

  // The roster renders immediately, in its original order, before stats
  // resolve — and the batch load never touches the shared loading flag.
  const rowsBefore = screen.getAllByRole("button", { name: /^Open / }).map((button) => button.getAttribute("aria-label"));
  expect(rowsBefore).toEqual(["Open Ryuzi", "Open Reviewer", "Open Fresh Agent"]);
  expect(screen.queryByText(/sessions ·/)).toBeNull();
  expect(useAgents.getState().loading).toBe(false);

  await waitFor(() => expect(getAgentStatsBatch).toHaveBeenCalledWith(LOCAL_RUNNER, ["ryuzi", "reviewer"]));
  expect(await screen.findByText(/3 sessions · 5m ago · \$1\.50 7d/)).toBeTruthy();

  // Still the same order after stats land.
  const rowsAfter = screen.getAllByRole("button", { name: /^Open / }).map((button) => button.getAttribute("aria-label"));
  expect(rowsAfter).toEqual(rowsBefore);
});

test("the built-in Fresh Agent row is excluded from the stats batch call and never renders a fragment even if data existed for it", async () => {
  render(<AgentsView />);
  await waitFor(() => expect(getAgentStatsBatch).toHaveBeenCalledWith(LOCAL_RUNNER, ["ryuzi", "reviewer"]));

  // Simulate stray stats keyed under the built-in id (e.g. a hypothetical
  // future backend quirk) — the skip must be enforced by `agent.builtin`,
  // not merely by "did the batch response happen to omit it".
  act(() => {
    useAgents.setState((s) => ({
      statsByAgent: { ...s.statsByAgent, fresh: { sessionCount: 9, lastActive: Date.now(), costUsd7d: 9 } },
    }));
  });
  expect(screen.queryByText(/9 sessions/)).toBeNull();
});

test("a rejected stats batch load never crashes the roster render and leaves rows without a fragment", async () => {
  getAgentStatsBatch.mockImplementationOnce(() => Promise.reject(new Error("transport closed")));
  render(<AgentsView />);

  await waitFor(() => expect(getAgentStatsBatch).toHaveBeenCalled());
  expect(screen.getByText("Reviews implementation quality and regressions.")).toBeTruthy();
  expect(screen.queryByText(/sessions ·/)).toBeNull();
});

test("no list row has an actions menu — Start chat/Duplicate/Delete live on the detail page", () => {
  render(<AgentsView />);
  expect(screen.queryByRole("button", { name: /^Actions for/ })).toBeNull();
});
