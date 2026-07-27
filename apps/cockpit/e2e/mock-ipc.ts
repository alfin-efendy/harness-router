import type { Page } from "@playwright/test";
import type {
  AgentConfigurationCatalogInfo,
  AgentDetailInfo,
  AgentModelInfo,
  AgentRegistryInfo,
  AgentRun,
  AgentRunRosterInfo,
  AgentStatsInfo,
  AgentStatsLite,
  AgentSummaryInfo,
  CommandFileInfo,
  CommandFileInputDto,
  CommandFileMutationDto,
  ComponentReleaseDetail,
  ConnectionInfo,
  CoreEvent,
  Message,
  ModelRouteTargetCapability,
  PetManifestEntryInfo,
  PluginDetail,
  PluginInfo,
  Session,
  SlashEntryInfo,
} from "../src/bindings";

/**
 * Fixtures mirror the generated types in src/bindings.ts (Project, Session,
 * ConnectionInfo).
 * Keep field names in sync when bindings regenerate.
 */
export const PROJECT = {
  projectId: "p-demo",
  name: "demo",
  workdir: "/tmp/demo",
  source: null,
  model: null,
  effort: null,
  permMode: "default",
  createdAt: 0,
  isGit: false,
};

const effort = (value: string, label: string) => ({ value, label, description: `${label} fixture effort` });

export const SELECTABLE_MODELS = [
  {
    kind: "concrete",
    requestValue: "fixture/model-alpha",
    displayName: "Model Alpha",
    preferenceKey: { family: "fixture", model: "model-alpha" },
    supported: [effort("light", "Light"), effort("medium", "Medium"), effort("high", "High")],
    configuredDefault: null,
    resolvedDefault: "medium",
    defaultSource: "provider",
  },
  {
    kind: "concrete",
    requestValue: "fixture/model-beta",
    displayName: "Model Beta",
    preferenceKey: { family: "fixture", model: "model-beta" },
    supported: [effort("high", "High"), effort("extra-high", "Extra high"), effort("ultra", "Ultra")],
    configuredDefault: null,
    resolvedDefault: "extra-high",
    defaultSource: "provider",
  },
  {
    kind: "namedRoute",
    requestValue: "route:safe",
    displayName: "Named safe route",
    preferenceKey: null,
    supported: [effort("high", "High")],
    configuredDefault: null,
    resolvedDefault: "high",
    defaultSource: "variesByTarget",
  },
];

export const NATIVE_RUNTIME = {
  id: "native",
  name: "Ryuzi",
  color: "#8B5CF6",
  initial: "R",
  connection: "In-process",
  binaryPath: "in-process",
  installedVersion: "0.0.0-fixture",
  latestVersion: null,
  npmPackage: null,
  models: SELECTABLE_MODELS.map((model) => model.requestValue),
  selectableModels: SELECTABLE_MODELS,
  enabled: true,
  model: "",
  permMode: "ask",
  flags: "",
  tiers: [],
  isDefault: true,
  runnable: true,
};

const PROVIDER_CATALOG = [
  {
    id: "fixture",
    name: "Fixture Provider",
    family: "fixture",
    color: "#6366F1",
    initial: "F",
    category: "api_key",
    format: "openai",
    requiresBaseUrl: false,
    models: ["model-alpha", "model-beta"],
    freeTier: false,
    riskNotice: false,
    usesDeviceGrant: false,
  },
];

const CONNECTIONS = [
  {
    id: "fixture-account",
    provider: "fixture",
    providerName: "Fixture Provider",
    color: "#6366F1",
    initial: "F",
    authType: "apiKey",
    label: "Fixture account",
    priority: 0,
    enabled: true,
    quotaCapability: null,
    models: ["model-alpha", "model-beta"],
    needsRelogin: false,
    builtin: false,
  },
] satisfies ConnectionInfo[];

export const ACCOUNT_CATALOG = [
  {
    id: "anthropic-oauth",
    name: "Claude Code",
    family: "anthropic",
    color: "#D97757",
    initial: "C",
    category: "oauth",
    format: "anthropic",
    requiresBaseUrl: false,
    models: ["claude-sonnet-4"],
    freeTier: false,
    riskNotice: false,
    usesDeviceGrant: false,
  },
  {
    id: "openai-oauth",
    name: "Codex",
    family: "openai",
    color: "#10A37F",
    initial: "O",
    category: "oauth",
    format: "openai",
    requiresBaseUrl: false,
    models: ["gpt-5.5"],
    freeTier: false,
    riskNotice: false,
    usesDeviceGrant: false,
  },
  {
    id: "kiro",
    name: "Kiro",
    family: "kiro",
    color: "#7C3AED",
    initial: "K",
    category: "device",
    format: "openai",
    requiresBaseUrl: false,
    models: ["kiro-auto"],
    freeTier: true,
    riskNotice: false,
    usesDeviceGrant: false,
  },
];

export const ACCOUNT_CONNECTIONS = [
  {
    id: "claude-personal",
    provider: "anthropic-oauth",
    providerName: "Claude Code",
    color: "#D97757",
    initial: "C",
    authType: "oauth",
    label: "Claude Personal",
    priority: 0,
    enabled: true,
    quotaCapability: "claude",
    models: ["claude-sonnet-4"],
    needsRelogin: true,
    builtin: false,
  },
  {
    id: "codex-primary",
    provider: "openai-oauth",
    providerName: "Codex",
    color: "#10A37F",
    initial: "O",
    authType: "oauth",
    label: "Codex Primary",
    priority: 0,
    enabled: true,
    quotaCapability: "codex",
    models: ["gpt-5.5"],
    needsRelogin: false,
    builtin: false,
  },
  {
    id: "codex-backup",
    provider: "openai-oauth",
    providerName: "Codex",
    color: "#10A37F",
    initial: "O",
    authType: "oauth",
    label: "Codex Backup",
    priority: 1,
    enabled: true,
    quotaCapability: "codex",
    models: ["gpt-5.5"],
    needsRelogin: false,
    builtin: false,
  },
  {
    id: "kiro-device",
    provider: "kiro",
    providerName: "Kiro",
    color: "#7C3AED",
    initial: "K",
    authType: "oauth",
    label: "Kiro Device",
    priority: 0,
    enabled: true,
    quotaCapability: null,
    models: ["kiro-auto"],
    needsRelogin: true,
    builtin: false,
  },
] satisfies ConnectionInfo[];

const initialProjectRuntime = {
  projectId: PROJECT.projectId,
  model: null,
  storedEffort: null,
  effectiveEffort: null,
  effectiveEffortLabel: null,
  effectiveSource: "none",
  storedEffortStatus: "valid",
  modelInfo: null,
};

export const SESSION = {
  sessionPk: "s-1",
  primaryAgentId: "ryuzi",
  primaryAgentSnapshot: { id: "ryuzi", name: "Ryuzi", avatarColor: "#7C3AED" },
  projectId: "p-demo",
  agentSessionId: null,
  worktreePath: null,
  branch: "main",
  title: null,
  status: "running",
  startedBy: null,
  createdAt: 0,
  lastActive: 0,
  resumeAttempts: 0,
  branchOwned: false,
  permMode: "default",
  kind: "project",
  speaker: null,
  agent: null,
  parentSessionPk: null,
};

/** A project-less chat session (kind "chat"), returned by start_chat_session. */
export const CHAT_SESSION = {
  ...SESSION,
  sessionPk: "c-1",
  projectId: null,
  branch: null,
  kind: "chat",
};

export const PROVIDER_FAMILY_ROUTE_SELECTIONS = [
  {
    requestedModel: "route:primary",
    resolvedProviderId: "fixture-provider-a",
    resolvedFamily: "fixture-family-a",
    resolvedModel: "shared-model",
    effectiveEffort: "high",
    connectionId: "fixture-account",
    resolvedModelDisplayName: "Shared Model",
    effectiveEffortLabel: "High",
    connectionLabel: "Fixture account",
    reason: "initial",
  },
  {
    requestedModel: "route:provider-family-change",
    resolvedProviderId: "fixture-provider-b",
    resolvedFamily: "fixture-family-b",
    resolvedModel: "shared-model",
    effectiveEffort: "high",
    connectionId: "fixture-account",
    resolvedModelDisplayName: "Shared Model",
    effectiveEffortLabel: "High",
    connectionLabel: "Fixture account",
    reason: "roundRobin",
  },
  {
    requestedModel: "route:mutable-alias-only",
    resolvedProviderId: "fixture-provider-b",
    resolvedFamily: "fixture-family-b",
    resolvedModel: "shared-model",
    effectiveEffort: "high",
    connectionId: "fixture-account",
    resolvedModelDisplayName: "Renamed Shared Model",
    effectiveEffortLabel: "High (renamed)",
    connectionLabel: "Renamed account label",
    reason: "quotaUnavailable",
  },
];

