import type { AppInfo, ComponentManifestInfo, InstalledSkillInfo, PluginInfo, PluginToolEntry } from "../bindings";

// Unified row model for the Plugins hub (design doc §6, task-7 brief). Pure
// data module — no React — merging three independent sources (plugins, MCP
// apps, skill sources) that each keep their own RPC, into one `HubItem[]`
// the new UI (Tasks 8/9/12) filters/sorts/renders. Daemon computes `status`
// for plugins already (spec §6); this module only translates the app-side
// `connected|error|unknown` vocabulary onto the same `HubStatus` union so
// every row — plugin, app, or skill — speaks one status language.

export type HubItemKind = "integration" | "gateway" | "provider" | "skill-pack" | "mcp-server";

export type HubStatus =
  | "ok"
  | "disabled"
  | "needs-setup"
  | "attach-failed"
  | "update-available"
  | "blocked"
  | "unchecked"
  | "not-installed";

export type HubNav =
  | { kind: "pluginDetail"; id: string }
  | { kind: "appDetail"; id: string }
  | { kind: "providerDetail"; provider: string };

export type HubItem = {
  rowKey: string; // unique across sources: "plugin:<id>" | "app:<id>" | "skill:<id>"
  id: string;
  kind: HubItemKind;
  name: string;
  description: string;
  icon: string | null; // pluginIcon() key; apps use initial/color instead
  appInitial?: string;
  appColor?: string;
  verified: boolean;
  experimental: boolean;
  pinned: boolean;
  installed: boolean;
  /** `PluginInfo.componentBacked` (apps/skill sources are never
   *  component-backed). Every kind's Install action opens the universal
   *  wizard now (Task 15 retired the classic catalog install modal) — the
   *  wizard's own `wizardKind` reads this (via `pluginDetail`, not this
   *  row) to pick the component adapter over the classic connector one. */
  componentBacked: boolean;
  status: HubStatus;
  statusDetail: string | null;
  countsLabel: string | null; // "12 tools" | "9 skills" | "2 tools" (apps)
  toolNames: string[]; // search corpus (apps: tool names; else [])
  categories: string[];
  blockedReason: string | null;
  nav: HubNav;
};

export type RailState = "all" | "installed" | "discover" | "attention" | "updates";
export type RailFilter = { state: RailState; kind: HubItemKind | "integrations" | null; category: string | null };

function pluginNav(plugin: PluginInfo): HubNav {
  // Providers navigate to the shared Models `providerDetail` surface keyed by
  // family (mirrors `openInstalled` in the old PluginsView); everything else
  // (integrations, gateways, skill-pack-kind plugins) opens its own detail page.
  if (plugin.kind === "provider") return { kind: "providerDetail", provider: plugin.family ?? plugin.id };
  return { kind: "pluginDetail", id: plugin.id };
}

function pluginCountsLabel(plugin: PluginInfo): string | null {
  if (plugin.toolCount != null) return `${plugin.toolCount} tools`;
  if (plugin.skillCount != null) return `${plugin.skillCount} skills`;
  return null;
}

function pluginToHubItem(plugin: PluginInfo): HubItem {
  return {
    rowKey: `plugin:${plugin.id}`,
    id: plugin.id,
    kind: plugin.kind as HubItemKind,
    name: plugin.name,
    description: plugin.description,
    icon: plugin.icon,
    verified: plugin.verified,
    experimental: plugin.experimental,
    pinned: plugin.pinned,
    installed: plugin.installed,
    componentBacked: plugin.componentBacked,
    status: plugin.status as HubStatus,
    statusDetail: plugin.statusDetail,
    countsLabel: pluginCountsLabel(plugin),
    toolNames: [],
    categories: plugin.categories,
    blockedReason: plugin.blockedReason,
    nav: pluginNav(plugin),
  };
}

const APP_STATUS_MAP: Record<string, HubStatus> = {
  connected: "ok",
  error: "attach-failed",
  unknown: "unchecked",
};

/** Translates the app-side `connected|error|unknown` vocabulary onto the shared
 *  `HubStatus` union. Used by `appToHubItem` (hub rows) and `AppDetailView`'s
 *  status pill to ensure both speak the same language. Unmapped statuses default
 *  to "unchecked". */
export function appStatusToHubStatus(status: string): HubStatus {
  return APP_STATUS_MAP[status] ?? "unchecked";
}

