import { expect, mock, test } from "bun:test";
import type { JobInfo } from "@/bindings";

mock.module("@/bindings", () => ({
  commands: {},
  events: { coreEventMsg: { listen: async () => () => {} } },
}));

const { duplicateJobInput, jobNeedsTarget } = await import("./store-scheduler");

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

test("jobNeedsTarget is true only for an empty (whitespace-only) project id", () => {
  expect(jobNeedsTarget(makeJob({ projectId: "" }))).toBe(true);
  expect(jobNeedsTarget(makeJob({ projectId: "   " }))).toBe(true);
  expect(jobNeedsTarget(makeJob({ projectId: "proj-1" }))).toBe(false);
});

test("duplicateJobInput copies every editable field, suffixes the name, and drops id/pluginId", () => {
  const job = makeJob({ pluginId: "acme" });
  const input = duplicateJobInput(job);

  expect(input).toEqual({
    name: "Nightly triage (copy)",
    mode: "cron",
    natural: "",
    cron: "0 2 * * *",
    projectId: "proj-1",
    branch: "main",
    gateway: "local",
    prompt: "Triage new issues and open a summary PR.",
    notifySuccess: true,
    notifyFail: false,
  });
  expect("id" in input).toBe(false);
  expect("pluginId" in input).toBe(false);
});
