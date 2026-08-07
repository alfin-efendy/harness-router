import { Check, Copy, Edit3, Plus, Send, Trash2, Webhook } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Button, Combobox, FormField, Input, Modal, ModalBody, ModalFooter, ModalHeader, SettingsCard, Switch, Textarea } from "@ryuzi/ui";
import type { AutomationHookDetail, AutomationHookInfo, AutomationHookInput, Project, TriggerKind } from "@/bindings";
import { ConfirmActionModal } from "@/components/modals/ConfirmActionModal";
import { Pill, PluginBadge } from "@/components/common/bits";
import { useAgents } from "@/store-agents";
import { useAutomations } from "@/store-automations";
import { useEndpoint } from "@/store-endpoint";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { useStore } from "@/store";
import { useNav } from "@/store-nav";
import { useNative } from "@/store-native";
import { usePlugins } from "@/store-plugins";

// The Claude Code alias spelling for each trigger, when one exists — ORDER
// MATTERS the same way `crate::automation::claude_alias_for` documents:
// `session.end`'s alias is deliberately `Stop`, never `SessionEnd` (Claude
// Code lists both, but `Stop` is the canonical one to surface). This mirrors
// the backend's already-resolved table rather than re-deriving an inverse
// map from `ryuzi_plugin_sdk::CLAUDE_ALIASES` in TS.
const TRIGGERS: { value: TriggerKind; label: string; alias?: string }[] = [
  { value: "session.start", label: "Session starts", alias: "SessionStart" },
  { value: "tool.before", label: "Before a tool runs", alias: "PreToolUse" },
  { value: "tool.after", label: "After a tool runs", alias: "PostToolUse" },
  { value: "session.end", label: "Session ends", alias: "Stop" },
  { value: "scheduler.run.success", label: "Scheduled run succeeds" },
  { value: "scheduler.run.failed", label: "Scheduled run fails" },
  { value: "gateway.status.changed", label: "Gateway status changes" },
  { value: "webhook.inbound", label: "Inbound webhook" },
];

const TRIGGER_OPTIONS = TRIGGERS.map((item) => ({
  value: item.value,
  label: item.label,
  description: item.alias ? `${item.alias} · ${item.value}` : item.value,
}));

/** The Claude Code alias for a trigger, when one exists — e.g. `"Stop"` for
 *  `"session.end"`. Powers the small mono caption on hook rows. */
export function triggerAlias(trigger: TriggerKind): string | undefined {
  return TRIGGERS.find((item) => item.value === trigger)?.alias;
}

/** `true` for an `agent.run` hook whose detail has an empty project id — the
 *  same "no plugin can guess the user's project" convention as the backend's
 *  `PluginHookInfo::needs_target` (`crate::api::plugins_api::plugin_hook_info`).
 *  `undefined` detail (not loaded yet) is treated as not needing a target so
 *  the row falls back to a plain toggle rather than blocking on a fetch. */
export function hookNeedsTarget(detail: AutomationHookDetail | undefined): boolean {
  return detail !== undefined && detail.action.kind === "agent.run" && detail.action.config.projectId.trim() === "";
}

const RUN_TONE: Record<string, string> = {
  success: "text-green-600",
  failed: "text-destructive",
  running: "text-blue-600",
  queued: "text-amber-600",
};

type HeaderDraft = { name: string; value: string; configured?: boolean };
type HookDraft = {
  name: string;
  triggerKind: TriggerKind;
  actionKind: "agent.run" | "webhook.outbound";
  projectId: string;
  branch: string;
  prompt: string;
  agentId: string | null;
  modelOverride: string | null;
  subtask: boolean;
  url: string;
  method: string;
  headers: HeaderDraft[];
  payloadTemplate: string;
  enabled: boolean;
};

function blankDraft(projectId = ""): HookDraft {
  return {
    name: "",
    triggerKind: "session.end",
    actionKind: "agent.run",
    projectId,
    branch: "",
    prompt: "",
    agentId: null,
    modelOverride: null,
    subtask: false,
    url: "",
    method: "POST",
    headers: [],
    payloadTemplate: "",
    enabled: true,
  };
}

