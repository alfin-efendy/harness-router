import { openUrl } from "@tauri-apps/plugin-opener";
import { Check } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Button, FormField, Input, Switch } from "@ryuzi/ui";
import { commands, events, type PluginDetail } from "@/bindings";
import { StatusDot } from "@/components/common/bits";
import { isDeviceFlowConnectable, OauthProfileConnections } from "@/components/plugins/OauthProfileConnections";
import { PluginToolsList } from "@/components/plugins/PluginToolsList";
import { declaredToolEntries } from "@/lib/plugin-hub";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { useNav } from "@/store-nav";
import { usePlugins } from "@/store-plugins";
// `FieldRow`/`permissionSummaryRows` are the one already-built UI this task
// reuses rather than duplicating (spec: SettingsStep/PermissionsStep). This
// creates an import cycle (PluginDetailView also imports
// `UniversalInstallWizard`, which imports this module) — safe because both
// are `function` declarations (hoisted) never invoked at module-evaluation
// time, only from within these components' own render/callbacks.
import { FieldRow, permissionSummaryRows } from "@/views/PluginDetailView";
import type { WizardCtx } from "./UniversalInstallWizard";

// Per-step components for the universal install wizard (Task 14). Every step
// gets the same two props — `ctx` (shared read/write surface, see
// `WizardCtx` in `UniversalInstallWizard.tsx`) and `onNext` (the shell's
// advance-or-close action) — so the shell's dispatch stays a flat switch
// with no per-step wiring beyond that.

/** True only for the duration this component instance stays mounted — every
 *  step's async continuation checks it before touching state, so navigating
 *  away mid-fetch (Back/Continue/Skip, or the wizard closing) never fires a
 *  stale setState. Exported (Task 15) so the connector/skill-pack/provider
 *  adapters (`steps-connector.tsx`/`steps-skillpack.tsx`/`steps-provider.tsx`)
 *  share the same guard instead of redeclaring it. */
export function useMountedRef() {
  const ref = useRef(true);
  useEffect(() => {
    ref.current = true;
    return () => {
      ref.current = false;
    };
  }, []);
  return ref;
}

// ---------- Overview ----------

export function OverviewStep({ ctx }: { ctx: WizardCtx; onNext: () => void }) {
  const info = ctx.detail?.info;
  // Shared `declaredToolEntries` (`@/lib/plugin-hub`) — same manifest→
  // PluginToolEntry mapping `PluginDetailView`'s pre-install fallback uses —
  // the wizard has no live `plugin_tools` fetch of its own pre-install, so
  // this is always the declared (not live) list.
  const tools = declaredToolEntries(ctx.releaseDetail?.activeManifest ?? null);

  return (
    <div className="flex flex-col gap-3">
      <div>
        <div className="text-[13.5px] font-semibold">{info?.name ?? ctx.pluginId}</div>
        {ctx.detail?.publisher && <div className="mt-0.5 text-[11.5px] text-muted-foreground">{ctx.detail.publisher}</div>}
        <p className="m-0 mt-1.5 text-[12.5px] leading-[1.55] text-muted-foreground">{info?.description || "No description provided."}</p>
      </div>
      <PluginToolsList entries={tools} live={false} />
    </div>
  );
}

// ---------- Permissions ----------

/** Continue disabled until the user accepts — the shell's `setContinueDisabled`
 *  is the one mechanism a step can use to gate the footer button (see
 *  `WizardCtx`'s doc); this is the only step in the plan that uses it. The
 *  effect clears it again on unmount so leaving this step never leaves a
 *  later step's Continue stuck disabled. */
export function PermissionsStep({ ctx }: { ctx: WizardCtx; onNext: () => void }) {
  const [accepted, setAccepted] = useState(false);
  const rows = permissionSummaryRows(ctx.releaseDetail?.activeManifest ?? null);

  useEffect(() => {
    ctx.setContinueDisabled(!accepted);
    return () => ctx.setContinueDisabled(false);
  }, [accepted, ctx]);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1.5 text-[12.5px]">
        {rows.map((r) => (
          <div key={r.label} className="flex gap-2">
            <span className="w-[75px] shrink-0 font-medium text-muted-foreground">{r.label}</span>
            <span className="min-w-0 flex-1 break-words">{r.value}</span>
          </div>
        ))}
      </div>
      <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
        <span className="text-[12.5px] font-medium">I understand and accept these permissions</span>
        <Switch on={accepted} onToggle={() => setAccepted((v) => !v)} label="Accept permissions" />
      </div>
    </div>
  );
}

