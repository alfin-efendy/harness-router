import { useEffect, useState } from "react";
import { AlertTriangle, ArrowLeft } from "lucide-react";
import { Badge, Button, Segmented, SettingsCard, SettingsCardTitle } from "@ryuzi/ui";
import { AgentActionsMenu } from "@/components/agents/AgentActionsMenu";
import { AgentAdvancedTab } from "@/components/agents/AgentAdvancedTab";
import { AgentAppsTab } from "@/components/agents/AgentAppsTab";
import { AgentAvatar } from "@/components/agents/AgentAvatar";
import { mutationFromDetail } from "@/components/agents/agentMutation";
import { AgentLearningTab } from "@/components/agents/AgentLearningTab";
import { AgentModelTab } from "@/components/agents/AgentModelTab";
import { AgentPermissionsTab } from "@/components/agents/AgentPermissionsTab";
import { AgentPersonalityCard } from "@/components/agents/AgentPersonalityCard";
import { AgentSkillsTab } from "@/components/agents/AgentSkillsTab";
import { FreshAgentModelCard } from "@/components/agents/FreshAgentModelCard";
import { PetPicker } from "@/components/agents/PetPicker";
import { SaveIndicator } from "@/components/agents/SaveIndicator";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { useStore } from "@/store";
import { useAgents } from "@/store-agents";
import { useNav } from "@/store-nav";

const TABS = [
  { id: "overview", label: "Overview" },
  { id: "model", label: "Model" },
  { id: "permissions", label: "Permissions" },
  { id: "skills", label: "Skills" },
  { id: "apps", label: "Apps & MCP" },
  { id: "learning", label: "Learning" },
  { id: "advanced", label: "Advanced" },
] as const;
type Tab = (typeof TABS)[number]["id"];
const COLORS: Record<string, string> = {
  violet: "#8B5CF6",
  blue: "#3B82F6",
  cyan: "#06B6D4",
  emerald: "#10B981",
  amber: "#F59E0B",
  rose: "#F43F5E",
};

function metric(value: number, singular: string, plural: string) {
  return `${value} ${value === 1 ? singular : plural}`;
}

export function AgentDetailView({ agentId }: { agentId: string }) {
  return <AgentDetailContent key={agentId} agentId={agentId} />;
}

