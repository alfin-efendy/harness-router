import type { ReactNode } from "react";
import { SettingsCard as Card, SettingsCardRow as CardRow } from "@ryuzi/ui";
import type { PluginToolEntry } from "@/bindings";
import { Pill } from "@/components/common/bits";

/** Fixed render order for the Tools & Skills tab — a plugin's agent-facing
 *  tools first, then its skills, then (for a provider) its models. Mirrors
 *  `PluginToolEntry.kind`'s three wire values. */
const GROUPS: { kind: string; label: string }[] = [
  { kind: "tool", label: "Tools" },
  { kind: "skill", label: "Skills" },
  { kind: "model", label: "Models" },
];

/**
 * The Tools & Skills tab's body (Task 10): every `plugin_tools` entry for a
 * plugin, grouped by kind in `GROUPS`'s order. A group heading only renders
 * when more than one kind is present — a tool-only or model-only plugin (the
 * common case) gets a flat list with no redundant "Tools" label. `live ===
 * false` means the entries came from the manifest's declared tools (no
 * release installed yet, or the plugin never runs live enumeration), so a
 * hint clarifies the list may still change.
 *
 * `renderTrailing` is optional (Task 12 wires per-tool permission controls
 * here) — when given, its output renders right-aligned on each row.
 */
export function PluginToolsList({
  entries,
  live,
  renderTrailing,
}: {
  entries: PluginToolEntry[];
  live: boolean;
  renderTrailing?: (name: string) => ReactNode;
}) {
  if (entries.length === 0) {
    return <Card className="px-[18px] py-3.5 text-[12.5px] text-muted-foreground">No tools declared.</Card>;
  }

  const groups = GROUPS.map((g) => ({ ...g, entries: entries.filter((e) => e.kind === g.kind) })).filter((g) => g.entries.length > 0);
  const showHeadings = groups.length > 1;

  return (
    <Card>
      {!live && (
        <div className="border-b border-border px-[18px] py-2.5 text-[11.5px] text-muted-foreground">
          Declared by the plugin — final list may differ after install.
        </div>
      )}
      {groups.map((g) => (
        <div key={g.kind}>
          {showHeadings && <div className="border-b border-border px-[18px] py-2 text-[12.5px] font-semibold">{g.label}</div>}
          {g.entries.map((e) => (
            <CardRow key={e.name}>
              <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="font-mono text-xs">{e.name}</span>
                {e.description && <span className="text-[12.5px] text-muted-foreground">{e.description}</span>}
              </div>
              {e.writes === true && <Pill variant="warn">writes</Pill>}
              {renderTrailing && <span className="ml-auto shrink-0">{renderTrailing(e.name)}</span>}
            </CardRow>
          ))}
        </div>
      ))}
    </Card>
  );
}
