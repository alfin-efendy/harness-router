import { useEffect, useState } from "react";
import { Button, Input, SettingsCard, SettingsCardRow, SettingsCardTitle, Switch } from "@ryuzi/ui";
import type { AgentDetailInfo, CatalogEntryInfo } from "@/bindings";
import { useAgents } from "@/store-agents";
import { useAgentConfigurationCatalog } from "@/store-agent-catalog";
import { mutationFromDetail } from "./agentMutation";

// Skills entries carry an owning pack name (see `CatalogEntryInfo.pack`); a
// skill installed standalone (not part of a plugin-bundled pack) carries
// `null`, which groups under this synthetic heading.
const STANDALONE = "Standalone";

type SkillGroup = { name: string; entries: CatalogEntryInfo[] };

// Named packs sort alphabetically; Standalone always trails so the
// deliberately-organized packs read first.
function groupByPack(entries: CatalogEntryInfo[]): SkillGroup[] {
  const order: string[] = [];
  const byName = new Map<string, CatalogEntryInfo[]>();
  for (const entry of entries) {
    const name = entry.pack ?? STANDALONE;
    const list = byName.get(name);
    if (list) list.push(entry);
    else {
      byName.set(name, [entry]);
      order.push(name);
    }
  }
  const named = order.filter((name) => name !== STANDALONE).sort((a, b) => a.localeCompare(b));
  const groups = named.map((name) => ({ name, entries: byName.get(name) as CatalogEntryInfo[] }));
  const standalone = byName.get(STANDALONE);
  if (standalone) groups.push({ name: STANDALONE, entries: standalone });
  return groups;
}

export function AgentSkillsTab({ detail }: { detail: AgentDetailInfo }) {
  const saving = useAgents((state) => state.saving);
  const catalog = useAgentConfigurationCatalog((state) => state.catalog);
  const catalogLoading = useAgentConfigurationCatalog((state) => state.loading);
  const catalogError = useAgentConfigurationCatalog((state) => state.error);
  const loadCatalog = useAgentConfigurationCatalog((state) => state.load);

  // Local-only UI state: search text. Enablement itself is derived straight
  // from `detail.skills` and saved immediately — there is no local copy to
  // resync.
  const [search, setSearch] = useState("");

  useEffect(() => {
    void loadCatalog();
  }, [loadCatalog]);

  const persist = (nextSkills: string[]) =>
    void useAgents.getState().update(detail.summary.id, { ...mutationFromDetail(detail), skills: nextSkills });

  const setSkillEnabled = (id: string, on: boolean) => {
    // Read the store directly rather than closing over the `saving` prop —
    // a click can land between a save kicking off and this component's
    // next render, and only a fresh read reliably blocks it.
    if (useAgents.getState().saving) return;
    persist(on ? (detail.skills.includes(id) ? detail.skills : [...detail.skills, id]) : detail.skills.filter((value) => value !== id));
  };

  const enableAll = (ids: string[]) => persist(Array.from(new Set([...detail.skills, ...ids])));
  const disableAll = (ids: string[]) => persist(detail.skills.filter((id) => !ids.includes(id)));

  if (catalogLoading || catalogError || catalog === null) {
    return (
      <SettingsCard>
        <div className="px-[18px] py-4 text-xs text-muted-foreground" role={catalogError ? "alert" : undefined}>
          {catalogLoading ? "Loading skills…" : catalogError ? `Couldn't load skills: ${catalogError}` : "Loading skills…"}
        </div>
      </SettingsCard>
    );
  }

  const query = search.trim().toLowerCase();
  const matches = (entry: CatalogEntryInfo) =>
    query === "" ||
    entry.label.toLowerCase().includes(query) ||
    entry.id.toLowerCase().includes(query) ||
    entry.description.toLowerCase().includes(query);

  const groups = groupByPack(catalog.skills)
    .map((group) => ({ ...group, entries: group.entries.filter(matches) }))
    .filter((group) => group.entries.length > 0);

  return (
    <div className="flex flex-col gap-3">
      <Input aria-label="Search skills" placeholder="Search skills…" value={search} onChange={(event) => setSearch(event.target.value)} />
      {groups.length === 0 ? (
        <SettingsCard>
          <p className="m-0 px-[18px] py-5 text-xs text-muted-foreground">No skills match your search.</p>
        </SettingsCard>
      ) : (
        groups.map((group) => {
          const ids = group.entries.map((entry) => entry.id);
          const enabledCount = ids.filter((id) => detail.skills.includes(id)).length;
          // Bulk actions only ever touch AVAILABLE entries — mirroring the
          // row-level toggle, which refuses unavailable ones.
          const availableIds = group.entries.filter((entry) => entry.available).map((entry) => entry.id);
          return (
            <div key={group.name} data-testid={`skill-group-${group.name}`}>
              <SettingsCard>
                <div className="flex items-center gap-3 border-b border-border px-[18px] py-3.5">
                  <span className="min-w-0 flex-1">
                    <SettingsCardTitle>{group.name}</SettingsCardTitle>
                    <span className="ml-2 text-[11px] text-muted-foreground">
                      {enabledCount}/{ids.length}
                    </span>
                  </span>
                  <Button variant="outline" size="sm" disabled={saving} onClick={() => enableAll(availableIds)}>
                    Enable all
                  </Button>
                  <Button variant="outline" size="sm" disabled={saving} onClick={() => disableAll(availableIds)}>
                    Disable all
                  </Button>
                </div>
                {group.entries.map((entry) => {
                  const on = detail.skills.includes(entry.id);
                  return (
                    <SettingsCardRow key={entry.id} className="gap-3">
                      <span className="min-w-0 flex-1">
                        <span className={`block text-[13px] font-medium${entry.available ? "" : " text-destructive"}`}>
                          {entry.available ? entry.label : `${entry.label} (unavailable)`}
                        </span>
                        <span className="block truncate text-[11px] text-muted-foreground">{entry.description || entry.id}</span>
                      </span>
                      <Switch
                        on={on}
                        label={`Enable skill ${entry.id}`}
                        onToggle={() => entry.available && setSkillEnabled(entry.id, !on)}
                      />
                    </SettingsCardRow>
                  );
                })}
              </SettingsCard>
            </div>
          );
        })
      )}
    </div>
  );
}
