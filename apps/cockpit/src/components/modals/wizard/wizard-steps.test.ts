import { expect, test } from "bun:test";
import { planWizardSteps, stepLabel, type WizardPlanInput } from "./wizard-steps";

// Base input with every gate off — each test flips only the fields it cares
// about, matching the spec §5 table this planner mirrors.
function input(overrides: Partial<WizardPlanInput> = {}): WizardPlanInput {
  return {
    kind: "integration",
    componentBacked: false,
    authKind: "none",
    hasSettings: false,
    trustRequired: false,
    hasOauthProfiles: false,
    freeBuiltin: false,
    ...overrides,
  };
}

test("component connector with oauth + settings includes all six steps", () => {
  const steps = planWizardSteps(input({ componentBacked: true, authKind: "oauth", hasSettings: true }));
  expect(steps).toEqual(["overview", "permissions", "install", "connect", "settings", "done"]);
});

// Spec A2: a provider still defaults to a connect step (paid providers need an
// account), EXCEPT the built-in free tiers whose connection is guaranteed by
// the daemon — those finish the wizard with no account form.
test("paid provider gets a connect step even with authKind none", () => {
  const steps = planWizardSteps(input({ kind: "provider", authKind: "none" }));
  expect(steps).toEqual(["overview", "install", "connect", "done"]);
});

test("built-in free provider skips the connect step", () => {
  const steps = planWizardSteps(input({ kind: "provider", authKind: "none", freeBuiltin: true }));
  expect(steps).toEqual(["overview", "install", "done"]);
});

test("freeBuiltin does not suppress connect for a token provider", () => {
  const steps = planWizardSteps(input({ kind: "provider", authKind: "token", freeBuiltin: true }));
  expect(steps).toEqual(["overview", "install", "connect", "done"]);
});

// Finding 1 — the daemon flags every `COMPONENT_BACKED_PROVIDER_IDS` entry
// `componentBacked: true` for release-management purposes only; the planner
// must NOT grow a permissions step for it (the provider adapter has no
// component release/manifest to summarize permissions from — see
// `wizardKind`'s doc in `UniversalInstallWizard.tsx`).
test("a component-backed provider skips permissions despite componentBacked", () => {
  const steps = planWizardSteps(input({ kind: "provider", componentBacked: true, authKind: "none" }));
  expect(steps).toEqual(["overview", "install", "connect", "done"]);
});

// trustRequired still wins even for a provider (defensive — providers never
// actually set skillTrust, but the gate's `|| trustRequired` must not be
// short-circuited by the kind !== "provider" exclusion).
test("trustRequired still adds permissions for a provider", () => {
  const steps = planWizardSteps(input({ kind: "provider", componentBacked: true, authKind: "none", trustRequired: true }));
  expect(steps).toEqual(["overview", "permissions", "install", "connect", "done"]);
});

test("curated skill pack (trust not required) skips permissions and connect", () => {
  const steps = planWizardSteps(input({ kind: "skill-pack" }));
  expect(steps).toEqual(["overview", "install", "done"]);
});

test("arbitrary-source skill pack adds a permissions step", () => {
  const steps = planWizardSteps(input({ kind: "skill-pack", trustRequired: true }));
  expect(steps).toEqual(["overview", "permissions", "install", "done"]);
});

test("token connector with no settings gets connect but not settings", () => {
  const steps = planWizardSteps(input({ authKind: "token" }));
  expect(steps).toEqual(["overview", "install", "connect", "done"]);
});

// Controller amendment: component OAuth lives in the manifest's declared
// profiles, not the top-level auth spec — hasOauthProfiles must plan a
// connect step even when authKind is "none".
test("component with authKind none but a declared oauth profile still includes connect", () => {
  const steps = planWizardSteps(input({ componentBacked: true, authKind: "none", hasOauthProfiles: true }));
  expect(steps).toEqual(["overview", "permissions", "install", "connect", "done"]);
});

test("component with authKind none and no declared oauth profile has no connect step", () => {
  const steps = planWizardSteps(input({ componentBacked: true, authKind: "none", hasOauthProfiles: false }));
  expect(steps).toEqual(["overview", "permissions", "install", "done"]);
});

test("stepLabel maps every step id to its display label", () => {
  expect(stepLabel("overview")).toBe("Overview");
  expect(stepLabel("permissions")).toBe("Permissions");
  expect(stepLabel("install")).toBe("Install");
  expect(stepLabel("connect")).toBe("Connect");
  expect(stepLabel("settings")).toBe("Settings");
  expect(stepLabel("done")).toBe("Done");
});
