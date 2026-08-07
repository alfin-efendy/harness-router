import { afterEach, expect, mock, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { CmdError, GatewayInfo, JobInfo, PluginInfo, Result } from "@/bindings";
import { useNav } from "@/store-nav";

let seededJobs: JobInfo[] = [];
const ok = (jobs: JobInfo[]): Result<JobInfo[], CmdError> => ({ status: "ok", data: jobs });
const listJobs = mock(async () => ok(seededJobs));
const toggleJob = mock(async () => ok(seededJobs));

mock.module("@/bindings", () => ({
  commands: { listJobs, toggleJob },
  events: { coreEventMsg: { listen: async () => () => {} } },
}));

const { SchedulerView } = await import("./SchedulerView");
const { useScheduler } = await import("@/store-scheduler");
const { useGateways } = await import("@/store-gateways");
const { usePlugins } = await import("@/store-plugins");

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

function seed(jobs: JobInfo[]) {
  seededJobs = jobs;
  useScheduler.setState({ jobs, loaded: true });
  useGateways.setState({ gateways: [gateway], loaded: true });
}

afterEach(() => {
  cleanup();
  useScheduler.setState({ jobs: [], loaded: false });
  useGateways.setState({ gateways: [], loaded: false });
  usePlugins.setState({ plugins: [] });
  useNav.setState({ history: { back: [], current: { kind: "automations", tab: "scheduler" }, forward: [] } });
  listJobs.mockClear();
  toggleJob.mockClear();
});

test("a normal job keeps its live enable switch", async () => {
  seed([makeJob()]);
  await act(async () => {
    render(<SchedulerView />);
  });

  expect(screen.getByText("Nightly triage")).toBeTruthy();
  expect(screen.getByRole("switch", { name: "Enable Nightly triage" }).getAttribute("aria-checked")).toBe("true");
  expect(screen.queryByRole("button", { name: "Set up…" })).toBeNull();
});

test("a plugin-owned job shows its badge, and a needs-target row offers Set up… instead of a blind toggle", async () => {
  usePlugins.setState({ plugins: [acmePlugin] });
  seed([makeJob({ pluginId: "acme", projectId: "", projectName: "" })]);
  await act(async () => {
    render(<SchedulerView />);
  });

  expect(screen.getByText("Plugin: Acme")).toBeTruthy();
  expect(screen.getByText("No project selected")).toBeTruthy();
  expect(screen.queryByRole("switch", { name: "Enable Nightly triage" })).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "Set up…" }));
  expect(useNav.getState().history.current).toEqual({ kind: "jobDetail", id: "job-1" });
});

test("a targetId deep link (from the plugin detail Automations tab) redirects straight into the job's detail view", async () => {
  seed([makeJob()]);
  await act(async () => {
    render(<SchedulerView targetJobId="job-1" />);
  });

  expect(useNav.getState().history.current).toEqual({ kind: "jobDetail", id: "job-1" });
});
