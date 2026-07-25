import { afterEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { deriveSetupChecklist, SetupChecklist, type SetupItem } from "./SetupChecklist";

afterEach(cleanup);

// ---------- deriveSetupChecklist (pure) ----------

test("a no-auth, no-required-settings plugin gets a single install item", () => {
  const items = deriveSetupChecklist({ installed: false, authKind: "none", authConfigured: true, requiredSettingsMissing: 0 });
  expect(items).toEqual([{ id: "install", label: "Install the plugin", done: false }]);
});

test("an installed oauth plugin that hasn't connected yet gets an undone connect item", () => {
  const items = deriveSetupChecklist({ installed: true, authKind: "oauth", authConfigured: false, requiredSettingsMissing: 0 });
  expect(items.map((i) => ({ id: i.id, done: i.done }))).toEqual([
    { id: "install", done: true },
    { id: "connect", done: false },
  ]);
});

test("a fully set up oauth plugin has every item done", () => {
  const items = deriveSetupChecklist({ installed: true, authKind: "oauth", authConfigured: true, requiredSettingsMissing: 0 });
  expect(items.length).toBeGreaterThan(0);
  expect(items.every((i) => i.done)).toBe(true);
});

test("required settings still missing adds an undone settings item", () => {
  const items = deriveSetupChecklist({ installed: true, authKind: "none", authConfigured: true, requiredSettingsMissing: 2 });
  expect(items).toEqual([
    { id: "install", label: "Install the plugin", done: true },
    { id: "settings", label: "Fill in required settings", done: false },
  ]);
});

test("no required settings means no settings item at all (not even a done one)", () => {
  const items = deriveSetupChecklist({ installed: true, authKind: "none", authConfigured: true, requiredSettingsMissing: 0 });
  expect(items.find((i) => i.id === "settings")).toBeUndefined();
});

test('a token-auth plugin gets a connect item too (authKind !== "none" covers token, not just oauth)', () => {
  const items = deriveSetupChecklist({ installed: true, authKind: "token", authConfigured: false, requiredSettingsMissing: 0 });
  expect(items.some((i) => i.id === "connect")).toBe(true);
});

// ---------- SetupChecklist (render) ----------

test("renders the card title and one row per item", () => {
  const items: SetupItem[] = [
    { id: "install", label: "Install the plugin", done: true },
    { id: "connect", label: "Connect your account", done: false },
  ];
  render(<SetupChecklist items={items} onAction={() => {}} />);

  expect(screen.getByText("Finish setting up")).toBeTruthy();
  expect(screen.getByText("Install the plugin")).toBeTruthy();
  expect(screen.getByText("Connect your account")).toBeTruthy();
});

test("a Button appears ONLY on the first undone row, never on a done row or a later undone row", () => {
  const items: SetupItem[] = [
    { id: "install", label: "Install the plugin", done: true },
    { id: "connect", label: "Connect your account", done: false },
    { id: "settings", label: "Fill in required settings", done: false },
  ];
  render(<SetupChecklist items={items} onAction={() => {}} />);

  const buttons = screen.getAllByRole("button");
  expect(buttons).toHaveLength(1);
  expect(buttons[0].textContent).toContain("Connect");
});

test("clicking the action button fires onAction with that row's id", () => {
  const items: SetupItem[] = [
    { id: "install", label: "Install the plugin", done: false },
    { id: "connect", label: "Connect your account", done: false },
  ];
  const onAction = mock((_id: string) => {});
  render(<SetupChecklist items={items} onAction={onAction} />);

  fireEvent.click(screen.getByRole("button"));
  expect(onAction).toHaveBeenCalledWith("install");
});

test("a done row's label is styled muted", () => {
  const items: SetupItem[] = [{ id: "install", label: "Install the plugin", done: true }];
  render(<SetupChecklist items={items} onAction={() => {}} />);

  expect(screen.getByText("Install the plugin").className).toContain("text-muted-foreground");
});

test("an undone row's label is NOT styled muted", () => {
  const items: SetupItem[] = [{ id: "install", label: "Install the plugin", done: false }];
  render(<SetupChecklist items={items} onAction={() => {}} />);

  expect(screen.getByText("Install the plugin").className).not.toContain("text-muted-foreground");
});
