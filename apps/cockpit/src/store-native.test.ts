import { afterEach, expect, spyOn, test } from "bun:test";
import type { CommandFileInfo, SlashEntryInfo } from "./bindings";
import { catalogKey, useNative } from "./store-native";
import { commands } from "./bindings";
import { LOCAL_RUNNER, sessKey } from "@/lib/session-key";

const s1 = sessKey(LOCAL_RUNNER, "s1");
const p1Agent1Key = catalogKey("p1", "agent-1");

function reset() {
  useNative.setState({
    agentsByProject: {},
    slashCatalogByKey: {},
    globalCommands: undefined,
    todosBySession: {},
    queuedBySession: {},
  });
}

afterEach(reset);

const globalCommand: CommandFileInfo = {
  name: "review",
  description: "Review the change",
  template: "Review $ARGUMENTS",
  agent: null,
  model: null,
  subtask: false,
  revision: "rev-1",
};

const globalSlashEntry: SlashEntryInfo = {
  name: "review",
  description: "Global review",
  kind: "command",
  origin: "global",
  home: false,
  session: false,
  requiresProject: false,
  effective: true,
  shadowsGlobal: false,
  agent: null,
  model: null,
  subtask: false,
};

test("catalogKey formats project/agent pairings with a placeholder for null", () => {
  expect(catalogKey("p1", "agent-1")).toBe("p1::agent-1");
  expect(catalogKey(null, "agent-1")).toBe("-::agent-1");
  expect(catalogKey("p1", null)).toBe("p1::-");
  expect(catalogKey(null, null)).toBe("-::-");
});

test("loadAgents caches the project's agents", async () => {
  reset();
  const spy = spyOn(commands, "nativeAgents").mockResolvedValue({
    status: "ok",
    data: [
      { name: "build", description: "Full access", mode: "primary", builtin: true },
      { name: "explore", description: "Read-only", mode: "subagent", builtin: true },
    ],
  });
  await useNative.getState().loadAgents(LOCAL_RUNNER, "p1");
  expect(spy).toHaveBeenCalledWith(LOCAL_RUNNER, "p1");
  expect(useNative.getState().agentsByProject.p1.map((a) => a.name)).toEqual(["build", "explore"]);
  spy.mockRestore();
});

test("loadAgents drops out-of-order responses (a stale fetch can't clobber newer data)", async () => {
  reset();
  type AgentsResult = Awaited<ReturnType<typeof commands.nativeAgents>>;
  const resolvers: Array<(v: AgentsResult) => void> = [];
  const spy = spyOn(commands, "nativeAgents").mockImplementation(() => new Promise<AgentsResult>((resolve) => resolvers.push(resolve)));
  const first = useNative.getState().loadAgents(LOCAL_RUNNER, "p1"); // older fetch…
  const second = useNative.getState().loadAgents(LOCAL_RUNNER, "p1"); // …superseded by this one
  // The newer fetch resolves first with the fresh list.
  resolvers[1]({ status: "ok", data: [{ name: "newer", description: "Newer", mode: "subagent", builtin: true }] });
  await second;
  // The older fetch resolves late with the stale list — it must be ignored.
  resolvers[0]({ status: "ok", data: [{ name: "older", description: "Older", mode: "subagent", builtin: true }] });
  await first;
  expect(useNative.getState().agentsByProject.p1.map((a) => a.name)).toEqual(["newer"]);
  spy.mockRestore();
});

