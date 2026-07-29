import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import type { AgentConfigurationCatalogInfo, CmdError, Result } from "./bindings";

const catalogA: AgentConfigurationCatalogInfo = {
  skills: [],
  nativeTools: [],
  pluginTools: [],
  apps: [],
};

const catalogB: AgentConfigurationCatalogInfo = {
  skills: [{ id: "test-skill", label: "Test Skill", description: "", available: true, commandScoped: false, pack: null, kind: null }],
  nativeTools: [],
  pluginTools: [],
  apps: [],
};

const getAgentConfigurationCatalog = mock(
  async (): Promise<Result<AgentConfigurationCatalogInfo, CmdError>> => ({ status: "ok", data: catalogA }),
);
mock.module("./bindings", () => ({ commands: { getAgentConfigurationCatalog } }));

const { useAgentConfigurationCatalog } = await import("./store-agent-catalog");

afterEach(() => {
  getAgentConfigurationCatalog.mockReset();
  getAgentConfigurationCatalog.mockResolvedValue({ status: "ok", data: catalogA });
});
beforeEach(() => {
  useAgentConfigurationCatalog.setState({ catalog: null, loading: false, error: null });
});

test("loads the agent catalog once for concurrent consumers and exposes its state", async () => {
  const first = useAgentConfigurationCatalog.getState().load();
  const second = useAgentConfigurationCatalog.getState().load();

  await Promise.all([first, second]);

  expect(getAgentConfigurationCatalog).toHaveBeenCalledTimes(1);
  expect(getAgentConfigurationCatalog).toHaveBeenCalledWith("local");
  expect(useAgentConfigurationCatalog.getState()).toMatchObject({ catalog: catalogA, loading: false, error: null });
});

test("exposes catalog request failures while leaving the catalog unset", async () => {
  getAgentConfigurationCatalog.mockResolvedValueOnce({ status: "error", error: { message: "catalog unavailable" } });

  await useAgentConfigurationCatalog.getState().load();

  expect(useAgentConfigurationCatalog.getState()).toMatchObject({ catalog: null, loading: false, error: "catalog unavailable" });
});

test("reload drops the cached catalog and refetches", async () => {
  // First load caches; getAgentConfigurationCatalog mock returns catalogA.
  await useAgentConfigurationCatalog.getState().load();
  expect(useAgentConfigurationCatalog.getState().catalog).toEqual(catalogA);
  // Mock now returns catalogB; plain load() must NOT refetch (cached)…
  getAgentConfigurationCatalog.mockResolvedValueOnce({ status: "ok", data: catalogB });
  await useAgentConfigurationCatalog.getState().load();
  expect(useAgentConfigurationCatalog.getState().catalog).toEqual(catalogA);
  // …but reload() must.
  await useAgentConfigurationCatalog.getState().reload();
  expect(useAgentConfigurationCatalog.getState().catalog).toEqual(catalogB);
});