function appToHubItem(app: AppInfo): HubItem {
  return {
    rowKey: `app:${app.id}`,
    id: app.id,
    kind: "mcp-server",
    name: app.name,
    description: app.desc,
    icon: null,
    appInitial: app.initial,
    appColor: app.color,
    verified: false,
    experimental: false,
    pinned: false,
    installed: true,
    componentBacked: false,
    status: appStatusToHubStatus(app.status),
    statusDetail: app.statusDetail,
    countsLabel: app.tools.length > 0 ? `${app.tools.length} tools` : null,
    toolNames: app.tools.map((t) => t.name),
    categories: [],
    blockedReason: null,
    nav: { kind: "appDetail", id: app.id },
  };
}

function skillToHubItem(skill: InstalledSkillInfo): HubItem {
  return {
    rowKey: `skill:${skill.id}`,
    id: skill.id,
    kind: "skill-pack",
    name: skill.name,
    description: skill.source,
    icon: null,
    verified: false,
    experimental: false,
    pinned: false,
    installed: true,
    componentBacked: false,
    status: "ok",
    statusDetail: null,
    countsLabel: `${skill.skillCount} skills`,
    toolNames: [],
    categories: [],
    blockedReason: null,
    // The detail view resolves skill sources via a plugin id; a source with
    // no plugin id (never attached to a plugin manifest) navs to its own id.
    nav: { kind: "pluginDetail", id: skill.pluginId ?? skill.id },
  };
}

/** Spec A3: one hub card per provider VENDOR, not per auth-method descriptor.
 *  Members sharing a `family` collapse into the head row (the member whose
 *  id equals the family — same display-head rule ModelsView and CatalogEntry
 *  use). Aggregation: installed if any member is installed; needs-setup only
 *  if every member needs setup (a healthy head must not be masked by an
 *  unconfigured sibling method); otherwise the head's own status. Non-provider
 *  rows pass through untouched. Input order is preserved (heads keep their
 *  catalog position — the `discover` rail relies on it). */
export function collapseProviderFamilies(plugins: PluginInfo[]): PluginInfo[] {
  const members = new Map<string, PluginInfo[]>();
  for (const p of plugins) {
    if (p.kind !== "provider") continue;
    const family = p.family ?? p.id;
    const list = members.get(family);
    if (list) list.push(p);
    else members.set(family, [p]);
  }
  const out: PluginInfo[] = [];
  for (const p of plugins) {
    if (p.kind !== "provider") {
      out.push(p);
      continue;
    }
    const family = p.family ?? p.id;
    const group = members.get(family) ?? [p];
    const head = group.find((m) => m.id === family) ?? group[0];
    if (p.id !== head.id) continue; // folded into the head's card
    if (group.length === 1) {
      out.push(p);
      continue;
    }
    out.push({
      ...head,
      installed: group.some((m) => m.installed),
      status: group.every((m) => m.status === "needs-setup") ? "needs-setup" : head.status,
    });
  }
  return out;
}

/** Builds the unified hub row set from the three independent sources. Skill
 *  sources that are already represented by a plugin row (same exclusion
 *  `PluginsView.tsx` uses for its "Skill sources" card) are dropped so a
 *  plugin-backed skill pack shows once, as its plugin row. */
export function buildHubItems(input: { plugins: PluginInfo[]; apps: AppInfo[]; skills: InstalledSkillInfo[] }): HubItem[] {
  const pluginIds = new Set(input.plugins.map((p) => p.id));
  const standaloneSkills = input.skills.filter((s) => !pluginIds.has(s.id) && !(s.pluginId && pluginIds.has(s.pluginId)));
  return [
    ...collapseProviderFamilies(input.plugins).map(pluginToHubItem),
    ...input.apps.map(appToHubItem),
    ...standaloneSkills.map(skillToHubItem),
  ];
}

const ATTENTION_STATUSES: ReadonlySet<HubStatus> = new Set(["needs-setup", "attach-failed", "blocked"]);

function isAttention(status: HubStatus): boolean {
  return ATTENTION_STATUSES.has(status);
}

function matchesState(item: HubItem, state: RailState): boolean {
  switch (state) {
    case "installed":
      return item.installed;
    case "discover":
      return !item.installed;
    case "attention":
      return isAttention(item.status);
    case "updates":
      return item.status === "update-available";
    case "all":
      return true;
  }
}