test("loadSlashCatalog drops out-of-order responses (a stale fetch can't clobber newer data)", async () => {
  reset();
  type CatalogResult = Awaited<ReturnType<typeof commands.slashCatalog>>;
  const resolvers: Array<(v: CatalogResult) => void> = [];
  const spy = spyOn(commands, "slashCatalog").mockImplementation(() => new Promise<CatalogResult>((resolve) => resolvers.push(resolve)));
  const first = useNative.getState().loadSlashCatalog(LOCAL_RUNNER, "p1", "agent-1"); // older fetch…
  const second = useNative.getState().loadSlashCatalog(LOCAL_RUNNER, "p1", "agent-1"); // …superseded by this one
  resolvers[1]({ status: "ok", data: [{ ...globalSlashEntry, name: "newer" }] });
  await second;
  resolvers[0]({ status: "ok", data: [{ ...globalSlashEntry, name: "older" }] });
  await first;
  expect(useNative.getState().slashCatalogByKey[p1Agent1Key].map((e) => e.name)).toEqual(["newer"]);
  spy.mockRestore();
});

test("global command CRUD calls the generated APIs and updates the global cache", async () => {
  reset();
  const deployCommand: CommandFileInfo = { ...globalCommand, name: "deploy", revision: "rev-deploy" };
  // Successful mutations refresh `globalCommands` from the server afterwards
  // (see refreshAfterGlobalCommandMutation), so each mutation's own reload
  // is sequenced to reflect the post-mutation server state.
  const listed = spyOn(commands, "globalCommandList")
    .mockResolvedValueOnce({ status: "ok", data: [globalCommand] })
    .mockResolvedValueOnce({ status: "ok", data: [globalCommand, deployCommand] })
    .mockResolvedValueOnce({ status: "ok", data: [{ ...globalCommand, description: "Updated" }, deployCommand] })
    .mockResolvedValueOnce({ status: "ok", data: [deployCommand] });
  const created = spyOn(commands, "globalCommandCreate").mockResolvedValue({ status: "ok", data: deployCommand });
  const updated = spyOn(commands, "globalCommandUpdate").mockResolvedValue({
    status: "ok",
    data: { ...globalCommand, description: "Updated" },
  });
  const deleted = spyOn(commands, "globalCommandDelete").mockResolvedValue({ status: "ok", data: null });
  // Defensive: guards against a slash catalog key tracked by an earlier test
  // (module-level bookkeeping isn't reset between tests) triggering a real,
  // unmocked IPC call during this test's mutation refreshes.
  const slashCatalog = spyOn(commands, "slashCatalog").mockResolvedValue({ status: "ok", data: [] });

  await useNative.getState().loadGlobalCommands(LOCAL_RUNNER);
  expect(listed).toHaveBeenCalledWith(LOCAL_RUNNER);
  expect(useNative.getState().globalCommands).toEqual([globalCommand]);

  await useNative.getState().createGlobalCommand(LOCAL_RUNNER, {
    name: "deploy",
    description: deployCommand.description,
    template: deployCommand.template,
    agent: null,
    model: null,
    subtask: false,
  });
  expect(created).toHaveBeenCalledWith(LOCAL_RUNNER, expect.objectContaining({ name: "deploy" }));
  expect(
    useNative
      .getState()
      .globalCommands?.map((c) => c.name)
      .sort(),
  ).toEqual(["deploy", "review"]);

  await useNative.getState().updateGlobalCommand(LOCAL_RUNNER, globalCommand, {
    description: "Updated",
    template: globalCommand.template,
    agent: null,
    model: null,
    subtask: false,
  });
  expect(updated).toHaveBeenCalledWith(LOCAL_RUNNER, "review", "rev-1", expect.objectContaining({ description: "Updated" }));
  expect(useNative.getState().globalCommands?.find((c) => c.name === "review")?.description).toBe("Updated");

  await useNative.getState().deleteGlobalCommand(LOCAL_RUNNER, { ...globalCommand, description: "Updated" });
  expect(deleted).toHaveBeenCalledWith(LOCAL_RUNNER, "review", "rev-1");
  expect(useNative.getState().globalCommands).toEqual([deployCommand]);

  listed.mockRestore();
  created.mockRestore();
  updated.mockRestore();
  deleted.mockRestore();
  slashCatalog.mockRestore();
});

