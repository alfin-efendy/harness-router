import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { Project, WorktreeHookStatus, WorktreeHookTrustResult } from "@/bindings";

let hookStatus: WorktreeHookStatus = { scripts: [], digest: null, trusted: false };
let trustResult: (digest: string) => WorktreeHookTrustResult = () => ({
  outcome: "recorded",
  status: { ...hookStatus, trusted: true },
});

const worktreeHookStatus = mock(async (_runnerId: string | null, _projectId: string) => ({
  status: "ok" as const,
  data: hookStatus,
}));
const trustWorktreeHooks = mock(async (_runnerId: string | null, _projectId: string, digest: string) => ({
  status: "ok" as const,
  data: trustResult(digest),
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
  trustResult = () => ({ outcome: "recorded", status: { ...hookStatus, trusted: true } });
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

test("trusting the set sends the reviewed digest and swaps in the trusted state", async () => {
  hookStatus = { scripts: ["tool.before/deny.sh"], digest: "abc123", trusted: false };
  render(<ProjectSettingsModal />);

  fireEvent.click(await screen.findByRole("button", { name: "Trust these scripts" }));

  await waitFor(() => expect(trustWorktreeHooks).toHaveBeenCalledTimes(1));
  expect(trustWorktreeHooks.mock.calls[0]?.[1]).toBe("p1");
  // The digest that was DISPLAYED travels with the click — that binding is
  // what stops a script swapped in mid-review from being trusted.
  expect(trustWorktreeHooks.mock.calls[0]?.[2]).toBe("abc123");
  expect(await screen.findByText("Trusted. Editing any of these scripts will revoke this and stop them running.")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Trust these scripts" })).toBeNull();
});

test("a set that changed under review is refused, and the new scripts are shown to review again", async () => {
  hookStatus = { scripts: ["tool.before/lint.sh"], digest: "abc123", trusted: false };
  trustResult = () => ({
    outcome: "changed",
    status: { scripts: ["tool.before/lint.sh", "session.start/pwn.sh"], digest: "def456", trusted: false },
  });
  render(<ProjectSettingsModal />);

  fireEvent.click(await screen.findByRole("button", { name: "Trust these scripts" }));

  expect(
    await screen.findByText(
      "These scripts changed while you were reviewing them, so nothing was trusted. The list above is the new one — review it again before trusting.",
    ),
  ).toBeTruthy();
  expect(screen.getByText("session.start/pwn.sh")).toBeTruthy();
  // Still untrusted, and a second click carries the NEW digest.
  fireEvent.click(screen.getByRole("button", { name: "Trust these scripts" }));
  await waitFor(() => expect(trustWorktreeHooks).toHaveBeenCalledTimes(2));
  expect(trustWorktreeHooks.mock.calls[1]?.[2]).toBe("def456");
});

test("a worktree with no hook scripts renders no Hook scripts section at all", async () => {
  render(<ProjectSettingsModal />);

  await waitFor(() => expect(worktreeHookStatus).toHaveBeenCalledTimes(1));
  expect(screen.queryByText("Hook scripts")).toBeNull();
});
