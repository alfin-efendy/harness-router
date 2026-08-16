import { useRef, useState } from "react";
import type { ManualMcpOauthClient } from "@/bindings";
import { Button, FormField, Input, Modal, ModalBody, ModalFooter, ModalHeader } from "@ryuzi/ui";

/**
 * Record an OAuth client id for an authorization server that offers no RFC 7591
 * dynamic client registration — the enterprise/self-hosted case where
 * registration is an admin action and the daemon has nothing to register with.
 *
 * Props-driven: it performs no RPC of its own, so the save/delete rules stay in
 * `store-apps.ts` and the daemon.
 *
 * There is deliberately NO token-endpoint field, and there must never be one.
 * The token endpoint is the binding the daemon's
 * `apps_api::require_registered_token_endpoint` checks before it POSTs an
 * authorization code, and it stays trustworthy only while a real discovery run
 * is its sole writer.
 */
export function ManualOauthClientModal({
  serverName,
  clients,
  onClose,
  onSave,
  onDelete,
}: {
  serverName: string;
  clients: ManualMcpOauthClient[];
  onClose: () => void;
  onSave: (issuer: string, clientId: string) => Promise<boolean>;
  onDelete: (issuer: string) => Promise<boolean>;
}) {
  const [issuer, setIssuer] = useState("");
  const [clientId, setClientId] = useState("");
  const [saving, setSaving] = useState(false);
  const issuerRef = useRef<HTMLInputElement>(null);

  const trimmedIssuer = issuer.trim();
  const trimmedClientId = clientId.trim();
  const canSave = trimmedIssuer.length > 0 && trimmedClientId.length > 0 && !saving;

  const save = async () => {
    if (!canSave) return;
    setSaving(true);
    const ok = await onSave(trimmedIssuer, trimmedClientId);
    setSaving(false);
    // Stays open on success: client ids are per authorization server, and a
    // user working through a tenant usually has more than one to record.
    if (ok) {
      setIssuer("");
      setClientId("");
    }
  };

  return (
    <Modal onClose={onClose} width={520} busy={saving} initialFocus={issuerRef}>
      <ModalHeader
        title="Client ID"
        description={`Some authorization servers do not let apps register themselves. Paste the issuer URL and the client id your administrator gave you, and connecting ${serverName} will use it instead of registering a new one.`}
      />
      <ModalBody>
        <div className="flex flex-col gap-3.5">
          {/* `aria-label` rather than relying on FormField's wrapping <label>:
              the hint text sits inside that label too, so the control's
              accessible name would otherwise be the label AND the hint. */}
          <FormField label="Issuer URL" hint="Copy it exactly from the sign-in error on this page — it is matched character for character.">
            <Input
              ref={issuerRef}
              aria-label="Issuer URL"
              placeholder="https://auth.example.com"
              value={issuer}
              onChange={(event) => setIssuer(event.target.value)}
            />
          </FormField>
          <FormField label="Client ID">
            <Input aria-label="Client ID" value={clientId} onChange={(event) => setClientId(event.target.value)} />
          </FormField>
          <div className="flex flex-col gap-2 border-t border-border pt-3.5">
            {clients.length === 0 ? (
              <span className="text-[12.5px] text-muted-foreground">No client ids recorded.</span>
            ) : (
              clients.map((entry) => (
                <div key={entry.issuer} className="flex items-center gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[12.5px]">{entry.issuer}</div>
                    <div className="truncate text-xs text-muted-foreground">{entry.clientId}</div>
                  </div>
                  <Button
                    variant="outline"
                    size="sm"
                    aria-label={`Remove client id for ${entry.issuer}`}
                    disabled={saving}
                    onClick={() => void onDelete(entry.issuer)}
                  >
                    Remove
                  </Button>
                </div>
              ))
            )}
          </div>
        </div>
      </ModalBody>
      <ModalFooter>
        <Button variant="outline" onClick={onClose} disabled={saving}>
          Cancel
        </Button>
        <Button onClick={() => void save()} disabled={!canSave}>
          {saving ? "Saving…" : "Save"}
        </Button>
      </ModalFooter>
    </Modal>
  );
}