test("successful global command create and delete refresh every loaded slash catalog", async () => {
  reset();
  const deployGlobal = { ...globalSlashEntry, name: "deploy" };
  const projectDeploy = { ...globalCommand, name: "deploy" };
  const effectiveProjectDeploy: SlashEntryInfo = { ...deployGlobal, origin: "project", shadowsGlobal: true };
  const slashCatalog = spyOn(commands, "slashCatalog")
    .mockResolvedValueOnce({ status: "ok", data: [deployGlobal] })
    .mockResolvedValueOnce({ status: "ok", data: [effectiveProjectDeploy] })
    .mockResolvedValueOnce({ status: "ok", data: [deployGlobal] });
  const created = spyOn(commands, "globalCommandCreate").mockResolvedValue({ status: "ok", data: projectDeploy });
  const deleted = spyOn(commands, "globalCommandDelete").mockResolvedValue({ status: "ok", data: null });
  const listed = spyOn(commands, "globalCommandList").mockResolvedValue({ status: "ok", data: [] });

  await useNative.getState().loadSlashCatalog(LOCAL_RUNNER, "p1", "agent-1");
  await useNative.getState().createGlobalCommand(LOCAL_RUNNER, projectDeploy);
  expect(useNative.getState().slashCatalogByKey[p1Agent1Key]).toEqual([effectiveProjectDeploy]);

  await useNative.getState().deleteGlobalCommand(LOCAL_RUNNER, projectDeploy);
  expect(useNative.getState().slashCatalogByKey[p1Agent1Key]).toEqual([deployGlobal]);
  expect(slashCatalog).toHaveBeenCalledTimes(3);

  slashCatalog.mockRestore();
  created.mockRestore();
  deleted.mockRestore();
  listed.mockRestore();
});

test("a successful global command mutation invalidates a deferred stale slash catalog load", async () => {
  reset();
  type CatalogResult = Awaited<ReturnType<typeof commands.slashCatalog>>;
  const resolvers: Array<(result: CatalogResult) => void> = [];
  const slashCatalog = spyOn(commands, "slashCatalog").mockImplementation(
    () => new Promise<CatalogResult>((resolve) => resolvers.push(resolve)),
  );
  const createdCommand = { ...globalCommand, name: "ship" };
  const created = spyOn(commands, "globalCommandCreate").mockResolvedValue({ status: "ok", data: createdCommand });
  const listed = spyOn(commands, "globalCommandList").mockResolvedValue({ status: "ok", data: [] });
  const staleLoad = useNative.getState().loadSlashCatalog(LOCAL_RUNNER, "p1", "agent-1");
  const mutation = useNative.getState().createGlobalCommand(LOCAL_RUNNER, createdCommand);

  await Promise.resolve();
  expect(slashCatalog).toHaveBeenCalledTimes(2);
  resolvers[1]({ status: "ok", data: [{ ...globalSlashEntry, name: "ship", origin: "project", shadowsGlobal: true }] });
  await mutation;
  resolvers[0]({ status: "ok", data: [globalSlashEntry] });
  await staleLoad;

  expect(useNative.getState().slashCatalogByKey[p1Agent1Key]).toEqual([
    { ...globalSlashEntry, name: "ship", origin: "project", shadowsGlobal: true },
  ]);
  slashCatalog.mockRestore();
  created.mockRestore();
  listed.mockRestore();
});

test("a successful global command create ignores a failed slash catalog reload", async () => {
  reset();
  const cachedEntry = { ...globalSlashEntry, name: "deploy" };
  const createdCommand = { ...globalCommand, name: "deploy" };
  const slashCatalog = spyOn(commands, "slashCatalog")
    .mockResolvedValueOnce({ status: "ok", data: [cachedEntry] })
    .mockRejectedValueOnce(new Error("slash catalog unavailable"));
  const created = spyOn(commands, "globalCommandCreate").mockResolvedValue({ status: "ok", data: createdCommand });
  const listed = spyOn(commands, "globalCommandList").mockResolvedValue({ status: "ok", data: [] });

  await useNative.getState().loadSlashCatalog(LOCAL_RUNNER, "p1", "agent-1");
  const result = await useNative.getState().createGlobalCommand(LOCAL_RUNNER, createdCommand);

  expect(result).toEqual({ status: "success" });
  expect(useNative.getState().slashCatalogByKey[p1Agent1Key]).toEqual([cachedEntry]);
  slashCatalog.mockRestore();
  created.mockRestore();
  listed.mockRestore();
});

