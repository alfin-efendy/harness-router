import { MoreHorizontal, RefreshCw, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import {
  Button,
  Menu,
  MenuContent,
  MenuItem,
  MenuTrigger,
  Segmented,
  SettingsCard as Card,
  SettingsCardHeader as CardHeader,
  SettingsCardHint as CardHint,
  SettingsCardRow as CardRow,
  SettingsCardTitle as CardTitle,
  Switch,
} from "@ryuzi/ui";
import type { PluginToolEntry } from "@/bindings";
import { BackButton, DetailHeader } from "@/components/common/DetailHeader";
import { Chip, StatusDot } from "@/components/common/bits";
import { PluginToolsList } from "@/components/plugins/PluginToolsList";
import { NATIVE_AGENT } from "@/constants";
import { statusPresentation, type HubStatus } from "@/lib/plugin-hub";
import { agentAllowed, appById, useApps } from "@/store-apps";
import { useGateways } from "@/store-gateways";
import { useNav } from "@/store-nav";
// Task 12: folds this MCP-server detail page into the same tabbed template
// `PluginDetailView` established (Task 9) — `visibleTabs`/`DetailTab` are
// the shared contract, imported rather than redefined here.
import { visibleTabs, type DetailTab } from "./PluginDetailView";

const rowLabel = "w-[120px] shrink-0 text-[13px] font-medium";

// `AppInfo.status` is the app-side `connected|error|unknown` vocabulary —
// the same one `plugin-hub.ts`'s `appToHubItem` already translates onto the
// shared `HubStatus` union for the hero status pill on the hub list, so the
// detail page's pill speaks the same language as the row it was opened from.
const APP_STATUS_MAP: Record<string, HubStatus> = {
  connected: "ok",
  error: "attach-failed",
  unknown: "unchecked",
};

const TAB_LABEL: Record<DetailTab, string> = {
  overview: "Overview",
  tools: "Tools",
  settings: "Settings",
  versions: "Versions",
  health: "Health",
};

export function AppDetailView({ id }: { id: string }) {
  const nav = useNav();
  const { apps, loaded, hydrate, probing, probe, remove, setScope, setToolPerm, toggleAgent } = useApps();
  const gateways = useGateways((s) => s.gateways);
  const [tab, setTab] = useState<DetailTab>("overview");
  const goApps = () => nav.navigate({ kind: "plugins" });

  useEffect(() => {
    if (!loaded) void hydrate();
  }, [loaded, hydrate]);

  const app = appById(apps, id);
  if (!app) return null;

  const isProbing = probing === app.id;
  const presentation = statusPresentation(APP_STATUS_MAP[app.status] ?? "unchecked");

  // An MCP app is always "installed" the moment it's added (there is no
  // separate install step), it has no auth-tab concept of its own (auth
  // lives inline on the Connection card, same as before), Settings always
  // has content (Scope + Agent access), and it never has a Versions tab —
  // matches the Task 12 brief's fixed `visibleTabs` input.
  const tabs = visibleTabs({
    installed: true,
    hasTools: app.tools.length > 0,
    hasAuth: false,
    hasSettings: true,
    hasVersions: false,
    hasHealth: app.status === "error",
  });
  const activeTab: DetailTab = tabs.includes(tab) ? tab : "overview";

  const toolEntries: PluginToolEntry[] = app.tools.map((t) => ({ name: t.name, description: t.desc, kind: "tool", writes: null }));

  const onRemove = () => {
    void remove(app.id);
    nav.goBack();
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-8 pb-10 pt-[22px]">
      <div className="mx-auto max-w-[720px]">
        <BackButton label="Plugins" onClick={goApps} />

        <DetailHeader
          chip={<Chip initial={app.initial} color={app.color} size={44} mono />}
          title={app.name}
          sub={[app.kind, app.version ? `v${app.version}` : null, app.publisher].filter(Boolean).join(" · ")}
        >
          <span className="flex shrink-0 items-center gap-1.5 text-xs" style={{ color: presentation.color ?? "var(--muted-foreground)" }}>
            <StatusDot color={presentation.color ?? "var(--muted-foreground)"} />
            {presentation.label}
          </span>
          <Button variant="outline" onClick={() => void probe(app.id)} disabled={isProbing}>
            <RefreshCw aria-hidden size={13} strokeWidth={2} className={isProbing ? "size-[13px] animate-spin" : "size-[13px]"} />
            {isProbing ? "Probing…" : "Probe"}
          </Button>
          <Menu>
            <MenuTrigger
              render={
                <Button variant="ghost" size="icon-sm" aria-label={`Actions for ${app.name}`}>
                  <MoreHorizontal aria-hidden size={15} strokeWidth={2} />
                </Button>
              }
            />
            <MenuContent>
              <MenuItem onClick={onRemove} className="text-destructive">
                <Trash2 aria-hidden size={13} strokeWidth={2} />
                Remove
              </MenuItem>
            </MenuContent>
          </Menu>
        </DetailHeader>

        <div className="mb-4 overflow-x-auto">
          <Segmented options={tabs.map((t) => ({ id: t, label: TAB_LABEL[t] }))} value={activeTab} onChange={setTab} />
        </div>

        {activeTab === "overview" && (
          <div data-testid="tab-panel-overview">
            <Card className="mb-3">
              <CardHeader>
                <CardTitle>About</CardTitle>
              </CardHeader>
              <div className="px-[18px] py-3.5 text-[12.5px] leading-[1.55] text-muted-foreground">
                {app.desc || "No description provided."}
              </div>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>Connection</CardTitle>
              </CardHeader>
              <CardRow>
                <span className={rowLabel}>{app.transport === "http" ? "URL" : "Command"}</span>
                <span className="flex-1 truncate font-mono text-xs text-muted-foreground">
                  {app.transport === "http" ? (app.url ?? "—") : [app.command, ...app.args].filter(Boolean).join(" ")}
                </span>
              </CardRow>
              {app.authKind === "env" ? (
                <CardRow>
                  <span className={rowLabel}>Environment</span>
                  <span className="flex-1 font-mono text-xs text-muted-foreground">{app.authDetail ?? "—"}</span>
                </CardRow>
              ) : (
                <div className="px-[18px] py-3.5 text-[12.5px] text-muted-foreground">
                  No authentication configured — runs with the environment it inherits.
                </div>
              )}
            </Card>
          </div>
        )}

        {activeTab === "tools" && (
          <div data-testid="tab-panel-tools">
            <PluginToolsList
              entries={toolEntries}
              live
              renderTrailing={(name) => {
                const t = app.tools.find((x) => x.name === name);
                if (!t) return null;
                return (
                  <Segmented
                    size="sm"
                    options={[
                      { id: "allow", label: "Allow" },
                      { id: "ask", label: "Ask" },
                      { id: "deny", label: "Deny" },
                    ]}
                    value={t.perm}
                    onChange={(perm) => void setToolPerm(app.id, t.name, perm)}
                  />
                );
              }}
            />
          </div>
        )}

        {activeTab === "settings" && (
          <div data-testid="tab-panel-settings">
            <Card className="mb-3">
              <CardHeader>
                <CardTitle>Scope</CardTitle>
                <CardHint>Where this plugin is attached</CardHint>
                <span className="flex-1" />
                <Segmented
                  options={[
                    { id: "global", label: "Global" },
                    { id: "select", label: "Select" },
                  ]}
                  value={app.scope}
                  onChange={(scope) => void setScope(app.id, scope, app.scopeGateways)}
                />
              </CardHeader>
              {app.scope === "select" && (
                <div className="flex flex-wrap gap-1.5 px-[18px] py-3">
                  {gateways.map((w) => {
                    const sel = app.scopeGateways.includes(w.id);
                    return (
                      <Button
                        key={w.id}
                        variant={sel ? "default" : "outline"}
                        size="sm"
                        onClick={() =>
                          void setScope(app.id, app.scope, sel ? app.scopeGateways.filter((x) => x !== w.id) : [...app.scopeGateways, w.id])
                        }
                        className="rounded-full"
                      >
                        <span className="font-mono text-[9.5px] font-semibold opacity-75">{w.badge}</span>
                        {w.name}
                      </Button>
                    );
                  })}
                </div>
              )}
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>Agent access</CardTitle>
                <CardHint>Whether the agent may call this plugin</CardHint>
              </CardHeader>
              <div className="flex items-center gap-3 px-[18px] py-[11px]">
                <StatusDot color={NATIVE_AGENT.color} size={8} />
                <span className="min-w-0 flex-1">
                  <span className="block text-[13px] font-medium">Allow the agent to use this app</span>
                  <span className="block text-[11px] text-muted-foreground">{NATIVE_AGENT.name} · applies to every session</span>
                </span>
                <Switch on={agentAllowed(app)} onToggle={() => void toggleAgent(app.id, !agentAllowed(app))} label="Agent access" />
              </div>
            </Card>
          </div>
        )}

        {activeTab === "health" && (
          <div data-testid="tab-panel-health">
            {app.statusDetail && (
              <Card className="px-[18px] py-3 text-[12.5px]">
                <span style={{ color: "#EF4444" }}>{app.statusDetail}</span>
              </Card>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
