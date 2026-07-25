import { afterEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { CmdError, CommandFileInfo, Result, SlashEntryInfo } from "@/bindings";

const command: CommandFileInfo = {
  name: "audit",
  description: "Review the current change",
  template: "Review $ARGUMENTS and $1",
  agent: null,
  model: null,
  subtask: false,
  revision: "rev-1",
};

const createdCommand: CommandFileInfo = { ...command, name: "ship", description: "", template: "Ship $ARGUMENTS", revision: "rev-2" };

// "audit" mirrors the editable global command above (so the editor can
// exclude it from its own "/" suggestions); "sync" and "deploy" are
// builtin-origin catalog entries, deliberately distinct from the static
// ["init", "review", "compact"] fallback so reserved-name tests can prove
// the catalog — not the hardcoded list — wins once it's loaded.
const catalog: SlashEntryInfo[] = [
  {
    name: "audit",
    description: "Review the current change",
    kind: "command",
    origin: "global",
    home: true,
    session: true,
    requiresProject: false,
    effective: true,
    shadowsGlobal: false,
    agent: null,
    model: null,
    subtask: false,
  },
  {
    name: "sync",
    description: "Sync the workspace",
    kind: "command",
    origin: "builtin",
    home: true,
    session: true,
    requiresProject: false,
    effective: true,
    shadowsGlobal: false,
    agent: null,
    model: null,
    subtask: false,
  },
  {
    name: "deploy",
    description: "Deploy everywhere",
    kind: "command",
    origin: "builtin",
    home: false,
    session: true,
    requiresProject: true,
    effective: true,
    shadowsGlobal: false,
    agent: null,
    model: null,
    subtask: false,
  },
];

const globalCommandList = mock(async () => ({ status: "ok" as const, data: [command] }));
const globalCommandCreate = mock(async () => ({ status: "ok" as const, data: createdCommand }));
const globalCommandUpdate = mock(async () => ({ status: "ok" as const, data: command }));
const globalCommandDelete = mock((): Promise<Result<null, CmdError>> => Promise.resolve({ status: "ok", data: null }));
const slashCatalog = mock(async () => ({ status: "ok" as const, data: catalog }));
const searchFiles = mock(async () => ({ status: "ok" as const, data: [] }));

mock.module("@/bindings", () => ({
  commands: { globalCommandList, globalCommandCreate, globalCommandUpdate, globalCommandDelete, slashCatalog, searchFiles },
  events: { coreEventMsg: { listen: async () => () => {} } },
}));

const { CommandsTab, deriveReservedCommandNames, globalCommandNameError, globalCommandPreview } = await import("./CommandsTab");
const { useNative } = await import("@/store-native");
const { useStore } = await import("@/store");

afterEach(() => {
  cleanup();
  useNative.setState({ globalCommands: undefined, slashCatalogByKey: {}, agentsByProject: {} });
  useStore.setState({ selectedProjectId: null });
  globalCommandList.mockClear();
  globalCommandCreate.mockClear();
  globalCommandUpdate.mockClear();
  globalCommandDelete.mockClear();
  slashCatalog.mockClear();
  searchFiles.mockClear();
});

test("lists global commands with no Project combobox, alongside read-only built-in catalog rows", async () => {
  render(<CommandsTab />);

  expect(await screen.findByText("/audit")).toBeTruthy();
  expect(globalCommandList).toHaveBeenCalledWith("local");
  expect(slashCatalog).toHaveBeenCalledWith("local", null, null);
  expect(screen.queryByRole("combobox", { name: "Project" })).toBeNull();
  expect(screen.getByText("Global commands are available in every project. Built-in commands are read-only.")).toBeTruthy();

  expect(await screen.findByText("/sync")).toBeTruthy();
  expect(screen.getByText("/deploy")).toBeTruthy();
  expect(screen.getAllByText("Built-in")).toHaveLength(2);
  expect(screen.queryByRole("button", { name: "Edit /sync" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Delete /sync" })).toBeNull();
  expect(screen.getByRole("button", { name: "Edit /audit" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Delete /audit" })).toBeTruthy();
});

test("validates global command names, previews positional placeholders, and derives reserved names from the loaded catalog with a static fallback", () => {
  expect(globalCommandNameError("Review", false, new Set())).toContain("lowercase");
  expect(globalCommandNameError("team/review", false, new Set())).toBeNull();
  expect(globalCommandNameError("sync", false, new Set(["sync"]))).toContain("Built-in");
  expect(globalCommandNameError("sync", true, new Set(["sync"]))).toBeNull();
  expect(globalCommandPreview("review", "Review $ARGUMENTS; compare $1 with $2")).toBe(
    "/review <arguments>\nReview <arguments>; compare <argument 1> with <argument 2>",
  );

  expect(deriveReservedCommandNames(undefined)).toEqual(new Set(["init", "review", "compact"]));
  expect(deriveReservedCommandNames([])).toEqual(new Set(["init", "review", "compact"]));
  expect(deriveReservedCommandNames(catalog)).toEqual(new Set(["sync", "deploy"]));
});

test("opens a simplified editor with only Name, Description, and Template fields", async () => {
  render(<CommandsTab />);
  await screen.findByText("/audit");

  expect(screen.getByRole("button", { name: "New command" }).hasAttribute("disabled")).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "New command" }));

  expect(await screen.findByLabelText("Name")).toBeTruthy();
  expect(screen.getByLabelText("Description")).toBeTruthy();
  expect(screen.getByLabelText("Template")).toBeTruthy();
  expect(screen.queryByRole("combobox", { name: "Agent" })).toBeNull();
  expect(screen.queryByRole("combobox", { name: "Model" })).toBeNull();
  expect(screen.queryByText("Run as subtask")).toBeNull();
  expect(screen.queryByRole("combobox", { name: "Project" })).toBeNull();
});

test("creates a global command with fixed agent/model/subtask defaults", async () => {
  render(<CommandsTab />);
  await screen.findByText("/audit");

  fireEvent.click(screen.getByRole("button", { name: "New command" }));
  fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "ship" } });
  fireEvent.change(screen.getByLabelText("Template"), { target: { value: "Ship $ARGUMENTS" } });
  fireEvent.click(screen.getByRole("button", { name: "Create" }));

  await waitFor(() =>
    expect(globalCommandCreate).toHaveBeenCalledWith("local", {
      name: "ship",
      description: "",
      template: "Ship $ARGUMENTS",
      agent: null,
      model: null,
      subtask: false,
    }),
  );
});

test("saves an existing command via globalCommandUpdate, disabling the name field", async () => {
  render(<CommandsTab />);
  await screen.findByText("/audit");

  fireEvent.click(screen.getByRole("button", { name: "Edit /audit" }));
  expect(((await screen.findByLabelText("Name")) as HTMLInputElement).disabled).toBe(true);
  fireEvent.change(screen.getByLabelText("Template"), { target: { value: "Updated template" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));

  await waitFor(() =>
    expect(globalCommandUpdate).toHaveBeenCalledWith(
      "local",
      "audit",
      "rev-1",
      expect.objectContaining({ template: "Updated template", agent: null, model: null, subtask: false }),
    ),
  );
});

test("preserves a legacy global command's hand-authored agent, model, and subtask on edit", async () => {
  const legacyCommand: CommandFileInfo = { ...command, name: "legacy", agent: "plan", model: "m", subtask: true };
  globalCommandList.mockResolvedValueOnce({ status: "ok", data: [legacyCommand] });
  render(<CommandsTab />);
  await screen.findByText("/legacy");

  fireEvent.click(screen.getByRole("button", { name: "Edit /legacy" }));
  fireEvent.change(await screen.findByLabelText("Description"), { target: { value: "Updated description" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));

  await waitFor(() =>
    expect(globalCommandUpdate).toHaveBeenCalledWith(
      "local",
      "legacy",
      "rev-1",
      expect.objectContaining({ description: "Updated description", agent: "plan", model: "m", subtask: true }),
    ),
  );
});

test("rejects a name reserved by the loaded catalog, even though the catalog can free up a name from the static fallback", async () => {
  render(<CommandsTab />);
  await screen.findByText("/audit");

  fireEvent.click(screen.getByRole("button", { name: "New command" }));
  fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "sync" } });
  expect(screen.getByText("Built-in commands cannot be created or updated.")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Create" }).hasAttribute("disabled")).toBe(true);

  // "review" sits in the static fallback list, but the loaded catalog's
  // builtin set (sync, deploy) takes precedence over it — proving the
  // reserved set really is catalog-derived, not hardcoded.
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "review" } });
  expect(screen.queryByText("Built-in commands cannot be created or updated.")).toBeNull();
});