/** Two main agents for the agent-management, delegation, and read-only
 * history journeys: Ryuzi (the executable default) and Reviewer (a second
 * executable agent used as a delegation target and, in the history journey,
 * a session owner deliberately absent from a narrower registry override). */
// avatarPet fixtures: Ryuzi carries "cloudlet" — mirroring the backend's
// DEFAULT_RYUZI_PET seed/backfill (crates/core/src/agents/bootstrap.rs) —
// so pet-aware surfaces (roster row, detail header, PetSprite) render the
// same avatar a real install shows. Reviewer stays pet-less (`null`) so the
// neutral fallback tile is exercised too. "cloudlet" is a real bundled slug
// (`public/pets/cloudlet/`), distinct from the Fresh Agent's own pet below.
export const RYUZI_AGENT = {
  id: "ryuzi",
  name: "Ryuzi",
  description: "General-purpose coding agent",
  avatarColor: "violet",
  avatarPet: "cloudlet",
  model: { kind: "concrete", name: "fixture/model-alpha", effort: "high" },
  builtin: false,
  skillCount: 1,
  toolCount: 4,
  knowledgeCount: 1,
  executable: true,
  validation: [],
  isDefault: true,
} satisfies AgentSummaryInfo;

export const REVIEWER_AGENT = {
  id: "reviewer",
  name: "Reviewer",
  description: "Reviews implementation quality and regressions",
  avatarColor: "amber",
  avatarPet: null,
  model: { kind: "route", route: "safe" },
  builtin: false,
  skillCount: 1,
  toolCount: 4,
  knowledgeCount: 1,
  executable: true,
  validation: [],
  isDefault: false,
} satisfies AgentSummaryInfo;

/** Backing store for both the Fresh Agent row's `model` and the registry's
 * `subagentModel` — the real backend mirrors the registry-wide subagent
 * model into `fresh_agent_summary`'s `model` field (see `agent_api.rs`), so
 * FRESH_AGENT.model derives from this single source instead of repeating
 * the literal (Task 9 review: was a duplicate literal that could drift). */
const SUBAGENT_MODEL: AgentModelInfo = { kind: "route", route: "smart" };

/** The synthetic, non-editable Fresh Agent row — mirrors the backend's
 * `fresh_agent_summary`/`fresh_agent_detail` (crates/core/src/api/agent_api.rs):
 * always appended LAST to the registry, model-only detail, never mutable.
 * `avatarPet: "sprout"` MUST stay in sync with the backend's
 * `FRESH_AGENT_PET` const (agent_api.rs) and the frontend's own
 * `FRESH_AGENT_PET` const (lib/pet-sprite.ts) — the Fresh Agent's pet is
 * hardcoded, never user-editable. */
export const FRESH_AGENT = {
  id: "fresh",
  name: "Fresh Agent",
  description: "Ephemeral, memoryless worker dispatched for delegated tasks.",
  avatarColor: "slate",
  avatarPet: "sprout",
  model: SUBAGENT_MODEL,
  builtin: true,
  skillCount: 0,
  toolCount: 0,
  knowledgeCount: 0,
  executable: true,
  validation: [],
  isDefault: false,
} satisfies AgentSummaryInfo;

const AGENT_REGISTRY = {
  agents: [RYUZI_AGENT, REVIEWER_AGENT, FRESH_AGENT],
  defaultAgentId: RYUZI_AGENT.id,
  recovery: [],
  subagentModel: SUBAGENT_MODEL,
} satisfies AgentRegistryInfo;

/** Same registry with Reviewer removed — the deleted-owner history journey
 * needs a session whose captured `primaryAgentId` no longer resolves against
 * the live roster (session-primary.ts's "deleted" branch). */
export const REGISTRY_WITHOUT_REVIEWER = {
  ...AGENT_REGISTRY,
  agents: [RYUZI_AGENT, FRESH_AGENT],
} satisfies AgentRegistryInfo;

export const RYUZI_DETAIL = {
  summary: RYUZI_AGENT,
  permissionRules: [],
  skills: ["general-coding"],
  nativeTools: [
    { tool: "read_file", decision: "allow" },
    { tool: "grep", decision: "allow" },
  ],
  pluginTools: [],
  apps: [],
  modelInfo: null,
  personality: { preset: "helpful", custom: null },
} satisfies AgentDetailInfo;

export const REVIEWER_DETAIL = {
  summary: REVIEWER_AGENT,
  permissionRules: [],
  skills: ["code-review"],
  nativeTools: [
    { tool: "read_file", decision: "allow" },
    { tool: "grep", decision: "allow" },
  ],
  pluginTools: [],
  apps: [],
  modelInfo: null,
  personality: { preset: "helpful", custom: null },
} satisfies AgentDetailInfo;

/** `get_agent("fresh")`'s detail: everything but the summary/model is empty
 * so the frontend renders a model-only view — mirrors the backend's
 * `fresh_agent_detail`. */
export const FRESH_AGENT_DETAIL = {
  summary: FRESH_AGENT,
  permissionRules: [],
  skills: [],
  nativeTools: [],
  pluginTools: [],
  apps: [],
  modelInfo: null,
  personality: { preset: "helpful", custom: null },
} satisfies AgentDetailInfo;

/** `get_agent` is dispatched dynamically by `agentId` (see installMockIPC's
 * `get_agent` branch below) — unlike the fixed-shape commands in FIXTURES,
 * this bag is looked up by id at call time. */
const AGENT_DETAILS: Record<string, AgentDetailInfo> = {
  ryuzi: RYUZI_DETAIL,
  reviewer: REVIEWER_DETAIL,
  fresh: FRESH_AGENT_DETAIL,
};

// ---------- Per-agent stats fixtures (PR3 Task 8) ----------
//
// `get_agent_stats`/`get_agent_stats_batch` back the detail view's Overview
// stat cards and the roster row's lazy stats fragment respectively (see
// AgentDetailView.tsx / AgentsView.tsx / lib/agent-stats.ts). Ryuzi carries
// real, non-zero figures across every field (sessions/cost/tokens/
// reliability/top-tools) so both surfaces have something concrete to render;
// Reviewer is deliberately all-zero to exercise the "no data yet" branch
// (`reliabilitySummary`'s em-dash fallback, `formatLastActive`'s "—") without
// a separate never-loaded agent. `lastActive` is computed relative to fixture
// module load, not hardcoded, so `formatRelativeTime`'s bucket ("Nd ago")
// stays stable for the lifetime of a single e2e run.

/** `get_agent_stats(agentId)`'s full detail-view shape for Ryuzi. */
export const RYUZI_STATS = {
  sessionCount: 5,
  lastActive: Date.now() - 3 * 24 * 60 * 60 * 1000,
  costUsd7d: 1.24,
  tokens7d: 45_231,
  runsTotal30d: 20,
  runsFailed30d: 2,
  topTools: [
    { tool: "read_file", count: 14, lastUsed: Date.now() - 3 * 24 * 60 * 60 * 1000 },
    { tool: "grep", count: 6, lastUsed: Date.now() - 4 * 24 * 60 * 60 * 1000 },
  ],
} satisfies AgentStatsInfo;

/** All-zero — Reviewer has no recorded sessions/runs in this fixture set. */
export const REVIEWER_STATS = {
  sessionCount: 0,
  lastActive: null,
  costUsd7d: 0,
  tokens7d: 0,
  runsTotal30d: 0,
  runsFailed30d: 0,
  topTools: [],
} satisfies AgentStatsInfo;

/** Internal lookup bag for the dynamically-dispatched `get_agent_stats`
 * command (see its branch in installMockIPC) — not a real Tauri command
 * name, same pattern as `AGENT_DETAILS` above. An id with no entry here
 * (including the Fresh Agent's "fresh") falls back to an inline all-zero
 * `AgentStatsInfo` in the dispatch branch itself, mirroring the real
 * backend's "unknown or synthetic agent id ... zeroes out" contract. */
const AGENT_STATS: Record<string, AgentStatsInfo> = {
  ryuzi: RYUZI_STATS,
  reviewer: REVIEWER_STATS,
};

/** Internal lookup bag for the dynamically-dispatched `get_agent_stats_batch`
 * command — the lite roster-row shape, sourced from the same figures as
 * `AGENT_STATS` above so the list row's stats fragment and the detail view's
 * Overview cards never disagree about the same agent. */
const AGENT_STATS_LITE: Record<string, AgentStatsLite> = {
  ryuzi: { sessionCount: RYUZI_STATS.sessionCount, lastActive: RYUZI_STATS.lastActive, costUsd7d: RYUZI_STATS.costUsd7d },
  reviewer: { sessionCount: REVIEWER_STATS.sessionCount, lastActive: REVIEWER_STATS.lastActive, costUsd7d: REVIEWER_STATS.costUsd7d },
};

