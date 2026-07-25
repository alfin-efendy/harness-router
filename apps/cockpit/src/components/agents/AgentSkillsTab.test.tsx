import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { AgentDetailInfo, AgentMutationInfo } from "@/bindings";
import { useAgentConfigurationCatalog } from "@/store-agent-catalog";

const getAgentConfigurationCatalog = mock(async () => ({
  status: "ok" as const,
  data: {
    skills: [
      {
        id: "requesting-code-review",
        label: "Requesting code review",
        description: "Review guidance",
        available: true,
        commandScoped: false,
        pack: "Code Review Pack",
      },
      {
        id: "receiving-code-review",
        label: "Receiving code review",
        description: "Receiving guidance",
        available: true,
        commandScoped: false,
        pack: "Code Review Pack",
      },
      {
        id: "systematic-debugging",
        label: "Systematic debugging",
        description: "Debugging guidance",
        available: true,
        commandScoped: false,
        pack: "Debug Pack",
      },
      {
        id: "legacy-debugging",
        label: "Legacy debugging",
        description: "No longer installed",
        available: false,
        commandScoped: false,
        pack: "Debug Pack",
      },
      { id: "another-skill", label: "Another skill", description: "Standalone skill", available: true, commandScoped: false, pack: null },
    ],
    nativeTools: [],
    pluginTools: [],
    apps: [],
  },
}));
mock.module("@/bindings", () => ({ commands: { getAgentConfigurationCatalog }, events: {} }));

const { AgentSkillsTab } = await import("./AgentSkillsTab");
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
    toolCount: 0,
    knowledgeCount: 0,
    executable: true,
    validation: [],
    isDefault: false,
  },
  permissionRules: [],
  skills: ["requesting-code-review"],
  nativeTools: [],
  pluginTools: [],
  apps: [],
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

test("groups catalog skills by pack, including a Standalone group for skills with no pack", async () => {
  render(<AgentSkillsTab detail={reviewerDetail} />);

  const codeReview = await screen.findByTestId("skill-group-Code Review Pack");
  expect(within(codeReview).getByText("Requesting code review")).toBeTruthy();
  expect(within(codeReview).getByText("Receiving code review")).toBeTruthy();

  const debug = screen.getByTestId("skill-group-Debug Pack");
  expect(within(debug).getByText("Systematic debugging")).toBeTruthy();

  const standalone = screen.getByTestId("skill-group-Standalone");
  expect(within(standalone).getByText("Another skill")).toBeTruthy();
});

test("group header shows enabled/total counts", async () => {
  render(<AgentSkillsTab detail={reviewerDetail} />);

  const codeReview = await screen.findByTestId("skill-group-Code Review Pack");
  expect(within(codeReview).getByText("1/2")).toBeTruthy();
});

test("Enable all fires update with every group id unioned into skills", async () => {
  render(<AgentSkillsTab detail={reviewerDetail} />);
  const codeReview = await screen.findByTestId("skill-group-Code Review Pack");

  fireEvent.click(within(codeReview).getByRole("button", { name: "Enable all" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith(
      "reviewer",
      expect.objectContaining({
        skills: expect.arrayContaining(["requesting-code-review", "receiving-code-review"]),
      }),
    ),
  );
  const [, mutation] = updateAgent.mock.calls[updateAgent.mock.calls.length - 1] as [string, AgentMutationInfo];
  expect(mutation.skills).toHaveLength(2);
});

test("Disable all fires update with every group id removed from skills, leaving unrelated skills untouched", async () => {
  render(<AgentSkillsTab detail={{ ...reviewerDetail, skills: ["requesting-code-review", "receiving-code-review", "another-skill"] }} />);
  const codeReview = await screen.findByTestId("skill-group-Code Review Pack");

  fireEvent.click(within(codeReview).getByRole("button", { name: "Disable all" }));

  await waitFor(() => expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ skills: ["another-skill"] })));
});

test("toggling an individual skill row fires update adding just that id", async () => {
  render(<AgentSkillsTab detail={reviewerDetail} />);
  const debug = await screen.findByTestId("skill-group-Debug Pack");

  fireEvent.click(within(debug).getByRole("switch", { name: "Enable skill systematic-debugging" }));

  await waitFor(() =>
    expect(updateAgent).toHaveBeenCalledWith(
      "reviewer",
      expect.objectContaining({ skills: ["requesting-code-review", "systematic-debugging"] }),
    ),
  );
});

test("toggling an enabled skill row fires update removing just that id", async () => {
  render(<AgentSkillsTab detail={reviewerDetail} />);
  const codeReview = await screen.findByTestId("skill-group-Code Review Pack");

  fireEvent.click(within(codeReview).getByRole("switch", { name: "Enable skill requesting-code-review" }));

  await waitFor(() => expect(updateAgent).toHaveBeenCalledWith("reviewer", expect.objectContaining({ skills: [] })));
});

test("search filters skills across all groups, hiding groups with no matches", async () => {
  render(<AgentSkillsTab detail={reviewerDetail} />);
  await screen.findByTestId("skill-group-Debug Pack");

  fireEvent.change(screen.getByRole("textbox", { name: "Search skills" }), { target: { value: "debug" } });

  expect(screen.getByTestId("skill-group-Debug Pack")).toBeTruthy();
  expect(screen.queryByTestId("skill-group-Code Review Pack")).toBeNull();
  expect(screen.queryByTestId("skill-group-Standalone")).toBeNull();
});

test("unavailable skill entries render marked unavailable and do not fire update when toggled", async () => {
  render(<AgentSkillsTab detail={reviewerDetail} />);
  const debug = await screen.findByTestId("skill-group-Debug Pack");
  expect(within(debug).getByText(/Legacy debugging \(unavailable\)/)).toBeTruthy();

  fireEvent.click(within(debug).getByRole("switch", { name: "Enable skill legacy-debugging" }));

  expect(updateAgent).not.toHaveBeenCalled();
});

test("no update fires from a row toggle while a save is already in flight", async () => {
  render(<AgentSkillsTab detail={reviewerDetail} />);
  const debug = await screen.findByTestId("skill-group-Debug Pack");
  useAgents.setState({ saving: true });

  fireEvent.click(within(debug).getByRole("switch", { name: "Enable skill systematic-debugging" }));

  expect(updateAgent).not.toHaveBeenCalled();
});
