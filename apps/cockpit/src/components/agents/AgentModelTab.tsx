import { useEffect, useState } from "react";
import { Combobox, SettingsCard, SettingsCardRow, SettingsCardTitle } from "@ryuzi/ui";
import type { AgentDetailInfo, AgentModelInfo } from "@/bindings";
import { ModelPicker } from "@/components/ModelPicker";
import { useAgents } from "@/store-agents";
import { mutationFromDetail } from "./agentMutation";

export function AgentModelTab({ detail }: { detail: AgentDetailInfo }) {
  const models = useAgents((state) => state.models);
  const saving = useAgents((state) => state.saving);
  const [model, setModel] = useState<AgentModelInfo>(detail.summary.model);

  useEffect(() => setModel(detail.summary.model), [detail]);

  const value = model.kind === "route" ? model.route : model.name;
  const info = models.find((candidate) => candidate.requestValue === value) ?? detail.modelInfo;
  const supported = model.kind === "concrete" ? (info?.supported ?? []) : [];

  // Autosave: every selection change persists immediately against the
  // CURRENT detail (mutationFromDetail), so unrelated fields (permissions,
  // skills, personality, …) are never clobbered by a stale snapshot.
  const persist = (next: AgentModelInfo) => {
    setModel(next);
    void useAgents.getState().update(detail.summary.id, { ...mutationFromDetail(detail), model: next });
  };

  const selectKind = (kind: string) => {
    if (kind === model.kind) return;
    const candidate = models.find((item) => (kind === "route" ? item.kind === "namedRoute" : item.kind === "concrete"));
    if (!candidate) return;
    persist(
      kind === "route"
        ? { kind: "route", route: candidate.requestValue }
        : { kind: "concrete", name: candidate.requestValue, effort: null },
    );
  };

  // Same no-op guard as selectKind: the pickers fire onValueChange even on
  // same-value reselects — don't burn a save + SaveIndicator flash on those.
  const selectModel = (requestValue: string) => {
    if (requestValue === value) return;
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
    if (effort === (model.effort ?? "")) return;
    persist({ ...model, effort: effort || null });
  };

  return (
    <SettingsCard>
      <div className="border-b border-border px-[18px] py-3.5">
        <SettingsCardTitle>Model assignment</SettingsCardTitle>
      </div>
      <SettingsCardRow className="gap-4">
        <span className="w-40 shrink-0 text-[13px] font-medium">Selection type</span>
        <Combobox
          aria-label="Agent model type"
          className="w-[190px]"
          options={[
            { value: "concrete", label: "Concrete model" },
            { value: "route", label: "Model route" },
          ]}
          value={model.kind}
          onValueChange={selectKind}
          disabled={saving}
        />
      </SettingsCardRow>
      <SettingsCardRow className="gap-4">
        <span className="w-40 shrink-0 text-[13px] font-medium">Model</span>
        <ModelPicker
          ariaLabel="Agent model"
          variant="field"
          models={models
            .filter((item) => (model.kind === "route" ? item.kind === "namedRoute" : item.kind === "concrete"))
            .map((item) => item.requestValue)}
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
