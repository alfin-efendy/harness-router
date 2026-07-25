import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { Button, Modal, ModalBody, ModalFooter, ModalHeader } from "@ryuzi/ui";
import { commands, type ComponentReleaseDetail, type PluginDetail, type PluginInstallBeginResult, type TrustPromptDto } from "@/bindings";
import { IconChip } from "@/components/common/bits";
import { pluginIcon } from "@/lib/plugin-icons";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { ConnectStep, DoneStep, InstallComponentStep, OverviewStep, PermissionsStep, SettingsStep } from "./steps-component";
import { ConnectorConnectStep, InstallConnectorStep } from "./steps-connector";
import { InstallSkillPackStep, SkillTrustStep } from "./steps-skillpack";
import { InstallProviderStep, ProviderConnectStep } from "./steps-provider";
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
 *  finishes rather than something that needs blocking up front.
 *
 *  Task 15 adds two more cross-step handoffs, one per non-component adapter
 *  that needs to hand a mutating call's result to the step immediately
 *  after it in the plan:
 *  - `skillTrust`/`setSkillTrust`: `steps-skillpack.tsx`'s install step
 *    stashes `beginSkillInstall`'s trust prompt here (`null` for a curated,
 *    already-trusted pack). The shell's own `plan` reads it back as
 *    `trustRequired` — setting it non-null on a curated-by-default source is
 *    what makes the permissions step appear reactively, without the shell
 *    ever hardcoding `trustRequired: false` again.
 *  - `connectorBegin`/`setConnectorBegin`: a classic (non-component)
 *    connector's install step stashes `beginPluginInstall`'s structured
 *    result here so the connect step can branch on it (token/manual-client-
 *    id/oauth-wait) without re-resolving it — though the connect step
 *    re-fetches it lazily itself if entered directly (the setup checklist's
 *    "Connect" resume jumps straight to this step, skipping install). */
export type WizardCtx = {
  pluginId: string;
  detail: PluginDetail | null;
  releaseDetail: ComponentReleaseDetail | null;
  plan: WizardStepId[];
  refresh: () => Promise<void>;
  setContinueDisabled: (v: boolean) => void;
  skillTrust: TrustPromptDto | null;
  setSkillTrust: (v: TrustPromptDto | null) => void;
  connectorBegin: PluginInstallBeginResult | null;
  setConnectorBegin: (v: PluginInstallBeginResult | null) => void;
};

/** Which per-kind adapter set owns this plugin's steps (Task 15). `kind`
 *  ("provider"/"skill-pack") wins FIRST (Finding 1 — review of Task 15): the
 *  daemon flags every one of the twelve `COMPONENT_BACKED_PROVIDER_IDS`
 *  (crates/core/src/plugins/component_catalog.rs) `componentBacked: true` so
 *  Cockpit can offer release management (install / active version /
 *  rollback) for their bundle, but that's a display-only flag for them — they
 *  still install/connect through the provider adapter
 *  (`installProvider`/`ConnectionMethodForm`), not `InstallComponentStep`'s
 *  fail-closed component-bundle path. Checking `componentBacked` before
 *  `kind` used to route every one of those provider ids into the wrong
 *  adapter. Everything else that IS component-backed drives overview's tool
 *  list and permissions' summary off its own release/manifest regardless of
 *  what `kind` string the manifest declares; anything left over is a classic
 *  (pre-component, catalog-manifest) connector — integrations, gateways, and
 *  any future kind this switch doesn't know about yet. */
function wizardKind(detail: PluginDetail | null): "component" | "connector" | "skill-pack" | "provider" {
  if (!detail) return "connector";
  if (detail.info.kind === "provider") return "provider";
  if (detail.info.kind === "skill-pack") return "skill-pack";
  if (detail.info.componentBacked) return "component";
  return "connector";
}

