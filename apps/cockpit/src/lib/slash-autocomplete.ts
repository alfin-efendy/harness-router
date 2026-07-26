import type { SlashEntryInfo } from "@/bindings";

/** The active "/" query: a draft that is exactly "/<partial-name>". */
export function activeSlashQuery(draft: string): string | null {
  const trimmed = draft.trimStart();
  if (!trimmed.startsWith("/") || trimmed.includes(" ")) return null;
  return trimmed.slice(1).toLowerCase();
}

/** Filter catalog entries for one composer surface. */
export function matchSlashEntries(
  entries: SlashEntryInfo[],
  query: string | null,
  surface: "home" | "session",
  hasProject: boolean,
): SlashEntryInfo[] {
  if (query === null) return [];
  return entries
    .filter((e) => e.effective)
    .filter((e) => (surface === "home" ? e.home : e.session))
    .filter((e) => !e.requiresProject || hasProject)
    .filter((e) => e.name.toLowerCase().startsWith(query))
    .slice(0, 6);
}
