import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { act, cleanup, render, screen, within } from "@testing-library/react";
import type { CmdError, ComponentReleaseDetail, PluginDetail, PluginFieldInfo, Result } from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";

// The shell talks only to the Tauri IPC boundary (`@/bindings`) — mock it,
// same pattern as InstallWizardModal.test.tsx.

function field(key: string, label: string): PluginFieldInfo {
  return { key, label, help: "", secret: false, required: false, valueSet: false, kind: "string", options: [], default: null };
}

// Componented-backed connector with oauth auth + a declared setting, so the
// planned sequence is the full six steps: overview, permissions, install,
// connect, settings, done — enough surface to exercise Back/Skip/Continue
// and the progress segment count together.
function detailFixture(): PluginDetail {
  return {
    info: {
      id: "notion",
      name: "Notion",
      description: "Notion MCP",
      icon: null,
      categories: ["docs"],
      slot: null,
      ownsSlot: false,
      verified: true,
      experimental: false,
      enabled: false,
      configured: false,
      source: "catalog",
      capabilities: ["connector"],
      kind: "integration",
      installed: false,
      family: null,
      pinned: false,
      sourceSpec: null,
      resolvedCommit: null,
      installedAt: null,
      updatedAt: null,
      trustTier: null,
      catalogVersion: null,
      componentBacked: true,
      blockedReason: null,
      status: "not-installed",
      statusDetail: null,
      authKind: "oauth",
      toolCount: null,
      skillCount: null,
    },
    auth: {
      kind: "oauth",
      setting: null,
      env: null,
      helpUrl: null,
      configured: false,
      oauthConnectAvailable: true,
      oauthConnectError: null,
      oauthTokenStored: false,
      oauthReconnectRequired: false,
    },
    settings: [field("plugin.notion.workspace", "Workspace")],
    mcp: [],
    models: [],
    homepage: null,
    publisher: "Notion",
  };
}

function releaseFixture(): ComponentReleaseDetail {
  return { pluginId: "notion", releases: [], activeVersion: null, activeManifest: null };
}

// Provider-shaped fixture with no permissions gate (not component-backed),
// no settings, and no oauth requirement other than the provider-kind rule
// itself — planWizardSteps collapses this down to overview/install/connect/
// done (4 steps), exercising the shell against a plan shorter than the
// full six-step fixture every other test in this file uses.
function providerDetailFixture(): PluginDetail {
  return {
    info: {
      id: "openai",
      name: "OpenAI",
      description: "OpenAI provider",
      icon: null,
      categories: ["llm"],
      slot: null,
      ownsSlot: false,
      verified: true,
      experimental: false,
      enabled: false,
      configured: false,
      source: "catalog",
      capabilities: ["provider"],
      kind: "provider",
      installed: false,
      family: "openai",
      pinned: false,
      sourceSpec: null,
      resolvedCommit: null,
      installedAt: null,
      updatedAt: null,
      trustTier: null,
      catalogVersion: null,
      componentBacked: false,
      blockedReason: null,
      status: "not-installed",
      statusDetail: null,
      authKind: "none",
      toolCount: null,
      skillCount: null,
    },
    auth: null,
    settings: [],
    mcp: [],
    models: [],
    homepage: null,
    publisher: "OpenAI",
  };
}

const ok = <T,>(data: T) => Promise.resolve({ status: "ok" as const, data });

let detailData: PluginDetail = detailFixture();
let releaseData: ComponentReleaseDetail = releaseFixture();

const pluginDetail = mock((_runnerId: string | null, _id: string): Promise<Result<PluginDetail, CmdError>> => ok(detailData));
const pluginReleaseDetail = mock(
  (_runnerId: string | null, _id: string): Promise<Result<ComponentReleaseDetail, CmdError>> => ok(releaseData),
);
const toastError = mock((_message: string) => {});

mock.module("@/bindings", () => ({
  commands: { pluginDetail, pluginReleaseDetail },
}));
mock.module("sonner", () => ({
  toast: { error: toastError, success: mock(() => {}), info: mock(() => {}), warning: mock(() => {}) },
  Toaster: () => null,
}));