test("a successful global command mutation invalidates a deferred stale global command load", async () => {
  reset();
  type CommandsResult = Awaited<ReturnType<typeof commands.globalCommandList>>;
  const resolvers: Array<(result: CommandsResult) => void> = [];
  const listed = spyOn(commands, "globalCommandList").mockImplementation(
    () => new Promise<CommandsResult>((resolve) => resolvers.push(resolve)),
  );
  const created = spyOn(commands, "globalCommandCreate").mockResolvedValue({ status: "ok", data: { ...globalCommand, name: "ship" } });
  // Defensive: guards against a slash catalog key tracked by an earlier test
  // triggering a real, unmocked IPC call during this mutation's refresh.
  const slashCatalog = spyOn(commands, "slashCatalog").mockResolvedValue({ status: "ok", data: [] });

  const staleLoad = useNative.getState().loadGlobalCommands(LOCAL_RUNNER); // older fetch…
  const mutation = useNative.getState().createGlobalCommand(LOCAL_RUNNER, { ...globalCommand, name: "ship" }); // …whose own refresh supersedes it

  await Promise.resolve();
  expect(listed).toHaveBeenCalledTimes(2);
  // The mutation's own refresh (the newer fetch) resolves first with the authoritative list.
  resolvers[1]({ status: "ok", data: [{ ...globalCommand, name: "ship" }] });
  await mutation;
  // The original stale load resolves late with outdated data — it must be ignored.
  resolvers[0]({ status: "ok", data: [globalCommand] });
  await staleLoad;

  expect(useNative.getState().globalCommands?.map((command) => command.name)).toEqual(["ship"]);
  listed.mockRestore();
  created.mockRestore();
  slashCatalog.mockRestore();
});

test("global command conflicts return structured outcomes and reload the latest global cache", async () => {
  reset();
  const listed = spyOn(commands, "globalCommandList").mockResolvedValue({
    status: "ok",
    data: [{ ...globalCommand, description: "Latest", revision: "rev-2" }],
  });
  const updated = spyOn(commands, "globalCommandUpdate").mockResolvedValue({
    status: "error",
    error: { message: "revision conflict" },
  });

  const result = await useNative.getState().updateGlobalCommand(LOCAL_RUNNER, globalCommand, {
    description: "Mine",
    template: globalCommand.template,
    agent: null,
    model: null,
    subtask: false,
  });

  expect(result).toEqual({ status: "conflict", message: "revision conflict" });
  expect(listed).toHaveBeenCalledWith(LOCAL_RUNNER);
  expect(useNative.getState().globalCommands?.[0]?.description).toBe("Latest");
  listed.mockRestore();
  updated.mockRestore();
});

test("a failed slash catalog load leaves the cache untouched", async () => {
  reset();
  const spy = spyOn(commands, "slashCatalog").mockResolvedValue({
    status: "error",
    error: { message: "boom" },
  });
  await useNative.getState().loadSlashCatalog(LOCAL_RUNNER, "p1", "agent-1");
  expect(useNative.getState().slashCatalogByKey[p1Agent1Key]).toBeUndefined();
  spy.mockRestore();
});

test("loadTodos caches a session's todo list", async () => {
  reset();
  const spy = spyOn(commands, "sessionTodos").mockResolvedValue({
    status: "ok",
    data: [
      { content: "step one", status: "completed" },
      { content: "step two", status: "in_progress" },
    ],
  });
  await useNative.getState().loadTodos(LOCAL_RUNNER, "s1");
  expect(spy).toHaveBeenCalledWith(LOCAL_RUNNER, "s1");
  const todos = useNative.getState().todosBySession[s1];
  expect(todos).toHaveLength(2);
  expect(todos[1]).toEqual({ content: "step two", status: "in_progress" });
  spy.mockRestore();
});

