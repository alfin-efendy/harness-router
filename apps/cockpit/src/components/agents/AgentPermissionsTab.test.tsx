import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { AgentDetailInfo, AgentMutationInfo } from "@/bindings";
import { useAgentConfigurationCatalog } from "@/store-agent-catalog";

const getAgentConfigurationCatalog = mock(async () => ({
  status: "ok" as const,
  data: {
    skills: [],
    nativeTools: [
      { id: "read", label: "Read", description: "Read files from disk", available: true, commandScoped: false },
      { id: "bash", label: "Bash", description: "Run shell commands", available: true, commandScoped: true },
      { id: "legacy_tool", label: "Legacy Tool", description: "No longer supported", available: false, commandScoped: false },
    ],
    pluginTools: [{ id: "github", label: "GitHub", description: "GitHub tools", available: true, commandScoped: false }],
    apps: [],
  },
}));
mock.module("@/bindings", () => ({ commands: { getAgentConfigurationCatalog }, events: {} }));

const { AgentPermissionsTab } = await import("./AgentPermissionsTab");
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
    toolCount: 2,
    knowledgeCount: 0,
    executable: true,
    validation: [],
    isDefault: false,
  },
  permissionRules: [
    { id: "rule-1", tool: "bash", decision: "allow", commandPrefix: "npm test" },
    { id: "rule-2", tool: "bash", decision: "deny", commandPrefix: "rm -rf" },
  ],
  skills: [],
  nativeTools: [{ tool: "bash", decision: "allow" }],
  pluginTools: [],
  apps: [],
  modelInfo: null,
  personality: { preset: "helpful", custom: null },
};

beforeEach(() => {
  getAgentConfigurationCatalog.mockClear();
  updateAgent.mockClear();
  useAgents.setState({ saving: false, update: updateAgent });
  useAgentConfigurationCatalog.setState({ catalog: null, loading: false, error: null });
});
afterEach(() => {
  cleanup();
  useAgentConfigurationCatalog.setState({ catalog: null, loading: false, error: null });
});

test("renders one row per catalog native tool with the right segment selected: explicit decisions and absent tools default to ask", async () => {
  render(<AgentPermissionsTab detail={reviewerDetail} />);

  const readRow = await screen.findByTestId("tool-row-read");
  expect(within(readRow).getByRole("button", { name: "Ask" }).getAttribute("aria-pressed")).toBe("true");

  const bashRow = screen.getByTestId("tool-row-bash");
  expect(within(bashRow).getByRole("button", { name: "Allow" }).getAttribute("aria-pressed")).toBe("true");
});

