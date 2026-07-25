import { useRef, useState } from "react";
import { Modal, ModalHeader } from "@ryuzi/ui";
import type { CatalogEntry } from "@/bindings";
import { Chip } from "@/components/common/bits";
import { ConnectionMethodForm, type ConnectionMethodFormHandle } from "@/components/connections/ConnectionMethodForm";

// Thin `Modal` wrapper (Task 15) around `ConnectionMethodForm`, which now
// owns every bit of the account-creation state machine (method chooser/
// oauth/device/apiKey) — this component is left with just the dialog chrome
// (header) plus the "always mounted, gated by `open`" contract its callers
// (`ProviderDetailView`) rely on: `open` toggles whether the dialog renders
// at all, so closing genuinely unmounts `ConnectionMethodForm` (resetting
// its state for the next open) rather than merely hiding it.
//
// The one wrinkle a thin wrapper introduces: the outer `Modal`'s own
// dismiss path (the header's X button, backdrop click, Escape) needs to run
// `ConnectionMethodForm`'s internal cancel/invalidate logic — not just call
// `onClose` directly — so a dismissal mid-flight correctly invalidates
// whatever async operation (oauth connect, device flow, add) is still
// pending, the same way its own Cancel button does. `ConnectionMethodForm`
// exposes that via `ref`.
export function AddConnectionModal({ open, onClose, family }: { open: boolean; onClose: () => void; family: string }) {
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<CatalogEntry | null>(null);
  const formRef = useRef<ConnectionMethodFormHandle>(null);

  if (!open) return null;

  return (
    <Modal onClose={() => formRef.current?.cancel()} width={480} busy={busy}>
      <ModalHeader
        leading={<Chip initial={selected?.initial ?? "C"} color={selected?.color ?? "#8B8B8B"} size={36} />}
        title="Add account"
        description={selected?.name ?? "Provider unavailable"}
      />
      <ConnectionMethodForm ref={formRef} family={family} onDone={onClose} onBusyChange={setBusy} onSelectedChange={setSelected} />
    </Modal>
  );
}
