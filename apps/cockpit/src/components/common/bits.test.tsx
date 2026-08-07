import { expect, test } from "bun:test";
import { pluginDisplayName } from "./bits";

test("resolves a plugin id to its manifest display name", () => {
  const plugins = [{ id: "acme", name: "Acme Toolkit" }];
  expect(pluginDisplayName(plugins, "acme")).toBe("Acme Toolkit");
});

test("falls back to the raw id when the plugin isn't in the loaded list", () => {
  expect(pluginDisplayName([], "acme")).toBe("acme");
  expect(pluginDisplayName([{ id: "other", name: "Other" }], "acme")).toBe("acme");
});