// ---------- Install ----------

/** Runs `installComponentPlugin` once on mount, auto-advancing on success
 *  (after `ctx.refresh()` so the next step sees the freshly installed
 *  release). `startedRef` guards the automatic mount-time attempt against
 *  re-firing if `attempt`'s identity changes (e.g. `ctx` changes after a
 *  successful `refresh()`) — a manual Retry click bypasses that guard on
 *  purpose, since it's a deliberate second attempt, not a re-entrant one. */
export function InstallComponentStep({ ctx, onNext }: { ctx: WizardCtx; onNext: () => void }) {
  const installComponentPlugin = usePlugins((s) => s.installComponentPlugin);
  const [status, setStatus] = useState<"installing" | "error">("installing");
  const mountedRef = useMountedRef();
  const startedRef = useRef(false);

  const attempt = useCallback(async () => {
    setStatus("installing");
    const res = await installComponentPlugin(ctx.pluginId);
    if (!mountedRef.current) return;
    if (res) {
      await ctx.refresh();
      if (!mountedRef.current) return;
      onNext();
    } else {
      setStatus("error");
    }
  }, [ctx, installComponentPlugin, mountedRef, onNext]);

  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    void attempt();
  }, [attempt]);

  return (
    <div className="flex flex-col items-center gap-3 py-6">
      {status === "installing" ? (
        <div className="flex items-center gap-2 text-[13px] text-muted-foreground">
          <StatusDot color="#3B82F6" size={8} pulse />
          Installing…
        </div>
      ) : (
        <>
          <div className="text-[13px] text-muted-foreground">Install failed — check the error above, then try again.</div>
          <Button size="sm" onClick={() => void attempt()}>
            Retry
          </Button>
        </>
      )}
    </div>
  );
}

// ---------- Connect ----------

/** Top-level plugin OAuth (non-component `detail.auth.kind === "oauth"`):
 *  begins the browser flow on mount, listens for the loopback callback's
 *  completion event (same `pluginOauthCompletedMsg` `PluginDetailView` and
 *  the connector adapter's own oauth-wait sub-state, `steps-connector.tsx`,
 *  also listen for) and auto-advances once it lands ok; a manual paste-code
 *  fallback covers the case the loopback never fires. */
