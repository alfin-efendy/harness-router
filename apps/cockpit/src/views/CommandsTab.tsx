import { useEffect, useMemo, useRef, useState } from "react";
import { Edit3, Plus, Search, Trash2 } from "lucide-react";
import { Button, FormField, Input, Modal, ModalBody, ModalFooter, ModalHeader, SettingsCard } from "@ryuzi/ui";
import type { CommandFileInfo, CommandFileInputDto, CommandFileMutationDto, PluginInfo, SlashEntryInfo } from "@/bindings";
import { ConfirmActionModal } from "@/components/modals/ConfirmActionModal";
import { PluginBadge } from "@/components/common/bits";
import { TemplateTextarea } from "@/components/composer/TemplateTextarea";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { catalogKey, useNative, type ProjectCommandMutationResult } from "@/store-native";
import { usePlugins } from "@/store-plugins";
import { useStore } from "@/store";

/** Best-effort plugin id for a `"plugin"`-origin catalog entry. A plugin
 *  command only carries its owner's id IN THE NAME, and only when it lost a
 *  bare-name contest to another plugin — `CommandRegistry::load_from_dirs_with_plugins`
 *  namespaces the loser as `"<plugin-id>/<name>"`; the winner keeps its bare
 *  name with no id recorded anywhere in `SlashEntryInfo`. Returns `null` for
 *  the unresolvable bare-name case, and the row falls back to a generic
 *  "Plugin" badge rather than guessing. */
export function pluginIdForCommand(name: string, plugins: Pick<PluginInfo, "id">[]): string | null {
  const slash = name.indexOf("/");
  if (slash === -1) return null;
  const candidate = name.slice(0, slash);
  return plugins.some((p) => p.id === candidate) ? candidate : null;
}

const NAME_ALLOWED = /^[a-z0-9_-]+(?:\/[a-z0-9_-]+)*$/;

// Falls back to this static list only when the catalog hasn't produced any
// builtin-origin entries yet (unloaded, or genuinely empty) — once the
// catalog is loaded, its builtin names are authoritative (see
// `deriveReservedCommandNames`).
const DEFAULT_RESERVED_NAMES = ["init", "review", "compact"];

/** The names a global command may not be created (or renamed) to: whatever
 *  the loaded "/" catalog reports as builtin-origin, falling back to the
 *  static default list before the catalog has loaded (or if it resolves
 *  empty). */
export function deriveReservedCommandNames(catalog: SlashEntryInfo[] | undefined): Set<string> {
  const builtinNames = (catalog ?? []).filter((entry) => entry.origin === "builtin").map((entry) => entry.name);
  return new Set(builtinNames.length > 0 ? builtinNames : DEFAULT_RESERVED_NAMES);
}

export function globalCommandNameError(name: string, editing: boolean, reservedNames: Set<string>): string | null {
  if (name.length === 0 || name.length > 80) return "Name must contain 1 through 80 characters.";
  if (!NAME_ALLOWED.test(name)) return "Use lowercase letters, digits, '-', '_', and '/' only.";
  if (!editing && reservedNames.has(name)) return "Built-in commands cannot be created or updated.";
  return null;
}

export function globalCommandPreview(name: string, template: string): string {
  const invocation = `/${name || "command"} <arguments>`;
  const body = template
    .replace(/\$ARGUMENTS/g, "<arguments>")
    .replace(/\$([1-9])/g, (_match: string, index: string) => `<argument ${index}>`);
  return `${invocation}\n${body}`;
}

type CommandDraft = { name: string; description: string; template: string };

function blankDraft(): CommandDraft {
  return { name: "", description: "", template: "" };
}

function draftFor(command: CommandFileInfo): CommandDraft {
  const { name, description, template } = command;
  return { name, description, template };
}

