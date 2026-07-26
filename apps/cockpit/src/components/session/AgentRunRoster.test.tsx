import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { AgentRun, AgentSummaryInfo } from "@/bindings";
import { __resetBundledPetsCacheForTests } from "@/lib/bundled-pets";

const getChildTranscript = mock(() => Promise.resolve({ status: "ok", data: [] }));
mock.module("@/bindings", () => ({ commands: { getChildTranscript } }));

import { useAgents } from "@/store-agents";
import { useDelegation, delegationSessionKey } from "@/store-delegation";

const { AgentRunRoster } = await import("./AgentRunRoster");

function agentSummary(id: string, overrides: Partial<AgentSummaryInfo> = {}): AgentSummaryInfo {
  return {
    id,
    name: id,
    description: "",
    avatarColor: "violet",
    avatarPet: null,
    model: { kind: "route", route: "free" },
    builtin: false,
    skillCount: 0,
    toolCount: 0,
    knowledgeCount: 0,
    executable: true,
    validation: [],
    isDefault: false,
    ...overrides,
  };
}

function run({ sourceToolCallId = null, dispatchIndex = null, ...overrides }: Partial<AgentRun> = {}): AgentRun {
  return {
    runId: "run-1",
    sessionPk: "s1",
    parentRunId: null,
    retryOf: null,
    sourceToolCallId,
    dispatchIndex,
    primaryAgentId: "lead",
    executingAgentId: "worker",
    executingAgentNameSnapshot: "Researcher",
    agentKind: "subagent",
    task: "Inspect the service logs",
    status: "running",
    startedAt: Date.now() - 2_000,
    finishedAt: null,
    toolCount: 3,
    resolvedModel: "model-a",
    resolvedEffort: "high",
    result: null,
    error: null,
    contextActiveTokens: null,
    contextUsableWindow: null,
    contextPercentLeft: null,
    contextWindow: null,
    cacheReadTokens: null,
    cacheCreationTokens: null,
    outputTokens: null,
    cost: null,
    ...overrides,
  };
}

beforeEach(() => useDelegation.setState({ bySession: {}, transcriptByRun: {}, selectedBySession: {} }));
afterEach(cleanup);

const originalFetch = globalThis.fetch;

test("shows the executing agent's pet, posed for the run status, instead of the Bot icon when it has one", async () => {
  __resetBundledPetsCacheForTests();
  globalThis.fetch = mock(() =>
    Promise.resolve({
      ok: true,
      json: () => Promise.resolve([{ slug: "sprout", displayName: "Sprout", submittedBy: null }]),
    } as Response),
  ) as unknown as typeof fetch;

  try {
    useAgents.setState({
      registry: {
        agents: [agentSummary("worker", { avatarPet: "sprout" })],
        defaultAgentId: "worker",
        recovery: [],
        subagentModel: { kind: "route", route: "free" },
      },
    });
    useDelegation.setState({
      bySession: { [delegationSessionKey("local", "s1")]: [run({ status: "running" })] },
    });

    render(<AgentRunRoster runnerId="local" sessionPk="s1" />);

    await waitFor(() => expect(screen.getByTestId("pet-sprite")).toBeTruthy());
    expect(document.querySelector("svg.lucide-bot")).toBeNull();
  } finally {
    globalThis.fetch = originalFetch;
    useAgents.setState({ registry: null });
  }
});

test("splits exactly queued/running into Active and terminal runs into Done", () => {
  useDelegation.setState({
    bySession: {
      [delegationSessionKey("local", "s1")]: [
        run({ runId: "queued", status: "queued" }),
        run({ runId: "active", status: "running" }),
        run({ runId: "done", status: "completed" }),
        run({ runId: "failed", status: "failed", error: "tool failed" }),
        run({ runId: "cancelled", status: "cancelled" }),
        run({ runId: "interrupted", status: "interrupted" }),
      ],
    },
  });

  render(<AgentRunRoster runnerId="local" sessionPk="s1" />);

  expect(screen.getByText("Active (2)")).toBeTruthy();
  expect(screen.getByText("Done (4)")).toBeTruthy();
  expect(screen.getAllByText("Subagent").length).toBeGreaterThan(0);
  expect(screen.getAllByText("3 tools").length).toBeGreaterThan(0);
  expect(screen.getByText("tool failed")).toBeTruthy();
});

test("labels a main delegate as Main agent", () => {
  useDelegation.setState({
    bySession: {
      [delegationSessionKey("local", "s1")]: [run({ agentKind: "main-delegate", executingAgentNameSnapshot: "Delegate" })],
    },
  });

  render(<AgentRunRoster runnerId="local" sessionPk="s1" />);

  expect(screen.getByText("Main agent")).toBeTruthy();
  expect(screen.queryByText("Subagent")).toBeNull();
});

test("selecting a roster card navigates to its full detail", () => {
  useDelegation.setState({ bySession: { [delegationSessionKey("local", "s1")]: [run()] } });

  render(<AgentRunRoster runnerId="local" sessionPk="s1" />);
  fireEvent.click(screen.getByRole("button", { name: /Researcher/i }));

  expect(useDelegation.getState().selectedBySession[delegationSessionKey("local", "s1")]).toBe("run-1");
});

test("keeps every retry attempt selectable in Done", () => {
  const first = run({ runId: "first", task: "Original investigation", status: "failed", error: "timed out" });
  const retry = run({ runId: "retry", retryOf: first.runId, task: "Retried investigation", status: "completed", result: "completed" });
  const key = delegationSessionKey("local", "s1");
  useDelegation.setState({ bySession: { [key]: [first, retry] } });

  render(<AgentRunRoster runnerId="local" sessionPk="s1" />);

  expect(screen.getByText("Done (2)")).toBeTruthy();
  expect(screen.getByText("Retry 2")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /Original investigation/i }));
  expect(useDelegation.getState().selectedBySession[key]).toBe("first");
  fireEvent.click(screen.getByRole("button", { name: /Retried investigation/i }));
  expect(useDelegation.getState().selectedBySession[key]).toBe("retry");
});