function draftFor(detail: AutomationHookDetail): HookDraft {
  const base = blankDraft();
  if (detail.action.kind === "agent.run") {
    return {
      ...base,
      name: detail.hook.name,
      triggerKind: detail.hook.triggerKind,
      actionKind: "agent.run",
      enabled: detail.hook.enabled,
      ...detail.action.config,
    };
  }
  return {
    ...base,
    name: detail.hook.name,
    triggerKind: detail.hook.triggerKind,
    enabled: detail.hook.enabled,
    actionKind: "webhook.outbound",
    url: detail.action.config.url,
    method: detail.action.config.method,
    headers: detail.action.config.headers.map((header) => ({ name: header.name, value: "", configured: header.configured })),
    payloadTemplate: detail.action.config.payloadTemplate ?? "",
  };
}

function toInput(draft: HookDraft): AutomationHookInput {
  const action =
    draft.actionKind === "agent.run"
      ? {
          kind: "agent.run" as const,
          config: {
            projectId: draft.projectId,
            branch: draft.branch,
            gatewayId: "local",
            prompt: draft.prompt,
            agentId: draft.agentId,
            modelOverride: draft.modelOverride,
            subtask: draft.subtask,
          },
        }
      : {
          kind: "webhook.outbound" as const,
          config: {
            url: draft.url,
            method: draft.method,
            headers: draft.headers
              .filter((header) => header.name.trim() && (header.configured || header.value))
              .map(({ name, value }) => ({ name: name.trim(), value })),
            payloadTemplate: draft.payloadTemplate.trim() || null,
          },
        };
  return { name: draft.name.trim(), triggerKind: draft.triggerKind, action, enabled: draft.enabled };
}

function triggerLabel(trigger: TriggerKind): string {
  return TRIGGERS.find((item) => item.value === trigger)?.label ?? trigger;
}

function eventPayloadExample(): string {
  return JSON.stringify(
    { event: "webhook.inbound", occurredAt: "2026-07-14T00:00:00Z", source: { kind: "webhook", id: "external" }, data: { example: true } },
    null,
    2,
  );
}

function HookRow({
  hook,
  detail,
  onEdit,
  onDuplicate,
}: {
  hook: AutomationHookInfo;
  detail?: AutomationHookDetail;
  onEdit: () => void;
  onDuplicate: () => void;
}) {
  const toggle = useAutomations((state) => state.toggle);
  const plugins = usePlugins((state) => state.plugins);
  const latest = detail?.runs[0];
  const alias = triggerAlias(hook.triggerKind);
  const needsTarget = hookNeedsTarget(detail);
  return (
    <SettingsCard className="flex items-center gap-3 px-[18px] py-3">
      <Button
        variant="ghost"
        onClick={onEdit}
        className="h-auto min-w-0 flex-1 justify-start gap-3 p-0 text-left font-normal text-foreground"
      >
        <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
          <Webhook aria-hidden size={17} />
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-1.5">
            <span className="truncate text-sm font-semibold">{hook.name}</span>
            {hook.pluginId !== null && <PluginBadge pluginId={hook.pluginId} plugins={plugins} className="shrink-0" />}
          </span>
          <span className="mt-0.5 flex items-center gap-1.5 truncate text-xs text-muted-foreground">
            {triggerLabel(hook.triggerKind)} · {hook.actionKind === "agent.run" ? "Agent run" : "Webhook delivery"}
            {alias && (
              <Pill variant="mono" className="shrink-0">
                {alias}
              </Pill>
            )}
          </span>
        </span>
      </Button>
      <span className="hidden text-right text-[11px] text-muted-foreground sm:block">
        {latest ? (
          <>
            <span className={RUN_TONE[latest.status] ?? ""}>{latest.status}</span>
            <br />
            {new Date(latest.finishedAt ?? latest.queuedAt).toLocaleString()}
          </>
        ) : (
          "No runs yet"
        )}
      </span>
      {needsTarget ? (
        <Button variant="outline" size="sm" onClick={onEdit}>
          Set up…
        </Button>
      ) : (
        <Switch
          on={hook.enabled}
          onToggle={() => void toggle(hook.id, !hook.enabled)}
          label={`${hook.enabled ? "Disable" : "Enable"} ${hook.name}`}
        />
      )}
      {hook.pluginId !== null ? (
        <Button variant="ghost" size="icon" aria-label={`Duplicate ${hook.name} as mine`} onClick={onDuplicate}>
          <Copy aria-hidden size={15} />
        </Button>
      ) : (
        <Button variant="ghost" size="icon" aria-label={`Edit ${hook.name}`} onClick={onEdit}>
          <Edit3 aria-hidden size={15} />
        </Button>
      )}
    </SettingsCard>
  );
}