test("exportSession returns the JSON payload", async () => {
  reset();
  const spy = spyOn(commands, "exportSession").mockResolvedValue({ status: "ok", data: '{"version":1}' });
  const out = await useNative.getState().exportSession(LOCAL_RUNNER, "s1");
  expect(spy).toHaveBeenCalledWith(LOCAL_RUNNER, "s1");
  expect(out).toBe('{"version":1}');
  spy.mockRestore();
});

test("shareSession returns the rendered HTML", async () => {
  reset();
  const spy = spyOn(commands, "shareSession").mockResolvedValue({
    status: "ok",
    data: "<!doctype html><title>x</title>",
  });
  const out = await useNative.getState().shareSession(LOCAL_RUNNER, "s1");
  expect(spy).toHaveBeenCalledWith(LOCAL_RUNNER, "s1");
  expect(out).toContain("<!doctype html>");
  spy.mockRestore();
});

test("importSession reports success", async () => {
  reset();
  const spy = spyOn(commands, "importSession").mockResolvedValue({
    status: "ok",
    data: {
      sessionPk: "new",
      primaryAgentId: null,
      primaryAgentSnapshot: null,
      projectId: "p1",
      agentSessionId: null,
      worktreePath: null,
      branch: null,
      title: "Imported",
      status: "ended",
      permMode: "default",
      startedBy: "import",
      createdAt: 0,
      lastActive: 0,
      resumeAttempts: 0,
      branchOwned: true,
      kind: "project",
      speaker: null,
      agent: null,
      parentSessionPk: null,
    },
  });
  const ok = await useNative.getState().importSession(LOCAL_RUNNER, "p1", '{"version":1}');
  expect(spy).toHaveBeenCalledWith(LOCAL_RUNNER, "p1", '{"version":1}');
  expect(ok).toBe(true);
  spy.mockRestore();
});

test("loadTodos drops out-of-order responses (a stale fetch can't clobber newer data)", async () => {
  reset();
  type TodosResult = Awaited<ReturnType<typeof commands.sessionTodos>>;
  const resolvers: Array<(v: TodosResult) => void> = [];
  const spy = spyOn(commands, "sessionTodos").mockImplementation(() => new Promise<TodosResult>((resolve) => resolvers.push(resolve)));
  const first = useNative.getState().loadTodos(LOCAL_RUNNER, "s1"); // older fetch…
  const second = useNative.getState().loadTodos(LOCAL_RUNNER, "s1"); // …superseded by this one
  // The newer fetch resolves first with the fresh list.
  resolvers[1]({ status: "ok", data: [{ content: "execute", status: "in_progress" }] });
  await second;
  // The older fetch resolves late with the stale list — it must be ignored.
  resolvers[0]({ status: "ok", data: [{ content: "plan", status: "completed" }] });
  await first;
  expect(useNative.getState().todosBySession[s1]).toEqual([{ content: "execute", status: "in_progress" }]);
  spy.mockRestore();
});

test("loadQueue keeps same session pks separate across runners", async () => {
  reset();
  const remote = "remote-1";
  const localKey = sessKey(LOCAL_RUNNER, "s1");
  const remoteKey = sessKey(remote, "s1");
  const spy = spyOn(commands, "sessionQueue").mockImplementation(async (runnerId) => ({
    status: "ok",
    data: [{ id: runnerId === LOCAL_RUNNER ? "local" : "remote", text: "queued" }],
  }));

  await useNative.getState().loadQueue(LOCAL_RUNNER, "s1");
  await useNative.getState().loadQueue(remote, "s1");

  expect(useNative.getState().queuedBySession[localKey]).toEqual([{ id: "local", text: "queued" }]);
  expect(useNative.getState().queuedBySession[remoteKey]).toEqual([{ id: "remote", text: "queued" }]);
  spy.mockRestore();
});

