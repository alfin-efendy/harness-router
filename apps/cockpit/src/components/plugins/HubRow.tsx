import { Pin } from "lucide-react";
import { Button, SettingsCard as Card } from "@ryuzi/ui";
import { Chip, IconChip, Pill, PluginStatusBadge, StatusDot } from "@/components/common/bits";
import { pluginIcon } from "@/lib/plugin-icons";
import { fixTargetTab, statusPresentation, type HubItem, type SurfaceBadge } from "@/lib/plugin-hub";

// Task 13: short surface badge labels (brief §"HubRow renders up to 3 badges
// + '+N' overflow, using the short labels Provider / Tools / MCP / Skills /
// Commands / Hooks / Jobs"). Replaces the old single per-kind pill.
const SURFACE_LABELS: Record<SurfaceBadge, string> = {
  provider: "Provider",
  tools: "Tools",
  mcp: "MCP",
  skills: "Skills",
  commands: "Commands",
  hooks: "Hooks",
  jobs: "Jobs",
};
const MAX_VISIBLE_SURFACES = 3;

/** One unified hub row (design doc §3 "Row anatomy"): a 38px icon/chip, a
 *  two-line body (name + badges / description + counts + status), and one
 *  contextual action button. The whole row (except the button) opens the
 *  detail page — including for not-yet-installed catalog entries. */
export function HubRow({
  item,
  onInstall,
  onOpen,
}: {
  item: HubItem;
  onInstall: () => void;
  onOpen: (tab?: "settings" | "health" | "versions") => void;
}) {
  const blocked = item.blockedReason !== null;
  const notInstalled = item.status === "not-installed";
  const fixTab = fixTargetTab(item.status);
  const presentation = statusPresentation(item.status);
  const buttonLabel = notInstalled ? "Install" : fixTab ? "Fix" : "Manage";

  const runAction = () => {
    if (notInstalled) onInstall();
    else onOpen(fixTab ?? undefined);
  };

  return (
    <Card className="mb-2 flex items-center gap-3 px-[18px] py-4">
      <Button
        variant="ghost"
        onClick={() => onOpen()}
        aria-label={`Open ${item.name}`}
        className="h-auto min-w-0 flex-1 justify-start gap-3 p-0 text-left"
      >
        {item.appInitial != null ? (
          <Chip initial={item.appInitial} color={item.appColor ?? "var(--muted-foreground)"} size={38} mono />
        ) : (
          <IconChip icon={pluginIcon(item.icon)} size={38} />
        )}
        <span className="min-w-0 flex-1">
          <span className="flex flex-wrap items-center gap-1.5">
            <span className="overflow-hidden text-ellipsis whitespace-nowrap text-sm font-semibold">{item.name}</span>
            {item.surfaces.slice(0, MAX_VISIBLE_SURFACES).map((s) => (
              <Pill key={s} variant="mono">
                {SURFACE_LABELS[s]}
              </Pill>
            ))}
            {item.surfaces.length > MAX_VISIBLE_SURFACES && <Pill variant="mono">+{item.surfaces.length - MAX_VISIBLE_SURFACES}</Pill>}
            <PluginStatusBadge verified={item.verified} experimental={item.experimental} />
            {item.pinned && (
              <Pill variant="mono">
                <Pin aria-hidden size={9} strokeWidth={2} className="mr-1 inline align-[-1px]" />
                Pinned
              </Pill>
            )}
          </span>
          <span className="mt-0.5 flex items-center gap-1.5 overflow-hidden whitespace-nowrap text-[11.5px] text-muted-foreground">
            {blocked ? (
              <span className="truncate text-destructive">{item.blockedReason}</span>
            ) : (
              <>
                <span className="truncate">{item.description}</span>
                {item.countsLabel && (
                  <>
                    <span aria-hidden>·</span>
                    <span className="shrink-0">{item.countsLabel}</span>
                  </>
                )}
                {presentation.label && (
                  <>
                    <span aria-hidden>·</span>
                    <StatusDot color={presentation.color ?? "var(--muted-foreground)"} size={6} />
                    <span className="shrink-0">{presentation.label}</span>
                  </>
                )}
              </>
            )}
          </span>
        </span>
      </Button>
      {!blocked && (
        <Button
          variant={notInstalled ? "default" : "outline"}
          size="sm"
          onClick={runAction}
          aria-label={`${buttonLabel} ${item.name}`}
          className="shrink-0"
        >
          {buttonLabel}
        </Button>
      )}
    </Card>
  );
}