function RunHistory({ detail }: { detail: AutomationHookDetail }) {
  const nav = useNav();
  if (detail.runs.length === 0) return <p className="text-xs text-muted-foreground">No hook runs yet.</p>;
  return (
    <div className="flex flex-col gap-2">
      {detail.runs.slice(0, 20).map((run) => (
        <div key={run.id} className="rounded-lg border border-border px-3 py-2 text-xs">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className={`font-medium ${RUN_TONE[run.status] ?? ""}`}>{run.status}</span>
            <span className="text-muted-foreground">{new Date(run.finishedAt ?? run.queuedAt).toLocaleString()}</span>
          </div>
          {run.lastHttpStatus !== null && <p className="mt-1 text-muted-foreground">HTTP {run.lastHttpStatus}</p>}
          {run.error && <p className="mt-1 text-destructive">{run.error}</p>}
          {run.sessionPk && (
            <Button
              variant="link"
              className="mt-1 h-auto p-0 text-xs"
              aria-label={`Open session ${run.sessionPk}`}
              onClick={() => {
                useStore.getState().setFocused({ runnerId: LOCAL_RUNNER, pk: run.sessionPk! });
                nav.navigate({ kind: "session" });
              }}
            >
              Open session {run.sessionPk}
            </Button>
          )}
          {run.attempts.slice(0, 3).map((attempt) => (
            <p key={attempt.ordinal} className="mt-1 text-muted-foreground">
              Attempt {attempt.ordinal} · {attempt.httpStatus === null ? "No HTTP response" : `HTTP ${attempt.httpStatus}`}
            </p>
          ))}
        </div>
      ))}
    </div>
  );
}

function InboundEndpoint({ hook, status }: { hook: AutomationHookInfo | null; status: ReturnType<typeof useEndpoint.getState>["status"] }) {
  const nav = useNav();
  const [copied, setCopied] = useState(false);
  if (!hook?.inboundPath) return null;
  const url = `${status?.baseUrl ?? "http://127.0.0.1/v1"}/automations/hooks/${hook.inboundPath}`;
  const copy = async (value: string) => {
    await navigator.clipboard?.writeText(value);
    setCopied(true);
  };
  if (!status?.running) {
    return (
      <SettingsCard className="p-3 text-xs">
        <p className="font-medium">The local endpoint is stopped.</p>
        <p className="mt-1 text-muted-foreground">Start it from Models before sending inbound webhooks.</p>
        <Button className="mt-2" variant="outline" size="sm" onClick={() => nav.navigate({ kind: "models" })}>
          Open Models
        </Button>
      </SettingsCard>
    );
  }
  const curl = `curl -X POST ${url} -H "Authorization: Bearer $RYUZI_API_KEY" -H "Content-Type: application/json" -d '${eventPayloadExample().replace(/\n/g, "")}'`;
  return (
    <SettingsCard className="flex flex-col gap-3 p-3 text-xs">
      <div>
        <p className="font-medium">Inbound endpoint</p>
        <p className="mt-1 break-all font-mono text-muted-foreground">{url}</p>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button variant="outline" size="sm" onClick={() => void copy(url)}>
          {copied ? <Check aria-hidden size={14} /> : <Copy aria-hidden size={14} />} Copy URL
        </Button>
        <Button variant="outline" size="sm" onClick={() => void copy(curl)}>
          <Copy aria-hidden size={14} /> Copy curl
        </Button>
      </div>
      <pre className="overflow-x-auto whitespace-pre-wrap rounded-md bg-muted p-2 text-[11px]">{curl}</pre>
      <p className="text-muted-foreground">Payload contract</p>
      <pre className="overflow-x-auto whitespace-pre-wrap rounded-md bg-muted p-2 text-[11px]">{eventPayloadExample()}</pre>
    </SettingsCard>
  );
}

