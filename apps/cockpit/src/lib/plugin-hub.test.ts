import { describe, expect, test } from "bun:test";
import type { AppInfo, InstalledSkillInfo, PluginInfo } from "../bindings";
import {
  appStatusToHubStatus,
  buildHubItems,
  featuredItems,
  filterHubItems,
  fixTargetTab,
  type HubItem,
  kindCounts,
  railCounts,
  statusPresentation,
} from "./plugin-hub";

function mkPlugin(overrides: Partial<PluginInfo> = {}): PluginInfo {
  return {
    id: "demo",
    name: "Demo Plugin",
    description: "A demo plugin.",
    icon: "cpu",
    categories: ["vcs"],
    slot: null,
    ownsSlot: false,
    verified: false,
    experimental: false,
    enabled: true,
    configured: false,
    source: "builtin",
    capabilities: [],
    kind: "integration",
    installed: true,
    family: null,
    pinned: false,
    sourceSpec: null,
    resolvedCommit: null,
    installedAt: null,
    updatedAt: null,
    trustTier: null,
    componentBacked: false,
    catalogVersion: null,
    blockedReason: null,
    status: "ok",
    statusDetail: null,
    authKind: "none",
    toolCount: null,
    skillCount: null,
    ...overrides,
  };
}

function mkApp(overrides: Partial<AppInfo> = {}): AppInfo {
  return {
    id: "demo-app",
    name: "Demo App",
    kind: "mcp",
    initial: "D",
    color: "#123456",
    desc: "A demo MCP server.",
    transport: "stdio",
    command: null,
    args: [],
    url: null,
    scope: "global",
    scopeGateways: [],
    status: "connected",
    statusDetail: null,
    version: null,
    publisher: null,
    authKind: "none",
    authDetail: null,
    tools: [],
    agentAccess: [],
    ...overrides,
  };
}

