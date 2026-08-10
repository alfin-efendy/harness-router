import { create } from "zustand";
import { toast } from "sonner";
import { commands, type AddAppInput, type AppInfo, type CmdError, type McpConnectStart, type Result } from "./bindings";

// Apps (MCP servers) domain store. Definitions persist in the engine; probes
// do a real MCP handshake; enabled servers attach to agent sessions for real.

type AppsState = {
  apps: AppInfo[];
  loaded: boolean;
  hydrating: boolean;
  probing: string | null;
  hydrate: () => Promise<void>;
  add: (input: AddAppInput) => Promise<boolean>;
  remove: (id: string) => Promise<void>;
  probe: (id: string) => Promise<void>;
  setScope: (id: string, scope: string, scopeGateways: string[]) => Promise<void>;
  setToolPerm: (id: string, tool: string, perm: string) => Promise<void>;
  /** Allow/deny the (single, native) agent to use this app. */
  toggleAgent: (id: string, allowed: boolean) => Promise<void>;
  /** Remote MCP server OAuth connect (Task 9). `beginMcpConnect` returns the
   *  daemon's authorize URL/state/verifier/issuerTokenEndpoint/clientId (or
   *  `null` on error); the caller opens the browser and holds those values
   *  locally until its OWN loopback callback captures the redirect
   *  (Cockpit's Rust side in production — see `apps_cmd.rs`'s
   *  `begin_mcp_connect`; `completeMcpConnect` is exposed here mainly for
   *  symmetry and direct testing). `issuerTokenEndpoint`/`clientId` must be
   *  threaded straight through to `completeMcpConnect` unchanged — they name
   *  the authorization server `beginMcpConnect` actually selected, and
   *  re-deriving them some other way risks completing against a different
   *  one than the one that issued the code. Both complete/disconnect refresh
   *  `apps` from the RPC's returned list on success, same as every other
   *  mutation in this store. */
  beginMcpConnect: (id: string) => Promise<McpConnectStart | null>;
  completeMcpConnect: (id: string, code: string, verifier: string, issuerTokenEndpoint: string, clientId: string) => Promise<boolean>;
  disconnectMcp: (id: string) => Promise<boolean>;
};

function applyResult(set: (partial: Partial<AppsState>) => void, res: Result<AppInfo[], CmdError>, action: string): boolean {
  if (res.status === "ok") {
    set({ apps: res.data, loaded: true });
    return true;
  }
  toast.error(`${action} failed: ${res.error.message}`);
  return false;
}

export const useApps = create<AppsState>((set, get) => ({
  apps: [],
  loaded: false,
  hydrating: false,
  probing: null,

  hydrate: async () => {
    if (get().hydrating) return;
    set({ hydrating: true });
    try {
      applyResult(set, await commands.listApps("local"), "App list");
    } finally {
      set({ hydrating: false });
    }
  },

  add: async (input) => applyResult(set, await commands.addApp("local", input), "Add app"),

  remove: async (id) => {
    applyResult(set, await commands.removeApp("local", id), "Remove app");
  },

  probe: async (id) => {
    set({ probing: id });
    try {
      applyResult(set, await commands.probeApp("local", id), "Probe");
    } finally {
      set({ probing: null });
    }
  },

  setScope: async (id, scope, scopeGateways) => {
    applyResult(set, await commands.updateAppScope("local", id, scope, scopeGateways), "Scope update");
  },

  setToolPerm: async (id, tool, perm) => {
    set({
      apps: get().apps.map((a) => (a.id === id ? { ...a, tools: a.tools.map((t) => (t.name === tool ? { ...t, perm } : t)) } : a)),
    });
    applyResult(set, await commands.setAppToolPerm("local", id, tool, perm), "Tool permission");
  },

  toggleAgent: async (id, allowed) => {
    set({
      apps: get().apps.map((a) =>
        a.id === id ? { ...a, agentAccess: a.agentAccess.map((x) => (x.agentId === "native" ? { ...x, allowed } : x)) } : a,
      ),
    });
    applyResult(set, await commands.toggleAppAgent("local", id, "native", allowed), "Agent access");
  },

  beginMcpConnect: async (id) => {
    const res = await commands.beginMcpConnect("local", id);
    if (res.status === "error") {
      toast.error(`Couldn't start the connection: ${res.error.message}`);
      return null;
    }
    return res.data;
  },

  completeMcpConnect: async (id, code, verifier, issuerTokenEndpoint, clientId) =>
    applyResult(set, await commands.completeMcpConnect("local", id, code, verifier, issuerTokenEndpoint, clientId), "Connect"),

  disconnectMcp: async (id) => applyResult(set, await commands.disconnectMcp("local", id), "Disconnect"),
}));

export function appById(apps: AppInfo[], id: string): AppInfo | undefined {
  return apps.find((a) => a.id === id);
}

/** Whether the native agent may use this app. Missing row = allowed. */
export function agentAllowed(app: AppInfo): boolean {
  return app.agentAccess.find((x) => x.agentId === "native")?.allowed ?? true;
}
