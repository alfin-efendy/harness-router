import { MenuPanel, MenuPanelItem as MenuItem, MenuPanelSection as MenuSectionLabel } from "@ryuzi/ui";
import type { SlashEntryInfo } from "@/bindings";

export function SlashCommandMenu({ entries, onPick }: { entries: SlashEntryInfo[]; onPick: (entry: SlashEntryInfo) => void }) {
  if (entries.length === 0) return null;
  return (
    <MenuPanel onClose={() => undefined} className="bottom-full left-2.5 z-50 mb-1.5 w-[320px]">
      <MenuSectionLabel>Commands</MenuSectionLabel>
      {entries.map((entry) => (
        <MenuItem key={`${entry.kind}:${entry.name}`} onClick={() => onPick(entry)} className="font-medium">
          <span className="font-mono text-[12px] text-muted-foreground">/{entry.name}</span>
          <span className="min-w-0 flex-1 truncate">{entry.description}</span>
          {entry.kind === "skill" ? (
            <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">Skill</span>
          ) : null}
        </MenuItem>
      ))}
    </MenuPanel>
  );
}
