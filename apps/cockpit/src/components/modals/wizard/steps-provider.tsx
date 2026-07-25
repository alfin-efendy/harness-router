import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@ryuzi/ui";
import { StatusDot } from "@/components/common/bits";
import { ConnectionMethodForm } from "@/components/connections/ConnectionMethodForm";
import { useConnections } from "@/store-connections";
import { useMountedRef } from "./steps-component";
import type { WizardCtx } from "./UniversalInstallWizard";

// Provider adapter (Task 15) — "install" registers the family into the
// persisted installed-providers set (`useConnections().installProvider`,
// visibility only — no credential yet); "connect" hands off to the same
// account form `AddConnectionModal` uses (`ConnectionMethodForm`, extracted
// from it in this task), skippable since a provider is fully usable once
// installed and an account can always be added later from its detail page.

export function InstallProviderStep({ ctx, onNext }: { ctx: WizardCtx; onNext: () => void }) {
  const installProvider = useConnections((s) => s.installProvider);
  const [status, setStatus] = useState<"installing" | "error">("installing");
  const mountedRef = useMountedRef();
  const startedRef = useRef(false);

  const attempt = useCallback(async () => {
    setStatus("installing");
    // Finding 2: `install_provider` (the daemon RPC `installProvider` calls)
    // bails on any id that isn't a family head — a member whose id differs
    // from its family (e.g. `anthropic-oauth`, family `anthropic`) must
    // register the FAMILY, not its own id.
    const ok = await installProvider(ctx.detail?.info.family ?? ctx.pluginId);
    if (!mountedRef.current) return;
    if (!ok) {
      setStatus("error");
      return;
    }
    await ctx.refresh();
    if (!mountedRef.current) return;
    onNext();
  }, [ctx, installProvider, mountedRef, onNext]);

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

export function ProviderConnectStep({ ctx, onNext }: { ctx: WizardCtx; onNext: () => void }) {
  // Finding 2: same family-head requirement as `InstallProviderStep` —
  // `ConnectionMethodForm` filters the catalog by `family`, which only ever
  // matches a family HEAD id, not a joining member's own id.
  return <ConnectionMethodForm family={ctx.detail?.info.family ?? ctx.pluginId} onDone={onNext} />;
}