// ---------- Pet manifest fixtures (PR3 Task 8) ----------
//
// `list_pet_manifest` — a tiny STATIC stand-in for petdex.dev's real
// `https://petdex.dev/api/manifest` response; no live network call happens
// in e2e. One entry ("sprout") intentionally reuses a bundled slug so the
// picker's "already-bundled entries are excluded from browse results" logic
// has a real case, though exercising the picker's Browse flow itself is an
// owed manual Tauri smoke, not part of this suite. The other two are
// petdex-only (never downloaded in these specs) with dummy `https://` sheet
// URLs that are never fetched.
export const PET_MANIFEST = [
  { slug: "sprout", displayName: "Sprout", kind: "plant", submittedBy: "Chen W.", spritesheetUrl: "/pets/sprout/sprite.webp" },
  {
    slug: "nebula-fox",
    displayName: "Nebula Fox",
    kind: "fox",
    submittedBy: "petdex-community",
    spritesheetUrl: "https://petdex.dev/pets/nebula-fox/sprite.webp",
  },
  {
    slug: "pixel-koi",
    displayName: "Pixel Koi",
    kind: "fish",
    submittedBy: null,
    spritesheetUrl: "https://petdex.dev/pets/pixel-koi/sprite.webp",
  },
] satisfies PetManifestEntryInfo[];

/** `get_agent_configuration_catalog` — backs the detail page's Skills/
 * Permissions/Apps & MCP tabs (`useAgentConfigurationCatalog`). Native-tool
 * ids line up with `RYUZI_DETAIL`/`REVIEWER_DETAIL.nativeTools` above
 * (`read_file`, `grep`, both `allow`) so the Permissions tab's rows render
 * with a real, non-default decision; `bash` adds a `commandScoped` row for
 * completeness even though no fixture agent has an explicit decision for it
 * (renders "Ask", the documented absent-decision default). Skill ids line up
 * with `RYUZI_DETAIL`/`REVIEWER_DETAIL.skills`. */
export const AGENT_CONFIGURATION_CATALOG = {
  skills: [
    {
      id: "general-coding",
      label: "General coding",
      description: "General-purpose coding guidance",
      available: true,
      commandScoped: false,
      pack: null,
      kind: null,
    },
    {
      id: "code-review",
      label: "Code review",
      description: "Reviews implementation quality and regressions",
      available: true,
      commandScoped: false,
      pack: null,
      kind: null,
    },
  ],
  nativeTools: [
    {
      id: "read_file",
      label: "Read file",
      description: "Read files from disk",
      available: true,
      commandScoped: false,
      pack: null,
      kind: null,
    },
    { id: "grep", label: "Grep", description: "Search file contents", available: true, commandScoped: false, pack: null, kind: null },
    { id: "bash", label: "Bash", description: "Run shell commands", available: true, commandScoped: true, pack: null, kind: null },
  ],
  // Hidden kinds only (provider/runtime): the Apps & MCP tab must not list
  // these, and nothing in the fixtures enables them — so the whole flat
  // Plugins section stays absent and the tab shows its empty state.
  pluginTools: [
    {
      id: "native",
      label: "Ryuzi",
      description: "Built-in agent runtime",
      available: true,
      commandScoped: false,
      pack: null,
      kind: "runtime",
    },
    {
      id: "anthropic",
      label: "Anthropic",
      description: "Model provider",
      available: true,
      commandScoped: false,
      pack: null,
      kind: "provider",
    },
  ],
  apps: [],
} satisfies AgentConfigurationCatalogInfo;

const EMPTY_AGENT_RUN_ROSTER: AgentRunRosterInfo = { rootRunId: null, runs: [] };
type ChildRunMockState = {
  agentRunRoster: AgentRunRosterInfo;
  childMessages: Record<string, Message[]>;
  retryChildMessages: Record<string, Message[]>;
};
export type MockIPCOverrides = Record<string, unknown> & Partial<ChildRunMockState>;
/** One active main-delegate run (Ryuzi → Reviewer) and one completed subagent
 * run, returned by `get_child_runs` for the delegation/child-transcript
 * journey. Subagents are ephemeral runtime workers with no agent profile
 * (AgentsView's SubagentSettings: "Subagents do not have profiles"), so
 * `executingAgentId` stays null for the subagent row. */
export const DELEGATE_ACTIVE_RUN = {
  runId: "run-active-1",
  sessionPk: CHAT_SESSION.sessionPk,
  parentRunId: null,
  retryOf: null,
  sourceToolCallId: null,
  dispatchIndex: null,
  primaryAgentId: "ryuzi",
  executingAgentId: "reviewer",
  executingAgentNameSnapshot: "Reviewer",
  agentKind: "main-delegate",
  task: "Review the diff for regressions",
  status: "running",
  startedAt: 0,
  finishedAt: null,
  toolCount: 2,
  resolvedModel: "fixture/model-alpha",
  resolvedEffort: "high",
  result: null,
  error: null,
  contextActiveTokens: null,
  contextUsableWindow: null,
  contextPercentLeft: null,
  contextWindow: null,
  cacheReadTokens: null,
  cacheCreationTokens: null,
  outputTokens: null,
  cost: null,
} satisfies AgentRun;

export const DELEGATE_DONE_RUN = {
  runId: "run-done-1",
  sessionPk: CHAT_SESSION.sessionPk,
  parentRunId: null,
  retryOf: null,
  sourceToolCallId: null,
  dispatchIndex: null,
  primaryAgentId: "ryuzi",
  executingAgentId: null,
  executingAgentNameSnapshot: "Subagent worker",
  agentKind: "subagent",
  task: "Run the test suite",
  status: "completed",
  startedAt: 0,
  finishedAt: 30_000,
  toolCount: 5,
  resolvedModel: "fixture/model-beta",
  resolvedEffort: null,
  result: "All tests passed.",
  error: null,
  contextActiveTokens: null,
  contextUsableWindow: null,
  contextPercentLeft: null,
  contextWindow: null,
  cacheReadTokens: null,
  cacheCreationTokens: null,
  outputTokens: null,
  cost: null,
} satisfies AgentRun;

export const REVIEWER_CHILD_TRANSCRIPT = [
  {
    sessionPk: CHAT_SESSION.sessionPk,
    seq: 1,
    role: "assistant",
    blockType: "text",
    payload: { text: "Reviewing the diff for regressions now." },
    toolCallId: null,
    status: null,
    toolKind: null,
    createdAt: 0,
    speaker: null,
  },
] satisfies Message[];

/** The parent chat session's own transcript, seeded so the delegation
 * journey can prove the main transcript survives a Right Panel round trip
 * (open the Reviewer child run, then Back). */
export const DELEGATION_PARENT_MESSAGE = {
  sessionPk: CHAT_SESSION.sessionPk,
  seq: 1,
  role: "assistant",
  blockType: "text",
  payload: { text: "Kicking off the review delegation." },
  toolCallId: null,
  status: null,
  toolKind: null,
  createdAt: 0,
  speaker: null,
} satisfies Message;

/** Legacy (no captured owner) and deleted-owner (captured owner absent from
 * the current registry) sessions for the read-only history journey. Both are
 * chat-first (`kind: "chat"`) and idle so `composeReadOnly` in SessionView is
 * driven purely by session-primary.ts's ownership logic, not by `running`. */
export const LEGACY_SESSION = {
  ...SESSION,
  sessionPk: "s-legacy",
  primaryAgentId: null,
  primaryAgentSnapshot: null,
  projectId: null,
  branch: null,
  kind: "chat",
  status: "idle",
  title: "Legacy history",
  permMode: "default",
} satisfies Session;

export const DELETED_OWNER_SESSION = {
  ...SESSION,
  sessionPk: "s-deleted",
  primaryAgentId: "reviewer",
  primaryAgentSnapshot: { id: "reviewer", name: "Reviewer", avatarColor: "amber" },
  projectId: null,
  branch: null,
  kind: "chat",
  status: "idle",
  title: "Deleted owner history",
  permMode: "default",
} satisfies Session;

export const LEGACY_MESSAGE = {
  sessionPk: "s-legacy",
  seq: 1,
  role: "assistant",
  blockType: "text",
  payload: { text: "This is the preserved legacy transcript." },
  toolCallId: null,
  status: null,
  toolKind: null,
  createdAt: 0,
  speaker: null,
} satisfies Message;

export const DELETED_OWNER_MESSAGE = {
  sessionPk: "s-deleted",
  seq: 1,
  role: "assistant",
  blockType: "text",
  payload: { text: "This is the preserved reviewer transcript." },
  toolCallId: null,
  status: null,
  toolKind: null,
  createdAt: 0,
  speaker: null,
} satisfies Message;

