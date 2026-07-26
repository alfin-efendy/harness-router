import { create } from "zustand";
import { toast } from "sonner";
import {
  commands,
  type AgentInfo,
  type ChatRequestOptions,
  type CommandFileInfo,
  type CommandFileInputDto,
  type CommandFileMutationDto,
  type QueuedMessageInfo,
  type SlashEntryInfo,
  type TodoItem,
} from "./bindings";
import { sessKey } from "./lib/session-key";

// Native-runtime metadata: the agents and slash commands available to a
// project, and a session's live todo list. Populated on demand from the
// native_agents / slash_catalog / session_todos Tauri commands.
//
// `agentsByProject` is keyed by projectId (projects live on the local
// engine). `slashCatalogByKey` is keyed by the project/agent pairing (see
// `catalogKey`) since the merged "/" catalog depends on both. `globalCommands`
// is a single flat list — global commands aren't project-scoped.
// `todosBySession`/`planCollapsed` are per-session, so they're keyed by
// `sessKey(runnerId, sessionPk)` — pks collide across runners.
export type ProjectCommandMutationResult =
  | { status: "success" }
  | { status: "conflict"; message: string }
  | { status: "error"; message: string };

/** Composite key for the slash catalog cache: a project/agent pairing. Like
 *  `agentsByProject`, catalogs are local-engine-only, so the runner isn't
 *  part of the key. */
export function catalogKey(projectId: string | null, agentId: string | null): string {
  return `${projectId ?? "-"}::${agentId ?? "-"}`;
}

type NativeState = {
  agentsByProject: Record<string, AgentInfo[]>;
  slashCatalogByKey: Record<string, SlashEntryInfo[]>;
  globalCommands: CommandFileInfo[] | undefined;
  todosBySession: Record<string, TodoItem[]>;
  queuedBySession: Record<string, QueuedMessageInfo[]>;
  // Whether the floating TODO List panel is collapsed to a pill, per session.
  planCollapsed: Record<string, boolean>;
  loadAgents: (runnerId: string, projectId: string) => Promise<void>;
  loadSlashCatalog: (runnerId: string, projectId: string | null, agentId: string | null) => Promise<void>;
  loadGlobalCommands: (runnerId: string) => Promise<void>;
  createGlobalCommand: (runnerId: string, input: CommandFileInputDto) => Promise<ProjectCommandMutationResult>;
  updateGlobalCommand: (runnerId: string, command: CommandFileInfo, input: CommandFileMutationDto) => Promise<ProjectCommandMutationResult>;
  deleteGlobalCommand: (runnerId: string, command: CommandFileInfo) => Promise<ProjectCommandMutationResult>;
  loadTodos: (runnerId: string, sessionPk: string) => Promise<void>;
  loadQueue: (runnerId: string, sessionPk: string) => Promise<void>;
  enqueueQueueMessage: (runnerId: string, sessionPk: string, prompt: string, options: ChatRequestOptions | null) => Promise<boolean>;
  removeQueueMessage: (runnerId: string, sessionPk: string, id: string) => Promise<boolean>;
  setPlanCollapsed: (runnerId: string, sessionPk: string, collapsed: boolean) => void;
  exportSession: (runnerId: string, sessionPk: string) => Promise<string | null>;
  importSession: (runnerId: string, projectId: string, data: string) => Promise<boolean>;
  shareSession: (runnerId: string, sessionPk: string) => Promise<string | null>;
};

// Monotonic fetch tokens prevent out-of-order agent, todo, slash-catalog,
// and global-command responses from replacing newer cache data. Keys
// identify the runner/project, the catalog pairing, or the session.
const agentFetchToken: Record<string, number> = {};
const todoFetchToken: Record<string, number> = {};
const slashCatalogFetchToken: Record<string, number> = {};
let globalCommandFetchToken = 0;
const queueFetchToken: Record<string, number> = {};

// The project/agent params behind each requested catalog key, so a global
// command mutation can refresh every known pairing without the caller
// re-supplying them. Populated as soon as a fetch is requested (not only
// once it resolves) so a mutation landing mid-flight still refreshes it.
const catalogParamsByKey: Record<string, { projectId: string | null; agentId: string | null }> = {};

function runnerProjectKey(runnerId: string, projectId: string): string {
  return `${runnerId}:${projectId}`;
}

function invalidateSlashCatalogFetch(key: string): void {
  slashCatalogFetchToken[key] = (slashCatalogFetchToken[key] ?? 0) + 1;
}

