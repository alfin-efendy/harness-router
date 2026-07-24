import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { Button, Modal, ModalBody, ModalFooter, ModalHeader } from "@ryuzi/ui";
import { commands, type ComponentReleaseDetail, type PluginDetail } from "@/bindings";
import { IconChip } from "@/components/common/bits";
import { pluginIcon } from "@/lib/plugin-icons";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { planWizardSteps, stepLabel, type WizardStepId } from "./wizard-steps";

// Steps whose body can be deferred without blocking install — Skip only
// shows up here (spec §5).
const SKIPPABLE_STEPS: ReadonlySet<WizardStepId> = new Set(["connect", "settings"]);

// Universal install wizard shell (spec §5). Fetches plugin/release detail
// once on mount to build a WizardPlanInput, plans the step sequence via
// planWizardSteps, and renders a segmented-progress shell around whichever
// step is current. Step bodies are Task 14/15's per-kind adapters — until
// those land every step renders a scaffold placeholder (its own label).
// Not yet reachable from any UI: no launch-point wires into this component
// in this task.
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

  useEffect(() => {
    let active = true;
    void (async () => {
      const [detailRes, releaseRes] = await Promise.all([
        commands.pluginDetail(LOCAL_RUNNER, pluginId),
        commands.pluginReleaseDetail(LOCAL_RUNNER, pluginId),
      ]);
      if (!active) return;
      if (detailRes.status === "ok") setDetail(detailRes.data);
      else toast.error(detailRes.error.message);
      if (releaseRes.status === "ok") setReleaseDetail(releaseRes.data);
      else toast.error(releaseRes.error.message);
    })();
    return () => {
      active = false;
    };
  }, [pluginId]);

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
  const isLast = clampedIndex === stepCount - 1;

  const back = () => {
    if (isFirst) return;
    setStepIndex(clampedIndex - 1);
  };
  // Skip and Continue both move the wizard forward — Skip just lets a
  // skippable step's body defer its own action first (Task 14/15 wire that
  // distinction into the step body itself). On the last step there's
  // nowhere further to go, so it closes the wizard instead.
  const advance = () => {
    if (isLast) {
      onClose();
      return;
    }
    setStepIndex(clampedIndex + 1);
  };

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
        <StepBody step={currentStep} />
      </ModalBody>
      <ModalFooter>
        <Button variant="outline" disabled={isFirst} onClick={back}>
          Back
        </Button>
        {SKIPPABLE_STEPS.has(currentStep) && (
          <Button variant="ghost" onClick={advance}>
            Skip
          </Button>
        )}
        <Button onClick={advance}>Continue</Button>
      </ModalFooter>
    </Modal>
  );
}

// Scaffold body for every step until Tasks 14/15 plug in the real per-kind
// components — rendering the step's own label is enough to exercise the
// shell (progress, Back/Skip/Continue, initialStep resume) end to end.
function StepBody({ step }: { step: WizardStepId }) {
  return <div className="text-[13px] text-muted-foreground">{stepLabel(step)}</div>;
}
