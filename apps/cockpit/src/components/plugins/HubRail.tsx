import { MonitorUp, RefreshCw } from "lucide-react";
import { Button, cn } from "@ryuzi/ui";
import type { CatalogStatus } from "@/bindings";
import {
  ALL_SURFACES,
  railCounts,
  type HubItem,
  type RailFilter,
  type RailState,
  surfaceCounts,
  type SurfaceBadge,
} from "@/lib/plugin-hub";

const STATE_ORDER: RailState[] = ["all", "installed", "discover", "attention", "updates"];
const STATE_LABELS: Record<RailState, string> = {
  all: "All",
  installed: "Installed",
  discover: "Discover",
  attention: "Needs attention",
  updates: "Updates",
};

const SURFACE_LABEL: Record<SurfaceBadge, string> = {
  provider: "Provider",
  tools: "Tools",
  mcp: "MCP",
  skills: "Skills",
  commands: "Commands",
  hooks: "Hooks",
  jobs: "Jobs",
};

// Task 13: the rail's second filter group is now surface-based (replacing
// the old kind taxonomy) — one entry per `SurfaceBadge`, in the backend's
// stable emission order (`ALL_SURFACES`).
const SURFACE_ENTRIES: { key: SurfaceBadge; label: string }[] = ALL_SURFACES.map((key) => ({ key, label: SURFACE_LABEL[key] }));

/** Subtle rail-footer status line summarizing the last `catalog_status`/
 *  `refresh_catalog` snapshot (replaces the old Browse-tab status line).
 *  Pure (and exported) so it stays unit-testable without mounting the view.
 *  Canonical home as of Task 8 — `PluginsView.tsx` re-exports it so existing
 *  importers keep working. */
export function catalogStatusLabel(status: CatalogStatus): string {
  if (!status.lastFetchAt) return "Catalog not yet fetched";
  const when = new Date(status.lastFetchAt).toLocaleString();
  const blockedPart = status.blocked > 0 ? `, ${status.blocked} blocked` : "";
  return `Catalog seq ${status.sequence} · ${status.entries} entries${blockedPart} · fetched ${when}`;
}

function RailRow({ active, label, count, onClick }: { active: boolean; label: string; count: number; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex w-full items-center justify-between gap-2 rounded-md px-2.5 py-1.5 text-left text-[12.5px] transition-colors",
        active ? "bg-accent font-medium text-accent-foreground" : "text-muted-foreground hover:bg-muted hover:text-foreground",
      )}
    >
      <span className="truncate">{label}</span>
      <span className="shrink-0 text-[11px] tabular-nums">{count}</span>
    </button>
  );
}

/** Left rail (design doc §3): state filters → kind filters → catalog
 *  categories → footer (catalog status, refresh, update-all, doctor). Each
 *  group is an independent filter axis — clicking a kind/category entry a
 *  second time clears it; state always has an active entry (no toggle-off,
 *  "All" is the reset). */
export function HubRail({
  items,
  filter,
  onChange,
  catalogStatus,
  onRefreshCatalog,
  refreshing,
  onOpenDoctor,
  updateAllEnabled,
  updatingAll,
  onUpdateAll,
}: {
  items: HubItem[];
  filter: RailFilter;
  onChange: (filter: RailFilter) => void;
  catalogStatus: CatalogStatus | null;
  onRefreshCatalog: () => void;
  refreshing: boolean;
  onOpenDoctor: () => void;
  updateAllEnabled: boolean;
  updatingAll: boolean;
  onUpdateAll: () => void;
}) {
  const counts = railCounts(items);
  const sCounts = surfaceCounts(items);
  const categories = Array.from(new Set(items.flatMap((i) => i.categories))).sort();

  return (
    // Sticky within the view's scroll container: `top-7` mirrors the
    // container's `py-7` so the pinned position keeps the resting vertical
    // rhythm, and the max-height (viewport minus the 44px title bar, the
    // main panel's 10px bottom margin, and that same `top-7` inset) lets the
    // rail scroll internally instead of clipping on short windows.
    <aside className="sticky top-7 flex max-h-[calc(100dvh-54px-1.75rem)] w-[190px] shrink-0 flex-col gap-4 self-start overflow-y-auto">
      <div className="flex flex-col gap-0.5">
        {STATE_ORDER.map((state) => (
          <RailRow
            key={state}
            label={STATE_LABELS[state]}
            count={counts[state]}
            active={filter.state === state}
            onClick={() => onChange({ ...filter, state })}
          />
        ))}
      </div>

      <div className="flex flex-col gap-0.5 border-t border-border pt-3">
        {SURFACE_ENTRIES.map((entry) => (
          <RailRow
            key={entry.key}
            label={entry.label}
            count={sCounts[entry.key] ?? 0}
            active={filter.surface === entry.key}
            onClick={() => onChange({ ...filter, surface: filter.surface === entry.key ? null : entry.key })}
          />
        ))}
      </div>

      {categories.length > 0 && (
        <div className="flex flex-col gap-0.5 border-t border-border pt-3">
          {categories.map((category) => (
            <RailRow
              key={category}
              label={category}
              count={items.filter((i) => i.categories.includes(category)).length}
              active={filter.category === category}
              onClick={() => onChange({ ...filter, category: filter.category === category ? null : category })}
            />
          ))}
        </div>
      )}

      <div className="flex flex-col gap-2 border-t border-border pt-3">
        {catalogStatus && <p className="m-0 text-[11px] leading-[1.4] text-muted-foreground">{catalogStatusLabel(catalogStatus)}</p>}
        <Button variant="outline" size="sm" onClick={onRefreshCatalog} disabled={refreshing} className="w-full justify-start">
          <RefreshCw aria-hidden size={12} strokeWidth={2} className={refreshing ? "animate-spin" : undefined} />
          {refreshing ? "Refreshing…" : "Refresh catalog"}
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={onUpdateAll}
          disabled={!updateAllEnabled || updatingAll}
          className="w-full justify-start"
        >
          <MonitorUp aria-hidden size={12} strokeWidth={2} className={updatingAll ? "animate-spin" : undefined} />
          {updatingAll ? "Updating…" : "Update all"}
        </Button>
        <Button variant="ghost" size="sm" onClick={onOpenDoctor} className="w-full justify-start text-muted-foreground">
          Doctor
        </Button>
      </div>
    </aside>
  );
}
