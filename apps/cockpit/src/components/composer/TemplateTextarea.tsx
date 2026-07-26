import { useEffect, useMemo, useRef, useState } from "react";
import { MenuPanel, MenuPanelItem, MenuPanelSection, Textarea } from "@ryuzi/ui";
import { commands, type AgentInfo, type SearchEntryInfo, type SlashEntryInfo } from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { useNative } from "@/store-native";
import { activeAgentMentionQuery, matchMentionAgents } from "@/lib/mentions";
import { activeSlashQuery } from "@/lib/slash-autocomplete";
import { SlashCommandMenu } from "./SlashCommandMenu";
import {
  activeContextQuery,
  contextPickerGroups,
  flattenContextPickerGroups,
  replaceActiveContextToken,
  type ContextPickerItem,
} from "@/lib/composer-context";
import { ContextPickerMenu } from "./ContextPickerMenu";

export type TemplateTextareaProps = {
  value: string;
  onChange: (next: string) => void;
  /** Authoring-hint context; null → @/file pickers stay silent. */
  projectId: string | null;
  slashEntries: SlashEntryInfo[];
  rows?: number;
  "aria-label"?: string;
  placeholder?: string;
};

const SEARCH_DEBOUNCE_MS = 120;

type LineSlashQuery = { start: number; end: number; query: string };

/** Adapts `activeSlashQuery` (built for single-line composer drafts) to a
 *  multi-line template: the query is scoped to the CURRENT line (the run of
 *  text from the last newline up to the caret), so a "/" mid-template only
 *  triggers the menu when it starts that line. */
function activeLineSlashQuery(value: string, caret: number): LineSlashQuery | null {
  const lineStart = value.lastIndexOf("\n", caret - 1) + 1;
  const linePrefix = value.slice(lineStart, caret);
  const query = activeSlashQuery(linePrefix);
  if (query === null) return null;
  const tokenStart = lineStart + (linePrefix.length - linePrefix.trimStart().length);
  return { start: tokenStart, end: caret, query };
}

type MentionCandidate = { id: string; name: string; description: string; executable: true; builtin: boolean };

function toMentionCandidates(agents: AgentInfo[]): MentionCandidate[] {
  return agents.map((agent) => ({
    id: agent.name,
    name: agent.name,
    description: agent.description,
    executable: true,
    builtin: agent.builtin,
  }));
}

/** A `/` at the start of the current line suggests catalog commands; a bare
 *  `@token` suggests agents; an `@token` that looks like a path (contains
 *  "/" or ".") suggests workspace files instead — mirroring the same
 *  discriminator HomeView uses to keep its mention and file pickers from
 *  colliding on the same trigger character. Exactly one popup is open at a
 *  time. Escape dismisses whichever is open (tracked by a token-position
 *  signature, so retyping the same trigger reopens it); Enter/Tab accepts
 *  the first match. */
