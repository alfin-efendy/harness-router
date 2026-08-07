import { afterEach, expect, test } from "bun:test";
import { cleanup, render, screen } from "@testing-library/react";
import type { PluginToolEntry } from "@/bindings";
import { PluginToolsList } from "./PluginToolsList";

afterEach(cleanup);

function entry(over: Partial<PluginToolEntry> = {}): PluginToolEntry {
  return { name: "search", description: "Searches the web", kind: "tool", writes: null, ...over };
}

test("renders a tool row with a mono name and description", () => {
  render(<PluginToolsList entries={[entry()]} live={true} />);
  const name = screen.getByText("search");
  expect(name.className).toContain("font-mono");
  expect(name.className).toContain("text-xs");
  const description = screen.getByText("Searches the web");
  expect(description.className).toContain("text-[12.5px]");
  expect(description.className).toContain("text-muted-foreground");
});

test("a writes:true tool shows a warn 'writes' pill", () => {
  render(<PluginToolsList entries={[entry({ name: "delete_file", writes: true })]} live={true} />);
  expect(screen.getByText("writes")).toBeTruthy();
});

test("a writes:false tool shows no writes pill", () => {
  render(<PluginToolsList entries={[entry({ writes: false })]} live={true} />);
  expect(screen.queryByText("writes")).toBeNull();
});

test("a writes:null tool (skills/models never carry writes) shows no writes pill", () => {
  render(<PluginToolsList entries={[entry({ kind: "skill", writes: null })]} live={true} />);
  expect(screen.queryByText("writes")).toBeNull();
});

test("a single kind renders its rows with no group heading", () => {
  render(<PluginToolsList entries={[entry(), entry({ name: "fetch" })]} live={true} />);
  expect(screen.queryByText("Tools")).toBeNull();
  expect(screen.getByText("search")).toBeTruthy();
  expect(screen.getByText("fetch")).toBeTruthy();
});

test("mixed kinds group under Tools/Skills/Models headings, in that order", () => {
  render(
    <PluginToolsList
      entries={[
        entry({ name: "gpt-4", description: "OpenAI GPT-4", kind: "model", writes: null }),
        entry({ name: "commit-helper", description: "Writes commit messages", kind: "skill", writes: null }),
        entry({ name: "search", description: "Searches the web", kind: "tool" }),
      ]}
      live={true}
    />,
  );
  const headings = screen.getAllByText(/^(Tools|Skills|Models)$/).map((n) => n.textContent);
  expect(headings).toEqual(["Tools", "Skills", "Models"]);
  expect(screen.getByText("search")).toBeTruthy();
  expect(screen.getByText("commit-helper")).toBeTruthy();
  expect(screen.getByText("gpt-4")).toBeTruthy();
});

test("group headings use semibold 12.5px styling", () => {
  render(<PluginToolsList entries={[entry({ name: "gpt-4", kind: "model" }), entry({ name: "search", kind: "tool" })]} live={true} />);
  const heading = screen.getByText("Tools");
  expect(heading.className).toContain("text-[12.5px]");
  expect(heading.className).toContain("font-semibold");
});

test("live:false shows the declared-list hint", () => {
  render(<PluginToolsList entries={[entry()]} live={false} />);
  expect(screen.getByText("Declared by the plugin — final list may differ after install.")).toBeTruthy();
});

test("live:true hides the declared-list hint", () => {
  render(<PluginToolsList entries={[entry()]} live={true} />);
  expect(screen.queryByText(/Declared by the plugin/)).toBeNull();
});

test("empty entries shows the empty state", () => {
  render(<PluginToolsList entries={[]} live={true} />);
  expect(screen.getByText("No tools declared.")).toBeTruthy();
});

test("empty entries with live:false still shows only the empty state (no hint alongside it)", () => {
  render(<PluginToolsList entries={[]} live={false} />);
  expect(screen.getByText("No tools declared.")).toBeTruthy();
});

test("formatName replaces the displayed mono name, without affecting renderTrailing's argument", () => {
  const seen: string[] = [];
  render(
    <PluginToolsList
      entries={[entry({ name: "search" })]}
      live={true}
      formatName={(name) => `mcp__github__${name}`}
      renderTrailing={(name) => {
        seen.push(name);
        return null;
      }}
    />,
  );
  expect(screen.getByText("mcp__github__search")).toBeTruthy();
  expect(screen.queryByText("search")).toBeNull();
  expect(seen).toEqual(["search"]);
});

test("without formatName the short name renders as before", () => {
  render(<PluginToolsList entries={[entry({ name: "search" })]} live={true} />);
  expect(screen.getByText("search")).toBeTruthy();
});

test("renderTrailing renders per-row output, right-aligned", () => {
  render(
    <PluginToolsList
      entries={[entry(), entry({ name: "fetch" })]}
      live={true}
      renderTrailing={(name) => <button type="button">{`toggle ${name}`}</button>}
    />,
  );
  expect(screen.getByRole("button", { name: "toggle search" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "toggle fetch" })).toBeTruthy();
});
