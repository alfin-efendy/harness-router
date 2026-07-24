import { CheckCircle2, Circle } from "lucide-react";
import {
  Button,
  cn,
  SettingsCard as Card,
  SettingsCardHeader as CardHeader,
  SettingsCardRow as CardRow,
  SettingsCardTitle as CardTitle,
} from "@ryuzi/ui";

/** The three setup steps a mid-setup plugin can still owe, always produced in
 *  this fixed order (install, then connect, then settings) — the order a
 *  user would naturally clear them in. */
export type SetupItemId = "install" | "connect" | "settings";
export type SetupItem = { id: SetupItemId; label: string; done: boolean };

const LABEL: Record<SetupItemId, string> = {
  install: "Install the plugin",
  connect: "Connect your account",
  settings: "Fill in required settings",
};

/**
 * Pure derivation of the Overview tab's "Finish setting up" checklist from
 * the same fields the detail view already has in hand: `PluginInfo.
 * installed`/`authKind` (Task 3), `detail.auth?.configured` (defaults to
 * `true` when there's no `auth` block at all — nothing to connect), and a
 * count of still-unset REQUIRED `detail.settings` fields.
 *
 * `install` always appears. `connect` only appears for a plugin that
 * declares some credential requirement (`authKind !== "none"`) — this
 * mirrors `hasAuthTab`'s own gate elsewhere in the view. `settings` only
 * appears while at least one required field is still unset; once
 * `requiredSettingsMissing` drops to zero the row (and its own "something
 * to still get right" signal) disappears entirely rather than lingering as
 * a permanently-checked row — a plugin with no required settings at all
 * produces the exact same "no row" outcome, which is the intended
 * "no required settings → no settings item" behavior.
 *
 * Permissions acceptance is NOT a separate item here — it's implied by a
 * completed component install (that accept-and-install gate lives on the
 * install path itself, see `ComponentReleaseCard`).
 */
export function deriveSetupChecklist(input: {
  installed: boolean;
  authKind: string;
  authConfigured: boolean;
  requiredSettingsMissing: number;
}): SetupItem[] {
  const items: SetupItem[] = [{ id: "install", label: LABEL.install, done: input.installed }];
  if (input.authKind !== "none") {
    items.push({ id: "connect", label: LABEL.connect, done: input.authConfigured });
  }
  if (input.requiredSettingsMissing > 0) {
    items.push({ id: "settings", label: LABEL.settings, done: input.requiredSettingsMissing === 0 });
  }
  return items;
}

// Distinct from "Configure" (the attach-failure banner's own button, which
// can render alongside this card on the same Overview tab) so the two never
// present two identically-labeled buttons at once.
const ACTION_LABEL: Record<SetupItemId, string> = {
  install: "Install",
  connect: "Connect",
  settings: "Add settings",
};

/**
 * The Overview tab's "Finish setting up" card (the view only mounts this
 * while something's actually left — see `PluginDetailView`'s own
 * render-when gate). One row per `SetupItem`: a ✓/○ glyph, the label, and a
 * single `Button size="sm"` on the FIRST undone row only — later undone rows
 * wait their turn rather than offering several actions at once when
 * clearing the first is what actually unblocks the rest.
 */
export function SetupChecklist({ items, onAction }: { items: SetupItem[]; onAction: (id: SetupItemId) => void }) {
  const firstUndoneId = items.find((item) => !item.done)?.id;
  return (
    <Card className="mb-3">
      <CardHeader>
        <CardTitle>Finish setting up</CardTitle>
      </CardHeader>
      {items.map((item) => (
        <CardRow key={item.id}>
          {item.done ? (
            <CheckCircle2 aria-hidden size={14} strokeWidth={2} className="shrink-0 text-green-500" />
          ) : (
            <Circle aria-hidden size={14} strokeWidth={2} className="shrink-0 text-muted-foreground/70" />
          )}
          <span className={cn("min-w-0 flex-1 text-[12.5px]", item.done && "text-muted-foreground")}>{item.label}</span>
          {item.id === firstUndoneId && (
            <Button size="sm" onClick={() => onAction(item.id)} className="shrink-0">
              {ACTION_LABEL[item.id]}
            </Button>
          )}
        </CardRow>
      ))}
    </Card>
  );
}