/** Route target capabilities for the route-effort journey: model-alpha
 * supports an explicit "high" override, model-beta supports none. Keyed to
 * match PROVIDER_CATALOG/CONNECTIONS' "fixture" family models above, so the
 * Route tab's target picker (routeTargetOptions) resolves two real targets
 * without any per-test override. */
export const ROUTE_TARGET_CAPABILITIES = [
  {
    provider: "fixture",
    model: "model-alpha",
    contextWindow: 128000,
    supported: [{ value: "high", label: "High", description: null }],
    providerDefault: null,
  },
  { provider: "fixture", model: "model-beta", contextWindow: 128000, supported: [], providerDefault: null },
] satisfies ModelRouteTargetCapability[];

// ---------- Slash catalog + global commands fixtures (slash-command-overhaul
// plan, SDD Task 12) ----------
//
// The project-less "/" catalog always includes the three embedded builtin
// commands (`init`/`review`/`compact` — see `SlashCatalog::entries` and its
// `no_project_load_lists_global_and_builtin_commands` test in
// crates/core/src/harness/native/slash_catalog.rs) alongside whatever global
// commands and bound global skills are configured. `home`/`session`/
// `requiresProject` below mirror the real builtins exactly: `init` is
// home+session+requires-project, `review` (agent: "plan") and `compact` are
// session-only. A "global" command and a "project" skill are included too so
// both the Automations "Commands" tab (which filters to `origin === "builtin"`
// for its read-only rows) and the composer "/" popup have non-trivial data.
export const SLASH_CATALOG = [
  {
    name: "init",
    description: "Analyze the codebase and write an AGENTS.md for future agents.",
    kind: "command",
    origin: "builtin",
    home: true,
    session: true,
    requiresProject: true,
    effective: true,
    shadowsGlobal: false,
    agent: null,
    model: null,
    subtask: false,
  },
  {
    name: "review",
    description: "Review the current working changes for bugs and issues.",
    kind: "command",
    origin: "builtin",
    home: false,
    session: true,
    requiresProject: false,
    effective: true,
    shadowsGlobal: false,
    agent: "plan",
    model: null,
    subtask: false,
  },
  {
    name: "compact",
    description: "Summarize older history to free context-window space.",
    kind: "command",
    origin: "builtin",
    home: false,
    session: true,
    requiresProject: false,
    effective: true,
    shadowsGlobal: false,
    agent: null,
    model: null,
    subtask: false,
  },
  {
    name: "ship",
    description: "Ship it",
    kind: "command",
    origin: "global",
    home: true,
    session: true,
    requiresProject: false,
    effective: true,
    shadowsGlobal: false,
    agent: null,
    model: null,
    subtask: false,
  },
  {
    name: "brainstorm",
    description: "Explore an idea",
    kind: "skill",
    origin: "project",
    home: true,
    session: true,
    requiresProject: false,
    effective: true,
    shadowsGlobal: false,
    agent: null,
    model: null,
    subtask: false,
  },
] satisfies SlashEntryInfo[];

/** `global_command_list` fixture — one saved global command ("ship"),
 *  matching the catalog's `origin: "global"` entry above so the two stay
 *  consistent. `global_command_create/update/delete` (see the dynamic
 *  dispatch in `installMockIPC`) echo the mutation's own input back rather
 *  than mutating this array — no e2e journey round-trips through
 *  create/edit/delete → list yet (that Automations → Commands flow is an
 *  owed manual Tauri smoke, per the SDD brief). */
export const GLOBAL_COMMANDS = [
  { name: "ship", description: "Ship it", template: "Ship $ARGUMENTS", agent: null, model: null, subtask: false, revision: "1" },
] satisfies CommandFileInfo[];

// ---------- Plugins hub fixtures (Task 16) ----------
//
// Three `list_plugins` rows covering the hub's unified-row cases the e2e
// spec (`plugins.e2e.ts`) exercises: an installed, component-backed,
// healthy connector (`github`); a not-yet-installed, non-component
// connector (`linear` — pre-install detail's Overview+Tools-only case,
// since it has no release footprint and isn't componentBacked, so
// `visibleTabs` hides Settings/Health/Versions before install); and a
// not-yet-installed, component-backed connector with no declared auth
// (`slack` — the universal wizard's shortest component plan, `overview →
// permissions → install → done`, since `authKind: "none"` and no manifest
// settings/OAuth profiles skip the connect/settings steps).

const HUB_GITHUB_PLUGIN: PluginInfo = {
  id: "github",
  name: "GitHub",
  description: "Repos, issues, and pull requests via GitHub's official connector.",
  icon: "github",
  categories: ["dev-tools"],
  slot: null,
  ownsSlot: false,
  verified: true,
  experimental: false,
  enabled: true,
  source: "component",
  capabilities: ["connector"],
  configured: true,
  kind: "integration",
  installed: true,
  family: null,
  pinned: false,
  sourceSpec: null,
  resolvedCommit: null,
  installedAt: 1_700_000_000_000,
  updatedAt: 1_700_000_000_000,
  trustTier: null,
  catalogVersion: null,
  componentBacked: true,
  blockedReason: null,
  status: "ok",
  statusDetail: null,
  authKind: "oauth",
  toolCount: 12,
  skillCount: null,
};

const HUB_LINEAR_PLUGIN: PluginInfo = {
  id: "linear",
  name: "Linear",
  description: "Track issues and projects in Linear.",
  icon: "linear",
  categories: ["project-management"],
  slot: null,
  ownsSlot: false,
  verified: false,
  experimental: false,
  enabled: false,
  source: "catalog",
  capabilities: ["connector"],
  configured: false,
  kind: "integration",
  installed: false,
  family: null,
  pinned: false,
  sourceSpec: null,
  resolvedCommit: null,
  installedAt: null,
  updatedAt: null,
  trustTier: null,
  catalogVersion: null,
  componentBacked: false,
  blockedReason: null,
  status: "not-installed",
  statusDetail: null,
  authKind: "token",
  toolCount: null,
  skillCount: null,
};

const HUB_SLACK_PLUGIN: PluginInfo = {
  id: "slack",
  name: "Slack",
  description: "Post messages and read channels from Slack.",
  icon: "slack",
  categories: ["messaging"],
  slot: null,
  ownsSlot: false,
  verified: true,
  experimental: false,
  enabled: false,
  source: "component",
  capabilities: ["connector"],
  configured: false,
  kind: "integration",
  installed: false,
  family: null,
  pinned: false,
  sourceSpec: null,
  resolvedCommit: null,
  installedAt: null,
  updatedAt: null,
  trustTier: null,
  catalogVersion: null,
  componentBacked: true,
  blockedReason: null,
  status: "not-installed",
  statusDetail: null,
  authKind: "none",
  toolCount: 5,
  skillCount: null,
};

export const PLUGIN_HUB_ROWS: PluginInfo[] = [HUB_GITHUB_PLUGIN, HUB_LINEAR_PLUGIN, HUB_SLACK_PLUGIN];

/** `plugin_detail` per id — every registered plugin resolves here regardless
 *  of `installed` (pre-install support, Task 7); an id outside this map
 *  falls through to the dynamic dispatch's "unknown plugin" error below,
 *  matching the daemon's real 404 contract (`PluginDetailView` special-cases
 *  the `"unknown plugin:"` prefix). */
const PLUGIN_DETAILS: Record<string, PluginDetail> = {
  github: {
    info: HUB_GITHUB_PLUGIN,
    auth: {
      kind: "oauth",
      setting: null,
      env: null,
      helpUrl: "https://github.com/settings/tokens",
      configured: true,
      oauthConnectAvailable: true,
      oauthConnectError: null,
      oauthTokenStored: true,
      oauthReconnectRequired: false,
    },
    settings: [],
    mcp: [],
    models: [],
    homepage: "https://github.com/github/github-mcp-server",
    publisher: "GitHub (official)",
  },
  linear: {
    info: HUB_LINEAR_PLUGIN,
    auth: {
      kind: "token",
      setting: "plugin.linear.token",
      env: null,
      helpUrl: null,
      configured: false,
      oauthConnectAvailable: false,
      oauthConnectError: null,
      oauthTokenStored: false,
      oauthReconnectRequired: false,
    },
    settings: [],
    mcp: [],
    models: [],
    homepage: null,
    publisher: "Linear",
  },
  slack: {
    info: HUB_SLACK_PLUGIN,
    // authKind "none" — no top-level auth block, matching `planWizardSteps`'s
    // `detail.auth?.kind ?? "none"` read so the wizard's Connect step stays
    // skipped for this fixture.
    auth: null,
    settings: [],
    mcp: [],
    models: [],
    homepage: null,
    publisher: "Slack",
  },
};

