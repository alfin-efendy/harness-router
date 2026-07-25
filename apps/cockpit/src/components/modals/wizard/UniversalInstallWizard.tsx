import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { Button, Modal, ModalBody, ModalFooter, ModalHeader } from "@ryuzi/ui";
import { commands, type ComponentReleaseDetail, type PluginDetail } from "@/bindings";
import { IconChip } from "@/components/common/bits";
import { pluginIcon } from "@/lib/plugin-icons";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { ConnectStep, DoneStep, InstallComponentStep, OverviewStep, PermissionsStep, SettingsStep } from "./steps-component";
import { planWizardSteps, stepLabel, type WizardStepId } from "./wizard-steps";

// Steps whose body can be deferred without blocking install — Skip only
// shows up here (spec §5).
const SKIPPABLE_STEPS: ReadonlySet<WizardStepId> = new Set(["connect", "settings"]);

/** Shared read/write surface every step component gets (Task 14). `refresh`
 *  re-fetches both `pluginDetail`/`pluginReleaseDetail` and updates the
 *  shell's state — steps call it after a mutating action (install/connect/
 *  a settings save) so the NEXT step (and a later re-render of the same one)
 *  sees fresh data.
 *
 *  `setContinueDisabled` is the one mechanism a step can use to gate the
 *  shell's own footer Continue button — chosen over a third `onNext`-sibling
 *  prop so every step component keeps the exact `({ ctx, onNext })` shape.
 *  Only `PermissionsStep` uses it today; a step that calls it MUST clear it
 *  again on unmount (its own effect cleanup) so a later step never inherits
 *  a stale `true`. Steps that don't call it at all leave Continue enabled —
 *  in particular `InstallComponentStep` deliberately does NOT use this: it
 *  guards its own re-entrancy via a mounted-ref instead (see that file), so
 *  a stray manual Continue click during install is a no-op once install
 *  finishes rather than something that needs blocking up front. */
export type WizardCtx = {
  pluginId: string;
  detail: PluginDetail | null;
  releaseDetail: ComponentReleaseDetail | null;
  plan: WizardStepId[];
  refresh: () => Promise<void>;
  setContinueDisabled: (v: boolean) => void;
};

