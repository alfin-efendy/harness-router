import type { AgentConfigurationCatalogInfo, AgentStatsLite, AgentToolUsageInfo, NativeToolDecisionInfo } from "../bindings";
import { formatRelativeTime } from "../store-learning";

/** Relative last-active label for stats surfaces — an em dash when the agent
 *  has no recorded activity at all (`lastActive` is `null`, e.g. a freshly
 *  created agent with zero sessions). */
export function formatLastActive(lastActive: number | null, now: number = Date.now()): string {
  return lastActive === null ? "—" : formatRelativeTime(lastActive, now);
}

/** Compact `k`/`M`-suffixed token count for the Cost card (`250` → `"250"`,
 *  `1_500` → `"1.5k"`, `2_500_000` → `"2.5M"`); a bare trailing `.0` is
 *  dropped so a round thousand reads `"1k"` rather than `"1.0k"`. */
export function formatCompactTokens(tokens: number): string {
  const trim = (value: string) => (value.endsWith(".0") ? value.slice(0, -2) : value);
  if (tokens >= 1_000_000) return `${trim((tokens / 1_000_000).toFixed(1))}M`;
  if (tokens >= 1_000) return `${trim((tokens / 1_000).toFixed(1))}k`;
  return `${tokens}`;
}

/** Two-decimal USD label. A genuine zero reads `"$0.00"`; a non-zero amount
 *  that would visually round to zero gets a distinct `"<$0.01"` instead, so
 *  it never looks indistinguishable from "no cost at all". */
export function formatUsd(usd: number): string {
  if (usd <= 0) return "$0.00";
  if (usd < 0.01) return "<$0.01";
  return `$${usd.toFixed(2)}`;
}

/** `N sessions · <relative last active> · $X 7d` — the list-row stats
 *  fragment, appended to an agent row's metadata line only once its lite
 *  stats have loaded. */
export function statsRowFragment(stats: AgentStatsLite, now: number = Date.now()): string {
  const sessions = `${stats.sessionCount} ${stats.sessionCount === 1 ? "session" : "sessions"}`;
  return `${sessions} · ${formatLastActive(stats.lastActive, now)} · ${formatUsd(stats.costUsd7d)} 7d`;
}

export type ReliabilitySummary = { percent: string; detail: string };

/** Reliability card figures: `(total-failed)/total` as a rounded percent
 *  plus a `failed of total runs` detail line. Both fall back to an em dash
 *  when there were no runs in the trailing 30 days — "no data" must never
 *  be misread as "0% reliable". */
export function reliabilitySummary(runsTotal30d: number, runsFailed30d: number): ReliabilitySummary {
  if (runsTotal30d <= 0) return { percent: "—", detail: "—" };
  const succeeded = Math.max(0, runsTotal30d - runsFailed30d);
  const percent = Math.round((succeeded / runsTotal30d) * 100);
  return { percent: `${percent}%`, detail: `${runsFailed30d} of ${runsTotal30d} runs` };
}

export type ConsiderOffCandidate = { id: string; label: string };

/**
 * Native tools with an explicit non-Off decision, plus bound plugin tools,
 * that never appear in `topTools` — a nudge to turn off configuration that
 * isn't actually being used. Suppressed entirely (returns `[]`) without a
 * loaded catalog, or when the agent had zero runs in the trailing 30 days:
 * no data is not the same claim as "unused".
 */
export function considerOffCandidates(
  nativeToolDecisions: NativeToolDecisionInfo[],
  boundPluginTools: string[],
  catalog: Pick<AgentConfigurationCatalogInfo, "nativeTools" | "pluginTools"> | null,
  topTools: AgentToolUsageInfo[],
  runsTotal30d: number,
): ConsiderOffCandidate[] {
  if (!catalog || runsTotal30d === 0) return [];
  const used = new Set(topTools.map((entry) => entry.tool));
  const explicitlyOn = new Set(nativeToolDecisions.filter((entry) => entry.decision !== "off").map((entry) => entry.tool));
  const bound = new Set(boundPluginTools);
  const candidates: ConsiderOffCandidate[] = [];
  for (const entry of catalog.nativeTools) {
    if (explicitlyOn.has(entry.id) && !used.has(entry.id)) candidates.push({ id: entry.id, label: entry.label });
  }
  for (const entry of catalog.pluginTools) {
    if (bound.has(entry.id) && !used.has(entry.id)) candidates.push({ id: entry.id, label: entry.label });
  }
  return candidates;
}