function HookEditor({
  hook,
  initialDraft,
  projects,
  onClose,
}: {
  hook: AutomationHookInfo | null;
  /** Seeds the draft for a "Duplicate as mine" create (Task 16) — ignored
   *  once `hook` is set, since an existing hook's own detail always wins. */
  initialDraft?: HookDraft;
  projects: Project[];
  onClose: () => void;
}) {
  const selectedProjectId = useStore((state) => state.selectedProjectId);
  const plugins = usePlugins((state) => state.plugins);
  const detail = useAutomations((state) => (hook ? state.detailsById[hook.id] : undefined));
  const loadDetail = useAutomations((state) => state.loadDetail);
  const create = useAutomations((state) => state.create);
  const update = useAutomations((state) => state.update);
  const remove = useAutomations((state) => state.remove);
  const testOutbound = useAutomations((state) => state.testOutbound);
  const endpointStatus = useEndpoint((state) => state.status);
  const endpointLoaded = useEndpoint((state) => state.loaded);
  const hydrateEndpoint = useEndpoint((state) => state.hydrate);
  const models = useAgents((state) => state.models);
  const [draft, setDraft] = useState<HookDraft>(() =>
    detail ? draftFor(detail) : (initialDraft ?? blankDraft(selectedProjectId ?? projects[0]?.projectId ?? "")),
  );
  const [saving, setSaving] = useState(false);
  // A plugin-owned hook is read-only except its target (project/branch) — the
  // only fields `needs_target`'s "Set up…" flow can legitimately change; the
  // plugin still authors the name/trigger/prompt. Hidden entirely when
  // duplicating (create mode never carries `hook`), so the copy is fully
  // editable.
  const pluginOwned = hook?.pluginId != null;
  const projectAgents = useNative((state) => (draft.projectId ? state.agentsByProject[draft.projectId] : undefined));
  const [deleting, setDeleting] = useState(false);
  const [deleteTrigger, setDeleteTrigger] = useState<HTMLButtonElement | null>(null);
  const nameRef = useRef<HTMLInputElement>(null);
  const dirtyRef = useRef(false);
  const patchDraft = (patch: (current: HookDraft) => HookDraft) => {
    dirtyRef.current = true;
    setDraft(patch);
  };

  useEffect(() => {
    if (hook && !detail) void loadDetail(hook.id);
  }, [detail, hook, loadDetail]);
  useEffect(() => {
    if (detail && !dirtyRef.current) setDraft(draftFor(detail));
  }, [detail]);
  useEffect(() => {
    if (draft.projectId) void useNative.getState().loadAgents(LOCAL_RUNNER, draft.projectId);
  }, [draft.projectId]);
  useEffect(() => {
    if (draft.triggerKind === "webhook.inbound" && !endpointLoaded) void hydrateEndpoint();
  }, [draft.triggerKind, endpointLoaded, hydrateEndpoint]);

  const isInbound = draft.triggerKind === "webhook.inbound";
  const valid = draft.name.trim() && (draft.actionKind === "agent.run" ? draft.projectId && draft.prompt.trim() : draft.url.trim());
  const save = async () => {
    if (!valid || saving) return;
    setSaving(true);
    const result = hook ? await update(hook.id, toInput(draft)) : await create(toInput(draft));
    setSaving(false);
    if (result) onClose();
  };
  const removeHook = async () => {
    if (!hook) return false;
    const removed = await remove(hook.id);
    if (removed) onClose();
    return removed;
  };
  const runTest = async () => {
    if (hook) await testOutbound(hook.id);
  };
  const projectOptions = projects.map((project) => ({ value: project.projectId, label: project.name, description: project.workdir }));
  const modelOptions = models.map((model) => ({ value: model.requestValue, label: model.displayName }));
  const agentOptions = (projectAgents ?? []).map((agent) => ({ value: agent.name, label: agent.name, description: agent.description }));
  const selectedProject = projects.find((project) => project.projectId === draft.projectId);

  return (
    <>
      <Modal onClose={onClose} width={680} busy={saving} initialFocus={nameRef}>
        <ModalHeader
          title={
            hook ? (
              <span className="flex items-center gap-2">
                {`Edit ${hook.name}`}
                {pluginOwned && <PluginBadge pluginId={hook.pluginId as string} plugins={plugins} />}
              </span>
            ) : (
              "New hook"
            )
          }
          description={
            pluginOwned
              ? "Installed by a plugin — only the target project and branch can be changed here."
              : "Hooks run locally when canonical automation events occur."
          }
        />
        <ModalBody className="flex flex-col gap-5">
          <section className="flex flex-col gap-3">
            <h3 className="text-sm font-semibold">Trigger</h3>
            <FormField label="Name">
              <Input
                ref={nameRef}
                aria-label="Name"
                value={draft.name}
                disabled={pluginOwned}
                onChange={(event) => patchDraft((current) => ({ ...current, name: event.target.value }))}
                placeholder="Notify on session end"
              />
            </FormField>
            <FormField label="When">
              <Combobox
                aria-label="Trigger"
                options={TRIGGER_OPTIONS}
                value={draft.triggerKind}
                disabled={pluginOwned}
                onValueChange={(triggerKind) =>
                  patchDraft((current) => ({
                    ...current,
                    triggerKind: triggerKind as TriggerKind,
                    actionKind: triggerKind === "webhook.inbound" ? "agent.run" : current.actionKind,
                  }))
                }
              />
            </FormField>
          </section>
          <section className="flex flex-col gap-3 border-t border-border pt-4">
            <h3 className="text-sm font-semibold">Action</h3>
            {!isInbound && (
              <FormField label="Action">
                <Combobox
                  aria-label="Action"
                  options={[
                    { value: "agent.run", label: "Run an agent" },
                    { value: "webhook.outbound", label: "Deliver a webhook" },
                  ]}
                  value={draft.actionKind}
                  disabled={pluginOwned}
                  onValueChange={(actionKind) =>
                    patchDraft((current) => ({ ...current, actionKind: actionKind as HookDraft["actionKind"] }))
                  }
                />
              </FormField>
            )}
            {isInbound && <p className="text-xs text-muted-foreground">Inbound webhooks always run an agent.</p>}
          </section>
          {draft.actionKind === "agent.run" ? (
            <section className="flex flex-col gap-3 border-t border-border pt-4">
              <h3 className="text-sm font-semibold">Prompt & target</h3>
              <FormField label="Project">
                <Combobox
                  aria-label="Project"
                  options={projectOptions}
                  value={draft.projectId}
                  onValueChange={(projectId) => patchDraft((current) => ({ ...current, projectId }))}
                  placeholder="Select a project"
                />
              </FormField>
              <FormField
                label="Branch"
                hint={selectedProject?.isGit ? "Leave blank for the project's default branch." : "This project is not git-aware."}
              >
                <Input
                  aria-label="Branch"
                  value={draft.branch}
                  onChange={(event) => patchDraft((current) => ({ ...current, branch: event.target.value }))}
                  placeholder="main"
                />
              </FormField>
              <p className="text-xs text-muted-foreground">Gateway: local</p>
              <div className="grid grid-cols-2 gap-3">
                <FormField label="Agent">
                  <Combobox
                    aria-label="Agent"
                    options={agentOptions}
                    value={draft.agentId ?? ""}
                    disabled={pluginOwned}
                    onValueChange={(agentId) => patchDraft((current) => ({ ...current, agentId: agentId || null }))}
                    placeholder="Default agent"
                  />
                </FormField>
                <FormField label="Model">
                  <Combobox
                    aria-label="Model"
                    options={modelOptions}
                    value={draft.modelOverride ?? ""}
                    disabled={pluginOwned}
                    onValueChange={(modelOverride) => patchDraft((current) => ({ ...current, modelOverride: modelOverride || null }))}
                    placeholder="Default model"
                  />
                </FormField>
              </div>
              <div className="flex items-center justify-between rounded-lg border border-border px-3 py-2">
                <span className="text-xs">Run as subtask</span>
                <Switch
                  on={draft.subtask}
                  disabled={pluginOwned}
                  onToggle={() => patchDraft((current) => ({ ...current, subtask: !current.subtask }))}
                  label="Run as subtask"
                />
              </div>
              <FormField label="Prompt" hint="Use $EVENT for the event payload.">
                <Textarea
                  aria-label="Prompt"
                  value={draft.prompt}
                  disabled={pluginOwned}
                  onChange={(event) => patchDraft((current) => ({ ...current, prompt: event.target.value }))}
                  rows={5}
                  placeholder="Review $EVENT"
                />
              </FormField>
              <InboundEndpoint hook={hook} status={endpointStatus} />
            </section>
          ) : (
            <section className="flex flex-col gap-3 border-t border-border pt-4">
              <h3 className="text-sm font-semibold">Webhook delivery</h3>
              <div className="grid grid-cols-2 gap-3">
                <FormField label="URL">
                  <Input
                    aria-label="URL"
                    value={draft.url}
                    disabled={pluginOwned}
                    onChange={(event) => patchDraft((current) => ({ ...current, url: event.target.value }))}
                    placeholder="https://example.com/hooks"
                  />
                </FormField>
                <FormField label="Method">
                  <Combobox
                    aria-label="Method"
                    options={[{ value: "POST", label: "POST" }]}
                    value={draft.method}
                    disabled={pluginOwned}
                    onValueChange={(method) => patchDraft((current) => ({ ...current, method }))}
                  />
                </FormField>
              </div>
              <div className="flex flex-col gap-2">
                <p className="text-xs font-medium">Headers</p>
                {draft.headers.map((header, index) => (
                  <div key={`${header.name}-${index}`} className="grid grid-cols-[1fr_1fr_auto] gap-2">
                    <Input
                      aria-label={`Header name ${index + 1}`}
                      value={header.name}
                      disabled={pluginOwned}
                      onChange={(event) =>
                        patchDraft((current) => ({
                          ...current,
                          headers: current.headers.map((item, itemIndex) =>
                            itemIndex === index ? { ...item, name: event.target.value } : item,
                          ),
                        }))
                      }
                      placeholder="Authorization"
                    />
                    <Input
                      aria-label={`Header value ${index + 1}`}
                      type="password"
                      value={header.value}
                      disabled={pluginOwned}
                      onChange={(event) =>
                        patchDraft((current) => ({
                          ...current,
                          headers: current.headers.map((item, itemIndex) =>
                            itemIndex === index ? { ...item, value: event.target.value, configured: false } : item,
                          ),
                        }))
                      }
                      placeholder={header.configured ? "Configured (enter to replace)" : "Value"}
                    />
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={`Remove header ${index + 1}`}
                      disabled={pluginOwned}
                      onClick={() =>
                        patchDraft((current) => ({
                          ...current,
                          headers: current.headers.filter((_item, itemIndex) => itemIndex !== index),
                        }))
                      }
                    >
                      <Trash2 aria-hidden size={14} />
                    </Button>
                  </div>
                ))}
                <Button
                  variant="outline"
                  size="sm"
                  disabled={pluginOwned}
                  className="w-fit"
                  onClick={() => patchDraft((current) => ({ ...current, headers: [...current.headers, { name: "", value: "" }] }))}
                >
                  <Plus aria-hidden size={14} /> Add header
                </Button>
                {detail?.action.kind === "webhook.outbound" &&
                  detail.action.config.headers.map(
                    (header) =>
                      header.configured && (
                        <p key={header.name} className="text-xs text-muted-foreground">
                          {header.name} configured
                        </p>
                      ),
                  )}
              </div>
              {/* biome-ignore lint/suspicious/noTemplateCurlyInString: backend placeholder syntax is literal user-facing documentation. */}
              <FormField label="Payload template" hint={'Optional JSON template. Only whole values "${event}" and "${run}" are replaced.'}>
                <Textarea
                  aria-label="Payload template"
                  value={draft.payloadTemplate}
                  disabled={pluginOwned}
                  onChange={(event) => patchDraft((current) => ({ ...current, payloadTemplate: event.target.value }))}
                  rows={5}
                  // biome-ignore lint/suspicious/noTemplateCurlyInString: backend placeholder syntax is intentionally shown literally.
                  placeholder={'{"event":"${event}"}'}
                />
              </FormField>
              {hook && (
                <Button variant="outline" onClick={() => void runTest()}>
                  <Send aria-hidden size={14} /> Test delivery
                </Button>
              )}
            </section>
          )}
          {detail && (
            <section className="flex flex-col gap-2 border-t border-border pt-4">
              <h3 className="text-sm font-semibold">Run history</h3>
              <RunHistory detail={detail} />
            </section>
          )}
        </ModalBody>
        <ModalFooter className="justify-between">
          <span>
            {hook && !pluginOwned && (
              <Button
                variant="ghost"
                className="text-destructive"
                onClick={(event) => {
                  setDeleteTrigger(event.currentTarget);
                  setDeleting(true);
                }}
              >
                <Trash2 aria-hidden size={14} /> Delete
              </Button>
            )}
          </span>
          <span className="flex gap-2">
            <Button variant="outline" onClick={onClose} disabled={saving}>
              Cancel
            </Button>
            <Button onClick={() => void save()} disabled={!valid || saving}>
              {saving ? "Saving…" : "Save hook"}
            </Button>
          </span>
        </ModalFooter>
      </Modal>
      <ConfirmActionModal
        open={deleting}
        title={`Delete ${hook?.name}?`}
        description="This hook and its configuration will be permanently deleted."
        confirmLabel="Delete hook"
        trigger={deleteTrigger}
        onClose={() => setDeleting(false)}
        onConfirm={removeHook}
      />
    </>
  );
}

