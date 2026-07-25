import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { AgentDetailInfo, AgentMutationInfo } from "@/bindings";
import { useAgentConfigurationCatalog } from "@/store-agent-catalog";

mock.module("@/bindings", () => ({ commands: {}, events: {} }));

const { AgentPersonalityCard } = await import("./AgentPersonalityCard");
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
    skillCount: 1,
    toolCount: 3,
    knowledgeCount: 0,
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
  modelInfo: null,
  personality: { preset: "helpful", custom: null },
};

beforeEach(() => {
  updateAgent.mockClear();
  useAgents.setState({ saving: false, update: updateAgent });
  useAgentConfigurationCatalog.setState({ catalog: null, loading: false, error: null });
});
afterEach(() => {
  cleanup();
  useAgentConfigurationCatalog.setState({ catalog: null, loading: false, error: null });
});

test("switching to Custom reveals the textarea without autosaving a blank value; committing happens on blur", async () => {
  render(<AgentPersonalityCard detail={reviewerDetail} />);

  expect(screen.queryByRole("textbox", { name: "Custom personality" })).toBeNull();

  fireEvent.click(screen.getByRole("combobox", { name: "Personality preset" }));
  fireEvent.click(await screen.findByRole("option", { name: /^Custom/ }));

  const textarea = screen.getByRole("textbox", { name: "Custom personality" });
  expect(textarea).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Save personality" })).toBeNull();

  // Blank text: no autosave, even on blur.
  fireEvent.change(textarea, { target: { value: "  " } });
  fireEvent.blur(textarea);
  expect(updateAgent).not.toHaveBeenCalled();

  // Non-blank text: nothing persists on keystroke (change) alone…
  fireEvent.change(textarea, { target: { value: "## Tone\n\n- Cite line numbers" } });
  expect(screen.getByText("Markdown preview")).toBeTruthy();
  expect(screen.getByRole("heading", { name: "Tone" })).toBeTruthy();
  expect(screen.getByText("Cite line numbers")).toBeTruthy();
  expect(updateAgent).not.toHaveBeenCalled();

  // …only on blur.
  fireEvent.blur(textarea);
  expect(updateAgent).toHaveBeenCalledWith(
    "reviewer",
    expect.objectContaining({ personality: { preset: "custom", custom: "## Tone\n\n- Cite line numbers" } }),
  );
});

test("blurring the custom textarea with an unchanged value does not re-autosave", () => {
  render(<AgentPersonalityCard detail={{ ...reviewerDetail, personality: { preset: "custom", custom: "Stay terse and precise." } }} />);

  const textarea = screen.getByRole("textbox", { name: "Custom personality" });
  fireEvent.blur(textarea);
  expect(updateAgent).not.toHaveBeenCalled();
});

test("a non-custom preset hides the textarea, shows its description, and autosaves as soon as it's picked", async () => {
  render(<AgentPersonalityCard detail={reviewerDetail} />);

  expect(screen.queryByRole("textbox", { name: "Custom personality" })).toBeNull();
  expect(screen.getByText(/You are a helpful, direct assistant\./)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Save personality" })).toBeNull();
  expect(updateAgent).not.toHaveBeenCalled();

  fireEvent.click(screen.getByRole("combobox", { name: "Personality preset" }));
  fireEvent.click(await screen.findByRole("option", { name: /^Concise/ }));

  expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ personality: { preset: "concise", custom: null } }));
});
