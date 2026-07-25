import { openUrl } from "@tauri-apps/plugin-opener";
import { Blocks, CircleAlert, Download, ExternalLink, MoreHorizontal, Pin, PinOff, RefreshCw, Trash2, Undo2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import {
  Badge,
  Button,
  Combobox,
  FormField,
  Input,
  Menu,
  MenuContent,
  MenuItem,
  MenuTrigger,
  Segmented,
  SettingsCard as Card,
  SettingsCardHeader as CardHeader,
  SettingsCardHint as CardHint,
  SettingsCardRow as CardRow,
  SettingsCardTitle as CardTitle,
  Switch,
} from "@ryuzi/ui";
import {
  commands,
  events,
  type ComponentManifestInfo,
  type ComponentReleaseDetail,
  type ComponentReleaseInfo,
  type ExtensionStatusEntry,
  type PluginDetail,
  type PluginToolEntry,
} from "@/bindings";
import { LOCAL_RUNNER } from "@/lib/session-key";
import { BackButton, DetailHeader } from "@/components/common/DetailHeader";
import { IconChip, Pill, PluginStatusBadge } from "@/components/common/bits";
import { InstallWizardModal } from "@/components/modals/InstallWizardModal";
import { UniversalInstallWizard } from "@/components/modals/wizard/UniversalInstallWizard";
import type { WizardStepId } from "@/components/modals/wizard/wizard-steps";
import { OauthProfileConnections } from "@/components/plugins/OauthProfileConnections";
import { PluginToolsList } from "@/components/plugins/PluginToolsList";
import { deriveSetupChecklist, SetupChecklist } from "@/components/plugins/SetupChecklist";
import { declaredToolEntries } from "@/lib/plugin-hub";
import { pluginIcon } from "@/lib/plugin-icons";
import { useNav } from "@/store-nav";
import { usePlugins } from "@/store-plugins";

const WARN = "#F59E0B";

/** First 8 characters of a resolved git commit SHA — the ledger stores the
 *  full hash; only a short prefix is useful in the UI (matches `git log
 *  --oneline`'s convention). */
function shortCommit(commit: string): string {
  return commit.slice(0, 8);
}

/** Localized date for a `plugin_installs` ledger timestamp (unix ms, per
 *  `PluginInfo.installedAt`/`updatedAt`). */
function formatLedgerTimestamp(ms: number): string {
  return new Date(ms).toLocaleDateString();
}

/** Human label for an `ExtensionStatusEntry.status` value (Track D
 *  observability, DT8). Pure and exported so it stays unit-testable without
 *  mounting the view — mirrors `PluginsView.tsx`'s `catalogStatusLabel`
 *  convention. */
export function extensionStatusLabel(status: string): string {
  switch (status) {
    case "running":
      return "Running";
    case "starting":
      return "Starting";
    case "restarting":
      return "Restarting";
    case "failed":
      return "Failed";
    case "stopped":
      return "Stopped";
    case "not-running":
      return "Not running";
    default:
      return status;
  }
}

/** `Pill` color variant for an `ExtensionStatusEntry.status` value — green-ish
 *  "primary" for healthy/running, "warn" amber for a mid-restart/transient
 *  state, "danger" red for failed, muted "secondary" for stopped/not-running. */
export function extensionStatusPillVariant(status: string): "primary" | "warn" | "danger" | "secondary" {
  switch (status) {
    case "running":
      return "primary";
    case "starting":
    case "restarting":
      return "warn";
    case "failed":
      return "danger";
    default:
      return "secondary";
  }
}

// ---------- Component-plugin (WASM bundle) release management — Task 12 ----------
//
// First-party components are registered manifest-only (`PluginSource::Component`,
// see `plugins::component_catalog`), so `pluginDetail` resolves for them and
// this view renders normally. A component with a release ledger but NO
// registered manifest still falls back to a component-only render (below)
// driven entirely by `plugin_release_detail`. Pure helpers here are exported
// so they stay unit-testable without mounting the view.

/** Human label for a `PluginBundleManifest.lifecycle` value (already a plain
 *  kebab-case string on the wire — see `ComponentManifestInfo.lifecycle`). */
export function componentLifecycleLabel(lifecycle: string): string {
  switch (lifecycle) {
    case "singleton":
      return "Singleton — one shared instance for the whole process";
    case "per-session":
      return "Per session — one instance per chat session";
    case "per-call":
      return "Per call — a fresh instance every call";
    default:
      return lifecycle;
  }
}

/** Publisher-verification label for one release row. `firstParty` is computed
 *  server-side (`ComponentReleaseInfo.firstParty`) so this never re-derives
 *  trust from a magic string client-side. */
export function firstPartyBadgeLabel(release: ComponentReleaseInfo): string {
  return release.firstParty ? "First-party" : `Third-party (key: ${release.signingKeyId})`;
}

/** The install/update permission-confirmation summary rows — `null` manifest
 *  (nothing installed yet, so nothing has been fetched+verified) renders a
 *  single honest placeholder rather than guessing at undeclared permissions. */
export function permissionSummaryRows(manifest: ComponentManifestInfo | null): { label: string; value: string }[] {
  if (!manifest) {
    return [
      {
        label: "Permissions",
        value: "Unknown until a release is fetched and its signature is verified — nothing is fetched until you accept and install.",
      },
    ];
  }
  const rows = [
    { label: "Publisher", value: manifest.publisher || "Unknown" },
    { label: "Lifecycle", value: componentLifecycleLabel(manifest.lifecycle) },
    { label: "Domains", value: manifest.domains.length > 0 ? manifest.domains.join(", ") : "None declared" },
  ];
  if (manifest.oauthProfiles.length > 0) {
    rows.push({
      label: "OAuth",
      value: manifest.oauthProfiles.map((p) => `${p.id} (${p.scopes.length > 0 ? p.scopes.join(", ") : "no scopes declared"})`).join("; "),
    });
  }
  return rows;
}

/** Whether `version` can be activated (installed/rolled back to) right now:
 *  it must be a recorded, non-revoked release that isn't already active.
 *  Pure so the "Roll back"/"Activate" affordance's gating is unit-testable —
 *  mirrors the daemon's own `rollback_component_plugin` no-op guard (a
 *  missing/revoked target is refused server-side too; this just keeps the
 *  button from offering an action that would only bounce). */
export function canActivateVersion(detail: ComponentReleaseDetail, version: string): boolean {
  if (detail.activeVersion === version) return false;
  const target = detail.releases.find((r) => r.version === version);
  return target !== undefined && !target.revoked;
}

// ---------- Tabbed scaffold — Task 9 ----------

/** The five sections this view can show, driven by the `Segmented` control
 *  under the hero. Every plugin always gets `overview`; the rest only appear
 *  when there's something to show in them (see `visibleTabs`). */
export type DetailTab = "overview" | "tools" | "settings" | "versions" | "health";

/** Pure tab-visibility gate — never touches component state so it stays
 *  unit-testable without mounting the view. `installed` is `PluginInfo.
 *  installed` (kind-specific "already set up" flag, not a component's
 *  release-ledger state): `settings`/`health` only make sense once a plugin
 *  is at least enabled/configured — before that, the hero's `Install` action
 *  is the only affordance (spec §4), except `versions`, whose own install
 *  gate lives on that tab for component-backed plugins regardless of
 *  `installed`. */
export function visibleTabs(input: {
  installed: boolean;
  hasTools: boolean;
  hasAuth: boolean;
  hasSettings: boolean;
  hasVersions: boolean;
  hasHealth: boolean;
}): DetailTab[] {
  const tabs: DetailTab[] = ["overview"];
  if (input.hasTools) tabs.push("tools");
  if (input.installed && (input.hasAuth || input.hasSettings)) tabs.push("settings");
  if (input.hasVersions) tabs.push("versions");
  if (input.installed && input.hasHealth) tabs.push("health");
  return tabs;
}

// One label+input+Save row, shared by the auth credential and every
// manifest-declared settings field. Values are never pre-filled from the
// engine (it never sends them back) — only a `valueSet` boolean decides the
// placeholder, so a saved secret can only ever be replaced, never revealed.
//
// Widget-by-kind: `bool` renders a `Switch` that saves immediately on
// toggle (no separate Save step — matches every other boolean setting in
// Cockpit, e.g. the plugin's own "Enabled" switch above); a non-empty
// `options` list renders a `Combobox` (enum/choice); `int` renders a
// numeric `Input`; anything else renders the original text/password
// `Input`. `onSave` always receives the value to persist explicitly (rather
// than reading component state) so the Bool row's immediate save can pass
// its freshly toggled value without racing the parent's async state update.
//
// Exported (Task 14) so the universal install wizard's `SettingsStep` (and
// its Connect step's token-auth branch) reuse this exact row instead of a
// second implementation — see `steps-component.tsx`.
export function FieldRow({
  label,
  help,
  kind = "string",
  secret,
  required,
  valueSet,
  value,
  options = [],
  defaultValue = null,
  onChange,
  onSave,
  saving,
}: {
  label: string;
  help?: string;
  /** `PluginFieldInfo.kind` — `"string" | "int" | "bool"` in practice, but
   *  typed loosely (matches the DTO's plain `string`) so an unrecognized
   *  value falls through to the default text/password `Input` rather than
   *  failing a type check. */
  kind?: string;
  secret: boolean;
  required: boolean;
  valueSet: boolean;
  value: string;
  options?: string[];
  defaultValue?: string | null;
  onChange: (v: string) => void;
  onSave: (v: string) => void;
  saving: boolean;
}) {
  const fieldLabel = required ? `${label} *` : label;
  const placeholder = valueSet
    ? "●●●● saved"
    : defaultValue != null
      ? `Default: ${defaultValue}`
      : required
        ? "Required — not set"
        : "Optional — not set";

  if (kind === "bool") {
    const on = value === "true" || (value === "" && defaultValue === "true");
    return (
      <div className="border-b border-border px-[18px] py-3 last:border-b-0">
        <div className="flex items-center justify-between gap-2">
          <span className="text-[13px] font-medium">{fieldLabel}</span>
          <span className={saving ? "pointer-events-none opacity-40" : ""}>
            <Switch
              on={on}
              onToggle={() => {
                const next = on ? "false" : "true";
                onChange(next);
                onSave(next);
              }}
              label={label}
            />
          </span>
        </div>
        {help && <p className="m-0 mt-1.5 text-xs text-muted-foreground">{help}</p>}
      </div>
    );
  }

  return (
    <div className="border-b border-border px-[18px] py-3 last:border-b-0">
      <div className="flex items-end gap-2">
        <FormField label={fieldLabel} className="min-w-0 flex-1">
          {options.length > 0 ? (
            <Combobox
              aria-label={label}
              options={options.map((o) => ({ value: o, label: o }))}
              value={value || null}
              onValueChange={onChange}
              placeholder={placeholder}
              className="w-full"
            />
          ) : (
            <Input
              type={kind === "int" ? "number" : secret ? "password" : "text"}
              value={value}
              onChange={(e) => onChange(e.target.value)}
              placeholder={placeholder}
            />
          )}
        </FormField>
        {/* Outside the FormField's <label> on purpose — button is a labelable
            element too, so nesting it inside would fold the label's (and
            hint's) text into "Save"'s accessible name. */}
        <Button size="sm" onClick={() => onSave(value)} disabled={saving || value.trim().length === 0}>
          {saving ? "Saving…" : "Save"}
        </Button>
      </div>
      {help && <p className="m-0 mt-1.5 text-xs text-muted-foreground">{help}</p>}
    </div>
  );
}

// The component (WASM bundle) release-management card: release version,
// publisher verification, domains/OAuth/lifecycle, an install/update
// permission-confirmation gate, and the release list (with a "Roll back"
// action per prior-good version — this doubles as "pin to this version"
// since there is no separate pin RPC for component releases; see the Task
// 12 report). Shared by both the full `PluginDetail` render's Versions tab
// (forward-compatible with a future generic registration) and the
// component-only fallback render below (today's actual mimo/opencode path,
// since they are never `CorePlugin`s). Task 9: doctor findings moved OUT to
// the Health tab (`PluginDetailView`'s own `idFindings`) — this card no
// longer needs the id/doctorFindings it used only to filter that list.
function ComponentReleaseCard({
  detail,
  permissionsAccepted,
  onAcceptedChange,
  installBusy,
  onInstall,
  onInstallWizard,
  activateBusyVersion,
  onActivateVersion,
}: {
  detail: ComponentReleaseDetail;
  permissionsAccepted: boolean;
  onAcceptedChange: (accepted: boolean) => void;
  installBusy: boolean;
  onInstall: () => void;
  /** Task 14 duplication cleanup: a never-installed component's fresh
   *  install now routes through the universal wizard (its own Permissions
   *  step owns the accept-switch dance) rather than this card's inline
   *  gate — that gate stays exactly as-is for the update/rollback case
   *  below, which is this tab's actual job (spec §4). */
  onInstallWizard: () => void;
  activateBusyVersion: string | null;
  onActivateVersion: (version: string) => void;
}) {
  const hasActive = detail.activeVersion !== null;
  const rows = permissionSummaryRows(detail.activeManifest);
  // Newest-first for the release list (the store returns oldest-first).
  const releases = [...detail.releases].reverse();

  return (
    <Card className="mb-3">
      <CardHeader>
        <CardTitle>Component plugin</CardTitle>
        <CardHint>{hasActive ? `v${detail.activeVersion} active` : "Not installed"}</CardHint>
      </CardHeader>

      <div className="border-b border-border px-[18px] py-3.5">
        <div className="mb-2 text-[12.5px] font-semibold">{hasActive ? "Permissions (current release)" : "Permissions"}</div>
        <div className="flex flex-col gap-1.5">
          {rows.map((r) => (
            <div key={r.label} className="flex gap-2 text-[12.5px]">
              <span className="w-[75px] shrink-0 font-medium text-muted-foreground">{r.label}</span>
              <span className="min-w-0 flex-1 break-words">{r.value}</span>
            </div>
          ))}
        </div>
      </div>

      {hasActive && (
        <div className="flex items-center justify-between gap-3 border-b border-border px-[18px] py-3">
          <span className="text-[12.5px] font-medium">I understand and accept these permissions</span>
          <Switch on={permissionsAccepted} onToggle={() => onAcceptedChange(!permissionsAccepted)} label="Accept permissions" />
        </div>
      )}

      <div className="flex justify-end gap-2 border-b border-border px-[18px] py-3">
        {hasActive ? (
          <Button size="sm" onClick={onInstall} disabled={!permissionsAccepted || installBusy}>
            <Download aria-hidden size={13} strokeWidth={2} />
            {installBusy ? "Installing…" : "Update to latest"}
          </Button>
        ) : (
          <Button size="sm" onClick={onInstallWizard}>
            <Download aria-hidden size={13} strokeWidth={2} />
            Install with wizard…
          </Button>
        )}
      </div>

      {releases.map((r) => (
        <CardRow key={r.version}>
          <span className="w-[70px] shrink-0 font-mono text-xs font-medium">{r.version}</span>
          {r.active && <Pill variant="primary">Active</Pill>}
          {r.revoked && <Pill variant="danger">Revoked</Pill>}
          <Pill variant={r.firstParty ? "mono" : "warn"}>{firstPartyBadgeLabel(r)}</Pill>
          <span className="shrink-0 text-[11.5px] text-muted-foreground">{formatLedgerTimestamp(r.installedAt)}</span>
          {r.revoked && r.revocationReason && (
            <span className="min-w-0 flex-1 truncate text-[11.5px] text-muted-foreground">— {r.revocationReason}</span>
          )}
          {canActivateVersion(detail, r.version) && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => onActivateVersion(r.version)}
              disabled={activateBusyVersion !== null}
              aria-label={`Roll back to ${r.version}`}
            >
              <Undo2 aria-hidden size={12} strokeWidth={2} />
              {activateBusyVersion === r.version ? "Rolling back…" : "Roll back"}
            </Button>
          )}
        </CardRow>
      ))}
    </Card>
  );
}

