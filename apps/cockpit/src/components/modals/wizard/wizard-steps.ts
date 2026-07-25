// Pure step planner for the universal install wizard (spec §5). No React —
// UniversalInstallWizard.tsx builds a WizardPlanInput from the fetched
// plugin/release detail and calls planWizardSteps once on mount.

export type WizardStepId = "overview" | "permissions" | "install" | "connect" | "settings" | "done";

export type WizardPlanInput = {
  kind: string; // PluginInfo.kind
  componentBacked: boolean;
  authKind: string; // "none" | "token" | "oauth"
  hasSettings: boolean; // detail.settings.length > 0
  trustRequired: boolean; // skill packs from arbitrary sources
  // Component OAuth is declared per-profile in the active/embedded manifest
  // (ComponentManifestInfo.oauthProfiles), not the top-level auth spec — a
  // component can need a connect step with authKind "none" when it declares
  // at least one profile. True when that manifest lists ≥1 OAuth profile.
  hasOauthProfiles: boolean;
};

// Rules (spec §5 table): overview and install and done are unconditional;
// permissions gates on component-backed or untrusted-source risk — EXCEPT a
// component-backed provider (Finding 1): the daemon marks all twelve
// `COMPONENT_BACKED_PROVIDER_IDS` (crates/core/src/plugins/component_catalog.rs)
// `componentBacked: true` so Cockpit can offer release management for their
// bundle, but they still install/connect through the provider adapter
// (`steps-provider.tsx`), which has no component release/manifest to summarize
// permissions for — so `kind === "provider"` is excluded from this gate
// regardless of `componentBacked`; settings gates on the manifest declaring
// [[settings]]; connect gates on either a top-level auth requirement,
// providers always needing a connection, or a component's declared OAuth
// profile.
export function planWizardSteps(i: WizardPlanInput): WizardStepId[] {
  const steps: WizardStepId[] = ["overview"];
  if ((i.componentBacked && i.kind !== "provider") || i.trustRequired) steps.push("permissions");
  steps.push("install");
  if (i.authKind !== "none" || i.kind === "provider" || i.hasOauthProfiles) steps.push("connect");
  if (i.hasSettings) steps.push("settings");
  steps.push("done");
  return steps;
}

const STEP_LABELS: Record<WizardStepId, string> = {
  overview: "Overview",
  permissions: "Permissions",
  install: "Install",
  connect: "Connect",
  settings: "Settings",
  done: "Done",
};

export function stepLabel(s: WizardStepId): string {
  return STEP_LABELS[s];
}
