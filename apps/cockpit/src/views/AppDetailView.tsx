import { openUrl } from "@tauri-apps/plugin-opener";
import { MoreHorizontal, RefreshCw, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
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
import { Chip, Pill, StatusDot } from "@/components/common/bits";
import { ManualOauthClientModal } from "@/components/modals/ManualOauthClientModal";
import { PluginToolsList } from "@/components/plugins/PluginToolsList";
import { NATIVE_AGENT } from "@/constants";
import { appStatusToHubStatus, statusPresentation } from "@/lib/plugin-hub";
import { agentAllowed, appById, useApps } from "@/store-apps";
import { useGateways } from "@/store-gateways";
import { useNav } from "@/store-nav";
// Task 12: folds this MCP-server detail page into the same tabbed template
// `PluginDetailView` established (Task 9) — `visibleTabs`/`DetailTab` are
// the shared contract, imported rather than redefined here.
import { visibleTabs, type DetailTab } from "./PluginDetailView";

const rowLabel = "w-[120px] shrink-0 text-[13px] font-medium";

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

// How long the post-Connect poll waits between refreshes, and the overall
// deadline before it gives up and offers "Try again" — the loopback callback
// server completes the token exchange out-of-band (Cockpit's own Rust
// process, per the Task 9 plan correction), so this is purely "has the
// server turned connected yet", not a provider-imposed interval. Mirrors
// `OauthProfileConnections.tsx`'s PKCE poll constants.
const CONNECT_POLL_INTERVAL_MS = 2000;
const CONNECT_POLL_TIMEOUT_MS = 5 * 60_000;

const TAB_LABEL: Record<DetailTab, string> = {
  overview: "Overview",
  tools: "Tools",
  // An MCP app never has its own commands/skills/hooks/jobs surface (Task
  // 14's Contents/Automations tabs are plugin-detail-only) — these two
  // entries exist purely so this `Record<DetailTab, string>` stays
  // exhaustive; `visibleTabs`'s `hasContents`/`hasAutomations: false` below
  // means neither ever appears in `tabs`.
  contents: "Contents",
  automations: "Automations",
  settings: "Settings",
  versions: "Versions",
  health: "Health",
};

export function AppDetailView({ id }: { id: string }) {
  const nav = useNav();
  const {
    apps,
    loaded,
    hydrate,
    probing,
    probe,
    remove,
    setScope,
    setToolPerm,
    toggleAgent,
    beginMcpConnect,
    disconnectMcp,
    manualOauthClients,
    hydrateManualOauthClients,
    setManualOauthClient,
    deleteManualOauthClient,
  } = useApps();
  const gateways = useGateways((s) => s.gateways);
  const [tab, setTab] = useState<DetailTab>("overview");
  const [connectBusy, setConnectBusy] = useState(false);
  const [connectPending, setConnectPending] = useState(false);
  const [connectExpired, setConnectExpired] = useState(false);
  const [clientIdOpen, setClientIdOpen] = useState(false);
  // The connect flow's IDENTITY, bumped by every Connect and by Cancel. Each
  // run of `startConnect` captures it and guards every state write with it, so
  // only the newest flow can write — the flow-identity guard
  // `OauthProfileConnections.tsx` uses, which this view had dropped in favour
  // of one shared boolean. That boolean was reset to `false` on entry, so
  // Cancel-then-Connect inside a single 2 s tick RESURRECTED the cancelled
  // loop: it woke, read `false`, and polled alongside the new one, then its
  // own earlier deadline fired `setConnectExpired(true)` — the card claiming
  // "The sign-in link expired" and hiding Cancel while a live flow and its
  // Rust loopback listener were still running, and a success double-toasting.
  const connectGenRef = useRef(0);
  // Set on unmount and never reset, so a loop that outlives the component
  // stops touching state after teardown.
  const unmountedRef = useRef(false);
  useEffect(() => () => void (unmountedRef.current = true), []);
  const goApps = () => nav.navigate({ kind: "plugins" });

  useEffect(() => {
    if (!loaded) void hydrate();
  }, [loaded, hydrate]);

  // Unconditional rather than gated on `oauthConnectAvailable`: `app` is not
  // resolved yet at this point in the component (hooks must run above the
  // `if (!app) return null` below), and the call is one cheap list read.
  useEffect(() => {
    void hydrateManualOauthClients();
  }, [hydrateManualOauthClients]);

  const app = appById(apps, id);
  if (!app) return null;

  const isProbing = probing === app.id;
  const presentation = statusPresentation(appStatusToHubStatus(app.status));

  // An MCP app is always "installed" the moment it's added (there is no
  // separate install step), it has no auth-tab concept of its own (auth
  // lives inline on the Connection card, same as before), Settings always
  // has content (Scope + Agent access), and it never has a Versions tab —
  // matches the Task 12 brief's fixed `visibleTabs` input.
  const tabs = visibleTabs({
    installed: true,
    hasTools: app.tools.length > 0,
    hasContents: false,
    hasAutomations: false,
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

  // Remote MCP server OAuth connect (Task 9). `beginMcpConnect` returns the
  // authorize URL immediately — the daemon has already discovered the
  // server's authorization server and registered a client. Cockpit's own
  // Rust process then captures the browser redirect in the background (the
  // plan's Task 9 correction: the loopback listener lives there, not the
  // daemon), so there is nothing more for this component to drive except
  // poll `list_apps` until the server reports connected — same shape as
  // `OauthProfileConnections.tsx`'s PKCE poll, just reading the refreshed
  // Apps list instead of `pluginReleaseDetail`.
  const startConnect = async () => {
    const gen = ++connectGenRef.current;
    // This flow is stale the moment Cancel or another Connect bumps the
    // generation (or the view unmounts) — checked before EVERY state write,
    // because an abandoned loop writing terminal state over a live flow is
    // exactly the failure this replaced.
    const stale = () => unmountedRef.current || connectGenRef.current !== gen;
    setConnectExpired(false);
    setConnectBusy(true);
    const start = await beginMcpConnect(app.id);
    // `busy` belongs to this flow: a second one cannot start while Connect is
    // disabled by it, so clearing it here can never un-disable another's.
    setConnectBusy(false);
    if (stale()) return;
    if (!start) {
      // The store already toasted it, but the daemon ALSO persisted the
      // reason on the row (`AppInfo::oauth_connect_error`) — and when that
      // reason is "record a client id for <issuer>", the issuer is a URL the
      // user has to copy. Refreshing puts it on the card, where a toast
      // cannot.
      await hydrate();
      return;
    }
    void openUrl(start.authorizeUrl);
    setConnectPending(true);

    const deadline = Date.now() + CONNECT_POLL_TIMEOUT_MS;
    while (Date.now() < deadline) {
      await sleep(CONNECT_POLL_INTERVAL_MS);
      if (stale()) return;
      await hydrate();
      if (stale()) return;
      const fresh = appById(useApps.getState().apps, app.id);
      if (fresh?.oauthTokenStored && !fresh.oauthReconnectRequired) {
        toast.success(`Connected ${app.name}`);
        setConnectPending(false);
        return;
      }
      // The exchange is refused/failed, and the daemon said why. Stopping
      // here is the difference between "the sign-in link expired" five
      // minutes from now and the actual reason, right now: the completion
      // runs in Cockpit's own background task, which discards the RPC error
      // (`apps_cmd.rs`'s `complete_local_mcp_callback`), so this persisted
      // field is the only thing that ever reaches the user. Rendered below,
      // straight off the row rather than into local state, so it survives
      // navigating away and back.
      if (fresh?.oauthConnectError) {
        setConnectPending(false);
        return;
      }
    }
    if (!stale()) {
      setConnectPending(false);
      setConnectExpired(true);
    }
  };

  // Abandons the in-flight loop BY IDENTITY rather than by a shared flag a
  // later Connect could clear, so the loop can never come back to life.
  const cancelConnect = () => {
    connectGenRef.current += 1;
    setConnectPending(false);
  };

  const onDisconnect = async () => {
    setConnectBusy(true);
    await disconnectMcp(app.id);
    setConnectBusy(false);
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
              {app.authKind === "env" && (
                <CardRow>
                  <span className={rowLabel}>Environment</span>
                  <span className="flex-1 font-mono text-xs text-muted-foreground">{app.authDetail ?? "—"}</span>
                </CardRow>
              )}
              {/* Gated on `oauthConnectAvailable`, NOT on `transport ===
                  "http"`. That comparison merely correlates with "the host
                  owns this server's credential": the daemon resolves auth by
                  whether the spec already carries an `Authorization` header
                  (`harness::native::mcp_http_credential`), and when it does —
                  atlassian-rovo's `Basic ${setting:…}` — a token connected
                  here is never used, never refreshed, never even read. This
                  card used to say "Not connected", offer Connect, take the
                  user through a real Atlassian consent screen, then show
                  "OAuth connected" while every session went on sending the
                  Basic header. The field is derived from that same predicate
                  server-side, so the two cannot drift. */}
              {app.oauthConnectAvailable ? (
                <>
                  <CardRow>
                    <span className={rowLabel}>OAuth</span>
                    <span className="flex-1 text-[12.5px] text-muted-foreground">
                      {app.oauthReconnectRequired
                        ? "Cockpit has a saved token for this server, but it needs to be reconnected."
                        : app.oauthTokenStored
                          ? "Cockpit has a saved OAuth token for this server."
                          : "Connect this server to an account before agents can use its tools."}
                    </span>
                    {app.oauthReconnectRequired ? (
                      <Pill variant="warn">Reconnect required</Pill>
                    ) : app.oauthTokenStored ? (
                      // "OAuth connected", not bare "Connected" — the hero's
                      // status pill (transport reachability) can ALSO read
                      // "Connected" at the same time, for a different thing.
                      <Pill variant="primary">OAuth connected</Pill>
                    ) : (
                      <Pill variant="secondary">Not connected</Pill>
                    )}
                    {app.oauthTokenStored && (
                      <Button variant="outline" size="sm" onClick={() => void onDisconnect()} disabled={connectBusy || connectPending}>
                        Disconnect
                      </Button>
                    )}
                    <Button size="sm" onClick={() => void startConnect()} disabled={connectBusy || connectPending}>
                      {connectBusy ? "Opening…" : app.oauthReconnectRequired || app.oauthTokenStored ? "Reconnect" : "Connect"}
                    </Button>
                  </CardRow>
                  <CardRow>
                    <span className={rowLabel}>Client ID</span>
                    <span className="flex-1 text-[12.5px] text-muted-foreground">
                      {manualOauthClients.length > 0
                        ? `${manualOauthClients.length} recorded for authorization servers that don't register apps automatically.`
                        : "Only needed if this server's authorization server won't register apps automatically."}
                    </span>
                    <Button variant="outline" size="sm" onClick={() => setClientIdOpen(true)}>
                      Client ID…
                    </Button>
                  </CardRow>
                  {connectPending && (
                    <div className="flex items-center gap-3 border-t border-border px-[18px] py-3">
                      <span className="text-[12.5px] text-muted-foreground">Waiting for you to finish signing in in the browser…</span>
                      <Button variant="ghost" size="sm" onClick={cancelConnect}>
                        Cancel
                      </Button>
                    </div>
                  )}
                  {/* A real failure outranks the timeout copy: "expired" is
                      what this card says when nothing at all came back, and
                      saying it over a refusal the daemon explained is how a
                      token exchange rejected in the first second read as a
                      five-minute hang. Hidden while a flow is pending, so the
                      previous attempt's reason never captions a live one. */}
                  {app.oauthConnectError && !connectPending ? (
                    <div className="flex items-start gap-3 border-t border-border px-[18px] py-3">
                      <span className="flex-1 text-[12.5px] text-muted-foreground">Sign-in failed: {app.oauthConnectError}</span>
                      <Button size="sm" onClick={() => void startConnect()}>
                        Try again
                      </Button>
                    </div>
                  ) : (
                    connectExpired &&
                    !connectPending && (
                      <div className="flex items-center gap-3 border-t border-border px-[18px] py-3">
                        <span className="text-[12.5px] text-muted-foreground">The sign-in link expired before you finished.</span>
                        <Button size="sm" onClick={() => void startConnect()}>
                          Try again
                        </Button>
                      </div>
                    )
                  )}
                </>
              ) : app.transport === "http" ? (
                <div className="px-[18px] py-3.5 text-[12.5px] text-muted-foreground">
                  Authenticated with a credential from this server's configuration — an API token, or a sign-in its plugin manages. There is
                  no separate OAuth connection to make here.
                </div>
              ) : (
                app.authKind !== "env" && (
                  <div className="px-[18px] py-3.5 text-[12.5px] text-muted-foreground">
                    No authentication configured — runs with the environment it inherits.
                  </div>
                )
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

        {clientIdOpen && (
          <ManualOauthClientModal
            serverName={app.name}
            clients={manualOauthClients}
            onClose={() => setClientIdOpen(false)}
            onSave={setManualOauthClient}
            onDelete={deleteManualOauthClient}
          />
        )}
      </div>
    </div>
  );
}
