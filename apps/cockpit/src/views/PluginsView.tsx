import { useEffect, useMemo, useState } from "react";
import { CircleAlert, Plus, RefreshCw, Search } from "lucide-react";
import { toast } from "sonner";
import { Button, Input, Menu, MenuContent, MenuItem, MenuTrigger, SettingsCard as Card } from "@ryuzi/ui";
import { commands, type ComponentBootstrapStatus } from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { DoctorPanel } from "@/components/DoctorPanel";
import { HubRail } from "@/components/plugins/HubRail";
import { HubRow } from "@/components/plugins/HubRow";
import { useApps } from "@/store-apps";
import { summarizeUpdateAll, usePlugins } from "@/store-plugins";
import { useSkills } from "@/store-skills";
import { AddAppModal } from "@/components/modals/AddAppModal";
import { InstallFromSourceModal } from "@/components/modals/InstallFromSourceModal";
import { SkillInstallModal } from "@/components/modals/SkillInstallModal";
import { UniversalInstallWizard } from "@/components/modals/wizard/UniversalInstallWizard";
import { useNav } from "@/store-nav";
import { buildHubItems, filterHubItems, type HubItem, type RailFilter } from "@/lib/plugin-hub";

const WARN = "#F59E0B";

// Rail-footer catalog status line moved to `HubRail.tsx` (Task 8) — this
// module re-exports it so existing importers (and this file's own history)
// keep resolving `catalogStatusLabel` from "@/views/PluginsView".
export { catalogStatusLabel } from "@/components/plugins/HubRail";

/** The retryable bootstrap banner's message, or `null` when there's nothing
 *  to show (not yet loaded, or the last automatic attempt at daemon start
 *  fully completed). Pure and exported so it stays unit-testable without
 *  mounting the view. */
export function bootstrapBannerMessage(status: ComponentBootstrapStatus | null): string | null {
  if (!status?.pending) return null;
  return status.message ?? "Some first-party component plugins couldn't be installed automatically.";
}