function matchesKind(item: HubItem, kind: HubItemKind | "integrations" | null): boolean {
  if (kind == null) return true;
  if (kind === "integrations") return item.kind === "integration" || item.kind === "gateway";
  return item.kind === kind;
}

/** Pure filter + sort for the hub list. `discover` preserves catalog (input)
 *  order; every other state sorts attention statuses first, then
 *  alphabetically by name. */
export function filterHubItems(items: HubItem[], filter: RailFilter, query: string): HubItem[] {
  const q = query.trim().toLowerCase();
  const filtered = items.filter((item) => {
    if (!matchesState(item, filter.state)) return false;
    if (!matchesKind(item, filter.kind)) return false;
    if (filter.category && !item.categories.includes(filter.category)) return false;
    if (q) {
      const haystack = [item.name, item.description, ...item.toolNames].join("\n").toLowerCase();
      if (!haystack.includes(q)) return false;
    }
    return true;
  });
  if (filter.state === "discover") return filtered;
  return [...filtered].sort((a, b) => {
    const aRank = isAttention(a.status) ? 0 : 1;
    const bRank = isAttention(b.status) ? 0 : 1;
    if (aRank !== bRank) return aRank - bRank;
    return a.name.localeCompare(b.name);
  });
}

/** Per-rail totals over the full (unfiltered by kind/category/query) item set. */
export function railCounts(items: HubItem[]): Record<RailState, number> {
  return {
    all: items.length,
    installed: items.filter((i) => i.installed).length,
    discover: items.filter((i) => !i.installed).length,
    attention: items.filter((i) => isAttention(i.status)).length,
    updates: items.filter((i) => i.status === "update-available").length,
  };
}

/** Per-kind totals plus an "integrations" aggregate (integration + gateway)
 *  matching the `RailFilter.kind` vocabulary's collapsed option. */
export function kindCounts(items: HubItem[]): Record<string, number> {
  const counts: Record<string, number> = {
    integration: 0,
    gateway: 0,
    provider: 0,
    "skill-pack": 0,
    "mcp-server": 0,
    integrations: 0,
  };
  for (const item of items) {
    counts[item.kind] = (counts[item.kind] ?? 0) + 1;
  }
  counts.integrations = counts.integration + counts.gateway;
  return counts;
}

/** Maps a component's declared manifest tools onto the same `PluginToolEntry`
 *  shape `plugin_tools` returns, so `PluginToolsList` never needs to branch
 *  on which source it came from. Shared by `PluginDetailView`'s pre-install
 *  `fallbackTools` and the universal install wizard's `OverviewStep`
 *  (`steps-component.tsx`) — both need the declared (not live) tool list
 *  before a component has ever been installed, or before its own live
 *  `plugin_tools` fetch has resolved. A `null` manifest (nothing verified/
 *  installed yet) maps to `[]`. */
export function declaredToolEntries(manifest: ComponentManifestInfo | null): PluginToolEntry[] {
  return (manifest?.tools ?? []).map((t) => ({
    name: t.name,
    description: t.description,
    kind: "tool",
    writes: t.writes,
  }));
}

const STATUS_PRESENTATION: Record<HubStatus, { label: string; color: string | null }> = {
  ok: { label: "Connected", color: "#22C55E" },
  disabled: { label: "Disabled", color: null },
  "needs-setup": { label: "Needs setup", color: "#F59E0B" },
  "attach-failed": { label: "Attach failed", color: "#EF4444" },
  "update-available": { label: "Update available", color: "#3B82F6" },
  blocked: { label: "Blocked", color: "#EF4444" },
  unchecked: { label: "Unchecked", color: null },
  "not-installed": { label: "", color: null },
};

export function statusPresentation(s: HubStatus): { label: string; color: string | null } {
  return STATUS_PRESENTATION[s];
}

// spec §6 fix-action → tab map: needs-setup|unchecked → "settings",
// attach-failed → "health", update-available → "versions", else null.
export function fixTargetTab(s: HubStatus): "settings" | "health" | "versions" | null {
  if (s === "needs-setup" || s === "unchecked") return "settings";
  if (s === "attach-failed") return "health";
  if (s === "update-available") return "versions";
  return null;
}