export function HooksTab({
  projects: providedProjects,
  targetHookId,
}: {
  projects?: Project[];
  /** Deep-linked from the plugin detail Automations tab's "Set up…" button
   *  (Task 14) via `AutomationsView`'s `targetId` — opens that hook's editor
   *  once it's loaded, letting the user fill in the target Task 10's backend
   *  guard is blocking on. */
  targetHookId?: string;
}) {
  const storeProjects = useStore((state) => state.projects);
  const projects = providedProjects ?? storeProjects;
  const hooks = useAutomations((state) => state.hooks);
  const detailsById = useAutomations((state) => state.detailsById);
  const loaded = useAutomations((state) => state.loaded);
  const load = useAutomations((state) => state.load);
  const loadDetail = useAutomations((state) => state.loadDetail);
  const [editing, setEditing] = useState<AutomationHookInfo | null | undefined>(undefined);
  const [duplicateSeed, setDuplicateSeed] = useState<HookDraft | undefined>(undefined);
  const openedTargetRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    void load();
  }, [load]);
  useEffect(() => {
    for (const hook of hooks) void loadDetail(hook.id);
  }, [hooks, loadDetail]);
  useEffect(() => {
    if (!targetHookId || openedTargetRef.current === targetHookId) return;
    const target = hooks.find((hook) => hook.id === targetHookId);
    if (!target) return;
    openedTargetRef.current = targetHookId;
    setDuplicateSeed(undefined);
    setEditing(target);
  }, [targetHookId, hooks]);
  const sortedHooks = useMemo(() => [...hooks].sort((left, right) => right.updatedAt - left.updatedAt), [hooks]);

  const duplicate = (hook: AutomationHookInfo) => {
    const detail = detailsById[hook.id];
    if (!detail) return;
    setDuplicateSeed({ ...draftFor(detail), name: `${detail.hook.name} (copy)` });
    setEditing(null);
  };
  const closeEditor = () => {
    setEditing(undefined);
    setDuplicateSeed(undefined);
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-8 py-7">
      <div className="mx-auto flex max-w-[860px] flex-col gap-5">
        <div className="flex items-start justify-between gap-3">
          <div>
            <h2 className="m-0 mb-1 text-[22px] font-semibold tracking-[-0.02em]">Hooks</h2>
            <p className="m-0 text-[13px] text-muted-foreground">Run local agents or deliver webhooks from automation events.</p>
          </div>
          <Button onClick={() => setEditing(null)}>
            <Plus aria-hidden size={14} /> New hook
          </Button>
        </div>
        {!loaded ? (
          <SettingsCard className="p-6 text-center text-[13px] text-muted-foreground">Loading hooks…</SettingsCard>
        ) : sortedHooks.length === 0 ? (
          <SettingsCard className="p-6 text-center text-[13px] text-muted-foreground">
            No hooks yet. Plugins you install can also add hook presets here.
          </SettingsCard>
        ) : (
          <div className="flex flex-col gap-2">
            {sortedHooks.map((hook) => (
              <HookRow
                key={hook.id}
                hook={hook}
                detail={detailsById[hook.id]}
                onEdit={() => setEditing(hook)}
                onDuplicate={() => duplicate(hook)}
              />
            ))}
          </div>
        )}
      </div>
      {editing !== undefined && (
        <HookEditor
          hook={editing}
          initialDraft={editing === null ? duplicateSeed : undefined}
          projects={projects}
          onClose={closeEditor}
        />
      )}
    </div>
  );
}