export function TemplateTextarea({
  value,
  onChange,
  projectId,
  slashEntries,
  rows = 6,
  "aria-label": ariaLabel,
  placeholder,
}: TemplateTextareaProps) {
  const [caret, setCaret] = useState(0);
  const [dismissedSignature, setDismissedSignature] = useState<string | null>(null);
  const [fileEntries, setFileEntries] = useState<SearchEntryInfo[]>([]);
  const projectAgents = useNative((state) => (projectId ? state.agentsByProject[projectId] : undefined));
  const searchSerial = useRef(0);

  // Nothing else loads `agentsByProject` for a hint project that hasn't been
  // visited elsewhere (e.g. no session opened in it yet) — without this, the
  // "@" agent popup would stay permanently empty for such a project.
  useEffect(() => {
    if (projectId) void useNative.getState().loadAgents(LOCAL_RUNNER, projectId);
  }, [projectId]);

  const trackCaret = (target: HTMLTextAreaElement) => setCaret(target.selectionStart ?? target.value.length);

  const lineSlash = useMemo(() => activeLineSlashQuery(value, caret), [value, caret]);
  const slashMatches = useMemo(
    () =>
      lineSlash === null
        ? []
        : slashEntries.filter((entry) => entry.effective && entry.name.toLowerCase().startsWith(lineSlash.query)).slice(0, 6),
    [slashEntries, lineSlash],
  );

  const mentionQuery = useMemo(() => activeAgentMentionQuery(value, caret), [value, caret]);
  const rawContextQuery = useMemo(() => activeContextQuery(value, caret), [value, caret]);
  const pathQuery = useMemo(() => {
    if (!rawContextQuery || rawContextQuery.query.length === 0) return null;
    return rawContextQuery.query.includes("/") || rawContextQuery.query.includes(".") ? rawContextQuery : null;
  }, [rawContextQuery]);
  const agentMatches = useMemo(() => {
    if (pathQuery !== null || mentionQuery === null) return [];
    return matchMentionAgents(toMentionCandidates(projectAgents ?? []), mentionQuery.query, null, []);
  }, [pathQuery, mentionQuery, projectAgents]);

  // Debounced workspace search for the path-like "@" query — skipped
  // entirely without a project hint, per the "silent without a project"
  // contract.
  useEffect(() => {
    if (!projectId || pathQuery === null) {
      setFileEntries([]);
      return;
    }
    const serial = ++searchSerial.current;
    const query = pathQuery.query;
    const timer = setTimeout(() => {
      void commands.searchFiles(LOCAL_RUNNER, projectId, query).then((res) => {
        if (searchSerial.current !== serial) return;
        setFileEntries(res.status === "ok" ? res.data : []);
      });
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [projectId, pathQuery]);

  const fileGroups = useMemo(
    () =>
      pathQuery === null
        ? []
        : contextPickerGroups({ query: pathQuery.query, project: null, agents: [], primaryAgentId: null, entries: fileEntries }),
    [pathQuery, fileEntries],
  );
  const fileItems = useMemo(() => flattenContextPickerGroups(fileGroups), [fileGroups]);

  const signature =
    lineSlash !== null
      ? `slash:${lineSlash.start}`
      : pathQuery !== null
        ? `path:${pathQuery.start}`
        : mentionQuery !== null
          ? `mention:${mentionQuery.start}`
          : null;

  const slashOpen = lineSlash !== null && slashMatches.length > 0 && signature !== dismissedSignature;
  const mentionOpen = pathQuery === null && mentionQuery !== null && agentMatches.length > 0 && signature !== dismissedSignature;
  const fileOpen = pathQuery !== null && fileItems.length > 0 && signature !== dismissedSignature;

  const dismiss = () => setDismissedSignature(signature);

  const pickSlash = (entry: SlashEntryInfo) => {
    if (lineSlash === null) return;
    const inserted = `/${entry.name} `;
    onChange(`${value.slice(0, lineSlash.start)}${inserted}${value.slice(lineSlash.end)}`);
    setCaret(lineSlash.start + inserted.length);
    setDismissedSignature(null);
  };

  const pickAgent = (agent: MentionCandidate) => {
    if (mentionQuery === null) return;
    const inserted = `@${agent.name} `;
    onChange(`${value.slice(0, mentionQuery.start)}${inserted}${value.slice(mentionQuery.end)}`);
    setCaret(mentionQuery.start + inserted.length);
    setDismissedSignature(null);
  };

  const pickFile = (item: ContextPickerItem) => {
    if (item.kind !== "workspace" || pathQuery === null) return;
    onChange(replaceActiveContextToken(value, caret, item.path));
    setCaret(pathQuery.start + item.path.length + 2);
    setDismissedSignature(null);
  };

  return (
    <div className="relative">
      <Textarea
        aria-label={ariaLabel}
        value={value}
        placeholder={placeholder}
        rows={rows}
        onChange={(event) => {
          onChange(event.target.value);
          trackCaret(event.target);
        }}
        onKeyUp={(event) => trackCaret(event.currentTarget)}
        onClick={(event) => trackCaret(event.currentTarget)}
        onKeyDown={(event) => {
          const open = slashOpen || mentionOpen || fileOpen;
          if (!open) return;
          if (event.key === "Escape") {
            event.preventDefault();
            // Contain it here — otherwise it bubbles to the enclosing
            // Dialog (CommandsTab's editor Modal), which treats Escape as
            // "close the whole modal", discarding the draft.
            event.stopPropagation();
            dismiss();
            return;
          }
          if (event.key !== "Enter" && event.key !== "Tab") return;
          event.preventDefault();
          if (slashOpen) {
            const first = slashMatches[0];
            if (first) pickSlash(first);
          } else if (mentionOpen) {
            const first = agentMatches[0];
            if (first) pickAgent(first);
          } else if (fileOpen) {
            const first = fileItems[0];
            if (first) pickFile(first);
          }
        }}
      />
      {slashOpen && <SlashCommandMenu entries={slashMatches} onPick={pickSlash} />}
      {mentionOpen && (
        <MenuPanel onClose={dismiss} className="bottom-full left-2.5 z-50 mb-1.5 w-[280px]">
          <MenuPanelSection>Agents</MenuPanelSection>
          {agentMatches.map((agent) => (
            <MenuPanelItem key={agent.id} onClick={() => pickAgent(agent)} className="font-medium">
              <span className="min-w-0 flex-1 truncate">{agent.name}</span>
              <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">{agent.description}</span>
            </MenuPanelItem>
          ))}
        </MenuPanel>
      )}
      {fileOpen && <ContextPickerMenu groups={fileGroups} activeIndex={0} onPick={pickFile} onClose={dismiss} />}
    </div>
  );
}
