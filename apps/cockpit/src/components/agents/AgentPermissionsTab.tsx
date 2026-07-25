import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Trash2, X } from "lucide-react";
import { Button, Input, Segmented, SettingsCard } from "@ryuzi/ui";
import type { AgentDetailInfo, CatalogEntryInfo, NativeToolDecisionInfo, PermissionRuleInfo } from "@/bindings";
import { useAgents } from "@/store-agents";
import { useAgentConfigurationCatalog } from "@/store-agent-catalog";
import { mutationFromDetail } from "./agentMutation";

// Base per-tool decision. Absent from `detail.nativeTools` means "ask" (see
// `AgentPermissions::native_decision` in the core crate).
const BASE_OPTIONS = [
  { id: "off", label: "Off" },
  { id: "ask", label: "Ask" },
  { id: "allow", label: "Allow" },
] as const;
type BaseDecision = (typeof BASE_OPTIONS)[number]["id"];

// Prefix-rule decision. Rules never carry "ask" — a rule either allows or
// denies the matching command, and (per `PermGate`) a matching rule always
// wins over the row's base decision above it.
const RULE_OPTIONS = [
  { id: "allow", label: "Allow" },
  { id: "deny", label: "Deny" },
] as const;
type RuleDecision = (typeof RULE_OPTIONS)[number]["id"];

type NewRuleDraft = { commandPrefix: string; decision: RuleDecision };

function upsertDecision(list: NativeToolDecisionInfo[], tool: string, decision: BaseDecision): NativeToolDecisionInfo[] {
  const index = list.findIndex((entry) => entry.tool === tool);
  if (index === -1) return [...list, { tool, decision }];
  return list.map((entry, i) => (i === index ? { ...entry, decision } : entry));
}

function pluralizeRules(count: number): string {
  return `${count} rule${count === 1 ? "" : "s"}`;
}

type ToolRowProps = {
  entry: CatalogEntryInfo;
  decision: BaseDecision;
  rules: PermissionRuleInfo[];
  expanded: boolean;
  draft: NewRuleDraft | undefined;
  saving: boolean;
  onToggleExpanded: () => void;
  onDecisionChange: (decision: BaseDecision) => void;
  onRuleDecisionChange: (ruleId: string, decision: RuleDecision) => void;
  onRuleDelete: (ruleId: string) => void;
  onDraftStart: () => void;
  onDraftChange: (patch: Partial<NewRuleDraft>) => void;
  onDraftCancel: () => void;
  onDraftConfirm: () => void;
};

