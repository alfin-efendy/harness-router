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
    ...overrides,
  };
}

test("component connector with oauth + settings includes all six steps", () => {
  const steps = planWizardSteps(input({ componentBacked: true, authKind: "oauth", hasSettings: true }));
  expect(steps).toEqual(["overview", "permissions", "install", "connect", "settings", "done"]);
});

test("provider always gets a connect step, even with authKind none", () => {
  const steps = planWizardSteps(input({ kind: "provider", authKind: "none" }));
  expect(steps).toEqual(["overview", "install", "connect", "done"]);
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
