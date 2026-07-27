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
    Promise.resolve(
      jsonResponse([
        { slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." },
        { slug: "boxcat", displayName: "Boxcat", submittedBy: "railly" },
      ]),
    ),
  ) as unknown as typeof fetch;
});

afterEach(() => {
  cleanup();
  globalThis.fetch = originalFetch;
});

test("associates accessible names with every create field — and there is no avatar color field", async () => {
  render(<AgentEditorModal open onClose={() => {}} />);

  expect(screen.getByRole("textbox", { name: "Name" })).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "Description" })).toBeTruthy();
  expect(screen.queryByRole("combobox", { name: "Avatar color" })).toBeNull();
  // The random prefill lands once the bundled roster resolves — the button
  // then reads "Change avatar", never "Choose an avatar".
  expect(await screen.findByRole("button", { name: "Change avatar" })).toBeTruthy();
});

test("prefills a random non-sprout bundled avatar and includes it in the create payload", async () => {
  render(<AgentEditorModal open onClose={() => {}} />);
  // With a [sprout, boxcat] roster and sprout reserved for the Fresh Agent,
  // boxcat is the only candidate — the "random" pick is deterministic here.
  await screen.findByRole("button", { name: "Change avatar" });

  fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Reviewer" } });
  fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "Reviews changes" } });
  fireEvent.click(screen.getByRole("button", { name: "Create" }));

  await waitFor(() => expect(createAgent).toHaveBeenCalledTimes(1));
  expect(createAgent.mock.calls[0]?.[1]).toEqual(expect.objectContaining({ avatarPet: "boxcat", avatarColor: "violet" }));
});

test("an explicit pick through the picker overrides the prefill", async () => {
  render(<AgentEditorModal open onClose={() => {}} />);
  fireEvent.click(await screen.findByRole("button", { name: "Change avatar" }));
  await screen.findByText("Sprout");
  fireEvent.click(screen.getByRole("button", { name: /Sprout/i }));

  fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Reviewer" } });
  fireEvent.change(screen.getByRole("textbox", { name: "Description" }), { target: { value: "Reviews changes" } });
  fireEvent.click(screen.getByRole("button", { name: "Create" }));

  await waitFor(() => expect(createAgent).toHaveBeenCalledTimes(1));
  expect(createAgent.mock.calls[0]?.[1]).toEqual(expect.objectContaining({ avatarPet: "sprout" }));
});
