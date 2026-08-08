import { CircleAlert, FolderGit2 } from "lucide-react";
import { useState } from "react";
import { Button, FormField, Input, Modal, ModalFooter } from "@ryuzi/ui";
import type { PluginSourceInstallBegin } from "@/bindings";
import { Pill, StatusDot } from "@/components/common/bits";
import { usePlugins } from "@/store-plugins";

const WARN = "#F59E0B";

type Step = "source" | "checking" | "trust";

/** Task 13's "Install from source…" entry (spec: local path or git URL,
 *  outside the catalog's tiered-trust model — Task 11's `install_sources`).
 *  Kept as its own modal rather than routed through `UniversalInstallWizard`
 *  for the same reason `SkillInstallModal` is: there is no catalog/plugin id
 *  yet for the wizard to fetch a `pluginDetail` for — only a source string
 *  the user is about to type (see `PluginsView.tsx`'s doc on
 *  `SkillInstallModal` for the precedent this mirrors). The trust step
 *  renders `PluginSourceInstallBegin`'s summary (network hosts, stdio
 *  commands spelled out, per-tool writes flags) — the same shape Task 15's
 *  wizard permissions step reuses when a source-installed plugin's
 *  permissions step doubles as this trust review. */
export function InstallFromSourceModal({ onClose }: { onClose: () => void }) {
  const beginSourceInstall = usePlugins((s) => s.beginSourceInstall);
  const confirmSourceInstall = usePlugins((s) => s.confirmSourceInstall);
  const [step, setStep] = useState<Step>("source");
  const [source, setSource] = useState("");
  const [trust, setTrust] = useState<PluginSourceInstallBegin | null>(null);
  const [busy, setBusy] = useState(false);

  const submitSource = async () => {
    const target = source.trim();
    if (target === "" || busy) return;
    setStep("checking");
    setBusy(true);
    const begin = await beginSourceInstall(target);
    setBusy(false);
    if (!begin) {
      setStep("source");
      return;
    }
    if (!begin.trustRequired) {
      // Nothing risky to review (signed/no mcp/no component surfaces) —
      // confirm straight away, same as a curated skill pack's immediate
      // `completed: true` path.
      setBusy(true);
      const installed = await confirmSourceInstall(begin.token, false);
      setBusy(false);
      if (installed) onClose();
      else setStep("source");
      return;
    }
    setTrust(begin);
    setStep("trust");
  };

  const accept = async () => {
    if (!trust || busy) return;
    setBusy(true);
    const installed = await confirmSourceInstall(trust.token, true);
    setBusy(false);
    if (installed) onClose();
  };

  const surfaceParts = trust
    ? [
        trust.surfaces.commands > 0 ? `${trust.surfaces.commands} commands` : null,
        trust.surfaces.skills > 0 ? `${trust.surfaces.skills} skills` : null,
        trust.surfaces.hooks > 0 ? `${trust.surfaces.hooks} hooks` : null,
        trust.surfaces.jobs > 0 ? `${trust.surfaces.jobs} jobs` : null,
      ].filter((p): p is string => p !== null)
    : [];

  return (
    <Modal onClose={onClose} width={480}>
      <div className="mb-1 flex items-center gap-2.5">
        <FolderGit2 aria-hidden size={18} strokeWidth={2} className="text-muted-foreground" />
        <span className="text-[15px] font-semibold tracking-[-0.01em]">Install from source</span>
      </div>

      {step === "source" && (
        <>
          <FormField label="Path or git URL" hint="A local plugin directory, or a git URL Cockpit can clone.">
            <Input
              value={source}
              onChange={(e) => setSource(e.target.value)}
              placeholder="/path/to/plugin or https://github.com/owner/repo"
              aria-label="Plugin source"
            />
          </FormField>
          <ModalFooter>
            <Button variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button onClick={() => void submitSource()} disabled={busy || source.trim() === ""}>
              Continue
            </Button>
          </ModalFooter>
        </>
      )}

      {step === "checking" && (
        <>
          <div className="flex items-center gap-2 py-6 text-[13px] text-muted-foreground">
            <StatusDot color="#3B82F6" size={8} pulse />
            Checking source…
          </div>
          <ModalFooter>
            <Button variant="outline" onClick={onClose}>
              Cancel
            </Button>
          </ModalFooter>
        </>
      )}

      {step === "trust" && trust && (
        <>
          <p className="mb-3 mt-0 text-[12.5px] text-muted-foreground">
            This source isn't from the signed catalog — review what it installs before Cockpit trusts it.
          </p>

          <div className="flex flex-col gap-2 rounded-md border border-border px-4 py-3 text-[12.5px]">
            <div>
              <span className="font-medium">Plugin: </span>
              {trust.name} <span className="text-muted-foreground">({trust.id})</span>
            </div>
            {trust.publisher && (
              <div>
                <span className="font-medium">Publisher: </span>
                {trust.publisher}
              </div>
            )}
            {surfaceParts.length > 0 && (
              <div>
                <span className="font-medium">Also installs: </span>
                {surfaceParts.join(", ")}
              </div>
            )}
          </div>

          {trust.mcpServers.length > 0 && (
            <div className="mt-3">
              <div className="mb-1 text-[12.5px] font-medium">MCP servers ({trust.mcpServers.length})</div>
              <ul className="m-0 flex list-none flex-col gap-1 rounded-md border border-border p-0 text-[12px] text-muted-foreground">
                {trust.mcpServers.map((server) => (
                  <li key={server.name} className="flex items-center gap-2 border-b border-border px-3 py-1.5 last:border-b-0">
                    <Pill variant="mono">{server.transport}</Pill>
                    <span className="font-medium text-foreground">{server.name}</span>
                    <span className="min-w-0 flex-1 truncate font-mono">{server.detail}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {trust.component && trust.component.networkHosts.length > 0 && (
            <div className="mt-3">
              <div className="mb-1 text-[12.5px] font-medium">Network hosts ({trust.component.networkHosts.length})</div>
              <ul className="m-0 list-none rounded-md border border-border p-0 text-[12px] text-muted-foreground">
                {trust.component.networkHosts.map((host) => (
                  <li key={host} className="border-b border-border px-3 py-1.5 font-mono last:border-b-0">
                    {host}
                  </li>
                ))}
              </ul>
            </div>
          )}

          {trust.component && trust.component.tools.length > 0 && (
            <div
              className="mt-3 flex flex-col gap-1.5 rounded-md border px-3 py-2.5 text-[12px]"
              style={trust.component.tools.some((t) => t.writes) ? { borderColor: WARN, color: WARN } : undefined}
            >
              <div className="flex items-center gap-2 font-medium">
                {trust.component.tools.some((t) => t.writes) && <CircleAlert aria-hidden size={14} strokeWidth={2} className="shrink-0" />}
                Tools ({trust.component.tools.length})
              </div>
              <ul className="m-0 flex list-none flex-col gap-1 p-0">
                {trust.component.tools.map((tool) => (
                  <li key={tool.name} className="flex items-center gap-2 font-mono">
                    {tool.name}
                    {tool.writes && <Pill variant="warn">writes</Pill>}
                  </li>
                ))}
              </ul>
            </div>
          )}

          <ModalFooter>
            <Button variant="outline" onClick={onClose}>
              Decline
            </Button>
            <Button onClick={() => void accept()} disabled={busy}>
              {busy ? "Installing…" : "Trust & Install"}
            </Button>
          </ModalFooter>
        </>
      )}
    </Modal>
  );
}
