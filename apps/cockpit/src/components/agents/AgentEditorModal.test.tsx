import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { AgentDetailInfo, AgentModelInfo, AgentMutationInfo, AgentRegistryInfo } from "@/bindings";
import { __resetBundledPetsCacheForTests } from "@/lib/bundled-pets";

const route = (value: string): AgentModelInfo => ({ kind: "route", route: value });

const createAgent = mock(async (_runnerId: string | null, input: AgentMutationInfo) => ({
  status: "ok" as const,
  data: {
    summary: {
      id: "reviewer",
      name: input.name,
      description: input.description,
      avatarColor: input.avatarColor,
      avatarPet: input.avatarPet,
      model: input.model,
      builtin: false,
      skillCount: 0,
      toolCount: 0,
      knowledgeCount: 0,
      executable: true,
      validation: [],
      isDefault: false,
    },
    permissionRules: [],
    skills: [],
    nativeTools: [],
    pluginTools: [],
    apps: [],
    modelInfo: null,
    personality: input.personality,
  } satisfies AgentDetailInfo,
}));

mock.module("@/bindings", () => ({ commands: { createAgent }, events: {} }));

const { AgentEditorModal } = await import("./AgentEditorModal");
const { useAgents } = await import("@/store-agents");

const registry: AgentRegistryInfo = {
  agents: [],
  defaultAgentId: "ryuzi",
  recovery: [],
  subagentModel: route("free"),
};

const originalFetch = globalThis.fetch;

function jsonResponse(body: unknown): Response {
  return { ok: true, json: () => Promise.resolve(body) } as Response;
}

beforeEach(() => {
  __resetBundledPetsCacheForTests();
  createAgent.mockClear();
  useAgents.setState({
    registry,
    detail: null,
    models: [],
    loaded: true,
    loading: false,
    saving: false,
  });
  globalThis.fetch = mock(() =>
    Promise.resolve(jsonResponse([{ slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." }])),
  ) as unknown as typeof fetch;
});

afterEach(() => {
  cleanup();
  globalThis.fetch = originalFetch;
});

test("associates accessible names with every create field", () => {
  render(<AgentEditorModal open onClose={() => {}} />);

  expect(screen.getByRole("textbox", { name: "Name" })).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "Description" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Avatar color" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Choose a pet" })).toBeTruthy();
});

test("choosing a bundled pet previews it and is included in the create payload", async () => {
  render(<AgentEditorModal open onClose={() => {}} />);

  fireEvent.click(screen.getByRole("button", { name: "Choose a pet" }));
  await screen.findByText("Sprout");
  fireEvent.click(screen.getByRole("button", { name: /Sprout/i }));

  expect(await screen.findByRole("button", { name: "Change pet" })).toBeTruthy();

  fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Reviewer" } });
  fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "Reviews changes" } });
  fireEvent.click(screen.getByRole("button", { name: "Create" }));

  await waitFor(() => expect(createAgent).toHaveBeenCalledTimes(1));
  expect(createAgent.mock.calls[0]?.[1]).toEqual(expect.objectContaining({ avatarPet: "sprout" }));
});