test("enqueueQueueMessage appends the server message and removeQueueMessage filters it", async () => {
  reset();
  const spyEnqueue = spyOn(commands, "enqueueSessionMessage").mockResolvedValue({ status: "ok", data: { id: "a", text: "hello" } });
  const spyRemove = spyOn(commands, "removeSessionMessage").mockResolvedValue({ status: "ok", data: true });

  expect(await useNative.getState().enqueueQueueMessage(LOCAL_RUNNER, "s1", "hello", null)).toBe(true);
  expect(useNative.getState().queuedBySession[s1]).toEqual([{ id: "a", text: "hello" }]);
  expect(await useNative.getState().removeQueueMessage(LOCAL_RUNNER, "s1", "a")).toBe(true);
  expect(useNative.getState().queuedBySession[s1]).toEqual([]);
  expect(spyEnqueue).toHaveBeenCalledWith(LOCAL_RUNNER, "s1", "hello", null);
  expect(spyRemove).toHaveBeenCalledWith(LOCAL_RUNNER, "s1", "a");
  spyEnqueue.mockRestore();
  spyRemove.mockRestore();
});

test("failed queue mutations leave the cached queue unchanged", async () => {
  reset();
  useNative.setState({ queuedBySession: { [s1]: [{ id: "a", text: "kept" }] } });
  const enqueue = spyOn(commands, "enqueueSessionMessage").mockResolvedValue({ status: "error", error: { message: "boom" } });
  const remove = spyOn(commands, "removeSessionMessage").mockResolvedValue({ status: "error", error: { message: "boom" } });

  expect(await useNative.getState().enqueueQueueMessage(LOCAL_RUNNER, "s1", "new", null)).toBe(false);
  expect(await useNative.getState().removeQueueMessage(LOCAL_RUNNER, "s1", "a")).toBe(false);
  expect(useNative.getState().queuedBySession[s1]).toEqual([{ id: "a", text: "kept" }]);
  enqueue.mockRestore();
  remove.mockRestore();
});

test("loadQueue drops an out-of-order stale response", async () => {
  reset();
  type QueueResult = Awaited<ReturnType<typeof commands.sessionQueue>>;
  const resolvers: Array<(value: QueueResult) => void> = [];
  const spy = spyOn(commands, "sessionQueue").mockImplementation(() => new Promise<QueueResult>((resolve) => resolvers.push(resolve)));

  const first = useNative.getState().loadQueue(LOCAL_RUNNER, "s1");
  const second = useNative.getState().loadQueue(LOCAL_RUNNER, "s1");
  resolvers[1]({ status: "ok", data: [{ id: "new", text: "newest" }] });
  await second;
  resolvers[0]({ status: "ok", data: [{ id: "old", text: "stale" }] });
  await first;

  expect(useNative.getState().queuedBySession[s1]).toEqual([{ id: "new", text: "newest" }]);
  spy.mockRestore();
});

test("a successful enqueue does not duplicate an id already loaded from the server", async () => {
  reset();
  type QueueResult = Awaited<ReturnType<typeof commands.sessionQueue>>;
  let resolveFetch!: (value: QueueResult) => void;
  const queue = spyOn(commands, "sessionQueue").mockImplementation(
    () =>
      new Promise<QueueResult>((resolve) => {
        resolveFetch = resolve;
      }),
  );
  const enqueue = spyOn(commands, "enqueueSessionMessage").mockResolvedValue({ status: "ok", data: { id: "new", text: "new message" } });

  const load = useNative.getState().loadQueue(LOCAL_RUNNER, "s1");
  resolveFetch({ status: "ok", data: [{ id: "new", text: "new message" }] });
  await load;
  expect(await useNative.getState().enqueueQueueMessage(LOCAL_RUNNER, "s1", "new message", null)).toBe(true);

  expect(useNative.getState().queuedBySession[s1]).toEqual([{ id: "new", text: "new message" }]);
  queue.mockRestore();
  enqueue.mockRestore();
});