function ToolRow({
  entry,
  decision,
  rules,
  expanded,
  draft,
  saving,
  onToggleExpanded,
  onDecisionChange,
  onRuleDecisionChange,
  onRuleDelete,
  onDraftStart,
  onDraftChange,
  onDraftCancel,
  onDraftConfirm,
}: ToolRowProps) {
  const isExpanded = entry.commandScoped && expanded;
  return (
    <div data-testid={`tool-row-${entry.id}`} className="border-b border-border last:border-b-0">
      <div className="flex items-center gap-3 px-[18px] py-3">
        {entry.commandScoped ? (
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={`${isExpanded ? "Collapse" : "Expand"} ${entry.label} prefix rules`}
            onClick={onToggleExpanded}
            disabled={!entry.available}
            className="shrink-0"
          >
            {isExpanded ? <ChevronDown aria-hidden size={14} /> : <ChevronRight aria-hidden size={14} />}
          </Button>
        ) : (
          <span aria-hidden className="size-7 shrink-0" />
        )}
        <span className="min-w-0 flex-1">
          <span className={`block text-[13px] font-medium${entry.available ? "" : " text-destructive"}`}>
            {entry.available ? entry.label : `${entry.label} (unavailable)`}
          </span>
          <span className="block truncate text-[11px] text-muted-foreground">{entry.description || entry.id}</span>
        </span>
        {entry.commandScoped ? <span className="shrink-0 text-[11px] text-muted-foreground">{pluralizeRules(rules.length)}</span> : null}
        <Segmented
          options={[...BASE_OPTIONS]}
          value={decision}
          onChange={onDecisionChange}
          size="sm"
          disabled={!entry.available || saving}
        />
      </div>
      {isExpanded ? (
        <div className="bg-muted/20 pl-[46px]">
          {rules.map((rule) => (
            <div
              key={rule.id}
              data-testid={`rule-row-${rule.id}`}
              className="flex items-center gap-2 border-t border-border/60 py-2 pr-[18px]"
            >
              <code className="min-w-0 flex-1 truncate text-[11.5px]">{rule.commandPrefix}</code>
              <Segmented
                options={[...RULE_OPTIONS]}
                value={rule.decision === "deny" ? "deny" : "allow"}
                onChange={(next) => onRuleDecisionChange(rule.id, next)}
                size="sm"
                disabled={saving}
              />
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Delete rule ${rule.commandPrefix}`}
                onClick={() => onRuleDelete(rule.id)}
                disabled={saving}
              >
                <Trash2 aria-hidden size={14} />
              </Button>
            </div>
          ))}
          {draft ? (
            <div className="flex items-center gap-2 border-t border-border/60 py-2 pr-[18px]">
              <Input
                aria-label={`New prefix rule for ${entry.label}`}
                placeholder="Command prefix"
                value={draft.commandPrefix}
                onChange={(event) => onDraftChange({ commandPrefix: event.target.value })}
                className="min-w-0 flex-1"
                autoFocus
              />
              <Segmented
                options={[...RULE_OPTIONS]}
                value={draft.decision}
                onChange={(decision) => onDraftChange({ decision })}
                size="sm"
              />
              <Button size="sm" aria-label="Confirm new rule" disabled={draft.commandPrefix.trim() === ""} onClick={onDraftConfirm}>
                Add
              </Button>
              <Button variant="ghost" size="icon-sm" aria-label="Cancel new rule" onClick={onDraftCancel}>
                <X aria-hidden size={14} />
              </Button>
            </div>
          ) : (
            <button
              type="button"
              className="block w-full border-t border-border/60 py-2 pr-[18px] text-left text-[11.5px] text-muted-foreground hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
              onClick={onDraftStart}
              disabled={saving}
            >
              ＋ Add prefix rule
            </button>
          )}
        </div>
      ) : null}
    </div>
  );
}

export function AgentPermissionsTab({ detail }: { detail: AgentDetailInfo }) {
  const saving = useAgents((state) => state.saving);
  const catalog = useAgentConfigurationCatalog((state) => state.catalog);
  const catalogLoading = useAgentConfigurationCatalog((state) => state.loading);
  const catalogError = useAgentConfigurationCatalog((state) => state.error);
  const loadCatalog = useAgentConfigurationCatalog((state) => state.load);

  // Local-only UI state: search text, which command-scoped rows are expanded,
  // and any in-progress "add a prefix rule" draft. Everything else (the
  // decisions and rules themselves) is derived straight from `detail` +
  // catalog and saved immediately — there is no local copy to resync.
  const [search, setSearch] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [drafts, setDrafts] = useState<Record<string, NewRuleDraft>>({});

  useEffect(() => {
    void loadCatalog();
  }, [loadCatalog]);

  const nativeCatalog = catalog?.nativeTools ?? [];
  const decisionByTool = useMemo(
    () => new Map(detail.nativeTools.map((entry) => [entry.tool, entry.decision as BaseDecision])),
    [detail.nativeTools],
  );
  const rulesByTool = useMemo(() => {
    const map = new Map<string, PermissionRuleInfo[]>();
    for (const rule of detail.permissionRules) {
      const list = map.get(rule.tool);
      if (list) list.push(rule);
      else map.set(rule.tool, [rule]);
    }
    return map;
  }, [detail.permissionRules]);

  const query = search.trim().toLowerCase();
  const rows =
    query === ""
      ? nativeCatalog
      : nativeCatalog.filter(
          (entry) =>
            entry.label.toLowerCase().includes(query) ||
            entry.id.toLowerCase().includes(query) ||
            entry.description.toLowerCase().includes(query),
        );

  const persist = (nextNativeTools: NativeToolDecisionInfo[], nextRules: PermissionRuleInfo[]) =>
    void useAgents.getState().update(detail.summary.id, {
      ...mutationFromDetail(detail),
      nativeTools: nextNativeTools,
      permissionRules: nextRules,
    });

  const setDecision = (tool: string, decision: BaseDecision) =>
    persist(upsertDecision(detail.nativeTools, tool, decision), detail.permissionRules);

  const setRuleDecision = (ruleId: string, decision: RuleDecision) =>
    persist(
      detail.nativeTools,
      detail.permissionRules.map((rule) => (rule.id === ruleId ? { ...rule, decision } : rule)),
    );

  const deleteRule = (ruleId: string) =>
    persist(
      detail.nativeTools,
      detail.permissionRules.filter((rule) => rule.id !== ruleId),
    );

  const toggleExpanded = (tool: string) =>
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(tool)) next.delete(tool);
      else next.add(tool);
      return next;
    });

  const startDraft = (tool: string) => setDrafts((current) => ({ ...current, [tool]: { commandPrefix: "", decision: "allow" } }));
  const changeDraft = (tool: string, patch: Partial<NewRuleDraft>) =>
    setDrafts((current) => (current[tool] ? { ...current, [tool]: { ...current[tool], ...patch } } : current));
  const cancelDraft = (tool: string) =>
    setDrafts((current) => {
      if (!(tool in current)) return current;
      const next = { ...current };
      delete next[tool];
      return next;
    });
  const confirmDraft = (tool: string) => {
    const draft = drafts[tool];
    const commandPrefix = draft?.commandPrefix.trim();
    if (!draft || !commandPrefix) return;
    const rule: PermissionRuleInfo = { id: crypto.randomUUID(), tool, decision: draft.decision, commandPrefix };
    persist(detail.nativeTools, [...detail.permissionRules, rule]);
    cancelDraft(tool);
  };

  if (catalogLoading || catalogError || catalog === null) {
    return (
      <SettingsCard>
        <div className="px-[18px] py-4 text-xs text-muted-foreground" role={catalogError ? "alert" : undefined}>
          {catalogLoading ? "Loading tools…" : catalogError ? `Couldn't load tools: ${catalogError}` : "Loading tools…"}
        </div>
      </SettingsCard>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <p className="m-0 text-[11.5px] text-muted-foreground">
        Off removes a tool from the model entirely. Ask prompts on every call, Allow runs it automatically — a matching prefix rule below
        always overrides that base decision.
      </p>
      <Input aria-label="Search tools" placeholder="Search tools…" value={search} onChange={(event) => setSearch(event.target.value)} />
      <SettingsCard>
        {rows.length === 0 ? (
          <p className="m-0 px-[18px] py-5 text-xs text-muted-foreground">No tools match your search.</p>
        ) : (
          rows.map((entry) => (
            <ToolRow
              key={entry.id}
              entry={entry}
              decision={decisionByTool.get(entry.id) ?? "ask"}
              rules={rulesByTool.get(entry.id) ?? []}
              expanded={expanded.has(entry.id)}
              draft={drafts[entry.id]}
              saving={saving}
              onToggleExpanded={() => toggleExpanded(entry.id)}
              onDecisionChange={(decision) => setDecision(entry.id, decision)}
              onRuleDecisionChange={setRuleDecision}
              onRuleDelete={deleteRule}
              onDraftStart={() => startDraft(entry.id)}
              onDraftChange={(patch) => changeDraft(entry.id, patch)}
              onDraftCancel={() => cancelDraft(entry.id)}
              onDraftConfirm={() => confirmDraft(entry.id)}
            />
          ))
        )}
      </SettingsCard>
    </div>
  );
}
