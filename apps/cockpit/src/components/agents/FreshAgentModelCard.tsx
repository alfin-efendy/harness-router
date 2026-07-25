import { useEffect, useState } from "react";
import { Combobox, SettingsCard, SettingsCardRow, SettingsCardTitle } from "@ryuzi/ui";
import type { AgentDetailInfo, AgentModelInfo } from "@/bindings";
import { ModelPicker } from "@/components/ModelPicker";
import { useAgents } from "@/store-agents";

/**
 * The Fresh Agent's only editable setting: the shared subagent model. Unlike
 * `AgentModelTab` (which mutates one registry agent via `update`), this row
 * mutates the registry-wide `subagentModel` via `updateSubagentModel` — the
 * Fresh Agent isn't a registry-backed agent, so there is no per-agent
 * mutation path for it (see `fresh_agent_summary` in agent_api.rs). Mirrors
 * the deleted `SubagentSettings` component: one flat model list (routes and
 * concrete models together), no selection-type toggle.
 */
export function FreshAgentModelCard({ detail }: { detail: AgentDetailInfo }) {
  const models = useAgents((state) => state.models);
  const saving = useAgents((state) => state.saving);
  const [model, setModel] = useState<AgentModelInfo>(detail.summary.model);

  useEffect(() => setModel(detail.summary.model), [detail]);

  const value = model.kind === "route" ? model.route : model.name;
  const info = models.find((candidate) => candidate.requestValue === value) ?? detail.modelInfo;
  const supported = model.kind === "concrete" ? (info?.supported ?? []) : [];

  const persist = (next: AgentModelInfo) => {
    setModel(next);
    void useAgents.getState().updateSubagentModel(next);
  };

  const selectModel = (requestValue: string) => {
    const candidate = models.find((item) => item.requestValue === requestValue);
    if (candidate?.kind === "namedRoute") {
      persist({ kind: "route", route: requestValue });
      return;
    }
    const effort = model.kind === "concrete" && candidate?.supported.some((option) => option.value === model.effort) ? model.effort : null;
    persist({ kind: "concrete", name: requestValue, effort });
  };

  const selectEffort = (effort: string) => {
    if (model.kind !== "concrete") return;
    persist({ ...model, effort: effort || null });
  };

  return (
    <SettingsCard>
      <div className="border-b border-border px-[18px] py-3.5">
        <SettingsCardTitle>Shared subagent model</SettingsCardTitle>
      </div>
      <SettingsCardRow className="gap-4">
        <span className="w-40 shrink-0 text-[13px] font-medium">Model</span>
        <ModelPicker
          ariaLabel="Agent model"
          variant="field"
          models={models.map((item) => item.requestValue)}
          value={value}
          onValueChange={selectModel}
          disabled={saving}
        />
        {supported.length > 0 && model.kind === "concrete" ? (
          <Combobox
            aria-label="Agent effort"
            className="w-[170px]"
            options={[
              { value: "", label: "Model default" },
              ...supported.map((option) => ({ value: option.value, label: option.label, description: option.description ?? undefined })),
            ]}
            value={model.effort ?? ""}
            onValueChange={selectEffort}
            disabled={saving}
          />
        ) : null}
      </SettingsCardRow>
    </SettingsCard>
  );
}