const { UniversalInstallWizard } = await import("./UniversalInstallWizard");

const onClose = mock(() => {});

async function renderWizard(initialStep?: Parameters<typeof UniversalInstallWizard>[0]["initialStep"]) {
  const result = render(<UniversalInstallWizard pluginId="notion" onClose={onClose} initialStep={initialStep} />);
  await act(async () => {});
  return result;
}

beforeEach(() => {
  detailData = detailFixture();
  releaseData = releaseFixture();
  pluginDetail.mockClear();
  pluginReleaseDetail.mockClear();
  toastError.mockClear();
  onClose.mockClear();
});

afterEach(() => {
  cleanup();
});

test("fetches detail and release on mount and renders the title and step 1 of M", async () => {
  await renderWizard();

  expect(pluginDetail).toHaveBeenCalledWith(LOCAL_RUNNER, "notion");
  expect(pluginReleaseDetail).toHaveBeenCalledWith(LOCAL_RUNNER, "notion");
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  expect(within(dialog).getByText("Step 1 of 6 — Overview")).toBeTruthy();
  expect(within(dialog).getByText("Overview")).toBeTruthy();
});

test("the progress bar has one segment per planned step", async () => {
  await renderWizard();

  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  const segments = dialog.querySelectorAll(".rounded-full.h-1, .h-1.rounded-full");
  expect(segments.length).toBe(6);
});

test("Continue advances to the next step and updates the header", async () => {
  await renderWizard();

  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  expect(within(dialog).getByText("Step 1 of 6 — Overview")).toBeTruthy();

  act(() => {
    within(dialog).getByRole("button", { name: "Continue" }).click();
  });

  expect(within(dialog).getByText("Step 2 of 6 — Permissions")).toBeTruthy();
  expect(within(dialog).getByText("Permissions")).toBeTruthy();
});

test("Back returns to the previous step and is disabled on the first step", async () => {
  await renderWizard();

  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  const backButton = within(dialog).getByRole("button", { name: "Back" }) as HTMLButtonElement;
  expect(backButton.disabled).toBe(true);

  act(() => {
    within(dialog).getByRole("button", { name: "Continue" }).click();
  });
  expect(within(dialog).getByText("Step 2 of 6 — Permissions")).toBeTruthy();
  expect((within(dialog).getByRole("button", { name: "Back" }) as HTMLButtonElement).disabled).toBe(false);

  act(() => {
    within(dialog).getByRole("button", { name: "Back" }).click();
  });
  expect(within(dialog).getByText("Step 1 of 6 — Overview")).toBeTruthy();
});

test("Skip only shows up on the connect and settings steps", async () => {
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });

  // overview (1 of 6) — no Skip.
  expect(within(dialog).queryByRole("button", { name: "Skip" })).toBeNull();

  // permissions (2 of 6) — no Skip.
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).queryByRole("button", { name: "Skip" })).toBeNull();

  // install (3 of 6) — no Skip.
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).queryByRole("button", { name: "Skip" })).toBeNull();

  // connect (4 of 6) — Skip appears.
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).getByText("Step 4 of 6 — Connect")).toBeTruthy();
  expect(within(dialog).queryByRole("button", { name: "Skip" })).not.toBeNull();

  // settings (5 of 6) — Skip appears.
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).getByText("Step 5 of 6 — Settings")).toBeTruthy();
  expect(within(dialog).queryByRole("button", { name: "Skip" })).not.toBeNull();

  // done (6 of 6) — no Skip.
  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).getByText("Step 6 of 6 — Done")).toBeTruthy();
  expect(within(dialog).queryByRole("button", { name: "Skip" })).toBeNull();
});

test("Continue on the last step closes the wizard", async () => {
  await renderWizard();
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });

  for (let i = 0; i < 5; i++) {
    act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  }
  expect(within(dialog).getByText("Step 6 of 6 — Done")).toBeTruthy();

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(onClose).toHaveBeenCalled();
});

test("initialStep resumes at that step's position in the plan", async () => {
  await renderWizard("settings");

  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  expect(within(dialog).getByText("Step 5 of 6 — Settings")).toBeTruthy();
});