/** `plugin_release_detail` overrides for ids with a release footprint —
 *  every other id (including never-installed `linear`/`slack`) falls back to
 *  the dynamic dispatch's empty-ledger default below, which mirrors the real
 *  `plugin_release_detail_is_empty_for_a_never_installed_plugin` behavior
 *  (`crates/core/src/api/plugins_api.rs`). */
const COMPONENT_RELEASES: Record<string, ComponentReleaseDetail> = {
  github: {
    pluginId: "github",
    releases: [
      {
        pluginId: "github",
        version: "1.4.0",
        sourceUrl: "https://plugins.ryuzi.dev/github/1.4.0.wasm",
        sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85",
        signingKeyId: "ryuzi-first-party",
        installedAt: 1_700_000_000_000,
        active: true,
        revoked: false,
        revocationReason: null,
        firstParty: true,
      },
    ],
    activeVersion: "1.4.0",
    activeManifest: {
      publisher: "Ryuzi",
      description: "GitHub connector — repos, issues, and pull requests.",
      lifecycle: "per-session",
      domains: ["api.github.com"],
      oauthProfiles: [],
      tools: [
        { name: "create_issue", description: "Open an issue", writes: true },
        { name: "list_repos", description: "List repositories", writes: false },
      ],
    },
    declaredManifest: null,
  },
};

/** Tauri command → resolved value (Result-typed commands get the raw data). */
const FIXTURES: Record<string, unknown> & ChildRunMockState = {
  list_projects: [PROJECT],
  list_sessions: [],
  list_messages: [],
  agentRunRoster: EMPTY_AGENT_RUN_ROSTER,
  childMessages: {} as Record<string, Message[]>,
  retryChildMessages: {} as Record<string, Message[]>,
  list_agents: AGENT_REGISTRY,
  refresh_agents: [],
  list_providers: [],
  list_provider_catalog: PROVIDER_CATALOG,
  list_connections: CONNECTIONS,
  // `list_installed_providers` is answered DYNAMICALLY in the invoke dispatch
  // below (from the live catalog + connections) so per-test connection
  // overrides still surface; the static `list_`-fallback `[]` would empty the
  // installed set and hide every provider row. Custom providers: none here.
  list_custom_providers: [],
  list_selectable_models: NATIVE_RUNTIME.selectableModels,
  list_runtimes: [NATIVE_RUNTIME],
  refresh_runtimes: [NATIVE_RUNTIME],
  list_gateways: [],
  probe_gateways: [],
  list_jobs: [],
  list_apps: [],
  // `slash_catalog` is keyed by project/agent pairing on the frontend
  // (`catalogKey`) but this mock answers every pairing with the same fixture
  // — fine for these specs, since none of them override it per-test. Global
  // command CRUD (`global_command_create/update/delete`) is dispatched
  // dynamically further down instead of listed here, since it takes an
  // `input`/`name`/`revision`.
  slash_catalog: SLASH_CATALOG,
  global_command_list: GLOBAL_COMMANDS,
  // Plugin-distribution commands invoked on the Plugins view mount. Without
  // these, the fallback returns `null` for the non-`list_`-prefixed ones
  // (`plugin_doctor`, `plugins_restart_required`), and the store then renders
  // `doctorFindings`/`restartRequired` from `null` — crashing the view and
  // wedging sidebar navigation.
  list_plugins: PLUGIN_HUB_ROWS,
  list_skills: [],
  plugin_doctor: [],
  plugins_restart_required: false,
  // The Browse tab's status line calls `catalog_status` on Plugins-view
  // mount (not just `list_`/`refresh_`-prefixed calls, which already
  // fall back to `[]` above) — without a fixture the unmocked-command
  // fallback returns `null`, and the store renders `catalogStatus` from
  // `null`, crashing the view. `refresh_catalog` shares the same
  // `CatalogStatus` shape.
  catalog_status: { sequence: 0, lastFetchAt: null, outcome: null, entries: 0, blocked: 0 },
  refresh_catalog: { sequence: 0, lastFetchAt: null, outcome: null, entries: 0, blocked: 0 },
  // An extension-capable plugin's detail view calls `extension_status` on
  // mount (Track D observability, DT8) — same "without a fixture the
  // unmocked fallback returns `null` and the view crashes" lesson as
  // `catalog_status` above. Empty list is a safe default (no plugin in these
  // fixtures declares an `extension` capability).
  extension_status: [],
  // Task 12: PluginsView's retryable bootstrap banner calls this on mount —
  // same "without a fixture the unmocked fallback returns `null` and the
  // view crashes" lesson as `catalog_status`/`extension_status` above.
  // `plugin_release_detail` (the other Task 12 mount-time call) is dispatched
  // dynamically further down instead of listed here, since it takes an `id`.
  component_bootstrap_status: { pending: false, message: null },
  // Spec B3: App.tsx's "Restart engine" banner button calls this — without a
  // fixture the unmocked fallback returns `null`, which is actually the same
  // resolved value the real `Result<null, CmdError>` unwraps to, but listing
  // it explicitly documents the call and keeps it out of the
  // `console.warn("[mock-ipc] unmocked command")` noise on every restart.
  restart_engine: null,
  get_setting: null,
  backdrop_capability: "none",
  system_accent_color: null,
  start_session: SESSION,
  project_runtime_info: initialProjectRuntime,
  update_project_runtime: initialProjectRuntime,
  provider_account_route: { provider: "fixture", strategy: "fallback" },
  list_model_statuses: [],
  list_all_model_statuses: [],
  connection_usage: null,
  set_model_effort_preference: null,
  start_chat_session: CHAT_SESSION,
  list_model_routes: [],
  list_model_route_target_capabilities: ROUTE_TARGET_CAPABILITIES,
  // Not session-scoped in this mock (get_child_runs/get_child_transcript take
  // a sessionPk but the fixture always answers with the same value) — empty
  // by default so a test that never dispatches has an empty roster; the
  // delegation journey overrides both per-test.
  get_child_runs: [],
  get_child_transcript: [],
  // Internal lookup bag for the dynamically-dispatched `get_agent` command
  // (see its branch in installMockIPC) — not a real Tauri command name.
  agent_details: AGENT_DETAILS,
  get_agent_configuration_catalog: AGENT_CONFIGURATION_CATALOG,
  // Internal lookup bags for the dynamically-dispatched `get_agent_stats`/
  // `get_agent_stats_batch` commands (Task 8) — not real Tauri command names,
  // same "not a closure, passed through `fixtures`" reasoning as
  // `agent_details` above.
  agent_stats: AGENT_STATS,
  agent_stats_lite: AGENT_STATS_LITE,
  // `list_pet_manifest` is a real, fixed-shape command (no per-call args) —
  // a plain FIXTURES entry, not a dynamic-dispatch lookup bag.
  list_pet_manifest: PET_MANIFEST,
  // `get_pet_sprite` — downloaded-pet serving isn't e2e-testable (no real
  // filesystem/network beneath the mock), so every slug resolves to `null`;
  // PetSprite's `resolveSrc` treats that as "unavailable" and renders the
  // color-tile fallback, same as a pet that was never downloaded on this
  // machine. Bundled pets never hit this path (`bundled: true` resolves
  // straight to the `/pets/<slug>/sprite.webp` public asset instead).
  get_pet_sprite: null,
  // `download_pet` — not exercised by any flow in this suite (browsing and
  // downloading a petdex pet is an owed manual Tauri smoke), but stubbed to
  // resolve so an incidental call never hangs a test.
  download_pet: null,
  // Internal lookup bags for the dynamically-dispatched `plugin_detail`/
  // `plugin_release_detail` commands (Task 16) — not real Tauri command
  // names. `page.addInitScript`'s callback is serialized via `.toString()`
  // and re-evaluated in the page, so it only ever sees its own `fixtures`
  // parameter — a direct reference to `PLUGIN_DETAILS`/`COMPONENT_RELEASES`
  // from inside the callback body would throw a `ReferenceError` in the
  // browser (no closure survives the serialization), same reasoning
  // `agent_details` above already established.
  plugin_details: PLUGIN_DETAILS,
  component_releases: COMPONENT_RELEASES,
};

/**
 * Installs a fake `window.__TAURI_INTERNALS__` before the app boots, so the
 * real `@tauri-apps/api` code path resolves against fixtures instead of a
 * missing Tauri bridge. `plugin:*` invokes (event listen, window show)
 * resolve to null. Every call is recorded on `window.__mockCalls`.
 */