function mkSkill(overrides: Partial<InstalledSkillInfo> = {}): InstalledSkillInfo {
  return {
    id: "demo-skill",
    name: "Demo Skill Pack",
    source: "https://github.com/example/demo-skill",
    pluginId: null,
    installedAt: "2026-01-01T00:00:00Z",
    skillCount: 3,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// buildHubItems — plugin mapping
// ---------------------------------------------------------------------------

describe("buildHubItems: plugin mapping", () => {
  test("maps a plugin 1:1: kind/status passthrough, rowKey/nav/icon", () => {
    const plugin = mkPlugin({ id: "github", kind: "integration", status: "needs-setup", statusDetail: "no token" });
    const [item] = buildHubItems({ plugins: [plugin], apps: [], skills: [] });
    expect(item.rowKey).toBe("plugin:github");
    expect(item.id).toBe("github");
    expect(item.kind).toBe("integration");
    expect(item.status).toBe("needs-setup");
    expect(item.statusDetail).toBe("no token");
    expect(item.icon).toBe("cpu");
    expect(item.nav).toEqual({ kind: "pluginDetail", id: "github" });
    expect(item.name).toBe("Demo Plugin");
    expect(item.description).toBe("A demo plugin.");
    expect(item.categories).toEqual(["vcs"]);
    expect(item.verified).toBe(false);
    expect(item.experimental).toBe(false);
    expect(item.pinned).toBe(false);
    expect(item.installed).toBe(true);
    expect(item.blockedReason).toBeNull();
    expect(item.toolNames).toEqual([]);
  });

  test("countsLabel: toolCount wins when set", () => {
    const plugin = mkPlugin({ toolCount: 12, skillCount: null });
    const [item] = buildHubItems({ plugins: [plugin], apps: [], skills: [] });
    expect(item.countsLabel).toBe("12 tools");
  });

  test("countsLabel: skillCount used when toolCount null", () => {
    const plugin = mkPlugin({ toolCount: null, skillCount: 9 });
    const [item] = buildHubItems({ plugins: [plugin], apps: [], skills: [] });
    expect(item.countsLabel).toBe("9 skills");
  });

  test("countsLabel: null when both toolCount and skillCount are null", () => {
    const plugin = mkPlugin({ toolCount: null, skillCount: null });
    const [item] = buildHubItems({ plugins: [plugin], apps: [], skills: [] });
    expect(item.countsLabel).toBeNull();
  });

  test("provider rows nav to providerDetail using family when set", () => {
    const plugin = mkPlugin({ id: "anthropic-oauth", kind: "provider", family: "anthropic" });
    const [item] = buildHubItems({ plugins: [plugin], apps: [], skills: [] });
    expect(item.nav).toEqual({ kind: "providerDetail", provider: "anthropic" });
  });

  test("provider rows fall back to id when family is null", () => {
    const plugin = mkPlugin({ id: "openai", kind: "provider", family: null });
    const [item] = buildHubItems({ plugins: [plugin], apps: [], skills: [] });
    expect(item.nav).toEqual({ kind: "providerDetail", provider: "openai" });
  });
});

// ---------------------------------------------------------------------------
// buildHubItems — app mapping
// ---------------------------------------------------------------------------

describe("buildHubItems: app mapping", () => {
  test("maps status connected -> ok", () => {
    const app = mkApp({ status: "connected" });
    const [item] = buildHubItems({ plugins: [], apps: [app], skills: [] });
    expect(item.status).toBe("ok");
  });

  test("maps status error -> attach-failed, carries statusDetail", () => {
    const app = mkApp({ status: "error", statusDetail: "connection refused" });
    const [item] = buildHubItems({ plugins: [], apps: [app], skills: [] });
    expect(item.status).toBe("attach-failed");
    expect(item.statusDetail).toBe("connection refused");
  });

  test("maps status unknown -> unchecked", () => {
    const app = mkApp({ status: "unknown" });
    const [item] = buildHubItems({ plugins: [], apps: [app], skills: [] });
    expect(item.status).toBe("unchecked");
  });

  test("maps unmapped app status (e.g. 'weird') to unchecked fallback", () => {
    const app = mkApp({ status: "weird" as any });
    const [item] = buildHubItems({ plugins: [], apps: [app], skills: [] });
    expect(item.status).toBe("unchecked");
  });

  test("kind is mcp-server, installed true, nav appDetail, icon null, uses initial/color", () => {
    const app = mkApp({ id: "slack", initial: "S", color: "#ff00ff" });
    const [item] = buildHubItems({ plugins: [], apps: [app], skills: [] });
    expect(item.kind).toBe("mcp-server");
    expect(item.installed).toBe(true);
    expect(item.nav).toEqual({ kind: "appDetail", id: "slack" });
    expect(item.icon).toBeNull();
    expect(item.appInitial).toBe("S");
    expect(item.appColor).toBe("#ff00ff");
    expect(item.rowKey).toBe("app:slack");
  });

  test("countsLabel is N tools when tools present", () => {
    const app = mkApp({
      tools: [
        { name: "search", desc: "Search", perm: "ask" },
        { name: "fetch", desc: "Fetch", perm: "allow" },
      ],
    });
    const [item] = buildHubItems({ plugins: [], apps: [app], skills: [] });
    expect(item.countsLabel).toBe("2 tools");
    expect(item.toolNames).toEqual(["search", "fetch"]);
  });

  test("countsLabel is null when tools is empty", () => {
    const app = mkApp({ tools: [] });
    const [item] = buildHubItems({ plugins: [], apps: [app], skills: [] });
    expect(item.countsLabel).toBeNull();
    expect(item.toolNames).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// appStatusToHubStatus — shared status translator
// ---------------------------------------------------------------------------

describe("appStatusToHubStatus", () => {
  test("maps connected → ok", () => {
    expect(appStatusToHubStatus("connected")).toBe("ok");
  });

  test("maps error → attach-failed", () => {
    expect(appStatusToHubStatus("error")).toBe("attach-failed");
  });

  test("maps unknown → unchecked", () => {
    expect(appStatusToHubStatus("unknown")).toBe("unchecked");
  });

  test("unmapped status (e.g. 'weird') → unchecked fallback", () => {
    expect(appStatusToHubStatus("weird")).toBe("unchecked");
  });
});

// ---------------------------------------------------------------------------
// buildHubItems — skill mapping + plugin-backed exclusion
// ---------------------------------------------------------------------------

describe("buildHubItems: skill mapping", () => {
  test("maps a standalone (non-plugin-backed) skill source", () => {
    const skill = mkSkill({ id: "my-skills", name: "My Skills", source: "https://github.com/me/skills", skillCount: 5, pluginId: null });
    const [item] = buildHubItems({ plugins: [], apps: [], skills: [skill] });
    expect(item.rowKey).toBe("skill:my-skills");
    expect(item.kind).toBe("skill-pack");
    expect(item.status).toBe("ok");
    expect(item.countsLabel).toBe("5 skills");
    expect(item.installed).toBe(true);
    // no plugin id -> nav to its own id
    expect(item.nav).toEqual({ kind: "pluginDetail", id: "my-skills" });
  });

  test("skill source with a pluginId navs to that plugin id", () => {
    const skill = mkSkill({ id: "attached-skill", pluginId: "superpowers" });
    const [item] = buildHubItems({ plugins: [], apps: [], skills: [skill] });
    expect(item.nav).toEqual({ kind: "pluginDetail", id: "superpowers" });
  });

  test("excludes a skill whose own id matches an existing plugin id", () => {
    const plugin = mkPlugin({ id: "superpowers" });
    const skill = mkSkill({ id: "superpowers", pluginId: null });
    const items = buildHubItems({ plugins: [plugin], apps: [], skills: [skill] });
    expect(items.filter((i) => i.kind === "skill-pack")).toHaveLength(0);
  });

  test("excludes a skill whose pluginId matches an existing plugin id", () => {
    const plugin = mkPlugin({ id: "superpowers" });
    const skill = mkSkill({ id: "some-other-id", pluginId: "superpowers" });
    const items = buildHubItems({ plugins: [plugin], apps: [], skills: [skill] });
    expect(items.filter((i) => i.kind === "skill-pack")).toHaveLength(0);
  });

  test("includes a skill unrelated to any plugin id", () => {
    const plugin = mkPlugin({ id: "superpowers" });
    const skill = mkSkill({ id: "manual-skill", pluginId: null });
    const items = buildHubItems({ plugins: [plugin], apps: [], skills: [skill] });
    expect(items.filter((i) => i.kind === "skill-pack")).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// rowKey uniqueness across sources
// ---------------------------------------------------------------------------

test("rowKey stays unique even when a plugin/app/skill share the same bare id", () => {
  const plugin = mkPlugin({ id: "shared" });
  const app = mkApp({ id: "shared" });
  const skill = mkSkill({ id: "shared-skill" }); // avoid triggering the plugin-backed exclusion
  const items = buildHubItems({ plugins: [plugin], apps: [app], skills: [skill] });
  const keys = items.map((i) => i.rowKey);
  expect(new Set(keys).size).toBe(keys.length);
  expect(keys).toContain("plugin:shared");
  expect(keys).toContain("app:shared");
  expect(keys).toContain("skill:shared-skill");
});

// ---------------------------------------------------------------------------
// filterHubItems
// ---------------------------------------------------------------------------

function items(): HubItem[] {
  return buildHubItems({
    plugins: [
      mkPlugin({ id: "github", name: "GitHub", description: "Repos and issues", kind: "integration", installed: true, status: "ok" }),
      mkPlugin({ id: "discord", name: "Discord", description: "Chat gateway", kind: "gateway", installed: true, status: "needs-setup" }),
      mkPlugin({
        id: "anthropic",
        name: "Anthropic",
        description: "Model provider",
        kind: "provider",
        installed: true,
        status: "attach-failed",
      }),
      mkPlugin({
        id: "notion",
        name: "Notion",
        description: "Docs and pages",
        kind: "integration",
        installed: false,
        status: "not-installed",
        verified: true,
      }),
      mkPlugin({
        id: "jira",
        name: "Jira",
        description: "Issue tracker",
        kind: "integration",
        installed: true,
        status: "update-available",
      }),
    ],
    apps: [mkApp({ id: "slack", name: "Slack", desc: "Team chat", tools: [{ name: "post-message", desc: "Post", perm: "ask" }] })],
    skills: [mkSkill({ id: "docs-skill", name: "Docs Helper", source: "https://example.com/docs" })],
  });
}

describe("filterHubItems: state", () => {
  test("installed = installed rows only", () => {
    const result = filterHubItems(items(), { state: "installed", kind: null, category: null }, "");
    expect(result.every((i) => i.installed)).toBe(true);
    expect(result.some((i) => i.id === "notion")).toBe(false);
  });

  test("discover = not installed rows only, keeps input (catalog) order", () => {
    const withTwoDiscover = buildHubItems({
      plugins: [
        mkPlugin({ id: "zeta", name: "Zeta", installed: false, status: "not-installed" }),
        mkPlugin({ id: "alpha", name: "Alpha", installed: false, status: "not-installed" }),
      ],
      apps: [],
      skills: [],
    });
    const result = filterHubItems(withTwoDiscover, { state: "discover", kind: null, category: null }, "");
    expect(result.map((i) => i.id)).toEqual(["zeta", "alpha"]); // input order preserved, NOT alphabetical
  });

  test("attention = needs-setup | attach-failed", () => {
    const result = filterHubItems(items(), { state: "attention", kind: null, category: null }, "");
    expect(result.map((i) => i.id).sort()).toEqual(["discord", "anthropic"].sort());
  });

  test("attention also includes blocked status", () => {
    const withBlocked = buildHubItems({
      plugins: [mkPlugin({ id: "blocked-one", installed: true, status: "blocked" })],
      apps: [],
      skills: [],
    });
    const result = filterHubItems(withBlocked, { state: "attention", kind: null, category: null }, "");
    expect(result.map((i) => i.id)).toEqual(["blocked-one"]);
  });

  test("updates = update-available only", () => {
    const result = filterHubItems(items(), { state: "updates", kind: null, category: null }, "");
    expect(result.map((i) => i.id)).toEqual(["jira"]);
  });

  test("all = every row", () => {
    const result = filterHubItems(items(), { state: "all", kind: null, category: null }, "");
    expect(result).toHaveLength(items().length);
  });
});

describe("filterHubItems: kind", () => {
  test("'integrations' matches integration|gateway", () => {
    const result = filterHubItems(items(), { state: "all", kind: "integrations", category: null }, "");
    expect(result.map((i) => i.id).sort()).toEqual(["discord", "github", "jira", "notion"].sort());
  });

  test("exact kind matches only that kind", () => {
    const result = filterHubItems(items(), { state: "all", kind: "provider", category: null }, "");
    expect(result.map((i) => i.id)).toEqual(["anthropic"]);
  });

  test("mcp-server kind matches app rows", () => {
    const result = filterHubItems(items(), { state: "all", kind: "mcp-server", category: null }, "");
    expect(result.map((i) => i.id)).toEqual(["slack"]);
  });
});

describe("filterHubItems: category", () => {
  test("matches items whose categories include the requested category", () => {
    const withCategory = buildHubItems({
      plugins: [mkPlugin({ id: "a", categories: ["vcs"] }), mkPlugin({ id: "b", categories: ["chat"] })],
      apps: [],
      skills: [],
    });
    const result = filterHubItems(withCategory, { state: "all", kind: null, category: "vcs" }, "");
    expect(result.map((i) => i.id)).toEqual(["a"]);
  });
});

describe("filterHubItems: search query", () => {
  test("matches on name case-insensitively", () => {
    const result = filterHubItems(items(), { state: "all", kind: null, category: null }, "GITHUB");
    expect(result.map((i) => i.id)).toEqual(["github"]);
  });

  test("matches on description case-insensitively", () => {
    const result = filterHubItems(items(), { state: "all", kind: null, category: null }, "issue tracker");
    expect(result.map((i) => i.id)).toEqual(["jira"]);
  });

  test("matches on a tool name (toolNames corpus) case-insensitively", () => {
    const result = filterHubItems(items(), { state: "all", kind: null, category: null }, "POST-MESSAGE");
    expect(result.map((i) => i.id)).toEqual(["slack"]);
  });

  test("no match returns empty", () => {
    const result = filterHubItems(items(), { state: "all", kind: null, category: null }, "zzz-nothing-matches");
    expect(result).toEqual([]);
  });
});

describe("filterHubItems: sort", () => {
  test("within 'all', attention statuses float first, then alphabetical", () => {
    const data = buildHubItems({
      plugins: [
        mkPlugin({ id: "zebra", name: "Zebra", status: "ok", installed: true }),
        mkPlugin({ id: "alpha", name: "Alpha", status: "needs-setup", installed: true }),
        mkPlugin({ id: "mango", name: "Mango", status: "ok", installed: true }),
        mkPlugin({ id: "yak", name: "Yak", status: "attach-failed", installed: true }),
      ],
      apps: [],
      skills: [],
    });
    const result = filterHubItems(data, { state: "all", kind: null, category: null }, "");
    expect(result.map((i) => i.name)).toEqual(["Alpha", "Yak", "Mango", "Zebra"]);
  });

  test("within 'installed', attention statuses float first, then alphabetical", () => {
    const data = buildHubItems({
      plugins: [
        mkPlugin({ id: "zebra", name: "Zebra", status: "ok", installed: true }),
        mkPlugin({ id: "alpha", name: "Alpha", status: "blocked", installed: true }),
      ],
      apps: [],
      skills: [],
    });
    const result = filterHubItems(data, { state: "installed", kind: null, category: null }, "");
    expect(result.map((i) => i.name)).toEqual(["Alpha", "Zebra"]);
  });

  test("within 'updates', items sort attention-first-then-alphabetical", () => {
    const data = buildHubItems({
      plugins: [
        mkPlugin({ id: "zebra", name: "Zebra", status: "update-available", installed: true }),
        mkPlugin({ id: "alpha", name: "Alpha", status: "update-available", installed: true }),
      ],
      apps: [],
      skills: [],
    });
    const result = filterHubItems(data, { state: "updates", kind: null, category: null }, "");
    expect(result.map((i) => i.name)).toEqual(["Alpha", "Zebra"]);
  });
});

// ---------------------------------------------------------------------------
// railCounts / kindCounts
// ---------------------------------------------------------------------------

test("railCounts totals match filterHubItems for each state", () => {
  const data = items();
  const counts = railCounts(data);
  expect(counts.all).toBe(data.length);
  expect(counts.installed).toBe(filterHubItems(data, { state: "installed", kind: null, category: null }, "").length);
  expect(counts.discover).toBe(filterHubItems(data, { state: "discover", kind: null, category: null }, "").length);
  expect(counts.attention).toBe(filterHubItems(data, { state: "attention", kind: null, category: null }, "").length);
  expect(counts.updates).toBe(filterHubItems(data, { state: "updates", kind: null, category: null }, "").length);
});

test("kindCounts totals: per-kind counts sum to the full item count, integrations aggregates integration+gateway", () => {
  const data = items();
  const counts = kindCounts(data);
  const perKindSum = counts.integration + counts.gateway + counts.provider + counts["skill-pack"] + counts["mcp-server"];
  expect(perKindSum).toBe(data.length);
  expect(counts.integrations).toBe(counts.integration + counts.gateway);
});

test("kindCounts pre-seeds all kinds and aggregates to zero even when absent from fixtures", () => {
  const data = buildHubItems({
    plugins: [mkPlugin({ id: "github", kind: "integration" })],
    apps: [],
    skills: [],
  });
  const counts = kindCounts(data);
  expect(counts.integration).toBe(1);
  expect(counts.gateway).toBe(0);
  expect(counts.provider).toBe(0);
  expect(counts["skill-pack"]).toBe(0);
  expect(counts["mcp-server"]).toBe(0);
  expect(counts.integrations).toBe(1);
});

// ---------------------------------------------------------------------------
// featuredItems
// ---------------------------------------------------------------------------

describe("featuredItems", () => {
  test("only not-installed, verified, non-mcp-server rows qualify", () => {
    const data = buildHubItems({
      plugins: [
        mkPlugin({ id: "a", installed: false, verified: true, kind: "integration" }),
        mkPlugin({ id: "b", installed: true, verified: true, kind: "integration" }), // installed, excluded
        mkPlugin({ id: "c", installed: false, verified: false, kind: "integration" }), // not verified, excluded
      ],
      apps: [mkApp({ id: "d" })], // installed:true always for apps -> excluded anyway
      skills: [],
    });
    const result = featuredItems(data);
    expect(result.map((i) => i.id)).toEqual(["a"]);
  });

  test("caps at 6 even when more qualify", () => {
    const plugins = Array.from({ length: 9 }, (_, i) => mkPlugin({ id: `p${i}`, installed: false, verified: true, kind: "integration" }));
    const data = buildHubItems({ plugins, apps: [], skills: [] });
    const result = featuredItems(data);
    expect(result).toHaveLength(6);
  });
});

// ---------------------------------------------------------------------------
// statusPresentation
// ---------------------------------------------------------------------------

test("statusPresentation covers every HubStatus", () => {
  expect(statusPresentation("ok")).toEqual({ label: "Connected", color: "#22C55E" });
  expect(statusPresentation("disabled")).toEqual({ label: "Disabled", color: null });
  expect(statusPresentation("needs-setup")).toEqual({ label: "Needs setup", color: "#F59E0B" });
  expect(statusPresentation("attach-failed")).toEqual({ label: "Attach failed", color: "#EF4444" });
  expect(statusPresentation("update-available")).toEqual({ label: "Update available", color: "#3B82F6" });
  expect(statusPresentation("blocked")).toEqual({ label: "Blocked", color: "#EF4444" });
  expect(statusPresentation("unchecked")).toEqual({ label: "Unchecked", color: null });
  expect(statusPresentation("not-installed")).toEqual({ label: "", color: null });
});

// ---------------------------------------------------------------------------
// fixTargetTab
// ---------------------------------------------------------------------------

test("fixTargetTab maps per spec §6", () => {
  expect(fixTargetTab("needs-setup")).toBe("settings");
  expect(fixTargetTab("unchecked")).toBe("settings");
  expect(fixTargetTab("attach-failed")).toBe("health");
  expect(fixTargetTab("update-available")).toBe("versions");
  expect(fixTargetTab("ok")).toBeNull();
  expect(fixTargetTab("disabled")).toBeNull();
  expect(fixTargetTab("blocked")).toBeNull();
  expect(fixTargetTab("not-installed")).toBeNull();
});