function AgentDetailContent({ agentId }: { agentId: string }) {
  const detail = useAgents((state) => (state.detail?.summary.id === agentId ? state.detail : null));
  const loading = useAgents((state) => state.loading);
  const [tab, setTab] = useState<Tab>("overview");
  const [petPickerOpen, setPetPickerOpen] = useState(false);
  const recentSessions = useAgents((state) => state.recentSessionsByAgent[agentId] ?? []);
  const setFocused = useStore((state) => state.setFocused);
  const nav = useNav();
  const leaveDeletedDetail = () => {
    if (nav.history.back.length > 0) nav.goBack();
    else nav.navigate({ kind: "agents" });
  };
  useEffect(() => {
    if (!detail) void useAgents.getState().loadDetail(agentId);
  }, [agentId, detail]);
  useEffect(() => {
    if (detail) void useAgents.getState().loadRecentSessions(agentId);
  }, [agentId, detail]);

  if (!detail)
    return (
      <div className="flex flex-1 items-center justify-center text-[13px] text-muted-foreground">
        {loading ? "Loading agent…" : "Agent not found."}
      </div>
    );
  const { summary } = detail;

  // The Fresh Agent is a synthetic, ephemeral worker (not a registry-backed
  // agent — see `fresh_agent_summary`/`fresh_agent_detail` in agent_api.rs):
  // no identity to edit, no permissions/skills/personality of its own, just
  // the shared subagent model. Render a reduced header (no Executable/
  // Invalid badge, no actions menu — there's nothing to duplicate/delete)
  // and skip the tab strip entirely.
  if (summary.builtin) {
    return (
      <div className="min-h-0 flex-1 overflow-y-auto px-8 py-5">
        <div className="mx-auto max-w-[920px]">
          <header className="flex h-[52px] items-center gap-3 border-b border-border">
            <Button variant="ghost" size="icon-sm" aria-label="Back" title="Back" onClick={nav.goBack} className="-ml-1 shrink-0">
              <ArrowLeft aria-hidden size={15} />
            </Button>
            <AgentAvatar pet={summary.avatarPet} colorHex={COLORS[summary.avatarColor] ?? summary.avatarColor} size={32} />
            <div className="min-w-0 flex-1">
              <h2 className="m-0 truncate text-lg font-semibold">{summary.name}</h2>
              <p className="m-0 truncate text-[11px] text-muted-foreground">{summary.description}</p>
            </div>
            <Badge variant="outline">Built-in</Badge>
            <SaveIndicator />
          </header>
          <div className="mt-4">
            <FreshAgentModelCard detail={detail} />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-8 py-5">
      <div className="mx-auto max-w-[920px]">
        <header className="flex h-[52px] items-center gap-3 border-b border-border">
          <Button variant="ghost" size="icon-sm" aria-label="Back" title="Back" onClick={nav.goBack} className="-ml-1 shrink-0">
            <ArrowLeft aria-hidden size={15} />
          </Button>
          <button
            type="button"
            aria-label={`Change ${summary.name}'s pet`}
            onClick={() => setPetPickerOpen(true)}
            className="shrink-0 rounded-lg focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
          >
            <AgentAvatar pet={summary.avatarPet} colorHex={COLORS[summary.avatarColor] ?? summary.avatarColor} size={32} />
          </button>
          <div className="min-w-0 flex-1">
            <h2 className="m-0 truncate text-lg font-semibold">{summary.name}</h2>
            <p className="m-0 truncate text-[11px] text-muted-foreground">{summary.description}</p>
          </div>
          {summary.isDefault ? <Badge variant="secondary">Default</Badge> : null}
          <Badge variant={summary.executable ? "outline" : "destructive"}>
            {summary.executable ? (
              "Executable"
            ) : (
              <>
                <AlertTriangle aria-hidden size={11} /> Invalid
              </>
            )}
          </Badge>
          <SaveIndicator />
          <AgentActionsMenu agent={summary} onDeleteSuccess={leaveDeletedDetail} />
        </header>
        <div className="my-4 overflow-x-auto" data-testid="agent-detail-tabs">
          <Segmented options={[...TABS]} value={tab} onChange={setTab} />
        </div>
        {summary.validation.length > 0 ? (
          <SettingsCard className="mb-3 border-destructive/40 px-[18px] py-3">
            <SettingsCardTitle>Configuration issues</SettingsCardTitle>
            <ul className="mb-0 mt-2 pl-4 text-xs text-destructive">
              {summary.validation.map((issue) => (
                <li key={`${issue.field}:${issue.message}`}>
                  <strong>{issue.field}:</strong> {issue.message}
                </li>
              ))}
            </ul>
          </SettingsCard>
        ) : null}
        {tab === "overview" ? (
          <div className="flex flex-col gap-3">
            <AgentPersonalityCard detail={detail} />
            <div className="grid grid-cols-3 gap-3">
              <SettingsCard className="px-[18px] py-4">
                <span className="block text-[11px] text-muted-foreground">Knowledge</span>
                <strong className="mt-1 block text-[13px]">
                  {metric(summary.knowledgeCount, "readable concept", "readable concepts")}
                </strong>
              </SettingsCard>
              <SettingsCard className="px-[18px] py-4">
                <span className="block text-[11px] text-muted-foreground">Skills</span>
                <strong className="mt-1 block text-[13px]">{metric(summary.skillCount, "enabled skill", "enabled skills")}</strong>
              </SettingsCard>
              <SettingsCard className="px-[18px] py-4">
                <span className="block text-[11px] text-muted-foreground">Tools</span>
                <strong className="mt-1 block text-[13px]">{metric(summary.toolCount, "enabled tool", "enabled tools")}</strong>
              </SettingsCard>
              <SettingsCard className="col-span-3 px-[18px] py-4">
                <SettingsCardTitle>Recent sessions</SettingsCardTitle>
                {recentSessions.length === 0 ? (
                  <p className="mb-0 mt-3 text-xs text-muted-foreground">No owned sessions yet.</p>
                ) : (
                  <div className="mt-3 divide-y divide-border">
                    {recentSessions.map((session) => (
                      <Button
                        key={session.sessionPk}
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          setFocused({ runnerId: LOCAL_RUNNER, pk: session.sessionPk });
                          nav.navigate({ kind: "session" });
                        }}
                        className="h-auto w-full justify-between gap-3 rounded-none px-0 py-2 text-left"
                      >
                        <span className="min-w-0 truncate font-medium">{session.title || "Untitled session"}</span>
                        <span className="shrink-0 text-muted-foreground">{session.status}</span>
                      </Button>
                    ))}
                  </div>
                )}
              </SettingsCard>
            </div>
          </div>
        ) : null}
        {tab === "model" ? <AgentModelTab detail={detail} /> : null}
        {tab === "permissions" ? <AgentPermissionsTab detail={detail} /> : null}
        {tab === "skills" ? <AgentSkillsTab detail={detail} /> : null}
        {tab === "apps" ? <AgentAppsTab detail={detail} /> : null}
        {tab === "learning" ? <AgentLearningTab agentId={agentId} /> : null}
        {tab === "advanced" ? <AgentAdvancedTab detail={detail} onDeleteSuccess={leaveDeletedDetail} /> : null}
      </div>
      <PetPicker
        open={petPickerOpen}
        onClose={() => setPetPickerOpen(false)}
        currentPet={summary.avatarPet}
        onSelect={(avatarPet) => void useAgents.getState().update(detail.summary.id, { ...mutationFromDetail(detail), avatarPet })}
      />
    </div>
  );
}