export function PluginDetailView({ id, initialTab }: { id: string; initialTab?: DetailTab }) {
  const nav = useNav();
  const {
    setEnabled,
    load: reloadPlugins,
    update: updatePlugin,
    pin: pinPlugin,
    uninstall: uninstallPlugin,
    doctorFindings,
    doctorLoaded,
    loadDoctor,
    installComponentPlugin,
    rollbackComponentPlugin,
    toolsById,
    toolsLiveById,
    loadTools,
  } = usePlugins();
  const [detail, setDetail] = useState<PluginDetail | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [authValue, setAuthValue] = useState("");
  const [savingAuth, setSavingAuth] = useState(false);
  const [fieldValues, setFieldValues] = useState<Record<string, string>>({});
  const [savingField, setSavingField] = useState<string | null>(null);
  const [oauthStateToken, setOauthStateToken] = useState<string | null>(null);
  const [oauthAuthorizeUrl, setOauthAuthorizeUrl] = useState("");
  const [oauthRedirectUri, setOauthRedirectUri] = useState("");
  const [oauthCode, setOauthCode] = useState("");
  const [oauthBusy, setOauthBusy] = useState<"begin" | "complete" | "disconnect" | null>(null);
  const [updatingPack, setUpdatingPack] = useState(false);
  const [extensionEntries, setExtensionEntries] = useState<ExtensionStatusEntry[]>([]);
  // Component-plugin (WASM bundle) release management — Task 12.
  const [releaseDetail, setReleaseDetail] = useState<ComponentReleaseDetail | null>(null);
  const [releaseLoaded, setReleaseLoaded] = useState(false);
  const [permissionsAccepted, setPermissionsAccepted] = useState(false);
  const [installBusy, setInstallBusy] = useState(false);
  const [activateBusyVersion, setActivateBusyVersion] = useState<string | null>(null);
  // Tabbed scaffold — Task 9. `initialTab` seeds the raw state, but the
  // ACTIVE tab (computed below, once `visibleTabs` can be evaluated) always
  // snaps back to "overview" when the raw value isn't currently visible —
  // covers both a stale deep link and data that hasn't loaded yet.
  const [tab, setTab] = useState<DetailTab>(initialTab ?? "overview");
  // Pre-install hero action for a non-component plugin — reuses the
  // existing catalog wizard (Task 15 migrates classic connectors onto the
  // universal one too).
  const [installWizardOpen, setInstallWizardOpen] = useState(false);
  // Task 14: the universal install wizard for component-backed plugins —
  // launched from the hero Install action, the checklist's connect/settings
  // actions (resuming at that step), and the Versions tab's never-installed
  // "Install with wizard…" button. `null` when closed.
  const [universalWizard, setUniversalWizard] = useState<{ initialStep?: WizardStepId } | null>(null);

  const load = useCallback(async () => {
    const res = await commands.pluginDetail(LOCAL_RUNNER, id);
    if (res.status === "ok") setDetail(res.data);
    // A component (WASM bundle) plugin id — mimo/opencode today — is never a
    // `CorePlugin`, so this 404s for it EXPECTEDLY; the component-only render
    // below (driven by `releaseDetail`) carries its own state, so this
    // specific, deterministic error is not worth alarming the user with.
    // Any other failure (a real 404, a network error, …) still toasts.
    else if (!res.error.message.startsWith("unknown plugin:")) toast.error(`Couldn't load plugin: ${res.error.message}`);
    setLoaded(true);
  }, [id]);

  const loadRelease = useCallback(async () => {
    const res = await commands.pluginReleaseDetail(LOCAL_RUNNER, id);
    if (res.status === "ok") setReleaseDetail(res.data);
    // Best-effort, same reasoning as `catalog_status` elsewhere: most plugin
    // ids simply have no component-release ledger rows, which is the normal
    // (not an error) case — never toast here.
    setReleaseLoaded(true);
  }, [id]);

  useEffect(() => {
    setDetail(null);
    setLoaded(false);
    setAuthValue("");
    setFieldValues({});
    setOauthStateToken(null);
    setOauthAuthorizeUrl("");
    setOauthRedirectUri("");
    setOauthCode("");
    setOauthBusy(null);
    setReleaseDetail(null);
    setReleaseLoaded(false);
    setPermissionsAccepted(false);
    setInstallBusy(false);
    setActivateBusyVersion(null);
    void load();
    void loadRelease();
  }, [load, loadRelease]);

  useEffect(() => {
    if (!doctorLoaded) void loadDoctor();
  }, [doctorLoaded, loadDoctor]);

  // Tools & Skills tab — Task 10. Fetches on mount and whenever `id` changes;
  // the store caches by id (`toolsById`/`toolsLiveById`), so navigating back
  // to a previously viewed plugin re-fetches rather than trusting a stale
  // cache (matches `pluginReleaseDetail`'s own no-staleness-guard behavior).
  useEffect(() => {
    void loadTools(id);
  }, [id, loadTools]);

  // Extension (Track D "code plugin") status — DT8. `extension_status` is a
  // params-free rpc returning every plugin's entries (mirrors `catalog_status`),
  // so this view fetches it only when the plugin actually declares the
  // capability, then filters down to its own `id` client-side (same pattern
  // `doctorFindings.find((f) => f.pluginId === id ...)` above uses).
  const isExtensionPlugin = detail?.info.capabilities.includes("extension") ?? false;
  useEffect(() => {
    if (!isExtensionPlugin) {
      setExtensionEntries([]);
      return;
    }
    let active = true;
    void commands.extensionStatus(LOCAL_RUNNER).then((res) => {
      if (active && res.status === "ok") setExtensionEntries(res.data.filter((e) => e.pluginId === id));
    });
    return () => {
      active = false;
    };
  }, [isExtensionPlugin, id]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;

    void events.pluginOauthAuthorizeUrlMsg
      .listen((event) => {
        if (!active || event.payload.pluginId !== id) return;
        setOauthAuthorizeUrl(event.payload.authorizeUrl);
      })
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [id]);

  // Loopback completions land as an event (the install wizard's callback
  // server also serves flows begun here) — pick them up so Connect finishes
  // without the manual code paste. The paste UI stays as the fallback.
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;

    void events.pluginOauthCompletedMsg
      .listen((event) => {
        if (!active || event.payload.pluginId !== id) return;
        if (!event.payload.ok) {
          toast.error(event.payload.error ?? "OAuth sign-in didn't finish.");
          return;
        }
        toast.success("Connected");
        setOauthStateToken(null);
        setOauthAuthorizeUrl("");
        setOauthRedirectUri("");
        setOauthCode("");
        void load().then(() => reloadPlugins());
      })
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [id, load, reloadPlugins]);

  const onInstallComponent = async () => {
    if (installBusy || !permissionsAccepted) return;
    setInstallBusy(true);
    const res = await installComponentPlugin(id);
    setInstallBusy(false);
    if (res) {
      setReleaseDetail(res);
      // A newly-installed/updated release may declare different permissions
      // than what was just accepted — require a fresh accept for the next
      // mutating action rather than carrying acceptance across releases.
      setPermissionsAccepted(false);
    }
  };

  const onActivateComponentVersion = async (version: string) => {
    if (activateBusyVersion !== null || !releaseDetail?.activeVersion) return;
    setActivateBusyVersion(version);
    const res = await rollbackComponentPlugin(id, releaseDetail.activeVersion, version);
    setActivateBusyVersion(null);
    if (res) setReleaseDetail(res);
  };

  if (!loaded || !releaseLoaded) {
    return (
      <div className="min-h-0 flex-1 overflow-y-auto px-8 pb-10 pt-[22px]">
        <div className="mx-auto max-w-[720px]">
          <BackButton label="Back" onClick={() => nav.goBack()} />
          <div className="text-[13px] text-muted-foreground">Loading…</div>
        </div>
      </div>
    );
  }

  // A component (WASM bundle) plugin — mimo/opencode today — is never a
  // `CorePlugin`. First-party bundles now ARE registered (see
  // `plugins::component_catalog`), so they resolve a normal `detail`; this
  // fallback remains for any component that has a release-ledger footprint
  // without a registered manifest — e.g. a bundle installed by id before its
  // manifest shipped, or a future third-party component.
  const isComponentOnly = !detail && releaseDetail !== null && releaseDetail.releases.length > 0;

  if (!detail && !isComponentOnly) {
    return (
      <div className="min-h-0 flex-1 overflow-y-auto px-8 pb-10 pt-[22px]">
        <div className="mx-auto max-w-[720px]">
          <BackButton label="Back" onClick={() => nav.goBack()} />
          <div className="text-[13px] text-muted-foreground">Plugin not found.</div>
        </div>
      </div>
    );
  }

  if (isComponentOnly && releaseDetail) {
    // No registered manifest to derive tools/auth/settings/health from — the
    // scaffold here is just the two tabs spec §4 calls for: an overview
    // (header only) and the release-management versions tab.
    const fallbackTabs = visibleTabs({
      installed: false,
      hasTools: false,
      hasAuth: false,
      hasSettings: false,
      hasVersions: true,
      hasHealth: false,
    });
    const fallbackActiveTab = fallbackTabs.includes(tab) ? tab : "overview";
    return (
      <div className="min-h-0 flex-1 overflow-y-auto px-8 pb-10 pt-[22px]">
        <div className="mx-auto max-w-[720px]">
          <BackButton label="Back" onClick={() => nav.goBack()} />
          <DetailHeader chip={<IconChip icon={Blocks} size={44} />} title={id} sub="Component plugin (WASM bundle)" />
          <div className="mb-4 overflow-x-auto">
            <Segmented
              options={fallbackTabs.map((t) => ({ id: t, label: t === "overview" ? "Overview" : "Versions" }))}
              value={fallbackActiveTab}
              onChange={setTab}
            />
          </div>
          {fallbackActiveTab === "versions" && (
            <div data-testid="tab-panel-versions">
              <ComponentReleaseCard
                detail={releaseDetail}
                permissionsAccepted={permissionsAccepted}
                onAcceptedChange={setPermissionsAccepted}
                installBusy={installBusy}
                onInstall={() => void onInstallComponent()}
                onInstallWizard={() => setUniversalWizard({})}
                activateBusyVersion={activateBusyVersion}
                onActivateVersion={(v) => void onActivateComponentVersion(v)}
              />
            </div>
          )}
        </div>
        {universalWizard && (
          <UniversalInstallWizard
            pluginId={id}
            initialStep={universalWizard.initialStep}
            onClose={() => {
              setUniversalWizard(null);
              void load();
              void loadRelease();
              void reloadPlugins();
            }}
          />
        )}
      </div>
    );
  }

  // `detail` is non-null past this point (the two early returns above cover
  // every `!detail` case).
  if (!detail) return null;

  const { info } = detail;
  const Icon = pluginIcon(info.icon);
  const experimental = info.experimental;
  // The source of truth on load is the ledger's persisted `pinned` flag —
  // `pin()` still paints `usePlugins`' state optimistically before this
  // view's next `load()` brings back the authoritative value.
  const pinned = info.pinned;
  // Doctor findings for THIS id — the attach-failed one drives the banner
  // (below); the full list drives the Health tab's findings card.
  const idFindings = doctorFindings.filter((f) => f.pluginId === id);
  // Doctor's `attach-failed` finding is the only signal today for a
  // connector that failed to attach — `PluginDetail` itself carries no
  // attach-status field (see the Task 11 report's DTO-gap note).
  const attachFailure = idFindings.find((f) => f.kind === "attach-failed");

  // ---------- Tabbed scaffold — Task 9 ----------
  const hasVersionsTab =
    releaseDetail !== null && (info.componentBacked || releaseDetail.releases.length > 0 || releaseDetail.activeVersion !== null);
  // A component's device-flow-connectable OAuth profiles (`OauthProfileConnections`)
  // also live on the Settings tab — without this, a component-backed plugin
  // with no generic `detail.auth`/`settings`/`mcp` (e.g. atlassian/bitbucket)
  // would have no way to reach its own Connect action.
  const hasComponentOauth = (releaseDetail?.activeManifest?.oauthProfiles ?? []).some((p) => p.deviceAuthorizationUrl && p.tokenUrl);
  const hasAuthTab = (!!detail.auth && detail.auth.kind !== "none") || hasComponentOauth;
  const hasSettingsTab = detail.settings.length > 0 || detail.mcp.length > 0;
  const hasHealthTab = isExtensionPlugin || idFindings.length > 0;

  // Tools & Skills tab — Task 10. `fallbackTools` is the pre-install case: a
  // component-backed plugin's declared manifest tools (Task 2), mapped via
  // the shared `declaredToolEntries` (`@/lib/plugin-hub`) to the same
  // `PluginToolEntry` shape `plugin_tools` returns — so `PluginToolsList`
  // never needs to branch on which source it came from, and the wizard's own
  // `OverviewStep` (`steps-component.tsx`) reuses the identical mapping
  // rather than duplicating it. Once the store's `loadTools(id)` resolves
  // (even to an empty list), its cache wins over the fallback — `id in
  // toolsById` is the "has this id's fetch completed" gate (an empty array
  // is falsy under `??`, so a plain `toolsById[id] ?? fallbackTools` would
  // wrongly keep showing the fallback after a real fetch resolved to zero
  // entries).
  const toolsLoaded = id in toolsById;
  const fallbackTools: PluginToolEntry[] = declaredToolEntries(releaseDetail?.activeManifest ?? null);
  const resolvedTools = toolsLoaded ? (toolsById[id] ?? []) : fallbackTools;
  const resolvedToolsLive = toolsLoaded ? (toolsLiveById[id] ?? false) : false;
  // A provider whose models only ever arrive via `plugin_tools` (`toolCount`
  // is null, no manifest tools) must still get a Tools tab once that fetch
  // resolves with entries — hence the `resolvedTools.length > 0` arm below,
  // not just the two pre-load signals.
  const hasToolsTab = (info.toolCount ?? 0) > 0 || fallbackTools.length > 0 || resolvedTools.length > 0;
  const tabs = visibleTabs({
    installed: info.installed,
    hasTools: hasToolsTab,
    hasAuth: hasAuthTab,
    hasSettings: hasSettingsTab,
    hasVersions: hasVersionsTab,
    hasHealth: hasHealthTab,
  });
  const activeTab: DetailTab = tabs.includes(tab) ? tab : "overview";
  // Before the store's fetch resolves and there's no manifest fallback
  // either, the label falls back to the list's own pre-fetch estimate
  // (`info.toolCount`) rather than flashing "(0)".
  const toolsTabCount = toolsLoaded || fallbackTools.length > 0 ? resolvedTools.length : (info.toolCount ?? 0);
  const TAB_LABEL: Record<DetailTab, string> = {
    overview: "Overview",
    tools: `Tools (${toolsTabCount})`,
    settings: "Settings",
    versions: "Versions",
    health: "Health",
  };
  // Whenever the release card would show at all (component-backed, or a
  // release footprint exists), give Overview a read-only permissions
  // snapshot too — the interactive accept-and-install flow stays exclusive
  // to the Versions tab's `ComponentReleaseCard`.
  const showPermissionSummary = hasVersionsTab;

  // Setup checklist — Task 11. Same inputs `hasAuthTab`/`hasSettingsTab`
  // above already derive from (`PluginInfo.authKind`/`installed`, `detail.
  // auth?.configured`, `detail.settings`), just reshaped into the pure
  // `deriveSetupChecklist` contract. Renders only while something's still
  // undone AND the install gate is actually reachable — either the plugin is
  // already installed (mid-setup: connect/settings left) or it's
  // component-backed (its own install gate lives on the Versions tab
  // regardless of `installed`, same condition `visibleTabs` uses for that
  // tab).
  const setupItems = deriveSetupChecklist({
    installed: info.installed,
    authKind: info.authKind,
    authConfigured: detail.auth?.configured ?? true,
    requiredSettingsMissing: detail.settings.filter((f) => f.required && !f.valueSet).length,
  });
  const showSetupChecklist = setupItems.some((item) => !item.done) && (info.installed || info.componentBacked);

  const onToggleEnabled = async () => {
    if (experimental) return;
    await setEnabled(id, !info.enabled);
    await load();
  };

  // Task 14: a component-backed plugin's fresh install goes through the
  // universal wizard (starting at Overview); a non-component plugin keeps
  // the existing catalog wizard (Task 15 migrates that path too).
  const onInstallClick = () => {
    if (info.componentBacked) setUniversalWizard({});
    else setInstallWizardOpen(true);
  };

  // `install` reuses the SAME hero handler above (not a duplicate branch);
  // `connect`/`settings` both resume the universal wizard at that specific
  // step (works for a non-component plugin too — e.g. a token/oauth
  // connector's own Connect step, see `steps-component.tsx`'s `ConnectStep`).
  const onSetupChecklistAction = (itemId: "install" | "connect" | "settings") => {
    if (itemId === "install") onInstallClick();
    else setUniversalWizard({ initialStep: itemId });
  };

  const onUninstall = async () => {
    const ok = await uninstallPlugin(id);
    if (ok) nav.goBack();
  };

  const onUpdatePack = async () => {
    if (updatingPack) return;
    setUpdatingPack(true);
    await updatePlugin(id, false);
    setUpdatingPack(false);
    await load();
  };

  const onTogglePin = async () => {
    // `pin()` reloads the LIST store; this view's `detail.info.pinned` comes
    // from a separate `pluginDetail` fetch, so reload it too or the pill/
    // button would stay on the pre-toggle value until the next navigation.
    await pinPlugin(id, !pinned, pinned ? undefined : "Pinned from Cockpit");
    await load();
  };

  const saveAuth = async () => {
    if (!detail.auth?.setting || authValue.trim().length === 0 || savingAuth) return;
    setSavingAuth(true);
    const res = await commands.setPluginSetting(LOCAL_RUNNER, detail.auth.setting, authValue.trim());
    if (res.status === "error") toast.error(res.error.message);
    else {
      toast.success("Saved");
      setAuthValue("");
    }
    setSavingAuth(false);
    await load();
    await reloadPlugins();
  };

  // Takes the value explicitly (rather than reading `fieldValues[key]`
  // itself) so a `FieldRow`'s immediate-save kinds (Bool's toggle) can pass
  // their freshly computed value without racing `setFieldValues`'s async
  // state update.
  const saveField = async (key: string, rawValue: string) => {
    const value = rawValue.trim();
    if (value.length === 0 || savingField) return;
    setSavingField(key);
    const res = await commands.setPluginSetting(LOCAL_RUNNER, key, value);
    if (res.status === "error") toast.error(res.error.message);
    else {
      toast.success("Saved");
      setFieldValues((v) => ({ ...v, [key]: "" }));
    }
    setSavingField(null);
    await load();
    await reloadPlugins();
  };

  const startOauth = async () => {
    if (!detail?.auth || oauthBusy) return;
    setOauthBusy("begin");
    const res = await commands.beginPluginOauth(LOCAL_RUNNER, id);
    if (res.status === "error") {
      toast.error(res.error.message);
      setOauthBusy(null);
      return;
    }
    setOauthStateToken(res.data.stateToken);
    setOauthAuthorizeUrl(res.data.authorizeUrl);
    setOauthRedirectUri(res.data.redirectUri);
    setOauthCode("");
    setOauthBusy(null);
  };

  const completeOauth = async () => {
    if (!oauthStateToken || oauthCode.trim().length === 0 || oauthBusy) return;
    setOauthBusy("complete");
    const res = await commands.completePluginOauth(LOCAL_RUNNER, id, oauthCode.trim(), oauthStateToken);
    if (res.status === "error") {
      toast.error(res.error.message);
      setOauthBusy(null);
      return;
    }
    toast.success("Connected");
    setOauthStateToken(null);
    setOauthAuthorizeUrl("");
    setOauthRedirectUri("");
    setOauthCode("");
    setOauthBusy(null);
    await load();
    await reloadPlugins();
  };

  const disconnectOauth = async () => {
    if (!detail?.auth?.oauthTokenStored || oauthBusy) return;
    setOauthBusy("disconnect");
    const res = await commands.disconnectPluginOauth(LOCAL_RUNNER, id);
    if (res.status === "error") toast.error(res.error.message);
    else {
      toast.success("Disconnected");
      setOauthStateToken(null);
      setOauthAuthorizeUrl("");
      setOauthRedirectUri("");
      setOauthCode("");
      await load();
      await reloadPlugins();
    }
    setOauthBusy(null);
  };

  const cancelOauth = () => {
    setOauthStateToken(null);
    setOauthAuthorizeUrl("");
    setOauthRedirectUri("");
    setOauthCode("");
  };

  // Shared between the Overview and Health tabs (spec: status pill in hero +
  // fix affordance on Overview, plus the full troubleshooting context on
  // Health) — same JSX, only one copy is ever mounted at a time (tab-gated).
  const attachFailureBanner = attachFailure && (
    <Card className="mb-3 flex items-start gap-3 px-[18px] py-3.5">
      <CircleAlert aria-hidden size={16} strokeWidth={2} className="mt-px shrink-0" style={{ color: WARN }} />
      <div className="min-w-0 flex-1">
        <div className="text-[13.5px] font-semibold">Attach failed</div>
        <div className="mt-1 text-[12.5px] text-muted-foreground">{attachFailure.message}</div>
        <div className="mt-1 text-[11.5px] text-muted-foreground">{attachFailure.suggestedAction}</div>
      </div>
      <Button variant="outline" size="sm" onClick={() => setTab("settings")} className="shrink-0">
        Configure
      </Button>
    </Card>
  );

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-8 pb-10 pt-[22px]">
      <div className="mx-auto max-w-[720px]">
        <BackButton label="Back" onClick={() => nav.goBack()} />

        <DetailHeader chip={<IconChip icon={Icon} size={44} />} title={info.name} sub={detail.publisher || info.description || info.id}>
          {experimental ? (
            <span className="pointer-events-none opacity-40">
              <Switch on={info.enabled} onToggle={() => void onToggleEnabled()} label="Enabled" />
            </span>
          ) : !info.installed ? (
            <Button onClick={onInstallClick}>Install</Button>
          ) : (
            <>
              <Switch on={info.enabled} onToggle={() => void onToggleEnabled()} label="Enabled" />
              <Menu>
                <MenuTrigger
                  render={
                    <Button variant="ghost" size="icon-sm" aria-label={`Actions for ${info.name}`}>
                      <MoreHorizontal aria-hidden size={15} strokeWidth={2} />
                    </Button>
                  }
                />
                <MenuContent>
                  {info.kind === "skill-pack" && (
                    <>
                      <MenuItem onClick={() => void onUpdatePack()} disabled={updatingPack}>
                        <RefreshCw aria-hidden size={13} strokeWidth={2} className={updatingPack ? "animate-spin" : undefined} />
                        {updatingPack ? "Updating…" : "Update"}
                      </MenuItem>
                      <MenuItem onClick={() => void onTogglePin()}>
                        {pinned ? <PinOff aria-hidden size={13} strokeWidth={2} /> : <Pin aria-hidden size={13} strokeWidth={2} />}
                        {pinned ? "Unpin" : "Pin"}
                      </MenuItem>
                    </>
                  )}
                  <MenuItem onClick={() => void onUninstall()} className="text-destructive">
                    <Trash2 aria-hidden size={13} strokeWidth={2} />
                    Uninstall
                  </MenuItem>
                </MenuContent>
              </Menu>
            </>
          )}
        </DetailHeader>

        <div className="mb-4 flex flex-wrap items-center gap-1.5">
          <PluginStatusBadge verified={info.verified} experimental={info.experimental} />
          {info.capabilities.includes("extension") && <Pill variant="mono">Runs code</Pill>}
          {pinned && (
            <Pill variant="mono">
              <Pin aria-hidden size={9} strokeWidth={2} className="mr-1 inline align-[-1px]" />
              Pinned
            </Pill>
          )}
          {info.categories.map((c) => (
            <Badge key={c} variant="outline">
              {c}
            </Badge>
          ))}
        </div>

        <div className="mb-4 overflow-x-auto">
          <Segmented options={tabs.map((t) => ({ id: t, label: TAB_LABEL[t] }))} value={activeTab} onChange={setTab} />
        </div>

        {activeTab === "overview" && (
          <div data-testid="tab-panel-overview">
            {(info.sourceSpec || info.resolvedCommit || info.installedAt != null || info.updatedAt != null) && (
              <Card className="mb-3">
                <CardHeader>
                  <CardTitle>Provenance</CardTitle>
                </CardHeader>
                {info.sourceSpec && (
                  <CardRow>
                    <span className="w-[100px] shrink-0 text-[13px] font-medium">Source</span>
                    <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">{info.sourceSpec}</span>
                  </CardRow>
                )}
                {info.resolvedCommit && (
                  <CardRow>
                    <span className="w-[100px] shrink-0 text-[13px] font-medium">Commit</span>
                    <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">
                      {shortCommit(info.resolvedCommit)}
                    </span>
                  </CardRow>
                )}
                {info.installedAt != null && (
                  <CardRow>
                    <span className="w-[100px] shrink-0 text-[13px] font-medium">Installed</span>
                    <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{formatLedgerTimestamp(info.installedAt)}</span>
                  </CardRow>
                )}
                {info.updatedAt != null && (
                  <CardRow>
                    <span className="w-[100px] shrink-0 text-[13px] font-medium">Updated</span>
                    <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{formatLedgerTimestamp(info.updatedAt)}</span>
                  </CardRow>
                )}
              </Card>
            )}

            {attachFailureBanner}

            {showSetupChecklist && <SetupChecklist items={setupItems} onAction={onSetupChecklistAction} />}

            <Card className="mb-3">
              <CardHeader>
                <CardTitle>About</CardTitle>
              </CardHeader>
              <div className="px-[18px] py-3.5 text-[12.5px] leading-[1.55] text-muted-foreground">
                {info.description || "No description provided."}
              </div>
              {detail.homepage && (
                <CardRow>
                  <span className="w-[100px] shrink-0 text-[13px] font-medium">Homepage</span>
                  <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">{detail.homepage}</span>
                  <Button variant="outline" size="sm" onClick={() => void openUrl(detail.homepage as string)}>
                    <ExternalLink aria-hidden size={12} strokeWidth={2} className="size-3" />
                    Open
                  </Button>
                </CardRow>
              )}
            </Card>

            {showPermissionSummary && (
              <Card className="mb-3">
                <CardHeader>
                  <CardTitle>Permissions</CardTitle>
                  {releaseDetail?.activeVersion != null && <CardHint>Current release</CardHint>}
                </CardHeader>
                <div className="px-[18px] py-3.5">
                  <div className="flex flex-col gap-1.5">
                    {permissionSummaryRows(releaseDetail?.activeManifest ?? null).map((r) => (
                      <div key={r.label} className="flex gap-2 text-[12.5px]">
                        <span className="w-[75px] shrink-0 font-medium text-muted-foreground">{r.label}</span>
                        <span className="min-w-0 flex-1 break-words">{r.value}</span>
                      </div>
                    ))}
                  </div>
                </div>
              </Card>
            )}
          </div>
        )}

        {activeTab === "tools" && (
          <div data-testid="tab-panel-tools">
            {/* Task 10: the real `plugin_tools`-backed list, grouped by kind
                (Tools/Skills/Models) — a provider's models moved here from
                the old Overview "Models" card, so this is now the ONE place
                a provider's model list shows. */}
            <PluginToolsList entries={resolvedTools} live={resolvedToolsLive} />
          </div>
        )}

        {activeTab === "settings" && (
          <div data-testid="tab-panel-settings">
            {detail.auth && detail.auth.kind !== "none" && (
              <Card className="mb-3">
                <CardHeader>
                  <CardTitle>Authentication</CardTitle>
                  <Pill variant={detail.auth.configured ? "primary" : "secondary"}>
                    {detail.auth.kind === "oauth" && detail.auth.oauthReconnectRequired
                      ? "Reconnect required"
                      : detail.auth.configured
                        ? "Configured"
                        : "Not configured"}
                  </Pill>
                </CardHeader>
                {detail.auth.kind === "oauth" ? (
                  <>
                    <div className="px-[18px] py-3.5 text-[12.5px] text-muted-foreground">
                      {detail.auth.oauthConnectAvailable
                        ? detail.auth.oauthReconnectRequired
                          ? "Cockpit has a saved token for this plugin, but it needs to be reconnected."
                          : detail.auth.oauthTokenStored
                            ? "Cockpit has a saved OAuth token for this plugin."
                            : "Cockpit can start OAuth for this plugin. After the browser redirects, paste the returned code below to finish connecting."
                        : (detail.auth.oauthConnectError ??
                          "Cockpit needs an authorize URL, token URL, and a saved client ID before it can start OAuth for this plugin.")}
                    </div>
                    {detail.auth.oauthConnectAvailable && (
                      <div className="border-t border-border px-[18px] py-3">
                        <div className="flex flex-wrap items-center justify-end gap-2">
                          {detail.auth.oauthTokenStored && (
                            <Button variant="outline" size="sm" onClick={() => void disconnectOauth()} disabled={oauthBusy !== null}>
                              {oauthBusy === "disconnect" ? "Disconnecting…" : "Disconnect"}
                            </Button>
                          )}
                          <Button size="sm" onClick={() => void startOauth()} disabled={oauthBusy !== null}>
                            {oauthBusy === "begin"
                              ? "Opening…"
                              : detail.auth.oauthReconnectRequired || detail.auth.oauthTokenStored
                                ? "Reconnect"
                                : "Connect"}
                          </Button>
                        </div>
                      </div>
                    )}
                    {oauthStateToken && (
                      <>
                        <div className="border-t border-border px-[18px] py-3">
                          <FormField label="Login URL">
                            <div className="flex min-w-0 gap-2">
                              <Input
                                readOnly
                                value={oauthAuthorizeUrl}
                                onFocus={(event) => event.currentTarget.select()}
                                className="min-w-0 font-mono text-[11.5px]"
                              />
                              <Button
                                variant="outline"
                                size="sm"
                                onClick={() => void openUrl(oauthAuthorizeUrl)}
                                disabled={oauthAuthorizeUrl.length === 0 || oauthBusy !== null}
                                className="shrink-0"
                              >
                                Open
                              </Button>
                            </div>
                          </FormField>
                          <div className="mt-3">
                            <FormField label="Authorization code">
                              <Input
                                value={oauthCode}
                                onChange={(event) => setOauthCode(event.target.value)}
                                placeholder="Paste the code value from the callback URL"
                              />
                            </FormField>
                          </div>
                          <p className="m-0 mt-1.5 text-xs text-muted-foreground">
                            Callback URL: <span className="font-mono text-[11px]">{oauthRedirectUri}</span>
                          </p>
                        </div>
                        <div className="flex justify-end gap-2 border-t border-border px-[18px] py-3">
                          <Button variant="outline" size="sm" onClick={cancelOauth} disabled={oauthBusy !== null}>
                            Cancel
                          </Button>
                          <Button
                            size="sm"
                            onClick={() => void completeOauth()}
                            disabled={oauthBusy !== null || oauthCode.trim().length === 0}
                          >
                            {oauthBusy === "complete" ? "Connecting…" : "Finish connect"}
                          </Button>
                        </div>
                      </>
                    )}
                  </>
                ) : detail.auth.setting ? (
                  <FieldRow
                    label="Credential"
                    help={detail.auth.env ? `Falls back to the ${detail.auth.env} environment variable if unset.` : undefined}
                    secret
                    required
                    valueSet={detail.auth.configured}
                    value={authValue}
                    onChange={setAuthValue}
                    onSave={() => void saveAuth()}
                    saving={savingAuth}
                  />
                ) : (
                  <div className="px-[18px] py-3.5 text-[12.5px] text-muted-foreground">
                    {detail.auth.env && (
                      <>
                        Set the <span className="font-mono text-xs">{detail.auth.env}</span> environment variable.
                      </>
                    )}
                    {!detail.auth.env && "No credential required beyond enabling the plugin."}
                  </div>
                )}
                {detail.auth.helpUrl && (
                  <div className="flex justify-end border-t border-border px-[18px] py-3">
                    <Button variant="outline" size="sm" onClick={() => void openUrl(detail.auth?.helpUrl as string)}>
                      <ExternalLink aria-hidden size={12} strokeWidth={2} className="size-3" />
                      Help
                    </Button>
                  </div>
                )}
              </Card>
            )}

            {detail.settings.length > 0 && (
              <Card className="mb-3">
                <CardHeader>
                  <CardTitle>Settings</CardTitle>
                </CardHeader>
                {detail.settings.map((f) => (
                  <FieldRow
                    key={f.key}
                    label={f.label}
                    help={f.help || undefined}
                    kind={f.kind}
                    secret={f.secret}
                    required={f.required}
                    valueSet={f.valueSet}
                    value={fieldValues[f.key] ?? ""}
                    options={f.options}
                    defaultValue={f.default}
                    onChange={(v) => setFieldValues((m) => ({ ...m, [f.key]: v }))}
                    onSave={(v) => void saveField(f.key, v)}
                    saving={savingField === f.key}
                  />
                ))}
              </Card>
            )}

            {detail.mcp.length > 0 && (
              <Card className="mb-3">
                <CardHeader>
                  <CardTitle>MCP servers</CardTitle>
                </CardHeader>
                {detail.mcp.map((m) => (
                  <CardRow key={m.name}>
                    <span className="w-[120px] shrink-0 text-[13px] font-medium">{m.name}</span>
                    <Pill variant="mono">{m.transport}</Pill>
                    <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground">{m.commandOrUrl}</span>
                  </CardRow>
                ))}
              </Card>
            )}

            {/* Device-grant OAuth connect for a component's declared profiles —
                renders itself null unless the active manifest declares a
                device-flow-connectable profile. Refreshes the release detail
                on connect/disconnect so the status badge reflects the new
                token. */}
            {releaseDetail?.activeManifest && (
              <OauthProfileConnections
                pluginId={id}
                profiles={releaseDetail.activeManifest.oauthProfiles}
                onChanged={() => void loadRelease()}
              />
            )}
          </div>
        )}

        {activeTab === "versions" && (
          <div data-testid="tab-panel-versions">
            {/* A component (WASM bundle) plugin is now BOTH a registered
                `CorePlugin` (so it has a real `detail`) AND has a release
                ledger, so this card renders alongside the normal detail. It
                shows for any component-backed plugin — even one never
                installed yet — so its install / permission-acceptance gate is
                reachable; a non-component plugin only shows it if it somehow
                has release footprint. */}
            {releaseDetail && hasVersionsTab && (
              <ComponentReleaseCard
                detail={releaseDetail}
                permissionsAccepted={permissionsAccepted}
                onAcceptedChange={setPermissionsAccepted}
                installBusy={installBusy}
                onInstall={() => void onInstallComponent()}
                onInstallWizard={() => setUniversalWizard({})}
                activateBusyVersion={activateBusyVersion}
                onActivateVersion={(v) => void onActivateComponentVersion(v)}
              />
            )}
          </div>
        )}

        {activeTab === "health" && (
          <div data-testid="tab-panel-health">
            {attachFailureBanner}

            {idFindings.length > 0 && (
              <Card className="mb-3">
                <CardHeader>
                  <CardTitle>Health</CardTitle>
                </CardHeader>
                {idFindings.map((f) => (
                  <div key={f.kind} className="border-b border-border px-[18px] py-3 text-[12px] text-muted-foreground last:border-b-0">
                    {f.message}
                    {f.suggestedAction && <div className="mt-1 text-[11px]">{f.suggestedAction}</div>}
                  </div>
                ))}
              </Card>
            )}

            {info.capabilities.includes("extension") && (
              <Card>
                <CardHeader>
                  <CardTitle>Extension</CardTitle>
                </CardHeader>
                {extensionEntries.length === 0 ? (
                  <div className="px-[18px] py-3.5 text-[12.5px] text-muted-foreground">No extension status reported yet.</div>
                ) : (
                  extensionEntries.map((e) => (
                    <CardRow key={e.name}>
                      <span className="w-[120px] shrink-0 truncate text-[13px] font-medium">{e.name}</span>
                      <Pill variant={extensionStatusPillVariant(e.status)}>{extensionStatusLabel(e.status)}</Pill>
                      {e.restartCount > 0 && (
                        <span className="shrink-0 text-[11.5px] text-muted-foreground">
                          {e.restartCount} restart{e.restartCount === 1 ? "" : "s"}
                        </span>
                      )}
                      {e.lastError && <span className="min-w-0 flex-1 truncate text-[11.5px] text-muted-foreground">{e.lastError}</span>}
                    </CardRow>
                  ))
                )}
              </Card>
            )}
          </div>
        )}
      </div>

      {installWizardOpen && (
        <InstallWizardModal
          pluginId={id}
          pluginName={info.name}
          pluginIcon={info.icon}
          onClose={() => {
            setInstallWizardOpen(false);
            void load();
            void reloadPlugins();
          }}
        />
      )}
      {universalWizard && (
        <UniversalInstallWizard
          pluginId={id}
          initialStep={universalWizard.initialStep}
          onClose={() => {
            setUniversalWizard(null);
            void load();
            void loadRelease();
            void reloadPlugins();
          }}
        />
      )}
    </div>
  );
}
