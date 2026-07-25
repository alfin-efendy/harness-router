import { useEffect } from "react";
import { Trash2 } from "lucide-react";
import { Button, SettingsCard, Switch } from "@ryuzi/ui";
import type { AgentDetailInfo, CatalogEntryInfo } from "@/bindings";
import { useAgents } from "@/store-agents";
import { useAgentConfigurationCatalog } from "@/store-agent-catalog";
import { mutationFromDetail } from "./agentMutation";

// A plugin tool's id is `provider.tool` (see `tool_filter_for_profile` in
// the native harness, which splits on the first '.' to resolve
// `mcp__provider__tool`/`ext__provider__tool`/`wasm__provider__tool`
// names). The Apps & MCP card for a given app nests every plugin tool whose
// provider segment matches that app's id.
function providerOf(id: string): string {
  const dot = id.indexOf(".");
  return dot === -1 ? id : id.slice(0, dot);
}

export function AgentAppsTab({ detail }: { detail: AgentDetailInfo }) {
  const saving = useAgents((state) => state.saving);
  const catalog = useAgentConfigurationCatalog((state) => state.catalog);
  const catalogLoading = useAgentConfigurationCatalog((state) => state.loading);
  const catalogError = useAgentConfigurationCatalog((state) => state.error);
  const loadCatalog = useAgentConfigurationCatalog((state) => state.load);

  useEffect(() => {
    void loadCatalog();
  }, [loadCatalog]);

  if (catalogLoading || catalogError || catalog === null) {
    return (
      <SettingsCard>
        <div className="px-[18px] py-4 text-xs text-muted-foreground" role={catalogError ? "alert" : undefined}>
          {catalogLoading ? "Loading apps…" : catalogError ? `Couldn't load apps: ${catalogError}` : "Loading apps…"}
        </div>
      </SettingsCard>
    );
  }

  const persist = (nextApps: string[], nextPluginTools: string[]) =>
    void useAgents.getState().update(detail.summary.id, {
      ...mutationFromDetail(detail),
      apps: nextApps,
      pluginTools: nextPluginTools,
    });

  // Catalog plugin tools grouped by provider segment, plus any enabled-but-
  // unknown tool ids (a stale/uninstalled plugin) surfaced as unavailable
  // rows under their own provider so they stay removable.
  const toolsByProvider = new Map<string, CatalogEntryInfo[]>();
  const addTool = (entry: CatalogEntryInfo) => {
    const provider = providerOf(entry.id);
    const list = toolsByProvider.get(provider);
    if (list) list.push(entry);
    else toolsByProvider.set(provider, [entry]);
  };
  for (const entry of catalog.pluginTools) addTool(entry);
  const knownToolIds = new Set(catalog.pluginTools.map((entry) => entry.id));
  for (const id of detail.pluginTools) {
    if (knownToolIds.has(id)) continue;
    addTool({ id, label: id, description: "", available: false, commandScoped: false, pack: null });
  }

  const appById = new Map(catalog.apps.map((app) => [app.id, app]));
  const appIds = Array.from(new Set([...catalog.apps.map((app) => app.id), ...detail.apps]));

  const setAppEnabled = (id: string, on: boolean) => {
    // Read the store directly rather than closing over the `saving` prop —
    // a click can land between a save kicking off and this component's
    // next render, and only a fresh read reliably blocks it.
    if (useAgents.getState().saving) return;
    const toolIds = (toolsByProvider.get(id) ?? []).map((tool) => tool.id);
    const nextApps = on ? (detail.apps.includes(id) ? detail.apps : [...detail.apps, id]) : detail.apps.filter((value) => value !== id);
    const nextPluginTools = on
      ? Array.from(new Set([...detail.pluginTools, ...toolIds]))
      : detail.pluginTools.filter((value) => !toolIds.includes(value));
    persist(nextApps, nextPluginTools);
  };

  const setToolEnabled = (id: string, on: boolean) => {
    if (useAgents.getState().saving) return;
    persist(
      detail.apps,
      on
        ? detail.pluginTools.includes(id)
          ? detail.pluginTools
          : [...detail.pluginTools, id]
        : detail.pluginTools.filter((value) => value !== id),
    );
  };

  const removeApp = (id: string) => {
    const toolIds = (toolsByProvider.get(id) ?? []).map((tool) => tool.id);
    persist(
      detail.apps.filter((value) => value !== id),
      detail.pluginTools.filter((value) => !toolIds.includes(value)),
    );
  };

  return (
    <div className="flex flex-col gap-3">
      {appIds.length === 0 ? (
        <SettingsCard>
          <p className="m-0 px-[18px] py-5 text-xs text-muted-foreground">No apps available.</p>
        </SettingsCard>
      ) : (
        appIds.map((id) => {
          const app = appById.get(id);
          const available = app !== undefined && app.available;
          const enabled = detail.apps.includes(id);
          const tools = toolsByProvider.get(id) ?? [];
          return (
            <div key={id} data-testid={`app-card-${id}`}>
              <SettingsCard>
                <div className="flex items-center gap-3 border-b border-border px-[18px] py-3.5">
                  <span className="min-w-0 flex-1">
                    <span className={`block text-[13px] font-medium${available ? "" : " text-destructive"}`}>
                      {available ? app.label : "Unavailable"}
                    </span>
                    <span className="block truncate text-[11px] text-muted-foreground">{available ? app.description || id : id}</span>
                  </span>
                  {available ? (
                    <Switch on={enabled} label={`Enable app ${id}`} onToggle={() => setAppEnabled(id, !enabled)} />
                  ) : (
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Remove unavailable app ${id}`}
                      disabled={saving}
                      onClick={() => removeApp(id)}
                    >
                      <Trash2 aria-hidden size={14} />
                    </Button>
                  )}
                </div>
                {tools.length > 0 ? (
                  <div className="pl-[46px]">
                    {tools.map((tool) => {
                      const toolOn = detail.pluginTools.includes(tool.id);
                      return (
                        <div
                          key={tool.id}
                          data-testid={`app-tool-row-${tool.id}`}
                          className="flex items-center gap-3 border-t border-border/60 py-2.5 pr-[18px]"
                        >
                          <span className="min-w-0 flex-1">
                            <span className={`block text-[12.5px] font-medium${tool.available ? "" : " text-destructive"}`}>
                              {tool.available ? tool.label : `${tool.label} (unavailable)`}
                            </span>
                            <span className="block truncate text-[11px] text-muted-foreground">{tool.description || tool.id}</span>
                          </span>
                          {tool.available ? (
                            <Switch on={toolOn} label={`Enable plugin tool ${tool.id}`} onToggle={() => setToolEnabled(tool.id, !toolOn)} />
                          ) : (
                            <Button
                              variant="ghost"
                              size="icon-sm"
                              aria-label={`Remove unavailable plugin tool ${tool.id}`}
                              disabled={saving}
                              onClick={() => setToolEnabled(tool.id, false)}
                            >
                              <Trash2 aria-hidden size={14} />
                            </Button>
                          )}
                        </div>
                      );
                    })}
                  </div>
                ) : null}
              </SettingsCard>
            </div>
          );
        })
      )}
    </div>
  );
}
