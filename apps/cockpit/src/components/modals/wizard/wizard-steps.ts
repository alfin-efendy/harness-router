// Pure step planner for the universal install wizard (spec §5). No React —
// UniversalInstallWizard.tsx builds a WizardPlanInput from the fetched
// plugin/release detail and calls planWizardSteps once on mount.

export type WizardStepId = "overview" | "contents" | "permissions" | "install" | "connect" | "settings" | "done";

export type WizardPlanInput = {
  kind: string; // PluginInfo.kind
  componentBacked: boolean;
  authKind: string; // "none" | "token" | "oauth"
  hasSettings: boolean; // detail.settings.length > 0
  trustRequired: boolean; // skill packs from arbitrary sources, or unsigned mcp/component surfaces
  // Component OAuth is declared per-profile in the active/embedded manifest
  // (ComponentManifestInfo.oauthProfiles), not the top-level auth spec — a
  // component can need a connect step with authKind "none" when it declares
  // at least one profile. True when that manifest lists ≥1 OAuth profile.
  hasOauthProfiles: boolean;
  // Spec A2: true for a provider whose free tier is a built-in, daemon-
  // guaranteed connection (descriptor category Free — surfaced to the UI as a
  // "free" entry in PluginInfo.categories). Such a provider needs no account
  // to work, so the wizard plans no connect step for it.
  freeBuiltin: boolean;
  // Task 15: true when commands.length + skills.length + hooks.length +
  // jobs.length > 0 — plans a "What you get" preview step right after
  // overview, before any install/permission gate.
  hasContents: boolean;
};

// Rules (spec §5 table, extended Task 15): overview and install and done are
// unconditional; contents (Task 15) comes right after overview whenever the
// plugin declares any commands/skills/hooks/jobs; permissions gates on
// component-backed or untrusted-source risk — EXCEPT a component-backed
// provider (Finding 1): the daemon marks all twelve
// `COMPONENT_BACKED_PROVIDER_IDS` (crates/core/src/plugins/component_catalog.rs)
// `componentBacked: true` so Cockpit can offer release management for their
// bundle, but they still install/connect through the provider adapter
// (`steps-provider.tsx`), which has no component release/manifest to summarize
// permissions for — so `kind === "provider"` is excluded from this gate
// regardless of `componentBacked`; settings gates on the manifest declaring
// [[settings]]; connect gates on either a top-level auth requirement,
// providers needing a connection unless their free tier is built in, or a
// component's declared OAuth profile.
export function planWizardSteps(i: WizardPlanInput): WizardStepId[] {
  const steps: WizardStepId[] = ["overview"];
  if (i.hasContents) steps.push("contents");
  if ((i.componentBacked && i.kind !== "provider") || i.trustRequired) steps.push("permissions");
  steps.push("install");
  if (i.authKind !== "none" || (i.kind === "provider" && !i.freeBuiltin) || i.hasOauthProfiles) steps.push("connect");
  if (i.hasSettings) steps.push("settings");
  steps.push("done");
  return steps;
}

const STEP_LABELS: Record<WizardStepId, string> = {
  overview: "Overview",
  contents: "What you get",
  permissions: "Permissions",
  install: "Install",
  connect: "Connect",
  settings: "Settings",
  done: "Done",
};

export function stepLabel(s: WizardStepId): string {
  return STEP_LABELS[s];
}
