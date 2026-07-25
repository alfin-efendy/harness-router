import { expect, test } from "@playwright/test";
import { installMockIPC } from "./mock-ipc";

// Plugins hub + universal install wizard (Task 16). Fixtures come from
// `mock-ipc.ts`'s `PLUGIN_HUB_ROWS` (three `list_plugins` rows: an installed
// component `github`, a not-yet-installed non-component `linear`, and a
// not-yet-installed component `slack`) plus the `plugin_detail`/
// `plugin_tools`/`install_component_plugin` dynamic dispatch branches added
// alongside them. Every test scopes tab-bar/row assertions to the `<main>`
// landmark (`App.tsx`'s routed content), since the sidebar rail shares a
// "Settings" label with the detail view's own Settings tab button.
test.beforeEach(async ({ page }) => {
  await installMockIPC(page);
  await page.goto("/");
  await page.getByText("Plugins", { exact: true }).first().click();
});

test("hub renders unified rows, Discover hides installed, and search narrows results", async ({ page }) => {
  const main = page.getByRole("main");

  // Unified rows: one row per `list_plugins` entry, all sources merged.
  await expect(main.getByRole("button", { name: "Open GitHub" })).toBeVisible();
  await expect(main.getByRole("button", { name: "Open Linear" })).toBeVisible();
  await expect(main.getByRole("button", { name: "Open Slack" })).toBeVisible();

  // Discover rail state hides the installed row, keeps the not-installed ones.
  await main.getByText("Discover", { exact: true }).click();
  await expect(main.getByRole("button", { name: "Open GitHub" })).toHaveCount(0);
  await expect(main.getByRole("button", { name: "Open Linear" })).toBeVisible();
  await expect(main.getByRole("button", { name: "Open Slack" })).toBeVisible();

  // Search narrows the (now "All") row set down to the matching name.
  await main.getByText("All", { exact: true }).click();
  await main.getByPlaceholder("Search plugins, tools, skills").fill("github");
  await expect(main.getByRole("button", { name: "Open GitHub" })).toBeVisible();
  await expect(main.getByRole("button", { name: "Open Linear" })).toHaveCount(0);
  await expect(main.getByRole("button", { name: "Open Slack" })).toHaveCount(0);
});

test("a not-installed row opens pre-install detail with only Overview and Tools tabs", async ({ page }) => {
  const main = page.getByRole("main");
  await main.getByRole("button", { name: "Open Linear" }).click();

  await expect(page.getByTestId("tab-panel-overview")).toBeVisible();
  await expect(main.getByRole("button", { name: "Overview" })).toBeVisible();
  await expect(main.getByRole("button", { name: /^Tools/ })).toBeVisible();
  // Settings/Versions/Health all gate on `PluginInfo.installed` (Settings
  // also needs auth/settings, Health needs extension/doctor findings) — none
  // of that applies to a never-installed, non-component row like `linear`.
  await expect(main.getByRole("button", { name: "Settings" })).toHaveCount(0);
  await expect(main.getByRole("button", { name: "Versions" })).toHaveCount(0);
  await expect(main.getByRole("button", { name: "Health" })).toHaveCount(0);
});

test("installing a component plugin walks Overview → Permissions → Install → Done", async ({ page }) => {
  const main = page.getByRole("main");
  await main.getByRole("button", { name: "Install Slack" }).click();

  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("Install Slack")).toBeVisible();
  await expect(dialog.getByText("Step 1 of 4 — Overview")).toBeVisible();

  await dialog.getByRole("button", { name: "Continue" }).click();
  await expect(dialog.getByText("Step 2 of 4 — Permissions")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Continue" })).toBeDisabled();

  await dialog.getByRole("switch", { name: "Accept permissions" }).click();
  await expect(dialog.getByRole("button", { name: "Continue" })).toBeEnabled();
  await dialog.getByRole("button", { name: "Continue" }).click();

  // Install runs automatically on entering the step and auto-advances to Done
  // once the mocked `install_component_plugin` resolves.
  await expect(dialog.getByText("Installed")).toBeVisible();
  await expect(dialog.getByText("create_issue")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Open plugin page" })).toBeVisible();
});

test("detail tabs switch and the Tools tab lists the mocked tool", async ({ page }) => {
  const main = page.getByRole("main");
  await main.getByRole("button", { name: "Open GitHub" }).click();
  await expect(page.getByTestId("tab-panel-overview")).toBeVisible();

  await main.getByRole("button", { name: /^Tools/ }).click();
  const toolsPanel = page.getByTestId("tab-panel-tools");
  await expect(toolsPanel).toBeVisible();
  await expect(toolsPanel.getByText("create_issue")).toBeVisible();

  await main.getByRole("button", { name: "Settings" }).click();
  const settingsPanel = page.getByTestId("tab-panel-settings");
  await expect(settingsPanel).toBeVisible();
  await expect(settingsPanel.getByText("Authentication")).toBeVisible();

  await main.getByRole("button", { name: "Versions" }).click();
  const versionsPanel = page.getByTestId("tab-panel-versions");
  await expect(versionsPanel).toBeVisible();
  await expect(versionsPanel.getByText("Component plugin")).toBeVisible();
});