test("a stale queue fetch cannot overwrite a successful enqueue", async () => {
  reset();
  type QueueResult = Awaited<ReturnType<typeof commands.sessionQueue>>;
  let resolveFetch!: (value: QueueResult) => void;
  const queue = spyOn(commands, "sessionQueue").mockImplementation(
    () =>
      new Promise<QueueResult>((resolve) => {
        resolveFetch = resolve;
      }),
  );
  const enqueue = spyOn(commands, "enqueueSessionMessage").mockResolvedValue({ status: "ok", data: { id: "new", text: "new message" } });

  const load = useNative.getState().loadQueue(LOCAL_RUNNER, "s1");
  expect(await useNative.getState().enqueueQueueMessage(LOCAL_RUNNER, "s1", "new message", null)).toBe(true);
  resolveFetch({ status: "ok", data: [{ id: "old", text: "stale message" }] });
  await load;

  expect(useNative.getState().queuedBySession[s1]).toEqual([{ id: "new", text: "new message" }]);
  queue.mockRestore();
  enqueue.mockRestore();
});

test("a stale queue fetch cannot restore a successfully removed message", async () => {
  reset();
  useNative.setState({ queuedBySession: { [s1]: [{ id: "remove", text: "remove me" }] } });
  type QueueResult = Awaited<ReturnType<typeof commands.sessionQueue>>;
  let resolveFetch!: (value: QueueResult) => void;
  const queue = spyOn(commands, "sessionQueue").mockImplementation(
    () =>
      new Promise<QueueResult>((resolve) => {
        resolveFetch = resolve;
      }),
  );
  const remove = spyOn(commands, "removeSessionMessage").mockResolvedValue({ status: "ok", data: true });

  const load = useNative.getState().loadQueue(LOCAL_RUNNER, "s1");
  expect(await useNative.getState().removeQueueMessage(LOCAL_RUNNER, "s1", "remove")).toBe(true);
  resolveFetch({ status: "ok", data: [{ id: "remove", text: "stale message" }] });
  await load;

  expect(useNative.getState().queuedBySession[s1]).toEqual([]);
  queue.mockRestore();
  remove.mockRestore();
});

test("a rejected queue load leaves cached messages unchanged", async () => {
  reset();
  useNative.setState({ queuedBySession: { [s1]: [{ id: "kept", text: "keep me" }] } });
  const queue = spyOn(commands, "sessionQueue").mockRejectedValue(new Error("boom"));

  await useNative.getState().loadQueue(LOCAL_RUNNER, "s1");

  expect(useNative.getState().queuedBySession[s1]).toEqual([{ id: "kept", text: "keep me" }]);
  queue.mockRestore();
});

test("a rejected queue enqueue returns false and leaves cached messages unchanged", async () => {
  reset();
  useNative.setState({ queuedBySession: { [s1]: [{ id: "kept", text: "keep me" }] } });
  const enqueue = spyOn(commands, "enqueueSessionMessage").mockRejectedValue(new Error("boom"));

  expect(await useNative.getState().enqueueQueueMessage(LOCAL_RUNNER, "s1", "new message", null)).toBe(false);

  expect(useNative.getState().queuedBySession[s1]).toEqual([{ id: "kept", text: "keep me" }]);
  enqueue.mockRestore();
});

test("a rejected queue removal returns false and leaves cached messages unchanged", async () => {
  reset();
  useNative.setState({ queuedBySession: { [s1]: [{ id: "kept", text: "keep me" }] } });
  const remove = spyOn(commands, "removeSessionMessage").mockRejectedValue(new Error("boom"));

  expect(await useNative.getState().removeQueueMessage(LOCAL_RUNNER, "s1", "kept")).toBe(false);

  expect(useNative.getState().queuedBySession[s1]).toEqual([{ id: "kept", text: "keep me" }]);
  remove.mockRestore();
});
