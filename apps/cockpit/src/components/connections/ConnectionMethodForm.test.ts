import { expect, test } from "bun:test";
import type { CatalogEntry, ConnectionInfo } from "@/bindings";
import { visibleMembers } from "./ConnectionMethodForm";

const entry = (over: Partial<CatalogEntry>): CatalogEntry => ({
  id: "mimo-free",
  name: "MiMo",
  family: "mimo-free",
  color: "#FF6900",
  initial: "M",
  category: "free",
  format: "openai",
  requiresBaseUrl: false,
  models: [],
  freeTier: false,
  riskNotice: false,
  usesDeviceGrant: false,
  ...over,
});

const conn = (over: Partial<ConnectionInfo>): ConnectionInfo => ({
  id: "c1",
  provider: "mimo-free",
  providerName: "MiMo",
  color: "#FF6900",
  initial: "M",
  authType: "free",
  label: "MiMo (free)",
  priority: 0,
  enabled: true,
  quotaCapability: null,
  models: [],
  needsRelogin: false,
  builtin: true,
  ...over,
});

test("hides the free method when its builtin connection exists", () => {
  const catalog = [entry({}), entry({ id: "mimo", name: "MiMo (Token Plan)", category: "api_key" })];
  const members = visibleMembers(catalog, "mimo-free", [conn({})]);
  expect(members.map((m) => m.id)).toEqual(["mimo"]);
});

test("keeps the free method when no builtin connection exists (other free providers)", () => {
  const catalog = [entry({ id: "other-free", family: "other-free" })];
  const members = visibleMembers(catalog, "other-free", []);
  expect(members.map((m) => m.id)).toEqual(["other-free"]);
});