function CommandEditor({
  command,
  reservedNames,
  slashEntries,
  projectId,
  onClose,
  onSave,
}: {
  command: CommandFileInfo | null;
  reservedNames: Set<string>;
  slashEntries: SlashEntryInfo[];
  projectId: string | null;
  onClose: () => void;
  onSave: (draft: CommandDraft) => Promise<ProjectCommandMutationResult>;
}) {
  const [draft, setDraft] = useState<CommandDraft>(() => (command ? draftFor(command) : blankDraft()));
  const [saving, setSaving] = useState(false);
  const descriptionRef = useRef<HTMLInputElement>(null);
  const nameError = globalCommandNameError(draft.name, command !== null, reservedNames);
  const valid = !nameError && draft.template.trim().length > 0;
  // Excludes the command being edited from its own "/" suggestions —
  // trivial self-reference isn't useful, though the runtime cycle guard (not
  // this filter) is what actually prevents recursive expansion.
  const editorSlashEntries = useMemo(() => slashEntries.filter((entry) => entry.name !== command?.name), [slashEntries, command]);

  const save = async () => {
    if (!valid || saving) return;
    setSaving(true);
    const result = await onSave({
      name: draft.name.trim(),
      description: draft.description.trim(),
      template: draft.template.trim(),
    });
    setSaving(false);
    if (result.status === "success" || result.status === "conflict") onClose();
  };

  return (
    <Modal onClose={onClose} width={560} busy={saving} initialFocus={command ? descriptionRef : undefined}>
      <ModalHeader
        title={command ? "Edit command" : "New command"}
        description="Global commands are available in every project, once saved."
      />
      <ModalBody className="flex flex-col gap-4">
        <FormField label="Name" hint={nameError ?? "Lowercase path, for example team/review."}>
          <Input
            aria-label="Name"
            value={draft.name}
            disabled={!!command}
            onChange={(event) => setDraft((current) => ({ ...current, name: event.target.value }))}
            placeholder="review"
          />
        </FormField>
        <FormField label="Description" hint="Optional summary shown in the command list.">
          <Input
            ref={descriptionRef}
            aria-label="Description"
            value={draft.description}
            onChange={(event) => setDraft((current) => ({ ...current, description: event.target.value }))}
            placeholder="Review the current change"
          />
        </FormField>
        {/* Not a <FormField>: its wrapping <label> would enclose
            TemplateTextarea's floating suggestion menus too, and a
            plain-text button nested in a <label> gets its accessible name
            derived from the label instead of its own content — breaking
            the menu items' names. A plain <div> with the same layout
            avoids that without changing how it looks. */}
        <div className="flex flex-col gap-1.5">
          <span className="text-xs font-semibold">Template</span>
          <TemplateTextarea
            aria-label="Template"
            value={draft.template}
            onChange={(template) => setDraft((current) => ({ ...current, template }))}
            projectId={projectId}
            slashEntries={editorSlashEntries}
            rows={6}
            placeholder="Review $ARGUMENTS"
          />
          <span className="text-xs text-muted-foreground">Use $ARGUMENTS for all arguments or $1 through $9 for positional arguments.</span>
        </div>
        <div className="rounded-lg border border-border bg-muted/30 px-3 py-2">
          <div className="text-[11px] font-semibold uppercase tracking-[0.04em] text-muted-foreground">Preview</div>
          <pre className="mt-1 whitespace-pre-wrap font-mono text-xs leading-5 text-foreground">
            {globalCommandPreview(draft.name, draft.template)}
          </pre>
        </div>
      </ModalBody>
      <ModalFooter>
        <Button variant="outline" onClick={onClose} disabled={saving}>
          Cancel
        </Button>
        <Button onClick={() => void save()} disabled={!valid || saving}>
          {saving ? "Saving…" : command ? "Save" : "Create"}
        </Button>
      </ModalFooter>
    </Modal>
  );
}

