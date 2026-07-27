import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, ChevronRight, Plus } from "lucide-react";
import { Badge, Button, SettingsCard, cn } from "@ryuzi/ui";
import type { AgentModelInfo, AgentSummaryInfo } from "@/bindings";
import { AgentAvatar } from "@/components/agents/AgentAvatar";
import { AgentEditorModal } from "@/components/agents/AgentEditorModal";
import { statsRowFragment } from "@/lib/agent-stats";
import { useAgents } from "@/store-agents";
import { useNav } from "@/store-nav";

function modelValue(model: AgentModelInfo): string {
  return model.kind === "route" ? model.route : model.name;
}

function modelLabel(model: AgentModelInfo): string {
  return modelValue(model);
}

function AgentRow({ agent }: { agent: AgentSummaryInfo }) {
  const nav = useNav();
  // Lite stats load lazily and separately from the roster (see the
  // `loadStatsBatch` effect in `AgentsView` below); the built-in row is
  // never included in that batch, so it never has a fragment to show.
  const stats = useAgents((s) => (agent.builtin ? undefined : s.statsByAgent[agent.id]));
  // Built-in rows (the synthetic Fresh Agent) are non-editable: dashed frame,
  // "Built-in" badge, no validation surface. No row carries an actions menu —
  // Start chat/Duplicate/Delete live on the detail header (AgentActionsMenu
  // in AgentDetailView). Every row opens a detail page on click.
  return (
    <SettingsCard className={cn("flex h-[92px] items-stretch", agent.builtin && "border-dashed")}>
      <Button
        type="button"
        variant="ghost"
        aria-label={`Open ${agent.name}`}
        onClick={() => nav.navigate({ kind: "agentDetail", agentId: agent.id })}
        className="h-full min-w-0 flex-1 justify-start gap-3 rounded-none px-[18px] text-left font-normal hover:bg-accent/50"
      >
        <AgentAvatar pet={agent.avatarPet} size={36} />
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-2">
            <span className="truncate text-[13.5px] font-semibold text-foreground">{agent.name}</span>
            {agent.builtin && <Badge variant="outline">Built-in</Badge>}
            {agent.isDefault && <Badge variant="secondary">Default</Badge>}
            {!agent.executable && !agent.builtin && (
              <Badge variant="destructive">
                <AlertTriangle aria-hidden size={11} strokeWidth={2} /> Invalid
              </Badge>
            )}
          </span>
          <span className="mt-1 block truncate text-xs text-muted-foreground">{agent.description}</span>
          <span className="mt-1.5 flex items-center gap-2.5 text-[11px] text-muted-foreground">
            <span className="font-mono text-foreground">{modelLabel(agent.model)}</span>
            <span>
              {agent.skillCount} {agent.skillCount === 1 ? "skill" : "skills"} · {agent.toolCount}{" "}
              {agent.toolCount === 1 ? "tool" : "tools"}
              {stats ? ` · ${statsRowFragment(stats)}` : null}
            </span>
          </span>
        </span>
        <ChevronRight aria-hidden size={14} strokeWidth={2} className="shrink-0 text-muted-foreground" />
      </Button>
    </SettingsCard>
  );
}

export function AgentsView() {
  const [createOpen, setCreateOpen] = useState(false);
  const registry = useAgents((s) => s.registry);
  const loading = useAgents((s) => s.loading);
  // The registry appends the built-in Fresh Agent row last — render in order,
  // no re-sorting here.
  const agents = useMemo(() => registry?.agents ?? [], [registry]);

  // Fire the lite-stats batch load lazily, after the registry has rendered —
  // never blocks or reorders the list (see `loadStatsBatch`'s non-blocking,
  // error-swallowing contract in store-agents.ts). Only re-fires when the
  // registry's agent set actually changes, not on every render. The
  // built-in Fresh Agent row is excluded — it has no stats to show.
  useEffect(() => {
    const ids = agents.filter((agent) => !agent.builtin).map((agent) => agent.id);
    if (ids.length > 0) void useAgents.getState().loadStatsBatch(ids);
  }, [agents]);

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-8 py-7">
      <div className="mx-auto max-w-[860px]">
        <div className="mb-5 flex min-h-10 items-start gap-3">
          <div className="min-w-0 flex-1">
            <h2 className="m-0 mb-1 text-[22px] font-semibold tracking-[-0.02em]">Agents</h2>
            <p className="m-0 text-[13px] text-muted-foreground">Manage the agents available in this workspace.</p>
          </div>
          <Button onClick={() => setCreateOpen(true)} aria-label="New agent" className="shrink-0">
            <Plus aria-hidden size={14} strokeWidth={2} /> New agent
          </Button>
        </div>
        <div className="flex flex-col gap-2.5">
          {agents.map((agent) => (
            <AgentRow key={agent.id} agent={agent} />
          ))}
          {!loading && agents.length === 0 && <p className="py-8 text-center text-[13px] text-muted-foreground">No agents found.</p>}
        </div>
      </div>
      <AgentEditorModal open={createOpen} onClose={() => setCreateOpen(false)} />
    </div>
  );
}