// Universal install wizard shell (spec §5). Fetches plugin/release detail
// once on mount to build a WizardPlanInput, plans the step sequence via
// planWizardSteps, and renders a segmented-progress shell around whichever
// step is current. Every install path now reaches this one component (Task
// 15 retired the classic catalog install modal): component-backed, classic
// connector, skill-pack, and provider all resolve through
// `wizardKind`/`StepBody`'s per-kind dispatch below.
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
  // Task 15 cross-step handoffs — see `WizardCtx`'s doc for what each feeds.
  const [skillTrust, setSkillTrust] = useState<TrustPromptDto | null>(null);
  const [connectorBegin, setConnectorBegin] = useState<PluginInstallBeginResult | null>(null);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // Task 15 (ported from the retired catalog install modal's `close()`/
  // unmount effect): a classic connector's oauth sign-in may still be
  // pending when the wizard closes — cancel it so the backend's loopback
  // listener / flow-map entry doesn't leak. Read via a ref (kept fresh every render)
  // rather than closing over `detail`/`connectorBegin` directly so the
  // unmount cleanup below always sees the latest values without
  // re-subscribing on every change. `cancel_plugin_install` is a no-op when
  // nothing is pending (including after a completed flow), so firing this on
  // every close — component-backed or not, oauth or not — is harmless.
  const closeStateRef = useRef({ pluginId, detail, connectorBegin });
  closeStateRef.current = { pluginId, detail, connectorBegin };
  const cancelPendingConnectorOauth = useCallback(() => {
    const { pluginId: pid, detail: d, connectorBegin: begin } = closeStateRef.current;
    if (!d || wizardKind(d) !== "connector") return;
    const authKind = begin?.authKind ?? d.auth?.kind ?? null;
    if (authKind === "oauth") {
      void commands.cancelPluginInstall(LOCAL_RUNNER, pid, begin?.oauthBegin?.stateToken ?? null);
    }
  }, []);
  const handleClose = useCallback(() => {
    cancelPendingConnectorOauth();
    onClose();
  }, [cancelPendingConnectorOauth, onClose]);
  useEffect(() => {
    return () => {
      cancelPendingConnectorOauth();
    };
  }, [cancelPendingConnectorOauth]);

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

  // trustRequired reflects `skillTrust` (Task 15): null until the skill-pack
  // install step's `beginSkillInstall` comes back with a trust prompt (a
  // curated, already-trusted pack never sets it) — recomputing the plan the
  // moment it's set is what makes the permissions step appear reactively,
  // without the currently-mounted install step needing to call `onNext()`
  // itself (see `WizardCtx`'s doc).
  const plan = useMemo<WizardStepId[]>(() => {
    if (!detail) return ["overview"];
    return planWizardSteps({
      kind: detail.info.kind,
      componentBacked: detail.info.componentBacked,
      authKind: detail.auth?.kind ?? "none",
      hasSettings: detail.settings.length > 0,
      trustRequired: skillTrust != null,
      hasOauthProfiles: (releaseDetail?.activeManifest?.oauthProfiles.length ?? 0) > 0,
    });
  }, [detail, releaseDetail, skillTrust]);

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
      handleClose();
      return;
    }
    setStepIndex(idx + 1);
  }, [handleClose]);

  // `setContinueDisabled`/`setSkillTrust`/`setConnectorBegin` (`useState`
  // setters) are excluded from the dep array on purpose — React guarantees
  // their identity is stable forever, so biome's exhaustive-deps rule flags
  // them as unnecessary.
  const ctx = useMemo<WizardCtx>(
    () => ({
      pluginId,
      detail,
      releaseDetail,
      plan,
      refresh,
      setContinueDisabled,
      skillTrust,
      setSkillTrust,
      connectorBegin,
      setConnectorBegin,
    }),
    [pluginId, detail, releaseDetail, plan, refresh, skillTrust, connectorBegin],
  );

  const name = detail?.info.name ?? pluginId;
  const Icon = pluginIcon(detail?.info.icon ?? null);

  return (
    <Modal onClose={handleClose} width={480}>
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

// Dispatches the current step id to its real per-kind component (Task 14
// component-backed pilot; Task 15 fills in connector/skill-pack/provider).
// "overview"/"settings"/"done" stay kind-agnostic (shared components from
// `steps-component.tsx`); "permissions" branches only for the skill-pack
// trust prompt (every other gated case — component permissions — uses the
// shared summary-rows view); "install"/"connect" are the two steps whose
// underlying mechanics genuinely differ per kind, so those branch on
// `wizardKind(ctx.detail)`.
function StepBody({ step, ctx, onNext }: { step: WizardStepId; ctx: WizardCtx; onNext: () => void }) {
  const kind = wizardKind(ctx.detail);
  switch (step) {
    case "overview":
      return <OverviewStep ctx={ctx} onNext={onNext} />;
    case "permissions":
      return ctx.skillTrust ? <SkillTrustStep ctx={ctx} onNext={onNext} /> : <PermissionsStep ctx={ctx} onNext={onNext} />;
    case "install":
      switch (kind) {
        case "connector":
          return <InstallConnectorStep ctx={ctx} onNext={onNext} />;
        case "skill-pack":
          return <InstallSkillPackStep ctx={ctx} onNext={onNext} />;
        case "provider":
          return <InstallProviderStep ctx={ctx} onNext={onNext} />;
        default:
          return <InstallComponentStep ctx={ctx} onNext={onNext} />;
      }
    case "connect":
      switch (kind) {
        case "connector":
          return <ConnectorConnectStep ctx={ctx} onNext={onNext} />;
        case "provider":
          return <ProviderConnectStep ctx={ctx} onNext={onNext} />;
        default:
          return <ConnectStep ctx={ctx} onNext={onNext} />;
      }
    case "settings":
      return <SettingsStep ctx={ctx} onNext={onNext} />;
    case "done":
      return <DoneStep ctx={ctx} onNext={onNext} />;
    default:
      return null;
  }
}
