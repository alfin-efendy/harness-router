import { openUrl } from "@tauri-apps/plugin-opener";
import { CircleAlert } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Button, FormField, Input } from "@ryuzi/ui";
import { commands, events, type PluginInstallBeginResult } from "@/bindings";
import { StatusDot } from "@/components/common/bits";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { TokenConnect, useMountedRef } from "./steps-component";
import type { WizardCtx } from "./UniversalInstallWizard";

// Classic (non-component, pre-component-catalog) connector adapter (Task
// 15) — ports the retired catalog install modal's single
// `begin_plugin_install` resolution + its token/manual-client-id/oauth-wait
// sub-states into the universal wizard's plan-based shell.
// `InstallConnectorStep` (below) is the "install" step; `ConnectorConnectStep`
// is the "connect" step. Both read/write `ctx.connectorBegin` (see
// `WizardCtx`'s doc) so a connect-step resume (the setup checklist's
// "Connect" action jumps straight past install) still works.

async function runBegin(pluginId: string): Promise<PluginInstallBeginResult | null> {
  const res = await commands.beginPluginInstall(LOCAL_RUNNER, pluginId);
  if (res.status === "error") {
    toast.error(res.error.message);
    return null;
  }
  return res.data;
}

// ---------- Install ----------

/** Runs `begin_plugin_install`'s 8-step resolution (env-var detection, RFC
 *  8414 discovery, DCR, and — when possible — starting the browser flow) —
 *  the same call the retired catalog install modal made at mount. There's no
 *  bundle to fetch for a classic connector (it's already a registered
 *  manifest), so this always advances to "connect" once resolved;
 *  `ConnectorConnectStep` is what actually decides whether there's anything
 *  left to collect. */
export function InstallConnectorStep({ ctx, onNext }: { ctx: WizardCtx; onNext: () => void }) {
  const [status, setStatus] = useState<"checking" | "error">("checking");
  const mountedRef = useMountedRef();
  const startedRef = useRef(false);

  const attempt = useCallback(async () => {
    setStatus("checking");
    const begin = await runBegin(ctx.pluginId);
    if (!mountedRef.current) return;
    if (!begin) {
      setStatus("error");
      return;
    }
    ctx.setConnectorBegin(begin);
    await ctx.refresh();
    if (!mountedRef.current) return;
    onNext();
  }, [ctx, mountedRef, onNext]);

  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    void attempt();
  }, [attempt]);

  return (
    <div className="flex flex-col items-center gap-3 py-6">
      {status === "checking" ? (
        <div className="flex items-center gap-2 text-[13px] text-muted-foreground">
          <StatusDot color="#3B82F6" size={8} pulse />
          Checking configuration…
        </div>
      ) : (
        <>
          <div className="text-[13px] text-muted-foreground">Couldn't check this plugin's configuration — try again.</div>
          <Button size="sm" onClick={() => void attempt()}>
            Retry
          </Button>
        </>
      )}
    </div>
  );
}

// ---------- Connect ----------

type ConnectSub = "probing" | "none" | "token" | "clientId" | "oauthWait" | "deadEnd";

// Mirrors the retired catalog install modal's `runBegin` branch order exactly
// (spec: env var / "none" auth need nothing; token/api-key collect a credential;
// external-oauth and needs-client-id both land on the manual client id
// form; an available oauth session waits on the browser flow; anything else
// is a dead end only Retry can resolve).
function subStateFor(begin: PluginInstallBeginResult): ConnectSub {
  if (begin.envVarPresent || begin.authKind === "none") return "none";
  if (begin.authKind === "api-key" || begin.authKind === "token") return "token";
  if (begin.authKind !== "oauth") return "none";
  if (begin.oauthExternal || begin.needsClientId) return "clientId";
  if (begin.oauthAvailable) return "oauthWait";
  return "deadEnd";
}

/** Manual OAuth client id sub-state — ports the retired catalog install
 *  modal's `manualClientId` step/`submitClientId` verbatim: saves the id, then
 *  (unless external-oauth, which brokers sign-in itself at first use)
 *  re-begins so the backend can retry DCR/discovery now that a client id is
 *  on the row. */