function PluginOauthConnect({ ctx, onNext }: { ctx: WizardCtx; onNext: () => void }) {
  const [authorizeUrl, setAuthorizeUrl] = useState("");
  const [redirectUri, setRedirectUri] = useState("");
  const [stateToken, setStateToken] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState<"begin" | "complete" | null>(null);
  const mountedRef = useMountedRef();
  const startedRef = useRef(false);

  const begin = useCallback(async () => {
    setBusy("begin");
    const res = await commands.beginPluginOauth(LOCAL_RUNNER, ctx.pluginId);
    if (!mountedRef.current) return;
    setBusy(null);
    if (res.status === "error") {
      toast.error(res.error.message);
      return;
    }
    setStateToken(res.data.stateToken);
    setAuthorizeUrl(res.data.authorizeUrl);
    setRedirectUri(res.data.redirectUri);
  }, [ctx.pluginId, mountedRef]);

  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    void begin();
  }, [begin]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;

    void events.pluginOauthCompletedMsg
      .listen((event) => {
        if (!active || event.payload.pluginId !== ctx.pluginId) return;
        if (!event.payload.ok) {
          toast.error(event.payload.error ?? "OAuth sign-in didn't finish.");
          return;
        }
        toast.success("Connected");
        void ctx.refresh().then(() => {
          if (active) onNext();
        });
      })
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [ctx, onNext]);

  const completeCode = async () => {
    if (!stateToken || code.trim().length === 0 || busy) return;
    setBusy("complete");
    const res = await commands.completePluginOauth(LOCAL_RUNNER, ctx.pluginId, code.trim(), stateToken);
    if (!mountedRef.current) return;
    setBusy(null);
    if (res.status === "error") {
      toast.error(res.error.message);
      return;
    }
    toast.success("Connected");
    await ctx.refresh();
    if (mountedRef.current) onNext();
  };

  return (
    <div className="flex flex-col gap-3">
      <p className="m-0 text-[12.5px] text-muted-foreground">
        {busy === "begin"
          ? "Preparing sign-in…"
          : "Sign in with your account — this finishes automatically. If the browser doesn't redirect back, paste the code below."}
      </p>
      {authorizeUrl && (
        <div>
          <Button size="sm" onClick={() => void openUrl(authorizeUrl)}>
            Open sign-in
          </Button>
        </div>
      )}
      <FormField label="Authorization code">
        <Input value={code} onChange={(e) => setCode(e.target.value)} placeholder="Paste the code value from the callback URL" />
      </FormField>
      {redirectUri && (
        <p className="m-0 text-xs text-muted-foreground">
          Callback URL: <span className="font-mono text-[11px]">{redirectUri}</span>
        </p>
      )}
      <div className="flex justify-end">
        <Button size="sm" onClick={() => void completeCode()} disabled={busy !== null || !stateToken || code.trim().length === 0}>
          {busy === "complete" ? "Connecting…" : "Finish connect"}
        </Button>
      </div>
    </div>
  );
}

/** Token/api-key auth (non-oauth `detail.auth.setting`): one `FieldRow`,
 *  same shape the Settings tab's own credential row uses. Exported (Task 15)
 *  so `steps-connector.tsx`'s `ConnectorConnectStep` reuses it verbatim for
 *  a classic connector's token/api-key sub-state, instead of duplicating it. */
export function TokenConnect({ ctx, auth }: { ctx: WizardCtx; auth: NonNullable<PluginDetail["auth"]> }) {
  const [value, setValue] = useState("");
  const [saving, setSaving] = useState(false);
  const mountedRef = useMountedRef();

  const save = async () => {
    if (!auth.setting || value.trim().length === 0 || saving) return;
    setSaving(true);
    const res = await commands.setPluginSetting(LOCAL_RUNNER, auth.setting, value.trim());
    if (!mountedRef.current) return;
    setSaving(false);
    if (res.status === "error") {
      toast.error(res.error.message);
      return;
    }
    toast.success("Saved");
    setValue("");
    await ctx.refresh();
  };

  return (
    <FieldRow
      label="Credential"
      help={auth.env ? `Falls back to the ${auth.env} environment variable if unset.` : undefined}
      secret
      required
      valueSet={auth.configured}
      value={value}
      onChange={setValue}
      onSave={() => void save()}
      saving={saving}
    />
  );
}

/** Dispatches on how this plugin connects (spec §5): a component's declared
 *  device-flow OAuth profiles first (reuses `OauthProfileConnections`
 *  wholesale — same card the Settings tab renders); else the top-level auth
 *  spec's own kind (oauth browser flow, or a token/api-key field); else a
 *  plain "nothing to do" message (e.g. a provider with no auth spec at all —
 *  today's launch points never route a provider through this wizard, but the
 *  plan always includes a connect step for `kind === "provider"`, so this
 *  stays crash-free for it regardless). */
export function ConnectStep({ ctx, onNext }: { ctx: WizardCtx; onNext: () => void }) {
  const profiles = ctx.releaseDetail?.activeManifest?.oauthProfiles ?? [];
  if (profiles.some(isDeviceFlowConnectable)) {
    return <OauthProfileConnections pluginId={ctx.pluginId} profiles={profiles} onChanged={() => void ctx.refresh()} />;
  }

  const auth = ctx.detail?.auth ?? null;
  if (auth?.kind === "oauth") return <PluginOauthConnect ctx={ctx} onNext={onNext} />;
  if (auth?.setting) return <TokenConnect ctx={ctx} auth={auth} />;

  return (
    <div className="text-[13px] text-muted-foreground">
      {auth?.env ? (
        <>
          Nothing more to connect here — set the <span className="font-mono text-xs">{auth.env}</span> environment variable if needed.
        </>
      ) : (
        "Nothing more to connect here."
      )}
    </div>
  );
}

