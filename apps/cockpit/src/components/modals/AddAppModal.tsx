import { LayoutGrid } from "lucide-react";
import { useState } from "react";
import { useApps } from "@/store-apps";
import { Button, FormField, Input, Modal, ModalBody, ModalFooter, ModalHeader, Segmented, Textarea } from "@ryuzi/ui";

// Add an MCP server by hand (stdio command or HTTP URL). Adding runs a real
// handshake, so the card lands with a true status and discovered tool list.
export function AddAppModal({ onClose }: { onClose: () => void }) {
  const add = useApps((s) => s.add);
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [transport, setTransport] = useState<"stdio" | "http">("stdio");
  const [command, setCommand] = useState("");
  const [url, setUrl] = useState("");
  const [env, setEnv] = useState("");
  const [saving, setSaving] = useState(false);

  // Global Constraint: remote MCP server URLs MUST be https:// (the MCP spec
  // requires it — see the plan's Global Constraints). The RPC also rejects a
  // plain http:// URL, but this is the UI's one chance to tell the user WHY
  // before it bounces off the backend as an opaque error.
  const trimmedUrl = url.trim();
  const httpsError = transport === "http" && trimmedUrl.length > 0 && !trimmedUrl.toLowerCase().startsWith("https://");

  const valid = name.trim().length > 0 && (transport === "stdio" ? command.trim().length > 0 : trimmedUrl.length > 0 && !httpsError);

  const submit = async () => {
    if (!valid || saving) return;
    setSaving(true);
    const parts = command.trim().split(/\s+/);
    const ok = await add({
      id: null,
      name: name.trim(),
      description: desc.trim(),
      kind: "MCP server",
      transport,
      command: transport === "stdio" ? (parts[0] ?? "") : null,
      args: transport === "stdio" ? parts.slice(1) : [],
      env: env
        .split("\n")
        .map((l) => l.trim())
        .filter((l) => l.includes("=")),
      url: transport === "http" ? url.trim() : null,
      version: null,
      publisher: null,
      color: null,
    });
    setSaving(false);
    if (ok) onClose();
  };

  return (
    <Modal onClose={onClose} width={480} busy={saving}>
      <ModalHeader
        leading={<LayoutGrid aria-hidden className="mt-0.5 size-4 text-muted-foreground" strokeWidth={2} />}
        title="Add MCP server"
        description="Point Cockpit at an MCP server. It connects immediately to verify and discover the tool list."
      />
      <ModalBody>
        <div className="flex flex-col gap-3">
          <div className="flex gap-3">
            <FormField label="Name" className="flex-1">
              <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="GitHub" />
            </FormField>
            <div className="flex flex-col gap-1.5">
              <span className="text-xs font-semibold">Transport</span>
              <Segmented
                options={[
                  { id: "stdio", label: "Stdio" },
                  { id: "http", label: "HTTP" },
                ]}
                value={transport}
                onChange={setTransport}
              />
            </div>
          </div>
          <FormField label="Description">
            <Input value={desc} onChange={(e) => setDesc(e.target.value)} placeholder="What agents use it for" />
          </FormField>
          {transport === "stdio" ? (
            <FormField label="Command">
              <Input
                className="font-mono text-xs"
                value={command}
                onChange={(e) => setCommand(e.target.value)}
                placeholder="npx -y @modelcontextprotocol/server-github"
              />
            </FormField>
          ) : (
            <FormField label="URL">
              <Input
                className="font-mono text-xs"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://mcp.example.com"
                aria-invalid={httpsError}
              />
              {httpsError && <p className="mt-1 text-xs text-destructive">Remote MCP servers must use https://.</p>}
            </FormField>
          )}
          <FormField label="Environment (KEY=value, one per line)">
            <Textarea
              className="resize-y font-mono text-xs"
              value={env}
              onChange={(e) => setEnv(e.target.value)}
              placeholder="GITHUB_TOKEN=ghp_…"
            />
          </FormField>
        </div>
      </ModalBody>
      <ModalFooter>
        <Button variant="outline" disabled={saving} onClick={onClose}>
          Cancel
        </Button>
        <Button disabled={!valid || saving} onClick={() => void submit()}>
          {saving ? "Connecting…" : "Add & connect"}
        </Button>
      </ModalFooter>
    </Modal>
  );
}