function CommandRow({
  command,
  onEdit,
  onDelete,
}: {
  command: CommandFileInfo;
  onEdit: () => void;
  onDelete: (trigger: HTMLButtonElement) => void;
}) {
  return (
    <SettingsCard className="flex min-h-[88px] items-stretch">
      <div className="min-w-0 flex-1 px-[18px] py-3">
        <div className="flex items-center gap-2">
          <span className="font-mono text-[13px] font-semibold">/{command.name}</span>
          <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">Global</span>
        </div>
        {command.description ? <p className="mt-1 truncate text-xs text-muted-foreground">{command.description}</p> : null}
        <p className="mt-1.5 truncate font-mono text-[11px] text-muted-foreground">{command.template}</p>
      </div>
      <div className="flex shrink-0 items-center gap-1 border-l border-border px-2">
        <Button variant="ghost" size="icon" aria-label={`Edit /${command.name}`} onClick={onEdit}>
          <Edit3 aria-hidden size={15} />
        </Button>
        <Button variant="ghost" size="icon" aria-label={`Delete /${command.name}`} onClick={(event) => onDelete(event.currentTarget)}>
          <Trash2 aria-hidden size={15} />
        </Button>
      </div>
    </SettingsCard>
  );
}

function BuiltinCommandRow({ entry }: { entry: SlashEntryInfo }) {
  return (
    <SettingsCard className="flex min-h-[88px] items-stretch">
      <div className="min-w-0 flex-1 px-[18px] py-3">
        <div className="flex items-center gap-2">
          <span className="font-mono text-[13px] font-semibold">/{entry.name}</span>
          <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">Built-in</span>
        </div>
        {entry.description ? <p className="mt-1 truncate text-xs text-muted-foreground">{entry.description}</p> : null}
      </div>
    </SettingsCard>
  );
}

// Read-only, like `BuiltinCommandRow` — a plugin's `commands/` directory owns
// these files (Task 8), so there's no edit/delete surface here (Task 16).
function PluginCommandRow({ entry, plugins }: { entry: SlashEntryInfo; plugins: PluginInfo[] }) {
  const pluginId = pluginIdForCommand(entry.name, plugins);
  return (
    <SettingsCard className="flex min-h-[88px] items-stretch">
      <div className="min-w-0 flex-1 px-[18px] py-3">
        <div className="flex items-center gap-2">
          <span className="font-mono text-[13px] font-semibold">/{entry.name}</span>
          {pluginId !== null ? (
            <PluginBadge pluginId={pluginId} plugins={plugins} />
          ) : (
            <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">Plugin</span>
          )}
        </div>
        {entry.description ? <p className="mt-1 truncate text-xs text-muted-foreground">{entry.description}</p> : null}
      </div>
    </SettingsCard>
  );
}