// ---------- Settings ----------

export function SettingsStep({ ctx }: { ctx: WizardCtx; onNext: () => void }) {
  const [values, setValues] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState<string | null>(null);
  const mountedRef = useMountedRef();
  const fields = ctx.detail?.settings ?? [];

  const save = async (key: string, raw: string) => {
    const value = raw.trim();
    if (value.length === 0 || saving) return;
    setSaving(key);
    const res = await commands.setPluginSetting(LOCAL_RUNNER, key, value);
    if (!mountedRef.current) return;
    if (res.status === "error") toast.error(res.error.message);
    else {
      toast.success("Saved");
      setValues((v) => ({ ...v, [key]: "" }));
      await ctx.refresh();
    }
    if (mountedRef.current) setSaving(null);
  };

  if (fields.length === 0) {
    return <div className="text-[13px] text-muted-foreground">No settings to configure.</div>;
  }

  return (
    <div className="flex flex-col">
      {fields.map((f) => (
        <FieldRow
          key={f.key}
          label={f.label}
          help={f.help || undefined}
          kind={f.kind}
          secret={f.secret}
          required={f.required}
          valueSet={f.valueSet}
          value={values[f.key] ?? ""}
          options={f.options}
          defaultValue={f.default}
          onChange={(v) => setValues((m) => ({ ...m, [f.key]: v }))}
          onSave={(v) => void save(f.key, v)}
          saving={saving === f.key}
        />
      ))}
    </div>
  );
}

// ---------- Done ----------

export function DoneStep({ ctx, onNext }: { ctx: WizardCtx; onNext: () => void }) {
  const nav = useNav();
  const loadTools = usePlugins((s) => s.loadTools);
  const toolsById = usePlugins((s) => s.toolsById);
  const toolsLiveById = usePlugins((s) => s.toolsLiveById);
  const startedRef = useRef(false);
  const enableStartedRef = useRef(false);

  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    void loadTools(ctx.pluginId);
  }, [ctx.pluginId, loadTools]);

  // Task 15: a classic (non-component) connector's install commits here —
  // ported from the retired catalog install modal's own done-effect
  // (`setPluginEnabled` unless experimental). Component/provider/skill-pack
  // installs already enable (or have no such concept — see
  // `curated_pack_row`'s doc) as part of their own install call, so this
  // stays scoped to the one kind that never otherwise flips it.
  const info = ctx.detail?.info;
  const isClassicConnector = !!info && !info.componentBacked && info.kind !== "provider" && info.kind !== "skill-pack";
  useEffect(() => {
    if (!isClassicConnector || enableStartedRef.current || info?.experimental) return;
    enableStartedRef.current = true;
    void commands.setPluginEnabled(LOCAL_RUNNER, ctx.pluginId, true).then((res) => {
      if (res.status === "error") toast.error(res.error.message);
    });
  }, [isClassicConnector, info, ctx.pluginId]);

  const entries = toolsById[ctx.pluginId] ?? [];
  const live = toolsLiveById[ctx.pluginId] ?? false;

  // Reuses the shell's own advance mechanism to close — `onNext` on the
  // (always-last) done step calls the shell's `onClose`, so this doesn't
  // need its own separate close handle.
  const openPluginPage = () => {
    nav.navigate({ kind: "pluginDetail", id: ctx.pluginId });
    onNext();
  };

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2 text-[13px] font-medium">
        <Check aria-hidden size={16} strokeWidth={2.5} style={{ color: "#22C55E" }} />
        Installed
      </div>
      <PluginToolsList entries={entries} live={live} />
      <div className="flex justify-end">
        <Button size="sm" onClick={openPluginPage}>
          Open plugin page
        </Button>
      </div>
    </div>
  );
}