test("a pluginDetail error toasts and still renders the shell", async () => {
  pluginDetail.mockImplementationOnce(() => Promise.resolve({ status: "error" as const, error: { message: "manifest read failed" } }));
  await renderWizard();

  expect(toastError).toHaveBeenCalledWith("manifest read failed");
  expect(screen.getByRole("dialog")).toBeTruthy();
});

test("a pluginReleaseDetail error toasts and still renders the shell", async () => {
  pluginReleaseDetail.mockImplementationOnce(() =>
    Promise.resolve({ status: "error" as const, error: { message: "release lookup failed" } }),
  );
  await renderWizard();

  expect(toastError).toHaveBeenCalledWith("release lookup failed");
  const dialog = screen.getByRole("dialog", { name: "Install Notion" });
  expect(within(dialog).getByText("Step 1 of 6 — Overview")).toBeTruthy();
});

test("initialStep falls back to step 1 when the plan doesn't include that step", async () => {
  pluginDetail.mockImplementationOnce(() => ok(providerDetailFixture()));
  await renderWizard("settings");

  const dialog = screen.getByRole("dialog", { name: "Install OpenAI" });
  expect(within(dialog).getByText("Step 1 of 4 — Overview")).toBeTruthy();
});

// Finding 1 — before both fetches settle, `plan` used to default to a single
// "overview" step, making isLast true on first paint; a Continue click
// during the round trip closed the wizard outright. A permanently-pending
// pluginDetail mock (same deterministic technique as
// PluginDetailView.test.tsx's `pluginToolsPendingIds`) freezes the shell
// mid-fetch so this is reproducible without racing a real promise.
test("Continue is disabled while the initial fetch is pending and does not close the wizard", async () => {
  pluginDetail.mockImplementationOnce(() => new Promise<never>(() => {}));
  render(<UniversalInstallWizard pluginId="notion" onClose={onClose} />);
  await act(async () => {});

  const dialog = screen.getByRole("dialog");
  expect(within(dialog).getByText("Loading…")).toBeTruthy();
  expect(within(dialog).queryByRole("button", { name: "Back" })).toBeNull();
  expect(within(dialog).queryByRole("button", { name: "Skip" })).toBeNull();
  const continueButton = within(dialog).getByRole("button", { name: "Continue" }) as HTMLButtonElement;
  expect(continueButton.disabled).toBe(true);

  act(() => {
    continueButton.click();
  });
  expect(onClose).not.toHaveBeenCalled();
});

// Finding 2 — every other shell test in this file plans the full six steps;
// this fixture (provider kind, no auth, no settings, not component-backed,
// no oauth profiles) collapses the plan to overview/install/connect/done so
// the progress math and step sequencing are exercised against a shorter plan.
test("renders a shortened 4-step plan for a provider with no settings or permissions gate", async () => {
  pluginDetail.mockImplementationOnce(() => ok(providerDetailFixture()));
  await renderWizard();

  const dialog = screen.getByRole("dialog", { name: "Install OpenAI" });
  expect(within(dialog).getByText("Step 1 of 4 — Overview")).toBeTruthy();
  const segments = dialog.querySelectorAll(".rounded-full.h-1, .h-1.rounded-full");
  expect(segments.length).toBe(4);

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).getByText("Step 2 of 4 — Install")).toBeTruthy();
  expect(within(dialog).queryByText("Permissions")).toBeNull();
  expect(within(dialog).queryByText("Settings")).toBeNull();

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).getByText("Step 3 of 4 — Connect")).toBeTruthy();
  expect(within(dialog).queryByText("Permissions")).toBeNull();
  expect(within(dialog).queryByText("Settings")).toBeNull();

  act(() => within(dialog).getByRole("button", { name: "Continue" }).click());
  expect(within(dialog).getByText("Step 4 of 4 — Done")).toBeTruthy();
  expect(within(dialog).queryByText("Permissions")).toBeNull();
  expect(within(dialog).queryByText("Settings")).toBeNull();
});
