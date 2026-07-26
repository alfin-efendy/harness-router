import { describe, expect, test } from "bun:test";
import { activeSlashQuery, matchSlashEntries } from "./slash-autocomplete";
import type { SlashEntryInfo } from "@/bindings";

const entry = (over: Partial<SlashEntryInfo>): SlashEntryInfo => ({
  name: "x",
  description: "",
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
  ...over,
});

describe("activeSlashQuery", () => {
  test("leading slash without space queries", () => {
    expect(activeSlashQuery("/re")).toBe("re");
    expect(activeSlashQuery("  /RE")).toBe("re");
    expect(activeSlashQuery("/re x")).toBeNull();
    expect(activeSlashQuery("plain")).toBeNull();
  });
});

describe("matchSlashEntries", () => {
  const entries = [
    entry({ name: "init", origin: "builtin", requiresProject: true }),
    entry({ name: "review", origin: "builtin", home: false }),
    entry({ name: "ship" }),
    entry({ name: "pdf", kind: "skill", origin: "project" }),
    entry({ name: "shadowed", effective: false }),
  ];
  test("home without project: only projectless home entries", () => {
    const names = matchSlashEntries(entries, "", "home", false).map((e) => e.name);
    expect(names).toContain("ship");
    expect(names).not.toContain("init");
    expect(names).not.toContain("review");
  });
  test("home with project adds init; session shows review", () => {
    expect(matchSlashEntries(entries, "in", "home", true).map((e) => e.name)).toContain("init");
    expect(matchSlashEntries(entries, "re", "session", true).map((e) => e.name)).toContain("review");
  });
  test("skills listed; non-effective dropped; capped at 6", () => {
    expect(matchSlashEntries(entries, "pd", "session", true)[0]?.kind).toBe("skill");
    expect(matchSlashEntries(entries, "sh", "session", true).map((e) => e.name)).not.toContain("shadowed");
  });
});