export function CommandsTab() {
  // Global commands have no project of their own — `selectedProjectId` is
  // read only as an authoring hint for the template editor's @-mention and
  // file pickers (see `TemplateTextarea`'s `projectId` prop).
  const selectedProjectId = useStore((state) => state.selectedProjectId);
  const [search, setSearch] = useState("");
  const [editing, setEditing] = useState<CommandFileInfo | null | undefined>(undefined);
  const [deleting, setDeleting] = useState<{ command: CommandFileInfo; trigger: HTMLButtonElement } | null>(null);
  const globalCommands = useNative((state) => state.globalCommands);
  const catalog = useNative((state) => state.slashCatalogByKey[catalogKey(null, null)]);
  const plugins = usePlugins((state) => state.plugins);

  useEffect(() => {
    void useNative.getState().loadGlobalCommands(LOCAL_RUNNER);
    void useNative.getState().loadSlashCatalog(LOCAL_RUNNER, null, null);
  }, []);

  const reservedNames = useMemo(() => deriveReservedCommandNames(catalog), [catalog]);
  const builtinEntries = useMemo(() => (catalog ?? []).filter((entry) => entry.origin === "builtin"), [catalog]);
  const pluginEntries = useMemo(() => (catalog ?? []).filter((entry) => entry.origin === "plugin"), [catalog]);

  const filteredCommands = useMemo(() => {
    const term = search.trim().toLowerCase();
    if (!term) return globalCommands ?? [];
    return (globalCommands ?? []).filter((command) =>
      [command.name, command.description, command.template].some((value) => value.toLowerCase().includes(term)),
    );
  }, [globalCommands, search]);

  const save = async (draft: CommandDraft): Promise<ProjectCommandMutationResult> => {
    if (editing) {
      const input: CommandFileMutationDto = {
        description: draft.description,
        template: draft.template,
        agent: editing.agent,
        model: editing.model,
        subtask: editing.subtask,
      };
      return useNative.getState().updateGlobalCommand(LOCAL_RUNNER, editing, input);
    }
    const input: CommandFileInputDto = {
      name: draft.name,
      description: draft.description,
      template: draft.template,
      agent: null,
      model: null,
      subtask: false,
    };
    return useNative.getState().createGlobalCommand(LOCAL_RUNNER, input);
  };

  const confirmDelete = async (): Promise<boolean> => {
    if (!deleting) return false;
    const result = await useNative.getState().deleteGlobalCommand(LOCAL_RUNNER, deleting.command);
    if (result.status === "success" || result.status === "conflict") {
      setDeleting(null);
      return true;
    }
    return false;
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-8 py-7">
      <div className="mx-auto flex max-w-[860px] flex-col gap-5">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <h2 className="text-[15px] font-semibold">Commands</h2>
          <Button onClick={() => setEditing(null)}>
            <Plus aria-hidden size={15} /> New command
          </Button>
        </div>

        <SettingsCard className="px-[18px] py-3 text-xs text-muted-foreground">
          Global commands are available in every project. Built-in commands are read-only.
        </SettingsCard>

        <div className="relative">
          <Search aria-hidden size={15} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
          <Input
            aria-label="Search commands"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Search commands"
            className="pl-9"
          />
        </div>
        <div className="flex flex-col gap-2">
          {globalCommands === undefined ? (
            <SettingsCard className="p-6 text-center text-[13px] text-muted-foreground">Loading global commands…</SettingsCard>
          ) : filteredCommands.length > 0 ? (
            filteredCommands.map((command) => (
              <CommandRow
                key={command.name}
                command={command}
                onEdit={() => setEditing(command)}
                onDelete={(trigger) => setDeleting({ command, trigger })}
              />
            ))
          ) : (
            <SettingsCard className="p-6 text-center text-[13px] text-muted-foreground">
              {search ? "No global commands match your search." : "No global commands yet."}
            </SettingsCard>
          )}
        </div>
        <div className="flex flex-col gap-2">
          <div className="text-xs font-semibold text-muted-foreground">Built-in commands</div>
          {catalog === undefined ? (
            <SettingsCard className="p-6 text-center text-[13px] text-muted-foreground">Loading built-in commands…</SettingsCard>
          ) : builtinEntries.length > 0 ? (
            builtinEntries.map((entry) => <BuiltinCommandRow key={entry.name} entry={entry} />)
          ) : (
            <SettingsCard className="p-6 text-center text-[13px] text-muted-foreground">No built-in commands.</SettingsCard>
          )}
        </div>
        {pluginEntries.length > 0 && (
          <div className="flex flex-col gap-2">
            <div className="text-xs font-semibold text-muted-foreground">Plugin commands</div>
            {pluginEntries.map((entry) => (
              <PluginCommandRow key={entry.name} entry={entry} plugins={plugins} />
            ))}
          </div>
        )}
      </div>
      {editing !== undefined && (
        <CommandEditor
          command={editing}
          reservedNames={reservedNames}
          slashEntries={catalog ?? []}
          projectId={selectedProjectId}
          onClose={() => setEditing(undefined)}
          onSave={save}
        />
      )}
      <ConfirmActionModal
        open={deleting !== null}
        title={deleting ? `Delete /${deleting.command.name}?` : "Delete command?"}
        description="This global command will be permanently deleted."
        confirmLabel="Delete command"
        busyLabel="Deleting…"
        trigger={deleting?.trigger ?? null}
        onClose={() => setDeleting(null)}
        onConfirm={confirmDelete}
      />
    </div>
  );
}