function ManualClientId({
  ctx,
  begin,
  onBegin,
  onNext,
}: {
  ctx: WizardCtx;
  begin: PluginInstallBeginResult;
  onBegin: (b: PluginInstallBeginResult) => void;
  onNext: () => void;
}) {
  const [clientId, setClientId] = useState("");
  const [busy, setBusy] = useState(false);
  const mountedRef = useMountedRef();

  const submit = async () => {
    if (clientId.trim().length === 0 || busy) return;
    setBusy(true);
    const saved = await commands.setPluginOauthClientId(LOCAL_RUNNER, ctx.pluginId, clientId.trim());
    if (!mountedRef.current) return;
    if (saved.status === "error") {
      setBusy(false);
      toast.error(saved.error.message);
      return;
    }
    if (begin.oauthExternal) {
      // The child server brokers the actual sign-in at first use — no
      // browser flow from Cockpit for external-OAuth plugins.
      setBusy(false);
      await ctx.refresh();
      if (mountedRef.current) onNext();
      return;
    }
    const res = await commands.beginPluginInstall(LOCAL_RUNNER, ctx.pluginId);
    if (!mountedRef.current) return;
    setBusy(false);
    if (res.status === "error") {
      toast.error(res.error.message);
      return;
    }
    ctx.setConnectorBegin(res.data);
    onBegin(res.data);
    if (!res.data.oauthAvailable) toast.error(res.data.dcrError ?? "Couldn't start the sign-in flow.");
  };

  return (
    <div className="flex flex-col gap-3">
      <p className="m-0 text-[12.5px] text-muted-foreground">
        {begin.oauthExternal
          ? "This plugin brokers its own sign-in the first time it runs. Create an OAuth client with the vendor and paste its client ID here."
          : "This plugin doesn't support automatic app registration. Create an OAuth app with the vendor and paste its client ID here."}
      </p>
      {begin.dcrError && (
        <div className="flex items-start gap-2 rounded-md border border-border px-4 py-3 text-[12.5px]" style={{ color: "#F59E0B" }}>
          <CircleAlert aria-hidden size={14} strokeWidth={2} className="mt-0.5 shrink-0" />
          {begin.dcrError}
        </div>
      )}
      <FormField label="OAuth client ID">
        <Input value={clientId} onChange={(e) => setClientId(e.target.value)} placeholder="Paste the client ID from the vendor's console" />
      </FormField>
      <div className="flex justify-end">
        <Button size="sm" disabled={busy || clientId.trim().length === 0} onClick={() => void submit()}>
          {busy ? "Saving…" : "Continue"}
        </Button>
      </div>
    </div>
  );
}

/** Browser oauth-wait sub-state — ports the retired catalog install modal's
 *  `waitingOauth` step: listens for the loopback callback's completion event
 *  (auto-advances on success), a manual paste-code fallback (and shuts down
 *  the loopback listener once a paste completes it, so it doesn't leak
 *  until the flow's own timeout), and a Retry that re-begins when the
 *  session itself failed. */