// Universal install wizard shell (spec §5). Fetches plugin/release detail
// once on mount to build a WizardPlanInput, plans the step sequence via
// planWizardSteps, and renders a segmented-progress shell around whichever
// step is current. Task 14 launch points (component-backed rows/hero/
// checklist) are the first UI to reach this component; Task 15 wires the
// skill-pack (trustRequired) adapter in.
export function UniversalInstallWizard({
  pluginId,
  onClose,
  initialStep,
}: {
  pluginId: string;
  onClose: () => void;
  initialStep?: WizardStepId;
}) {
  const [detail, setDetail] = useState<PluginDetail | null>(null);
  const [releaseDetail, setReleaseDetail] = useState<ComponentReleaseDetail | null>(null);
  const [stepIndex, setStepIndex] = useState(0);
  // Guards the premature-close race: before both fetches settle, `plan`
  // defaults to a single "overview" step, so isLast is (wrongly) true and a
  // Continue click during the round trip would close the wizard outright.
  // Stays true until the Promise.all below settles, success or error alike.
  const [loading, setLoading] = useState(true);
  // The one shell mechanism a step can use to gate Continue — see `WizardCtx`'s doc.
  const [continueDisabled, setContinueDisabled] = useState(false);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Shared by the mount effect below and every step's `ctx.refresh()` — a
  // mutating step action (install/connect/settings save) calls this so the
  // NEXT step's `ctx.detail`/`ctx.releaseDetail` is current.
  const refresh = useCallback(async () => {
    const [detailRes, releaseRes] = await Promise.all([
      commands.pluginDetail(LOCAL_RUNNER, pluginId),
      commands.pluginReleaseDetail(LOCAL_RUNNER, pluginId),
    ]);
    if (!mountedRef.current) return;
    if (detailRes.status === "ok") setDetail(detailRes.data);
    else toast.error(detailRes.error.message);
    if (releaseRes.status === "ok") setReleaseDetail(releaseRes.data);
    else toast.error(releaseRes.error.message);
  }, [pluginId]);

  useEffect(() => {
    let active = true;
    void (async () => {
      await refresh();
      if (active) setLoading(false);
    })();
    return () => {
      active = false;
    };
  }, [refresh]);

  // trustRequired is always false from this shell — Task 15's skill-pack
  // adapter is the one caller that knows a pack's source isn't curated and
  // passes true through its own wizard entry point.
  const plan = useMemo<WizardStepId[]>(() => {
    if (!detail) return ["overview"];
    return planWizardSteps({
      kind: detail.info.kind,
      componentBacked: detail.info.componentBacked,
      authKind: detail.auth?.kind ?? "none",
      hasSettings: detail.settings.length > 0,
      trustRequired: false,
      hasOauthProfiles: (releaseDetail?.activeManifest?.oauthProfiles.length ?? 0) > 0,
    });
  }, [detail, releaseDetail]);

  // Checklist-resume hook (Task 14): once the real plan is known, jump to
  // initialStep's position in it. A step the plan skips falls back to 0
  // rather than landing on a mismatched index.
  useEffect(() => {
    if (!detail || !initialStep) return;
    const idx = plan.indexOf(initialStep);
    setStepIndex(idx >= 0 ? idx : 0);
  }, [detail, plan, initialStep]);

  const stepCount = plan.length;
  const clampedIndex = Math.min(stepIndex, stepCount - 1);
  const currentStep = plan[clampedIndex];
  const isFirst = clampedIndex === 0;

  // Read via refs (kept fresh every render, below) rather than closing over
  // `clampedIndex`/`stepCount` directly — a step's `onNext` (== `advance`)
  // can fire from a delayed async continuation (install success, an oauth
  // completion event) well after the user has since navigated elsewhere by
  // hand, and a stale closure would silently step from the WRONG index.
  const clampedIndexRef = useRef(clampedIndex);
  clampedIndexRef.current = clampedIndex;
  const stepCountRef = useRef(stepCount);
  stepCountRef.current = stepCount;

  const back = useCallback(() => {
    const idx = clampedIndexRef.current;
    if (idx === 0) return;
    setStepIndex(idx - 1);
  }, []);
  // Skip and Continue both move the wizard forward — Skip just lets a
  // skippable step's body defer its own action first. On the last step
  // there's nowhere further to go, so it closes the wizard instead — this is
  // also the mechanism `DoneStep`'s own "Open plugin page" button reuses via
  // `onNext`.
  const advance = useCallback(() => {
    const idx = clampedIndexRef.current;
    const count = stepCountRef.current;
    if (idx >= count - 1) {
      onClose();
      return;
    }
    setStepIndex(idx + 1);
  }, [onClose]);

  // `setContinueDisabled` (a `useState` setter) is excluded from the dep
  // array on purpose — React guarantees its identity is stable forever, so
  // biome's exhaustive-deps rule flags it as unnecessary.
  const ctx = useMemo<WizardCtx>(
    () => ({ pluginId, detail, releaseDetail, plan, refresh, setContinueDisabled }),
    [pluginId, detail, releaseDetail, plan, refresh],
  );

  const name = detail?.info.name ?? pluginId;
  const Icon = pluginIcon(detail?.info.icon ?? null);

  return (
    <Modal onClose={onClose} width={480}>
      <ModalHeader
        leading={<IconChip icon={Icon} size={28} />}
        title={`Install ${name}`}
        description={`Step ${clampedIndex + 1} of ${stepCount} — ${stepLabel(currentStep)}`}
      />
      <div className="mt-[18px] flex gap-1">
        {plan.map((step, idx) => (
          <div key={step} className={`h-1 flex-1 rounded-full ${idx <= clampedIndex ? "bg-primary" : "bg-muted"}`} />
        ))}
      </div>
      <ModalBody className="mt-3">
        {loading ? (
          <div className="text-[13px] text-muted-foreground">Loading…</div>
        ) : (
          <StepBody step={currentStep} ctx={ctx} onNext={advance} />
        )}
      </ModalBody>
      <ModalFooter>
        {!loading && (
          <Button variant="outline" disabled={isFirst} onClick={back}>
            Back
          </Button>
        )}
        {!loading && SKIPPABLE_STEPS.has(currentStep) && (
          <Button variant="ghost" onClick={advance}>
            Skip
          </Button>
        )}
        <Button disabled={loading || continueDisabled} onClick={advance}>
          Continue
        </Button>
      </ModalFooter>
    </Modal>
  );
}

// Dispatches the current step id to its real per-kind component (Task 14).
// A flat switch over the closed `WizardStepId` union — TypeScript proves
// this is exhaustive without a `default`, but one stays as a defensive
// runtime fallback (never reachable given `plan` only ever contains these
// six ids).
function StepBody({ step, ctx, onNext }: { step: WizardStepId; ctx: WizardCtx; onNext: () => void }) {
  switch (step) {
    case "overview":
      return <OverviewStep ctx={ctx} onNext={onNext} />;
    case "permissions":
      return <PermissionsStep ctx={ctx} onNext={onNext} />;
    case "install":
      return <InstallComponentStep ctx={ctx} onNext={onNext} />;
    case "connect":
      return <ConnectStep ctx={ctx} onNext={onNext} />;
    case "settings":
      return <SettingsStep ctx={ctx} onNext={onNext} />;
    case "done":
      return <DoneStep ctx={ctx} onNext={onNext} />;
    default:
      return null;
  }
}