// A global command mutation can affect the effective catalog for every
// project/agent pairing (not just one), so every tracked pairing — loaded
// or still in flight — is invalidated.
function invalidateAllSlashCatalogFetches(): void {
  for (const key of Object.keys(slashCatalogFetchToken)) {
    invalidateSlashCatalogFetch(key);
  }
}

function invalidateGlobalCommandFetch(): void {
  globalCommandFetchToken += 1;
}

async function refreshAfterGlobalCommandMutation(runnerId: string): Promise<void> {
  const globalReload = useNative
    .getState()
    .loadGlobalCommands(runnerId)
    .catch(() => {
      // Global command cache reloads are best-effort after a persisted mutation.
    });
  // Iterate every project/agent pairing ever requested (not just the ones
  // that have already resolved into `slashCatalogByKey`) so a mutation that
  // lands while a first load is still in flight still refreshes it. Fired
  // concurrently with the global reload — each is independently best-effort.
  const catalogReloads = Object.keys(catalogParamsByKey).map((key) => {
    const params = catalogParamsByKey[key];
    return useNative
      .getState()
      .loadSlashCatalog(runnerId, params.projectId, params.agentId)
      .catch(() => {
        // Slash catalog reloads are best-effort after a persisted mutation.
      });
  });
  await Promise.all([globalReload, ...catalogReloads]);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export const useNative = create<NativeState>((set) => ({
  agentsByProject: {},
  slashCatalogByKey: {},
  globalCommands: undefined,
  todosBySession: {},
  queuedBySession: {},
  planCollapsed: {},

  loadAgents: async (runnerId, projectId) => {
    const key = runnerProjectKey(runnerId, projectId);
    const token = (agentFetchToken[key] ?? 0) + 1;
    agentFetchToken[key] = token;
    const res = await commands.nativeAgents(runnerId, projectId);
    if (res.status === "ok" && agentFetchToken[key] === token) {
      set((s) => ({ agentsByProject: { ...s.agentsByProject, [projectId]: res.data } }));
    }
  },

  loadSlashCatalog: async (runnerId, projectId, agentId) => {
    const key = catalogKey(projectId, agentId);
    const token = (slashCatalogFetchToken[key] ?? 0) + 1;
    slashCatalogFetchToken[key] = token;
    // Track the pairing as soon as it's requested (not just once it resolves)
    // so a mutation landing mid-flight still knows to refresh it.
    catalogParamsByKey[key] = { projectId, agentId };
    const res = await commands.slashCatalog(runnerId, projectId, agentId);
    if (res.status === "ok" && slashCatalogFetchToken[key] === token) {
      set((s) => ({ slashCatalogByKey: { ...s.slashCatalogByKey, [key]: res.data } }));
    }
  },

  loadGlobalCommands: async (runnerId) => {
    const token = (globalCommandFetchToken ?? 0) + 1;
    globalCommandFetchToken = token;
    try {
      const res = await commands.globalCommandList(runnerId);
      if (res.status === "ok" && globalCommandFetchToken === token) {
        set({ globalCommands: res.data });
      } else if (res.status === "error" && globalCommandFetchToken === token) {
        toast.error(`Couldn't load global commands: ${res.error.message}`);
      }
    } catch (error) {
      if (globalCommandFetchToken === token) toast.error(`Couldn't load global commands: ${errorMessage(error)}`);
    }
  },

  createGlobalCommand: async (runnerId, input) => {
    invalidateGlobalCommandFetch();
    try {
      const res = await commands.globalCommandCreate(runnerId, input);
      if (res.status !== "ok") {
        const message = res.error.message;
        toast.error(`Create command failed: ${message}`);
        return { status: "error", message };
      }
      invalidateGlobalCommandFetch();
      invalidateAllSlashCatalogFetches();
      set((s) => ({
        globalCommands: [...(s.globalCommands ?? []), res.data].sort((a, b) => a.name.localeCompare(b.name)),
      }));
      await refreshAfterGlobalCommandMutation(runnerId);
      return { status: "success" };
    } catch (error) {
      const message = errorMessage(error);
      toast.error(`Create command failed: ${message}`);
      return { status: "error", message };
    }
  },

  updateGlobalCommand: async (runnerId, command, input) => {
    invalidateGlobalCommandFetch();
    try {
      const res = await commands.globalCommandUpdate(runnerId, command.name, command.revision, input);
      if (res.status !== "ok") {
        const message = res.error.message;
        const conflict = /modified externally|revision conflict/i.test(message);
        toast.error(conflict ? "Command changed externally. Reloaded the latest version." : `Update command failed: ${message}`);
        if (conflict) {
          await useNative.getState().loadGlobalCommands(runnerId);
          return { status: "conflict", message };
        }
        return { status: "error", message };
      }
      invalidateGlobalCommandFetch();
      invalidateAllSlashCatalogFetches();
      set((s) => ({
        globalCommands: (s.globalCommands ?? []).map((current) => (current.name === command.name ? res.data : current)),
      }));
      await refreshAfterGlobalCommandMutation(runnerId);
      return { status: "success" };
    } catch (error) {
      const message = errorMessage(error);
      toast.error(`Update command failed: ${message}`);
      return { status: "error", message };
    }
  },

  deleteGlobalCommand: async (runnerId, command) => {
    invalidateGlobalCommandFetch();
    try {
      const res = await commands.globalCommandDelete(runnerId, command.name, command.revision);
      if (res.status !== "ok") {
        const message = res.error.message;
        const conflict = /modified externally|revision conflict/i.test(message);
        toast.error(conflict ? "Command changed externally. Reloaded the latest version." : `Delete command failed: ${message}`);
        if (conflict) {
          await useNative.getState().loadGlobalCommands(runnerId);
          return { status: "conflict", message };
        }
        return { status: "error", message };
      }
      invalidateGlobalCommandFetch();
      invalidateAllSlashCatalogFetches();
      set((s) => ({
        globalCommands: (s.globalCommands ?? []).filter((current) => current.name !== command.name),
      }));
      await refreshAfterGlobalCommandMutation(runnerId);
      return { status: "success" };
    } catch (error) {
      const message = errorMessage(error);
      toast.error(`Delete command failed: ${message}`);
      return { status: "error", message };
    }
  },

  loadTodos: async (runnerId, sessionPk) => {
    const key = sessKey(runnerId, sessionPk);
    const token = (todoFetchToken[key] ?? 0) + 1;
    todoFetchToken[key] = token;
    const res = await commands.sessionTodos(runnerId, sessionPk);
    if (res.status === "ok" && todoFetchToken[key] === token) {
      set((s) => ({ todosBySession: { ...s.todosBySession, [key]: res.data } }));
    }
  },

  loadQueue: async (runnerId, sessionPk) => {
    const key = sessKey(runnerId, sessionPk);
    const token = (queueFetchToken[key] ?? 0) + 1;
    queueFetchToken[key] = token;
    try {
      const res = await commands.sessionQueue(runnerId, sessionPk);
      if (res.status === "ok" && queueFetchToken[key] === token) {
        set((s) => ({ queuedBySession: { ...s.queuedBySession, [key]: res.data } }));
      }
    } catch {
      // Generated IPC commands may reject; retain the last known durable queue.
    }
  },

  enqueueQueueMessage: async (runnerId, sessionPk, prompt, options) => {
    try {
      const res = await commands.enqueueSessionMessage(runnerId, sessionPk, prompt, options);
      if (res.status !== "ok") return false;
      const key = sessKey(runnerId, sessionPk);
      queueFetchToken[key] = (queueFetchToken[key] ?? 0) + 1;
      set((s) => {
        const queued = s.queuedBySession[key] ?? [];
        const next = queued.some((message) => message.id === res.data.id) ? queued : [...queued, res.data];
        return { queuedBySession: { ...s.queuedBySession, [key]: next } };
      });
      return true;
    } catch {
      return false;
    }
  },

  removeQueueMessage: async (runnerId, sessionPk, id) => {
    try {
      const res = await commands.removeSessionMessage(runnerId, sessionPk, id);
      if (res.status !== "ok") return false;
      const key = sessKey(runnerId, sessionPk);
      queueFetchToken[key] = (queueFetchToken[key] ?? 0) + 1;
      set((s) => ({
        queuedBySession: { ...s.queuedBySession, [key]: (s.queuedBySession[key] ?? []).filter((message) => message.id !== id) },
      }));
      return true;
    } catch {
      return false;
    }
  },

  setPlanCollapsed: (runnerId, sessionPk, collapsed) =>
    set((s) => ({ planCollapsed: { ...s.planCollapsed, [sessKey(runnerId, sessionPk)]: collapsed } })),

  // Returns the session's portable JSON, or null on failure.
  exportSession: async (runnerId, sessionPk) => {
    const res = await commands.exportSession(runnerId, sessionPk);
    return res.status === "ok" ? res.data : null;
  },

  // Imports a previously exported session JSON under a project.
  importSession: async (runnerId, projectId, data) => {
    const res = await commands.importSession(runnerId, projectId, data);
    return res.status === "ok";
  },

  // Renders the session as a self-contained HTML document, or null on failure.
  shareSession: async (runnerId, sessionPk) => {
    const res = await commands.shareSession(runnerId, sessionPk);
    return res.status === "ok" ? res.data : null;
  },
}));
