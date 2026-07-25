import { CircleAlert } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Badge, Button, Switch } from "@ryuzi/ui";
import { commands } from "@/bindings";
import { StatusDot } from "@/components/common/bits";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { usePlugins } from "@/store-plugins";
import { useMountedRef } from "./steps-component";
import type { WizardCtx } from "./UniversalInstallWizard";

const WARN = "#F59E0B";
const DANGER = "#EF4444";

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

// Skill-pack adapter (Task 15) — a two-phase install (`begin_skill_install`/
// `confirm_skill_install`, see `SkillInstallModal`'s doc) mapped onto the
// wizard's fixed plan shape. `InstallSkillPackStep` (the "install" step)
// mounts TWICE for a source that isn't curated-and-trusted-by-default: once
// to `beginSkillInstall` (stashing the trust prompt on `ctx.skillTrust`,
// which makes the plan reactively insert a "permissions" step right before
// "install" — see `WizardCtx`'s doc), and again — after the user accepts on
// that permissions step — to `confirmSkillInstall`. A curated pack whose
// manifest doesn't run code resolves `completed: true` on the first pass and
// never revisits "install" at all.

export function InstallSkillPackStep({ ctx, onNext }: { ctx: WizardCtx; onNext: () => void }) {
  const loadPlugins = usePlugins((s) => s.load);
  const [status, setStatus] = useState<"installing" | "error">("installing");
  const mountedRef = useMountedRef();
  const startedRef = useRef(false);

  const confirm = useCallback(async () => {
    if (!ctx.skillTrust) return;
    setStatus("installing");
    const res = await commands.confirmSkillInstall(LOCAL_RUNNER, ctx.skillTrust.token);
    if (!mountedRef.current) return;
    if (res.status === "error") {
      toast.error(res.error.message);
      setStatus("error");
      return;
    }
    toast.success(`${res.data.name} installed`);
    await Promise.all([ctx.refresh(), loadPlugins()]);
    if (!mountedRef.current) return;
    // Deliberately NOT `onNext()`: clearing `skillTrust` shrinks the plan by
    // removing "permissions" from right before this step's own position —
    // the shell's `clampedIndex` (unchanged) lands on "done" the moment that
    // recompute happens, same reactive mechanism `begin()`'s trust-required
    // branch below relies on for the opposite (growing) direction. Calling
    // `onNext()` here too would double-advance (or close the wizard outright
    // if this settles before the shrink's own re-render, since by then the
    // shell already believes it's on the last step).
    ctx.setSkillTrust(null);
  }, [ctx, mountedRef, loadPlugins]);

  const begin = useCallback(async () => {
    setStatus("installing");
    const res = await commands.beginSkillInstall(LOCAL_RUNNER, ctx.pluginId);
    if (!mountedRef.current) return;
    if (res.status === "error") {
      toast.error(res.error.message);
      setStatus("error");
      return;
    }
    if (res.data.completed) {
      toast.success(res.data.plugin ? `${res.data.plugin.name} installed` : "Installed");
      await Promise.all([ctx.refresh(), loadPlugins()]);
      if (!mountedRef.current) return;
      onNext();
      return;
    }
    // Trust required — stash it and stop here. `onNext()` is deliberately
    // NOT called: the plan reactively inserts "permissions" right before
    // this step the moment `ctx.skillTrust` is non-null, so the shell shows
    // it next without this step needing to navigate anywhere itself.
    ctx.setSkillTrust(res.data.trust);
  }, [ctx, mountedRef, loadPlugins, onNext]);

  const attempt = useCallback(() => (ctx.skillTrust ? confirm() : begin()), [ctx.skillTrust, confirm, begin]);

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

/** The "permissions" step when `ctx.skillTrust` is set (dispatched by
 *  `StepBody` in `UniversalInstallWizard.tsx`) — same trust-review content
 *  `SkillInstallModal`'s own "trust" step shows, wired to the shell's
 *  `setContinueDisabled` gate (same pattern the component adapter's
 *  `PermissionsStep` uses) instead of its own Trust & Install button: the
 *  shell's Continue re-enters "install" for the confirm pass above. */
export function SkillTrustStep({ ctx }: { ctx: WizardCtx; onNext: () => void }) {
  const [accepted, setAccepted] = useState(false);
  const trust = ctx.skillTrust;

  useEffect(() => {
    ctx.setContinueDisabled(!accepted);
    return () => ctx.setContinueDisabled(false);
  }, [accepted, ctx]);

  if (!trust) return null;

  return (
    <div className="flex flex-col gap-3">
      <p className="m-0 text-[12.5px] text-muted-foreground">
        {trust.curated
          ? "This is a curated pack, but it runs code — review what it installs before Cockpit trusts it."
          : "This source isn't a curated pack — review what it installs before Cockpit trusts it."}
      </p>

      {trust.runsCode && (
        <div
          className="flex items-center gap-2.5 rounded-md border px-3 py-2.5 text-[12.5px] font-medium"
          style={{ borderColor: DANGER, color: DANGER }}
        >
          <CircleAlert aria-hidden size={16} strokeWidth={2} className="shrink-0" />
          <span className="flex items-center gap-2">
            <Badge variant="destructive">Runs code</Badge>
            This plugin runs code in a supervised subprocess — review it carefully before trusting it.
          </span>
        </div>
      )}

      <div className="flex flex-col gap-2 rounded-md border border-border px-4 py-3 text-[12.5px]">
        <div>
          <span className="font-medium">Source: </span>
          <span className="font-mono text-xs">{trust.sourceSpec}</span>
        </div>
        {trust.ownerRepo !== trust.sourceSpec && (
          <div>
            <span className="font-medium">Repository: </span>
            <span className="font-mono text-xs">{trust.ownerRepo}</span>
          </div>
        )}
        {trust.resolvedCommit && (
          <div>
            <span className="font-medium">Commit: </span>
            <span className="font-mono text-xs">{trust.resolvedCommit.slice(0, 12)}</span>
          </div>
        )}
        <div>
          <span className="font-medium">Size: </span>
          {formatBytes(trust.totalBytes)}
        </div>
      </div>

      {trust.skills.length > 0 && (
        <div>
          <div className="mb-1 text-[12.5px] font-medium">Skills ({trust.skills.length})</div>
          <ul className="m-0 list-none rounded-md border border-border p-0 text-[12px] text-muted-foreground">
            {trust.skills.map((s) => (
              <li key={s} className="border-b border-border px-3 py-1.5 font-mono last:border-b-0">
                {s}
              </li>
            ))}
          </ul>
        </div>
      )}

      {trust.hookScripts.length > 0 && (
        <div className="flex flex-col gap-1.5 rounded-md border px-3 py-2.5 text-[12px]" style={{ borderColor: WARN, color: WARN }}>
          <div className="flex items-center gap-2 font-medium">
            <CircleAlert aria-hidden size={14} strokeWidth={2} className="shrink-0" />
            Hook scripts ({trust.hookScripts.length}) — these run automatically when triggered
          </div>
          <ul className="m-0 list-none p-0 pl-[22px] font-mono">
            {trust.hookScripts.map((h) => (
              <li key={h}>{h}</li>
            ))}
          </ul>
        </div>
      )}

      <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
        <span className="text-[12.5px] font-medium">I understand and accept these permissions</span>
        <Switch on={accepted} onToggle={() => setAccepted((v) => !v)} label="Accept permissions" />
      </div>
    </div>
  );
}