test("clicking a segment fires update with the changed decision, preserving unrelated tools and rules; no update fires while saving", async () => {
  render(<AgentPermissionsTab detail={reviewerDetail} />);
  const readRow = await screen.findByTestId("tool-row-read");

  fireEvent.click(within(readRow).getByRole("button", { name: "Allow" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith(
      "reviewer",
      expect.objectContaining({
        nativeTools: expect.arrayContaining([
          { tool: "bash", decision: "allow" },
          { tool: "read", decision: "allow" },
        ]),
        permissionRules: reviewerDetail.permissionRules,
      }),
    ),
  );

  // While a save is in flight, both the base-decision and rule-decision
  // Segmented controls must be disabled so a rapid click can't race the
  // in-flight mutation and clobber it.
  const callsBeforeSaving = updateAgent.mock.calls.length;
  useAgents.setState({ saving: true });
  await waitFor(() => expect(within(readRow).getByRole("button", { name: "Ask" }).hasAttribute("disabled")).toBe(true));

  fireEvent.click(within(readRow).getByRole("button", { name: "Ask" }));

  const bashRow = screen.getByTestId("tool-row-bash");
  fireEvent.click(within(bashRow).getByRole("button", { name: "Expand Bash prefix rules" }));
  const rule1 = screen.getByTestId("rule-row-rule-1");
  const denyButton = within(rule1).getByRole("button", { name: "Deny" });
  expect(denyButton.hasAttribute("disabled")).toBe(true);
  fireEvent.click(denyButton);

  expect(updateAgent.mock.calls.length).toBe(callsBeforeSaving);
});

test("expanding Bash shows its prefix rules with their own Allow/Deny control", async () => {
  render(<AgentPermissionsTab detail={reviewerDetail} />);
  const bashRow = await screen.findByTestId("tool-row-bash");

  fireEvent.click(within(bashRow).getByRole("button", { name: "Expand Bash prefix rules" }));

  const rule1 = screen.getByTestId("rule-row-rule-1");
  expect(within(rule1).getByText("npm test")).toBeTruthy();
  expect(within(rule1).getByRole("button", { name: "Allow" }).getAttribute("aria-pressed")).toBe("true");
  expect(within(rule1).getByRole("button", { name: "Delete rule npm test" })).toBeTruthy();

  const rule2 = screen.getByTestId("rule-row-rule-2");
  expect(within(rule2).getByText("rm -rf")).toBeTruthy();
  expect(within(rule2).getByRole("button", { name: "Deny" }).getAttribute("aria-pressed")).toBe("true");
});

test("adding a prefix rule fires update with the appended rule", async () => {
  render(<AgentPermissionsTab detail={reviewerDetail} />);
  const bashRow = await screen.findByTestId("tool-row-bash");
  fireEvent.click(within(bashRow).getByRole("button", { name: "Expand Bash prefix rules" }));

  fireEvent.click(screen.getByRole("button", { name: "＋ Add prefix rule" }));
  fireEvent.change(screen.getByRole("textbox", { name: "New prefix rule for Bash" }), { target: { value: "git status" } });
  fireEvent.click(screen.getByRole("button", { name: "Confirm new rule" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith(
      "reviewer",
      expect.objectContaining({
        permissionRules: expect.arrayContaining([
          ...reviewerDetail.permissionRules,
          expect.objectContaining({ tool: "bash", decision: "allow", commandPrefix: "git status" }),
        ]),
      }),
    ),
  );
  const [, mutation] = updateAgent.mock.calls[updateAgent.mock.calls.length - 1] as [string, AgentMutationInfo];
  expect(mutation.permissionRules).toHaveLength(3);
});

test("deleting a prefix rule fires update without it", async () => {
  render(<AgentPermissionsTab detail={reviewerDetail} />);
  const bashRow = await screen.findByTestId("tool-row-bash");
  fireEvent.click(within(bashRow).getByRole("button", { name: "Expand Bash prefix rules" }));

  fireEvent.click(screen.getByRole("button", { name: "Delete rule npm test" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith(
      "reviewer",
      expect.objectContaining({
        permissionRules: [{ id: "rule-2", tool: "bash", decision: "deny", commandPrefix: "rm -rf" }],
      }),
    ),
  );
});

test("non-commandScoped rows have no expander", async () => {
  render(<AgentPermissionsTab detail={reviewerDetail} />);
  const readRow = await screen.findByTestId("tool-row-read");

  expect(within(readRow).queryByRole("button", { name: /prefix rules/ })).toBeNull();
});

test("unavailable catalog entries render disabled", async () => {
  render(<AgentPermissionsTab detail={reviewerDetail} />);
  const legacyRow = await screen.findByTestId("tool-row-legacy_tool");

  expect(within(legacyRow).getByText(/Legacy Tool/)).toBeTruthy();
  for (const label of ["Off", "Ask", "Allow"]) {
    expect(within(legacyRow).getByRole("button", { name: label }).hasAttribute("disabled")).toBe(true);
  }
});

test("search input filters the tool list", async () => {
  render(<AgentPermissionsTab detail={reviewerDetail} />);
  await screen.findByTestId("tool-row-bash");

  fireEvent.change(screen.getByRole("textbox", { name: "Search tools" }), { target: { value: "bash" } });

  expect(screen.getByTestId("tool-row-bash")).toBeTruthy();
  expect(screen.queryByTestId("tool-row-read")).toBeNull();
});