export function PluginsView() {
  const nav = useNav();
  const { apps, loaded: appsLoaded, hydrate } = useApps();
  const {
    plugins,
    loaded: pluginsLoaded,
    load: loadPlugins,
    catalogStatus,
    refreshCatalog,
    componentBootstrapStatus,
    loadComponentBootstrapStatus,
    retryComponentBootstrap,
  } = usePlugins();
  const skills = useSkills((s) => s.skills);
  const refreshSkills = useSkills((s) => s.refresh);

  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<RailFilter>({ state: "all", surface: null, category: null });
  const [addAppOpen, setAddAppOpen] = useState(false);
  const [installFromSourceOpen, setInstallFromSourceOpen] = useState(false);
  const [skillInstall, setSkillInstall] = useState<{ initialSource?: string } | null>(null);
  const [updatingAll, setUpdatingAll] = useState(false);
  const [doctorOpen, setDoctorOpen] = useState(false);
  const [refreshingCatalog, setRefreshingCatalog] = useState(false);
  const [retryingBootstrap, setRetryingBootstrap] = useState(false);

  useEffect(() => {
    void hydrate();
  }, [hydrate]);

  useEffect(() => {
    if (!pluginsLoaded) void loadPlugins();
  }, [pluginsLoaded, loadPlugins]);

  // Component (WASM bundle) plugins now ARE `CorePlugin`s and appear in
  // `plugins` like any other row; this fetch only adds the daemon-start
  // bootstrap-retry banner's status, which a `PluginInfo` row does not carry.
  useEffect(() => {
    void loadComponentBootstrapStatus();
  }, [loadComponentBootstrapStatus]);

  useEffect(() => {
    void refreshSkills();
  }, [refreshSkills]);

  const items = useMemo(() => buildHubItems({ plugins, apps, skills }), [plugins, apps, skills]);
  const rows = useMemo(() => filterHubItems(items, filter, query), [items, filter, query]);
  const updateAllEnabled = useMemo(
    () => items.some((i) => i.status === "update-available") || items.some((i) => i.surfaces.includes("skills") && i.installed),
    [items],
  );

  // Task 15: every kind's Install now opens the universal wizard — the
  // provider/skill-pack/connector adapters (`steps-provider.tsx`/
  // `steps-skillpack.tsx`/`steps-connector.tsx`) each know how to run their
  // own install action from inside it. `SkillInstallModal` stays for the
  // ONE path that isn't a Browse-tile install: "+ Add ▾ → Add skill source"
  // manual entry (`setSkillInstall({})`, below), where there's no catalog
  // `HubItem`/plugin id yet for the wizard to fetch a detail for.
  const [wizardPluginId, setWizardPluginId] = useState<string | null>(null);
  const installBusy = skillInstall !== null || wizardPluginId !== null;

  const startInstall = (item: HubItem) => {
    if (installBusy) return;
    setWizardPluginId(item.id);
  };

  const openItem = (item: HubItem, tab?: "settings" | "health" | "versions") => {
    if (tab && item.nav.kind === "pluginDetail") {
      nav.navigate({ kind: "pluginDetail", id: item.nav.id, tab });
      return;
    }
    nav.navigate(item.nav);
  };

  const runUpdateAll = async () => {
    if (updatingAll) return;
    setUpdatingAll(true);
    const res = await commands.updateAllPlugins(LOCAL_RUNNER);
    setUpdatingAll(false);
    if (res.status === "error") {
      toast.error(`Update all failed: ${res.error.message}`);
      return;
    }
    toast.success(`Update all — ${summarizeUpdateAll(res.data)}`);
    await loadPlugins();
  };

  const runRefreshCatalog = async () => {
    if (refreshingCatalog) return;
    setRefreshingCatalog(true);
    await refreshCatalog();
    setRefreshingCatalog(false);
  };

  const runRetryBootstrap = async () => {
    if (retryingBootstrap) return;
    setRetryingBootstrap(true);
    await retryComponentBootstrap();
    setRetryingBootstrap(false);
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-8 py-7">
      <div className="mx-auto max-w-[980px]">
        <div className="mb-5 flex items-center gap-3">
          <h2 className="m-0 flex-1 text-[22px] font-semibold tracking-[-0.02em]">Plugins</h2>
          <div className="relative w-[260px]">
            <Search aria-hidden size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
            <Input className="pl-9" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search plugins, tools, skills" />
          </div>
          <Menu>
            <MenuTrigger
              render={
                <Button variant="outline">
                  <Plus size={14} /> Add
                </Button>
              }
            />
            <MenuContent>
              <MenuItem onClick={() => setAddAppOpen(true)}>Add MCP server</MenuItem>
              <MenuItem onClick={() => setSkillInstall({})}>Add skill source</MenuItem>
            </MenuContent>
          </Menu>
          <Button variant="outline" onClick={() => setInstallFromSourceOpen(true)}>
            Install from source…
          </Button>
        </div>

        {bootstrapBannerMessage(componentBootstrapStatus) && (
          <Card className="mb-3 flex items-start gap-3 px-[18px] py-3.5">
            <CircleAlert aria-hidden size={16} strokeWidth={2} className="mt-px shrink-0" style={{ color: WARN }} />
            <div className="min-w-0 flex-1">
              <div className="text-[13.5px] font-semibold">Component plugins need attention</div>
              <div className="mt-1 text-[12.5px] text-muted-foreground">{bootstrapBannerMessage(componentBootstrapStatus)}</div>
            </div>
            <Button variant="outline" size="sm" onClick={() => void runRetryBootstrap()} disabled={retryingBootstrap} className="shrink-0">
              <RefreshCw aria-hidden size={13} strokeWidth={2} className={retryingBootstrap ? "animate-spin" : undefined} />
              {retryingBootstrap ? "Retrying…" : "Retry"}
            </Button>
          </Card>
        )}

        <div className="flex gap-6">
          <HubRail
            items={items}
            filter={filter}
            onChange={setFilter}
            catalogStatus={catalogStatus}
            onRefreshCatalog={() => void runRefreshCatalog()}
            refreshing={refreshingCatalog}
            onOpenDoctor={() => setDoctorOpen(true)}
            updateAllEnabled={updateAllEnabled}
            updatingAll={updatingAll}
            onUpdateAll={() => void runUpdateAll()}
          />
          <div className="min-w-0 flex-1">
            {rows.map((item) => (
              <HubRow key={item.rowKey} item={item} onInstall={() => startInstall(item)} onOpen={(tab) => openItem(item, tab)} />
            ))}
            {appsLoaded && pluginsLoaded && rows.length === 0 && (
              <Card className="p-6 text-center text-[13px] text-muted-foreground">
                {items.length === 0
                  ? "Nothing here yet. Browse the catalog or add an MCP server by hand."
                  : "No plugins match this filter."}
              </Card>
            )}
          </div>
        </div>
      </div>
      {addAppOpen && <AddAppModal onClose={() => setAddAppOpen(false)} />}
      {installFromSourceOpen && (
        <InstallFromSourceModal
          onClose={() => {
            setInstallFromSourceOpen(false);
            void loadPlugins();
          }}
        />
      )}
      {skillInstall && (
        <SkillInstallModal
          initialSource={skillInstall.initialSource}
          onClose={() => {
            setSkillInstall(null);
            void loadPlugins();
            void refreshSkills();
          }}
        />
      )}
      {wizardPluginId && (
        <UniversalInstallWizard
          pluginId={wizardPluginId}
          onClose={() => {
            setWizardPluginId(null);
            void loadPlugins();
            // A skill-pack install closes through this same path now (Task
            // 15) — refresh skills too, same as `SkillInstallModal`'s onClose
            // used to, so a fresh pack's rows show up without a manual reload.
            void refreshSkills();
          }}
        />
      )}
      {doctorOpen && <DoctorPanel onClose={() => setDoctorOpen(false)} />}
    </div>
  );
}