export async function installMockIPC(page: Page, overrides: MockIPCOverrides = {}): Promise<void> {
  await page.addInitScript(
    (fixtures) => {
      const calls: Array<{ cmd: string; args: unknown }> = [];
      const storageKey = "ryuzi.e2e.route-state.v1";
      // Command names Plan 6 (agentic cleanup) permanently deleted from the
      // Tauri invoke surface — the single-agent settings/memory/Learning/
      // curator/orch commands, and the `learning_cmd` trio. If the UI ever
      // calls one of these again (a regression `check-agentic-cleanup.ts`
      // can't catch, since it only scans source text, not runtime
      // invocations), the unmocked-command fallback below throws instead of
      // silently resolving — so an accidental call fails the test
      // immediately rather than degrading quietly. The slash-command-overhaul
      // plan (SDD Task 3/6) similarly deleted the old per-project native
      // command commands once `slash_catalog` and the global command CRUD
      // replaced them — same fail-fast reasoning applies to `native_commands`
      // and the `*_project_command(s)` family below.
      const removedCommands = new Set([
        "search_sessions",
        "list_skill_usage",
        "set_skill_pinned",
        "get_agent_settings",
        "set_agent_settings",
        "read_memory",
        "write_memory",
        "learning_graph",
        "curator_status",
        "curator_rollback",
        "orch_submit",
        "orch_list_roots",
        "orch_tasks",
        "orch_cancel",
        "orch_retry",
        "orch_answer_block",
        "orch_steer",
        "native_commands",
        "list_project_commands",
        "read_project_command",
        "create_project_command",
        "update_project_command",
        "delete_project_command",
      ]);
      type RouteIdentity = {
        resolvedProviderId: string;
        resolvedFamily: string;
        resolvedModel: string;
        effectiveEffort: string | null;
        connectionId: string;
      };
      type RouteSelection = RouteIdentity & {
        requestedModel: string | null;
        resolvedModelDisplayName: string;
        effectiveEffortLabel: string | null;
        connectionLabel: string;
        reason: string;
      };
      type DurableState = {
        sessions: (typeof SESSION)[];
        messages: Message[];
        agentRunRoster: AgentRunRosterInfo;
        childMessages: Record<string, Message[]>;
        route: RouteIdentity | null;
        routeRequests: number;
        modelRoutes: Array<{
          id: string;
          name: string;
          enabled: boolean;
          strategy: string;
          targets: Array<{ provider: string; model: string; effort: string | null }>;
          createdAt: number;
          updatedAt: number;
        }>;
      };
      const stored = localStorage.getItem(storageKey);
      const durable: DurableState = stored
        ? (JSON.parse(stored) as DurableState)
        : {
            sessions: fixtures.list_sessions as (typeof SESSION)[],
            // Seeds pre-existing history (e.g. legacy/deleted-owner
            // transcripts) — most fixtures start empty and grow only via
            // observeRoute's route-switch notices.
            messages: (fixtures.list_messages as Message[] | undefined) ?? [],
            agentRunRoster: (fixtures.agentRunRoster as AgentRunRosterInfo | undefined) ?? EMPTY_AGENT_RUN_ROSTER,
            childMessages: (fixtures.childMessages as Record<string, Message[]> | undefined) ?? {},
            route: null,
            routeRequests: 0,
            modelRoutes: fixtures.list_model_routes as DurableState["modelRoutes"],
          };
      durable.agentRunRoster ??= (fixtures.agentRunRoster as AgentRunRosterInfo | undefined) ?? EMPTY_AGENT_RUN_ROSTER;
      durable.childMessages ??= (fixtures.childMessages as Record<string, Message[]> | undefined) ?? {};
      let sessions = durable.sessions;
      let agentRunRoster = durable.agentRunRoster;
      let childMessages = durable.childMessages;
      let connections = fixtures.list_connections as ConnectionInfo[];
      let modelRoutes = durable.modelRoutes;
      const quotaAttempts = new Map<string, number>();
      const pendingQuota = new Map<string, (value: unknown) => void>();
      let projectRuntime = fixtures.project_runtime_info as {
        projectId: string;
        model: string | null;
        storedEffort: string | null;
        effectiveEffort: string | null;
        effectiveEffortLabel: string | null;
        effectiveSource: string;
        storedEffortStatus: string;
        modelInfo: (typeof SELECTABLE_MODELS)[number] | null;
      };
      let cbId = 1;
      const eventHandlers = new Map<string, number[]>();
      const w = window as unknown as Record<string, unknown>;
      w.__mockCalls = calls;
      w.__resolveMockQuota = (id: string) => {
        pendingQuota.get(id)?.(quotaFor(id, 99));
        pendingQuota.delete(id);
      };

      const quotaFor = (id: string, usedOverride?: number) => ({
        provider: id.startsWith("claude") ? "anthropic-oauth" : "openai-oauth",
        plan: id.startsWith("claude") ? "Claude Pro" : "ChatGPT Plus",
        message: null,
        limitReached: false,
        reviewLimitReached: false,
        resetCredits: id.startsWith("codex") ? { availableCount: 2, refreshAt: "2030-01-01T00:00:00Z" } : null,
        quotas: [
          {
            label: id.startsWith("claude") ? "5 hour" : "Codex primary",
            usedPercentage: usedOverride ?? (id.endsWith("backup") ? 35 : 20),
            remainingPercentage: 100 - (usedOverride ?? (id.endsWith("backup") ? 35 : 20)),
            resetAt: "2030-01-01T00:00:00Z",
          },
        ],
      });

      const persist = () => {
        durable.sessions = sessions;
        durable.agentRunRoster = agentRunRoster;
        durable.childMessages = childMessages;
        durable.modelRoutes = modelRoutes;
        localStorage.setItem(storageKey, JSON.stringify(durable));
      };

      const emitCoreEvent = (event: Record<string, unknown>) => {
        // The real CoreEventMsg envelope is `{ runnerId, event }` (bindings.ts) —
        // store.ts's listener destructures both and keys per-session state by
        // `sessKey(runnerId, session_pk)`. All fixture sessions here are started
        // on the local runner (see LOCAL_RUNNER in src/lib/session-key.ts), so
        // the mock must stamp the same id or the live event lands under a
        // different composite key than the one the UI is reading from.
        for (const handler of eventHandlers.get("core-event-msg") ?? []) {
          const callback = (window as unknown as Record<string, (payload: unknown) => void>)[`_${handler}`];
          callback?.({ event: "core-event-msg", id: 0, payload: { runnerId: "local", event } });
        }
      };

      const appendChildMessage = (runId: string, message: Message) => {
        const rows = childMessages[runId] ?? [];
        const index = message.toolCallId
          ? rows.findIndex((row) => row.toolCallId === message.toolCallId)
          : rows.findIndex((row) => row.seq === message.seq);
        const next = rows.slice();
        if (index >= 0) next[index] = message;
        else next.push(message);
        childMessages = { ...childMessages, [runId]: next.sort((left, right) => left.seq - right.seq) };
      };

      type MockCoreEventInput = {
        event: CoreEvent;
        roster?: AgentRunRosterInfo;
        childMessage?: { runId: string; message: Message };
      };

      w.__emitMockCoreEvent = (input: MockCoreEventInput) => {
        if (input.roster) agentRunRoster = input.roster;
        if (input.childMessage) appendChildMessage(input.childMessage.runId, input.childMessage.message);
        persist();
        emitCoreEvent(input.event as unknown as Record<string, unknown>);
      };

      const createRetryRun = (args: unknown): AgentRun => {
        const { runId } = args as { runId: string };
        const previous = agentRunRoster.runs.find((run) => run.runId === runId);
        if (!previous) throw new Error(`Unknown child run: ${runId}`);
        const retried: AgentRun = {
          ...previous,
          runId: `${previous.runId}-retry`,
          retryOf: previous.runId,
          status: "queued",
          startedAt: null,
          finishedAt: null,
          toolCount: 0,
          result: null,
          error: null,
        };
        agentRunRoster = { ...agentRunRoster, runs: [...agentRunRoster.runs, retried] };
        childMessages = {
          ...childMessages,
          [retried.runId]: fixtures.retryChildMessages?.[previous.runId] ?? childMessages[retried.runId] ?? [],
        };
        persist();
        return retried;
      };

      const observeRoute = (sessionPk: string) => {
        const modelInfo = (fixtures.list_runtimes as (typeof NATIVE_RUNTIME)[])[0].selectableModels.find(
          (model) => model.requestValue === projectRuntime.model,
        );
        const useBackup = durable.routeRequests >= 2;
        const scripted = fixtures.route_selections as RouteSelection[] | undefined;
        const currentSelection: RouteSelection = scripted?.[Math.min(durable.routeRequests, scripted.length - 1)] ?? {
          requestedModel: projectRuntime.model,
          resolvedProviderId: "fixture",
          resolvedFamily: modelInfo?.preferenceKey?.family ?? "fixture",
          resolvedModel: modelInfo?.preferenceKey?.model ?? projectRuntime.model ?? "default",
          effectiveEffort: projectRuntime.effectiveEffort,
          connectionId: useBackup ? "fixture-backup" : "fixture-account",
          resolvedModelDisplayName: modelInfo?.displayName ?? projectRuntime.model ?? "Default model",
          effectiveEffortLabel: projectRuntime.effectiveEffortLabel,
          connectionLabel: useBackup ? "Backup account" : "Fixture account",
          reason: useBackup ? "roundRobin" : "initial",
        };
        const current: RouteIdentity = {
          resolvedProviderId: currentSelection.resolvedProviderId,
          resolvedFamily: currentSelection.resolvedFamily,
          resolvedModel: currentSelection.resolvedModel,
          effectiveEffort: currentSelection.effectiveEffort,
          connectionId: currentSelection.connectionId,
        };
        const previous = durable.route;
        durable.route = current;
        durable.routeRequests += 1;

        let text: string | null = null;
        if (previous) {
          const modelChanged =
            previous.resolvedFamily !== current.resolvedFamily ||
            previous.resolvedModel !== current.resolvedModel ||
            previous.effectiveEffort !== current.effectiveEffort;
          const accountChanged =
            previous.resolvedProviderId !== current.resolvedProviderId || previous.connectionId !== current.connectionId;
          const reason =
            {
              ordered: "account order",
              roundRobin: "round robin",
              authenticationUnavailable: "authentication unavailable",
              quotaUnavailable: "quota unavailable",
              rateLimit: "rate limit",
              providerUnavailable: "provider unavailable",
              transportUnavailable: "transport unavailable",
            }[currentSelection.reason] ?? null;
          if (modelChanged) {
            text = `Switched to ${currentSelection.resolvedModelDisplayName}${currentSelection.effectiveEffortLabel ? ` · ${currentSelection.effectiveEffortLabel}` : ""}`;
            if (accountChanged && reason) text += ` via ${currentSelection.connectionLabel} · ${reason}`;
          } else if (accountChanged) {
            text = `Account switched to ${currentSelection.connectionLabel}${reason ? ` · ${reason}` : ""}`;
          }
        }

        if (text) {
          const message: Message = {
            sessionPk,
            seq: durable.messages.length + 1,
            role: "system",
            blockType: "notice",
            payload: { text },
            toolCallId: null,
            status: null,
            toolKind: null,
            createdAt: Date.now(),
            speaker: null,
          };
          durable.messages.push(message);
          persist();
          emitCoreEvent({
            kind: "message",
            session_pk: message.sessionPk,
            seq: message.seq,
            role: message.role,
            block_type: message.blockType,
            payload: message.payload,
            tool_call_id: message.toolCallId,
            status: message.status,
            tool_kind: message.toolKind,
          });
        } else {
          persist();
        }
      };

      w.__TAURI_INTERNALS__ = {
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { label: "main", windowLabel: "main" },
        },
        plugins: {},
        transformCallback: (cb: (payload: unknown) => void) => {
          const id = cbId++;
          Object.defineProperty(window, `_${id}`, { value: cb, configurable: true });
          return id;
        },
        invoke: (cmd: string, args: unknown) => {
          calls.push({ cmd, args });
          if (cmd === "plugin:event|listen") {
            const registration = args as { event: string; handler: number };
            eventHandlers.set(registration.event, [...(eventHandlers.get(registration.event) ?? []), registration.handler]);
            return Promise.resolve(registration.handler);
          }
          if (cmd === "plugin:event|unlisten") return Promise.resolve(null);
          if (cmd.startsWith("plugin:")) return Promise.resolve(null);
          if (cmd === "list_sessions") return Promise.resolve(sessions);
          if (cmd === "list_connections") return Promise.resolve(connections);
          if (cmd === "list_installed_providers") {
            // The Models list filters to installed families. Derive them live
            // from the (possibly per-test-overridden) catalog + connections so
            // every provider row surfaces, mirroring production where a
            // connected/seeded provider is installed.
            const catalog = (fixtures.list_provider_catalog as Array<{ id: string; family: string }>) ?? [];
            const familyOf = (provider: string) => catalog.find((entry) => entry.id === provider)?.family ?? provider;
            const families = new Set<string>([
              ...catalog.map((entry) => entry.family),
              ...connections.map((connection) => familyOf(connection.provider)),
            ]);
            return Promise.resolve([...families]);
          }
          if (cmd === "list_messages") {
            const { sessionPk } = args as { sessionPk: string };
            return Promise.resolve(durable.messages.filter((message) => message.sessionPk === sessionPk));
          }
          if (cmd === "get_child_runs") return Promise.resolve(agentRunRoster);
          if (cmd === "get_child_transcript") {
            const { runId } = args as { runId: string };
            return Promise.resolve(childMessages[runId] ?? []);
          }
          if (cmd === "retry_child_run") return Promise.resolve(createRetryRun(args));
          if (cmd === "get_agent") {
            const { agentId } = args as { agentId: string };
            const details = fixtures.agent_details as Record<string, unknown>;
            return Promise.resolve(details[agentId] ?? null);
          }
          // Task 8: `get_agent_stats` — the detail view's Overview stat cards.
          // An id with no entry in `agent_stats` (an unknown id, or the
          // synthetic Fresh Agent's "fresh") resolves to an inline all-zero
          // `AgentStatsInfo`, mirroring the real backend's "unknown or
          // synthetic agent id ... zeroes out" contract rather than erroring.
          if (cmd === "get_agent_stats") {
            const { agentId } = args as { agentId: string };
            const stats = fixtures.agent_stats as Record<string, AgentStatsInfo>;
            return Promise.resolve(
              stats[agentId] ?? {
                sessionCount: 0,
                lastActive: null,
                costUsd7d: 0,
                tokens7d: 0,
                runsTotal30d: 0,
                runsFailed30d: 0,
                topTools: [],
              },
            );
          }
          // Task 8: `get_agent_stats_batch` — the roster row's lazy stats
          // fragment. Real backend shape (see `agent_stats_batch_returns_
          // entries_only_for_requested_ids` in crates/core/src/api/
          // agent_api.rs): a map with exactly one entry per requested agent
          // id — never more (unrequested agents are excluded), never fewer
          // (an id with no stats, e.g. unknown or synthetic, still gets a
          // zeroed `AgentStatsLite` entry, same "zeroes out" contract as
          // `get_agent_stats` above) — mirrored here via `?? zeroed`.
          if (cmd === "get_agent_stats_batch") {
            const { agentIds } = args as { agentIds: string[] };
            const lite = fixtures.agent_stats_lite as Record<string, AgentStatsLite>;
            const batch: Record<string, AgentStatsLite> = {};
            for (const id of agentIds) {
              batch[id] = lite[id] ?? { sessionCount: 0, lastActive: null, costUsd7d: 0 };
            }
            return Promise.resolve(batch);
          }
          // The Fresh Agent is a synthetic, non-editable row — the real backend
          // rejects these four mutation commands for id "fresh" with a 409
          // conflict ("fresh agent is built-in", the `FRESH_AGENT_ID` guards in
          // `crates/core/src/api/agent_api.rs`). Mirrored here so e2e can't
          // silently pass against a mock that would otherwise let the mutation
          // through (previously these commands had no mock handling at all and
          // fell through to the generic `Promise.resolve(null)` default below
          // for every id, fresh included).
          if (
            (cmd === "update_agent" || cmd === "delete_agent" || cmd === "set_default_agent" || cmd === "duplicate_agent") &&
            (args as { agentId?: string } | undefined)?.agentId === "fresh"
          ) {
            return Promise.reject({ message: "fresh agent is built-in" });
          }
          // Task 12: PluginsView (bootstrap-retry banner + "Component plugins"
          // section) and PluginDetailView's component-only fallback both call
          // this on mount, for arbitrary ids (mimo/opencode today). Dispatched
          // dynamically (echoing `id` back) rather than a single static
          // FIXTURES entry so two different requested ids never collide on
          // the same fixture object (which would break `pluginId`-keyed lists).
          if (cmd === "plugin_release_detail") {
            const { id } = args as { id: string };
            const releases = fixtures.component_releases as Record<string, unknown>;
            return Promise.resolve(releases[id] ?? { pluginId: id, releases: [], activeVersion: null, activeManifest: null, declaredManifest: null });
          }
          // Task 16: `plugin_detail` — pre-install support (Task 7) means this
          // resolves for a registered-but-not-yet-installed plugin too (the hub's
          // Discover rows), not just installed ones. An id outside `plugin_details`
          // rejects with the "unknown plugin:" prefix `PluginDetailView` special-cases
          // (its own component-only fallback render).
          if (cmd === "plugin_detail") {
            const { id } = args as { id: string };
            const details = fixtures.plugin_details as Record<string, unknown>;
            const detail = details[id];
            if (!detail) return Promise.reject({ message: `unknown plugin: ${id}` });
            return Promise.resolve(detail);
          }
          // Task 16: `plugin_tools` — echoes a single declared tool for any id,
          // mirroring a component's declared-tools manifest response (`live:
          // false`) regardless of install state, so the Tools tab/wizard Overview
          // and Done steps always have something to render.
          if (cmd === "plugin_tools") {
            const { pluginId } = args as { pluginId: string };
            return Promise.resolve({
              pluginId,
              live: false,
              entries: [{ name: "create_issue", description: "Open an issue", kind: "tool", writes: true }],
            });
          }
          // Task 16: `install_component_plugin` — the universal wizard's
          // `InstallComponentStep` calls this for any component-backed plugin;
          // echoes a fresh single-release `ComponentReleaseDetail` back so the
          // step's `ctx.refresh()` (and the Versions tab, if reopened) has
          // something coherent to show post-install.
          if (cmd === "install_component_plugin") {
            const { id } = args as { id: string; version: string | null };
            return Promise.resolve({
              pluginId: id,
              releases: [
                {
                  pluginId: id,
                  version: "1.0.0",
                  sourceUrl: `https://plugins.ryuzi.dev/${id}/1.0.0.wasm`,
                  sha256: "0".repeat(64),
                  signingKeyId: "ryuzi-first-party",
                  installedAt: Date.now(),
                  active: true,
                  revoked: false,
                  revocationReason: null,
                  firstParty: true,
                },
              ],
              activeVersion: "1.0.0",
              activeManifest: {
                publisher: "Ryuzi",
                description: "Installed via the e2e fixture.",
                lifecycle: "per-session",
                domains: [],
                oauthProfiles: [],
                tools: [],
              },
              declaredManifest: null,
            });
          }
          if (cmd === "list_model_routes") return Promise.resolve(modelRoutes);
          if (cmd === "save_model_route") {
            const { route } = args as { route: (typeof modelRoutes)[number] };
            modelRoutes = modelRoutes.some((current) => current.id === route.id)
              ? modelRoutes.map((current) => (current.id === route.id ? route : current))
              : [...modelRoutes, route];
            persist();
            return Promise.resolve(modelRoutes);
          }
          if (cmd === "start_session") {
            const session = fixtures.start_session as typeof SESSION;
            sessions = [session];
            persist();
            observeRoute(session.sessionPk);
            return Promise.resolve(session);
          }
          if (cmd === "start_chat_session") {
            const session = fixtures.start_chat_session as typeof CHAT_SESSION;
            sessions = [session];
            persist();
            observeRoute(session.sessionPk);
            return Promise.resolve(session);
          }
          if (cmd === "continue_session") {
            observeRoute((args as { sessionPk: string }).sessionPk);
            return Promise.resolve(null);
          }
          if (cmd === "stop_session") {
            const { sessionPk } = args as { sessionPk: string };
            sessions = sessions.map((session) => (session.sessionPk === sessionPk ? { ...session, status: "idle" as const } : session));
            persist();
            return Promise.resolve(null);
          }
          if (cmd === "project_runtime_info") return Promise.resolve(projectRuntime);
          if (cmd === "update_project_runtime") {
            const update = args as { model: string | null; effort: string | null };
            const modelInfo = (fixtures.list_runtimes as (typeof NATIVE_RUNTIME)[])[0].selectableModels.find(
              (model) => model.requestValue === update.model,
            );
            const fallback = modelInfo?.supported.find((option) => option.value === modelInfo.resolvedDefault) ?? null;
            const selected = modelInfo?.supported.find((option) => option.value === update.effort) ?? fallback;
            projectRuntime = {
              projectId: projectRuntime.projectId,
              model: update.model,
              storedEffort: update.effort,
              effectiveEffort: selected?.value ?? null,
              effectiveEffortLabel: selected?.label ?? null,
              effectiveSource: update.effort ? "project" : modelInfo ? "provider" : "none",
              storedEffortStatus: "valid",
              modelInfo: modelInfo ?? null,
            };
            return Promise.resolve(projectRuntime);
          }
          if (cmd === "connection_provider_quota") {
            const { id } = args as { id: string };
            const attempts = (quotaAttempts.get(id) ?? 0) + 1;
            quotaAttempts.set(id, attempts);
            if (fixtures.quota_failure_once === id && attempts === 1) {
              return Promise.reject({ message: "Provider quota unavailable" });
            }
            const delayKey = `ryuzi.e2e.delayed-quota.${id}`;
            if (fixtures.delayed_quota === id && !localStorage.getItem(delayKey)) {
              localStorage.setItem(delayKey, "pending");
              return new Promise((resolve) => pendingQuota.set(id, resolve));
            }
            return Promise.resolve(quotaFor(id));
          }
          if (cmd === "reset_codex_credit") return Promise.resolve({ consumed: true, availableCount: 1 });
          if (cmd === "rename_connection") {
            const { id, label } = args as { id: string; label: string };
            connections = connections.map((connection) => (connection.id === id ? { ...connection, label } : connection));
            return Promise.resolve(connections);
          }
          if (cmd === "set_connection_enabled") {
            const { id, enabled } = args as { id: string; enabled: boolean };
            connections = connections.map((connection) => (connection.id === id ? { ...connection, enabled } : connection));
            return Promise.resolve(connections);
          }
          if (cmd === "move_connection") {
            const { id, dir } = args as { id: string; dir: number };
            const from = connections.findIndex((connection) => connection.id === id);
            const to = Math.max(0, Math.min(connections.length - 1, from + dir));
            if (from >= 0 && from !== to) {
              const next = [...connections];
              const [moved] = next.splice(from, 1);
              next.splice(to, 0, moved);
              connections = next.map((connection, priority) => ({ ...connection, priority }));
            }
            return Promise.resolve(connections);
          }
          if (cmd === "remove_connection") {
            const { id } = args as { id: string };
            connections = connections.filter((connection) => connection.id !== id);
            return Promise.resolve(connections);
          }
          if (cmd === "test_connection") return Promise.resolve({ ok: true, message: "Connection works" });
          if (cmd === "reconnect_oauth") {
            const { connectionId } = args as { connectionId: string };
            connections = connections.map((connection) =>
              connection.id === connectionId ? { ...connection, needsRelogin: false } : connection,
            );
            return Promise.resolve(connections);
          }
          // Global command CRUD (slash-command-overhaul plan, SDD Task 12):
          // `global_command_read` is dispatched dynamically by `name` against
          // the static `global_command_list` fixture; create/update/delete
          // echo the mutation's own input back as a fresh `CommandFileInfo`
          // rather than mutating that array — see the comment on
          // `GLOBAL_COMMANDS` above for why a stateful mock isn't needed yet.
          if (cmd === "global_command_read") {
            const { name } = args as { name: string };
            const found = (fixtures.global_command_list as CommandFileInfo[]).find((command) => command.name === name);
            if (!found) return Promise.reject({ message: `unknown global command: ${name}` });
            return Promise.resolve(found);
          }
          if (cmd === "global_command_create") {
            const { input } = args as { input: CommandFileInputDto };
            return Promise.resolve({
              name: input.name,
              description: input.description,
              template: input.template,
              agent: input.agent,
              model: input.model,
              subtask: input.subtask ?? false,
              revision: "1",
            } satisfies CommandFileInfo);
          }
          if (cmd === "global_command_update") {
            const { name, revision, input } = args as { name: string; revision: string; input: CommandFileMutationDto };
            return Promise.resolve({
              name,
              description: input.description,
              template: input.template,
              agent: input.agent,
              model: input.model,
              subtask: input.subtask ?? false,
              revision: `${revision}-1`,
            } satisfies CommandFileInfo);
          }
          if (cmd === "global_command_delete") return Promise.resolve(null);
          if (cmd in fixtures) return Promise.resolve(fixtures[cmd]);
          if (removedCommands.has(cmd)) {
            throw new Error(`[mock-ipc] removed command invoked by UI: ${cmd} — this should never be called`);
          }
          console.warn("[mock-ipc] unmocked command:", cmd);
          if (cmd.startsWith("list_") || cmd.startsWith("refresh_") || cmd.startsWith("probe_")) {
            return Promise.resolve([]);
          }
          return Promise.resolve(null);
        },
      };
    },
    { ...FIXTURES, ...overrides },
  );
}

export async function mockCalls(page: Page): Promise<Array<{ cmd: string; args: Record<string, unknown> | undefined }>> {
  return page.evaluate(() => (window as unknown as { __mockCalls: [] }).__mockCalls);
}
