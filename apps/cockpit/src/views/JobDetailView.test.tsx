import { afterEach, expect, mock, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { CmdError, GatewayInfo, JobInfo, JobInput, PluginInfo, Project, Result, RunInfo } from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { useNav } from "@/store-nav";

// Mock the Tauri boundary before the view (and the stores it pulls in) load.
let seededJobs: JobInfo[] = [];
const ok = (jobs: JobInfo[]): Result<JobInfo[], CmdError> => ({ status: "ok", data: jobs });

const listJobs = mock(async () => ok(seededJobs));
const updateJob = mock(async () => ok(seededJobs));
const toggleJob = mock(async () => ok(seededJobs));
const deleteJob = mock(async () => ok([]));
const runJobNow = mock(async () => ok(seededJobs));
const createJob = mock<(runner: string, input: JobInput) => Promise<Result<JobInfo[], CmdError>>>(async () => ok(seededJobs));
const parseNaturalSchedule = mock(async () => null);

mock.module("@/bindings", () => ({
  commands: { listJobs, updateJob, toggleJob, deleteJob, runJobNow, createJob, parseNaturalSchedule },
  events: { coreEventMsg: { listen: async () => () => {} } },
}));

const { JobDetailView } = await import("./JobDetailView");
const { useScheduler } = await import("@/store-scheduler");
const { useGateways } = await import("@/store-gateways");
const { usePlugins } = await import("@/store-plugins");
const { useStore } = await import("@/store");

const acmePlugin: PluginInfo = {
  id: "acme",
  name: "Acme",
  description: "Acme integration",
  icon: null,
  categories: [],
  slot: null,
  ownsSlot: false,
  verified: true,
  experimental: false,
  enabled: true,
  source: "catalog",
  capabilities: [],
  configured: false,
  kind: "integration",
  installed: true,
  family: null,
  pinned: false,
  sourceSpec: null,
  resolvedCommit: null,
  installedAt: null,
  updatedAt: null,
  trustTier: null,
  catalogVersion: null,
  componentBacked: false,
  blockedReason: null,
  status: "ok",
  statusDetail: null,
  authKind: "none",
  toolCount: null,
  skillCount: null,
  surfaces: [],
  provenance: "catalog",
  trusted: true,
};

const project: Project = {
  projectId: "proj-1",
  name: "ryuzi",
  workdir: "C:/ryuzi",
  source: null,
  model: null,
  effort: null,
  permMode: "default",
  createdAt: null,
  isGit: true,
};

const gateway: GatewayInfo = {
  id: "local",
  name: "Local host",
  badge: "L",
  kind: "local",
  detail: "",
  metaLine: "",
  status: "connected",
  latency: null,
  daemonVersion: "0.4.0",
  uptime: null,
  lastSeenMs: null,
  resources: [],
  fingerprint: null,
  fsMode: "full",
  paths: [],
};

function makeJob(overrides: Partial<JobInfo> = {}): JobInfo {
  return {
    id: "job-1",
    name: "Nightly triage",
    cron: "0 2 * * *",
    mode: "cron",
    natural: "",
    projectId: "proj-1",
    projectName: "ryuzi",
    branch: "main",
    gateway: "local",
    enabled: true,
    prompt: "Triage new issues and open a summary PR.",
    notifySuccess: true,
    notifyFail: false,
    nextRunMs: null,
    history: [],
    ...overrides,
  };
}

// Seeding loaded: true keeps the mount effect from re-hydrating over IPC.
function seed(jobs: JobInfo[]) {
  seededJobs = jobs;
  useScheduler.setState({ jobs, loaded: true });
  useGateways.setState({ gateways: [gateway] });
}

afterEach(() => {
  cleanup();
  runJobNow.mockClear();
  updateJob.mockClear();
  toggleJob.mockClear();
  deleteJob.mockClear();
  createJob.mockClear();
  usePlugins.setState({ plugins: [] });
  useStore.setState({ projects: [] });
});

test("renders the job identity, prompt, and target chips from store data", () => {
  seed([makeJob()]);
  render(<JobDetailView id="job-1" />);

  expect(screen.getByText("Nightly triage")).toBeTruthy();
  // Cron shows in the header pill and the schedule footer.
  expect(screen.getAllByText("0 2 * * *").length).toBeGreaterThanOrEqual(2);
  expect(screen.getByDisplayValue("Triage new issues and open a summary PR.")).toBeTruthy();
  // Ryuzi-only: no agent picker; the target chips are project/branch/gateway.
  expect(screen.queryByRole("button", { name: "Claude Code" })).toBeNull();
  expect(screen.getByText("ryuzi")).toBeTruthy();
  expect(screen.getByText("main")).toBeTruthy();
  expect(screen.getByText("Local host")).toBeTruthy();
});

test("renders section cards and reflects the enabled/notification switches", () => {
  seed([makeJob()]);
  render(<JobDetailView id="job-1" />);

  expect(screen.getByText("Prompt & target")).toBeTruthy();
  expect(screen.getByText("Schedule")).toBeTruthy();
  expect(screen.getByText("Notifications")).toBeTruthy();
  expect(screen.getByText("Run history")).toBeTruthy();
  expect(screen.getByRole("switch", { name: "Enabled" }).getAttribute("aria-checked")).toBe("true");
  expect(screen.getByRole("switch", { name: "Notify on success" }).getAttribute("aria-checked")).toBe("true");
  expect(screen.getByRole("switch", { name: "Notify on failure" }).getAttribute("aria-checked")).toBe("false");
});

test("shows the empty run-history state when the job has no runs", () => {
  seed([makeJob()]);
  render(<JobDetailView id="job-1" />);

  expect(screen.getByText("0 runs · 0 failed")).toBeTruthy();
  expect(screen.getByText(/No runs yet/)).toBeTruthy();
});

test("renders run history rows with status labels, notes, and errors", () => {
  const history: RunInfo[] = [
    {
      id: "run-1",
      status: "success",
      startedAtMs: Date.now() - 3_600_000,
      durationMs: 90_000,
      addLines: 12,
      delLines: 3,
      note: "Opened PR #42",
      error: null,
      sessionPk: "sess-1",
    },
    {
      id: "run-2",
      status: "failed",
      startedAtMs: Date.now() - 7_200_000,
      durationMs: 5_000,
      addLines: null,
      delLines: null,
      note: null,
      error: "agent exited with code 1",
      sessionPk: null,
    },
  ];
  seed([makeJob({ history })]);
  render(<JobDetailView id="job-1" />);

  expect(screen.getByText("2 runs · 1 failed")).toBeTruthy();
  expect(screen.getByText("Success")).toBeTruthy();
  expect(screen.getByText("Failed")).toBeTruthy();
  expect(screen.getByText(/Opened PR #42/)).toBeTruthy();
  expect(screen.getByText("agent exited with code 1")).toBeTruthy();
  expect(screen.getByText("1m 30s")).toBeTruthy();
  // Only the run with a session gets the jump-to-session action.
  expect(screen.getAllByRole("button", { name: "Open session" })).toHaveLength(1);
});

test("clicking Run now invokes the runJobNow command with the job id", async () => {
  seed([makeJob()]);
  render(<JobDetailView id="job-1" />);

  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Run now" }));
  });

  expect(runJobNow).toHaveBeenCalledTimes(1);
  expect(runJobNow).toHaveBeenCalledWith(LOCAL_RUNNER, "job-1");
});

test("a successful delete returns to Automations Scheduler", async () => {
  seed([makeJob()]);
  useNav.setState({ history: { back: [], current: { kind: "jobDetail", id: "job-1" }, forward: [] } });
  render(<JobDetailView id="job-1" />);

  await act(async () => fireEvent.click(screen.getByTitle("Delete job")));
  expect(useNav.getState().history.current).toEqual({ kind: "automations", tab: "scheduler" });
});

test("a failed delete keeps the job detail open", async () => {
  deleteJob.mockResolvedValueOnce({ status: "error", error: { message: "No connection" } });
  seed([makeJob()]);
  useNav.setState({ history: { back: [], current: { kind: "jobDetail", id: "job-1" }, forward: [] } });
  render(<JobDetailView id="job-1" />);

  await act(async () => fireEvent.click(screen.getByTitle("Delete job")));
  expect(useNav.getState().history.current).toEqual({ kind: "jobDetail", id: "job-1" });
});

test("shows a not-found placeholder for an unknown job id", () => {
  seed([makeJob()]);
  render(<JobDetailView id="missing" />);

  expect(screen.getByText("Job not found.")).toBeTruthy();
});

test("a branchless (non-git) job hides the branch chip", () => {
  seed([makeJob({ branch: "" })]);
  render(<JobDetailView id="job-1" />);
  expect(screen.getByText("ryuzi")).toBeTruthy();
  expect(screen.queryByText("main")).toBeNull();
});

test("a plugin-owned job shows its badge, hides Delete, locks its prompt/notifications, and keeps the enable switch live", () => {
  usePlugins.setState({ plugins: [acmePlugin] });
  seed([makeJob({ pluginId: "acme" })]);
  render(<JobDetailView id="job-1" />);

  expect(screen.getByText("Plugin: Acme")).toBeTruthy();
  expect(screen.queryByTitle("Delete job")).toBeNull();
  expect((screen.getByDisplayValue("Triage new issues and open a summary PR.") as HTMLTextAreaElement).disabled).toBe(true);
  expect((screen.getByRole("switch", { name: "Notify on success" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("switch", { name: "Notify on failure" }) as HTMLButtonElement).disabled).toBe(true);
  // The enable switch is the one thing a plugin-owned row's read-only rule
  // never touches.
  const enabled = screen.getByRole("switch", { name: "Enabled" }) as HTMLButtonElement;
  expect(enabled.disabled).toBe(false);
});

test("Duplicate as mine creates a suffixed, plugin-free copy and returns to the Scheduler list", async () => {
  usePlugins.setState({ plugins: [acmePlugin] });
  seed([makeJob({ pluginId: "acme" })]);
  useNav.setState({ history: { back: [], current: { kind: "jobDetail", id: "job-1" }, forward: [] } });
  render(<JobDetailView id="job-1" />);

  await act(async () => fireEvent.click(screen.getByRole("button", { name: "Duplicate as mine" })));

  expect(createJob).toHaveBeenCalledTimes(1);
  const input = createJob.mock.calls[0]?.[1];
  expect(input).toEqual(
    expect.objectContaining({
      name: "Nightly triage (copy)",
      projectId: "proj-1",
      branch: "main",
      prompt: "Triage new issues and open a summary PR.",
    }),
  );
  expect(Object.keys(input as object)).not.toContain("pluginId");
  expect(useNav.getState().history.current).toEqual({ kind: "automations", tab: "scheduler" });
});

test("a needs-target job's Project field is editable and patches the job on selection", async () => {
  usePlugins.setState({ plugins: [acmePlugin] });
  useStore.setState({ projects: [project] });
  seed([makeJob({ pluginId: "acme", projectId: "", projectName: "", branch: "" })]);
  render(<JobDetailView id="job-1" />);

  expect(screen.queryByText("ryuzi")).toBeNull();
  fireEvent.click(screen.getByRole("combobox", { name: "Project" }));
  const option = await screen.findByRole("option", { name: /ryuzi/ });
  await act(async () => fireEvent.click(option));

  expect(updateJob).toHaveBeenCalledWith(LOCAL_RUNNER, "job-1", expect.objectContaining({ projectId: "proj-1" }));
});