test("falls back to the static reserved names when the catalog is empty", async () => {
  slashCatalog.mockResolvedValueOnce({ status: "ok", data: [] });
  render(<CommandsTab />);
  await screen.findByText("/audit");

  fireEvent.click(screen.getByRole("button", { name: "New command" }));
  fireEvent.change(await screen.findByLabelText("Name"), { target: { value: "init" } });
  expect(screen.getByText("Built-in commands cannot be created or updated.")).toBeTruthy();
});

test("search filters global command rows by name, description, or template text", async () => {
  render(<CommandsTab />);
  await screen.findByText("/audit");

  fireEvent.change(screen.getByLabelText("Search commands"), { target: { value: "nomatch" } });
  expect(screen.getByText("No global commands match your search.")).toBeTruthy();

  fireEvent.change(screen.getByLabelText("Search commands"), { target: { value: "audit" } });
  expect(screen.getByText("/audit")).toBeTruthy();
});

test("opens deletion confirmation from a trigger and confirms or cancels the requested command", async () => {
  render(<CommandsTab />);
  await screen.findByText("/audit");

  const trigger = screen.getByRole("button", { name: "Delete /audit" });
  fireEvent.click(trigger);
  expect(await screen.findByRole("dialog", { name: "Delete /audit?" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(document.activeElement).toBe(trigger));
  expect(globalCommandDelete).not.toHaveBeenCalled();

  fireEvent.click(trigger);
  fireEvent.click(screen.getByRole("button", { name: "Delete command" }));
  await waitFor(() => expect(globalCommandDelete).toHaveBeenCalledWith("local", "audit", "rev-1"));
});

test("closes deletion confirmation after a conflict reloads the latest command", async () => {
  globalCommandDelete.mockResolvedValueOnce({ status: "error" as const, error: { message: "Command was modified externally." } });
  render(<CommandsTab />);
  await screen.findByText("/audit");

  fireEvent.click(screen.getByRole("button", { name: "Delete /audit" }));
  expect(await screen.findByRole("dialog", { name: "Delete /audit?" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Delete command" }));

  await waitFor(() => expect(globalCommandDelete).toHaveBeenCalledWith("local", "audit", "rev-1"));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Delete /audit?" })).toBeNull());
});

test("suggests catalog commands (excluding the one being edited) from a fresh line while typing the template", async () => {
  render(<CommandsTab />);
  await screen.findByText("/audit");

  fireEvent.click(screen.getByRole("button", { name: "Edit /audit" }));
  const template = (await screen.findByLabelText("Template")) as HTMLTextAreaElement;
  const nextValue = `${command.template}\n/dep`;
  fireEvent.change(template, { target: { value: nextValue, selectionStart: nextValue.length } });

  // "/deploy" appears twice while the menu is open — once in the read-only
  // built-in row behind the modal, once as the suggestion — so the menu
  // item (the only one that's a button) is what disambiguates it.
  const suggestion = await screen.findByRole("button", { name: /\/deploy/ });
  // "audit" is excluded from its own editor's suggestions — only its list
  // row (not a second, menu-item copy) renders the text.
  expect(screen.getAllByText("/audit")).toHaveLength(1);

  fireEvent.click(suggestion);
  expect(template.value).toBe(`${command.template}\n/deploy `);
});