function OauthWait({
  ctx,
  begin,
  onBegin,
  onNext,
}: {
  ctx: WizardCtx;
  begin: PluginInstallBeginResult;
  onBegin: (b: PluginInstallBeginResult) => void;
  onNext: () => void;
}) {
  const [code, setCode] = useState("");
  const [pasteOpen, setPasteOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"complete" | "retry" | null>(null);
  const mountedRef = useMountedRef();

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;

    void events.pluginOauthCompletedMsg
      .listen((event) => {
        if (!active || event.payload.pluginId !== ctx.pluginId) return;
        if (!event.payload.ok) {
          setError(event.payload.error ?? "Sign-in didn't finish.");
          return;
        }
        setError(null);
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

  const retry = async () => {
    if (busy) return;
    setBusy("retry");
    setError(null);
    const res = await commands.beginPluginInstall(LOCAL_RUNNER, ctx.pluginId);
    if (!mountedRef.current) return;
    setBusy(null);
    if (res.status === "error") {
      setError(res.error.message);
      return;
    }
    ctx.setConnectorBegin(res.data);
    onBegin(res.data);
    if (!res.data.oauthAvailable) setError(res.data.dcrError ?? "Couldn't restart the sign-in flow.");
  };

  const finishPaste = async () => {
    const stateToken = begin.oauthBegin?.stateToken;
    if (!stateToken || code.trim().length === 0 || busy) return;
    setBusy("complete");
    const res = await commands.completePluginOauth(LOCAL_RUNNER, ctx.pluginId, code.trim(), stateToken);
    if (!mountedRef.current) return;
    setBusy(null);
    if (res.status === "error") {
      setError(res.error.message);
      return;
    }
    setError(null);
    // The loopback callback server is still listening for this flow's
    // redirect — a manual paste bypasses it, so shut it down explicitly or
    // it leaks until the flow's own timeout.
    await commands.cancelPluginInstall(LOCAL_RUNNER, ctx.pluginId, stateToken);
    await ctx.refresh();
    if (mountedRef.current) onNext();
  };

  return (
    <div className="flex flex-col gap-3">
      <p className="m-0 text-[13px] font-medium">Browser opened — finish signing in there.</p>
      <p className="m-0 text-[12.5px] text-muted-foreground">Cockpit is listening for the redirect and will finish automatically.</p>
      {error && (
        <div className="flex items-start gap-2 rounded-md border border-border px-4 py-3 text-[12.5px]" style={{ color: "#F59E0B" }}>
          <CircleAlert aria-hidden size={14} strokeWidth={2} className="mt-0.5 shrink-0" />
          {error}
        </div>
      )}
      {(pasteOpen || begin.callbackMode === "manual") && (
        <FormField label="Authorization code">
          <Input value={code} onChange={(e) => setCode(e.target.value)} placeholder="Paste the code value from the callback URL" />
        </FormField>
      )}
      <div className="flex flex-wrap items-center justify-end gap-2">
        {error && (
          <Button variant="outline" size="sm" disabled={busy !== null} onClick={() => void retry()}>
            Retry
          </Button>
        )}
        {!pasteOpen && begin.callbackMode !== "manual" && (
          <Button variant="ghost" size="sm" disabled={busy !== null} onClick={() => setPasteOpen(true)}>
            {error ? "Paste code instead" : "Having trouble? Paste the code manually"}
          </Button>
        )}
        {(pasteOpen || begin.callbackMode === "manual") && (
          <Button
            size="sm"
            disabled={busy !== null || code.trim().length === 0 || !begin.oauthBegin?.stateToken}
            onClick={() => void finishPaste()}
          >
            {busy === "complete" ? "Connecting…" : "Finish sign-in"}
          </Button>
        )}
        <Button
          variant="outline"
          size="sm"
          disabled={!begin.oauthBegin?.authorizeUrl}
          onClick={() => void openUrl(begin.oauthBegin?.authorizeUrl ?? "")}
        >
          Reopen browser
        </Button>
      </div>
    </div>
  );
}

/** The "connect" step for a classic connector — reads `ctx.connectorBegin`
 *  (populated by `InstallConnectorStep` moments earlier) and branches into
 *  the right sub-state; self-fetches it if entered directly (the setup
 *  checklist's "Connect" resume jumps straight here, skipping "install"). A
 *  `none` sub-state (env var already set, or nothing to collect) advances
 *  past this step automatically — same as the retired catalog install
 *  modal's `envVarPresent`/`authKind === "none"` branch skipping straight to
 *  settings/done. */
export function ConnectorConnectStep({ ctx, onNext }: { ctx: WizardCtx; onNext: () => void }) {
  const [begin, setBegin] = useState<PluginInstallBeginResult | null>(ctx.connectorBegin);
  const mountedRef = useMountedRef();
  const startedRef = useRef(false);

  const ensureBegin = useCallback(async () => {
    if (ctx.connectorBegin) {
      setBegin(ctx.connectorBegin);
      return;
    }
    const b = await runBegin(ctx.pluginId);
    if (!mountedRef.current || !b) return;
    ctx.setConnectorBegin(b);
    setBegin(b);
  }, [ctx, mountedRef]);

  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    void ensureBegin();
  }, [ensureBegin]);

  const sub: ConnectSub = begin ? subStateFor(begin) : "probing";

  useEffect(() => {
    if (sub === "none") onNext();
  }, [sub, onNext]);

  if (sub === "probing" || sub === "none" || !begin) {
    return (
      <div className="flex items-center gap-2 py-6 text-[13px] text-muted-foreground">
        <StatusDot color="#3B82F6" size={8} pulse />
        Checking configuration…
      </div>
    );
  }
  if (sub === "token") {
    const auth = ctx.detail?.auth;
    if (auth) return <TokenConnect ctx={ctx} auth={auth} />;
  }
  if (sub === "clientId") return <ManualClientId ctx={ctx} begin={begin} onBegin={setBegin} onNext={onNext} />;
  if (sub === "oauthWait") return <OauthWait ctx={ctx} begin={begin} onBegin={setBegin} onNext={onNext} />;
  return <div className="text-[13px] text-muted-foreground">{begin.dcrError ?? "Couldn't resolve a sign-in method for this plugin."}</div>;
}
