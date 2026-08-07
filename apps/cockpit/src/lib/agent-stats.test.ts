import { expect, test } from "bun:test";
import type { AgentConfigurationCatalogInfo, AgentToolUsageInfo, CatalogEntryInfo, NativeToolDecisionInfo } from "../bindings";
import {
  considerOffCandidates,
  formatCompactTokens,
  formatLastActive,
  formatUsd,
  reliabilitySummary,
  statsRowFragment,
} from "./agent-stats";

function entry(id: string, overrides: Partial<CatalogEntryInfo> = {}): CatalogEntryInfo {
  return { id, label: id, description: "", available: true, commandScoped: false, pack: null, kind: null, ...overrides };
}

function tool(name: string, count: number, lastUsed = 0): AgentToolUsageInfo {
  return { tool: name, count, lastUsed };
}

function decisions(...pairs: [string, string][]): NativeToolDecisionInfo[] {
  return pairs.map(([toolName, decision]) => ({ tool: toolName, decision }));
}

const catalog: Pick<AgentConfigurationCatalogInfo, "nativeTools" | "pluginTools"> = {
  nativeTools: [entry("read", { label: "Read" }), entry("bash", { label: "Bash" }), entry("grep", { label: "Grep" })],
  pluginTools: [entry("github", { label: "GitHub" })],
};

test("formatLastActive renders an em dash for no activity and a relative label otherwise", () => {
  const now = 1_000_000;
  expect(formatLastActive(null, now)).toBe("—");
  expect(formatLastActive(now - 5 * 60_000, now)).toBe("5m ago");
});

test("formatCompactTokens compacts to k/M and drops a bare trailing .0", () => {
  expect(formatCompactTokens(0)).toBe("0");
  expect(formatCompactTokens(999)).toBe("999");
  expect(formatCompactTokens(1_000)).toBe("1k");
  expect(formatCompactTokens(1_500)).toBe("1.5k");
  expect(formatCompactTokens(1_000_000)).toBe("1M");
  expect(formatCompactTokens(2_500_000)).toBe("2.5M");
});

test("formatUsd shows two decimals, a sub-cent floor label, and a plain zero", () => {
  expect(formatUsd(0)).toBe("$0.00");
  expect(formatUsd(0.004)).toBe("<$0.01");
  expect(formatUsd(12.3)).toBe("$12.30");
});

test("statsRowFragment composes sessions, relative activity, and 7d cost with correct pluralization", () => {
  const now = 1_000_000;
  expect(statsRowFragment({ sessionCount: 1, lastActive: now - 60_000, costUsd7d: 1.5 }, now)).toBe("1 session · 1m ago · $1.50 7d");
  expect(statsRowFragment({ sessionCount: 3, lastActive: null, costUsd7d: 0 }, now)).toBe("3 sessions · — · $0.00 7d");
});

test("reliabilitySummary renders an em dash for zero runs and a rounded percent otherwise", () => {
  expect(reliabilitySummary(0, 0)).toEqual({ percent: "—", detail: "—" });
  expect(reliabilitySummary(10, 2)).toEqual({ percent: "80%", detail: "2 of 10 runs" });
  expect(reliabilitySummary(1, 1)).toEqual({ percent: "0%", detail: "1 of 1 runs" });
});

test("considerOffCandidates flags explicitly-on native tools missing from top tools", () => {
  const result = considerOffCandidates(decisions(["read", "allow"], ["bash", "ask"]), catalog, [tool("read", 5)], 12);
  expect(result).toEqual([{ id: "bash", label: "Bash" }]);
});

test("considerOffCandidates excludes tools already in top tools, an explicit off decision, and an implicit (absent) default-ask decision", () => {
  // "read" is explicitly allowed but used → excluded. "bash" is explicitly
  // off → excluded. "grep" never appears in nativeToolDecisions at all,
  // which means an implicit default-ask decision (not "explicit") →
  // excluded too.
  const result = considerOffCandidates(decisions(["read", "allow"], ["bash", "off"]), catalog, [tool("read", 5)], 12);
  expect(result).toEqual([]);
});

test("considerOffCandidates never flags a plugin catalog entry, even with a 30d run and nothing in top tools", () => {
  // Plugin catalog ids (e.g. "github") are manifest ids; topTools records
  // the namespaced runtime tool name actually invoked
  // (wasm__github__create_issue, etc.), which never literally matches — so
  // the plugin branch is dropped entirely rather than produce this false
  // "unused" positive on every bound plugin. `catalog.pluginTools` (see the
  // fixture above) has a "github" entry, but it can never appear in the
  // result since plugin catalog entries are no longer diffed at all.
  const result = considerOffCandidates([], catalog, [], 12);
  expect(result).toEqual([]);
});

test("considerOffCandidates suppresses entirely without a loaded catalog or with zero runs in the trailing 30 days", () => {
  expect(considerOffCandidates(decisions(["read", "allow"]), null, [], 12)).toEqual([]);
  expect(considerOffCandidates(decisions(["read", "allow"]), catalog, [], 0)).toEqual([]);
});
