import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { Project, WorktreeHookStatus } from "@/bindings";

let hookStatus: WorktreeHookStatus = { scripts: [], digest: null, trusted: false };

const worktreeHookStatus = mock(async (_runnerId: string | null, _projectId: string) => ({
  status: "ok" as const,
  data: hookStatus,
}));
const trustWorktreeHooks = mock(async (_runnerId: string | null, _projectId: string) => ({
  status: "ok" as const,
  data: { ...hookStatus, trusted: true },
}));

mock.module("@/bindings", () => ({
  commands: { worktreeHookStatus, trustWorktreeHooks },
  events: {},
}));

const { ProjectSettingsModal } = await import("./ProjectSettingsModal");
const { useStore } = await import("@/store");
const { useNav } = await import("@/store-nav");

const project: Project = {
  projectId: "p1",
  name: "Ryuzi",
  workdir: "C:\\code\\ryuzi",
  source: null,
  model: null,
  effort: null,
  permMode: "default",
  createdAt: 1,
  isGit: true,
};

beforeEach(() => {
  worktreeHookStatus.mockClear();
  trustWorktreeHooks.mockClear();
  hookStatus = { scripts: [], digest: null, trusted: false };
  useStore.setState({ projects: [project] });
  useNav.setState({ projectSettingsFor: "p1" });
});

afterEach(cleanup);

test("an untrusted hook set is listed with a warning and a Trust button", async () => {
  hookStatus = { scripts: ["tool.before/deny.sh"], digest: "abc123", trusted: false };
  render(<ProjectSettingsModal />);

  expect(await screen.findByText("Hook scripts")).toBeTruthy();
  expect(screen.getByText("tool.before/deny.sh")).toBeTruthy();
  expect(screen.getByText("These scripts have not been trusted and will not run. Review them before trusting.")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Trust these scripts" })).toBeTruthy();
});

test("trusting the set calls the command once and swaps in the trusted state", async () => {
  hookStatus = { scripts: ["tool.before/deny.sh"], digest: "abc123", trusted: false };
  render(<ProjectSettingsModal />);

  fireEvent.click(await screen.findByRole("button", { name: "Trust these scripts" }));

  await waitFor(() => expect(trustWorktreeHooks).toHaveBeenCalledTimes(1));
  expect(trustWorktreeHooks.mock.calls[0]?.[1]).toBe("p1");
  expect(await screen.findByText("Trusted. Editing any of these scripts will revoke this and stop them running.")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Trust these scripts" })).toBeNull();
});

test("a worktree with no hook scripts renders no Hook scripts section at all", async () => {
  render(<ProjectSettingsModal />);

  await waitFor(() => expect(worktreeHookStatus).toHaveBeenCalledTimes(1));
  expect(screen.queryByText("Hook scripts")).toBeNull();
});
