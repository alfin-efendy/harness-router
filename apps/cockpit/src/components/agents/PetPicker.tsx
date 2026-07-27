import { Download, Search } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Button, Input, Modal, ModalBody, ModalFooter, ModalHeader } from "@ryuzi/ui";
import { commands, type PetManifestEntryInfo } from "@/bindings";
import { useBundledPets } from "@/lib/bundled-pets";
import { NEUTRAL_AVATAR_COLOR } from "@/lib/pet-sprite";
import { PetSprite } from "./PetSprite";

const BROWSE_RESULTS_CAP = 40;

type ManifestState = "idle" | "loading" | "ready" | "error";
type DownloadState = "idle" | "downloading" | "error";

function matches(query: string, ...values: (string | null | undefined)[]): boolean {
  if (!query) return true;
  return values.some((value) => value?.toLowerCase().includes(query));
}

export type PetPickerProps = {
  open: boolean;
  onClose: () => void;
  /** Currently-selected pet slug, if any — highlighted in the bundled grid and required for the Clear action to appear. */
  currentPet: string | null;
  /** Fires with the chosen slug, or `null` when the user clears back to the color look. The picker closes itself right after. */
  onSelect: (slug: string | null) => void;
};

/**
 * Grid of bundled pets + a lazily-expanded "Browse petdex.dev" search, used
 * everywhere an agent's pet avatar can be chosen (the editor modal's Pet
 * field, the detail header's avatar button). Downloading a browsed entry
 * (`commands.downloadPet`) makes it locally available and selectable; a pet
 * that's neither bundled nor downloaded on this machine simply can't be
 * picked here (no sync logic in this PR — see the brief).
 */
export function PetPicker({ open, onClose, currentPet, onSelect }: PetPickerProps) {
  const bundledPets = useBundledPets();
  const bundledSlugs = useMemo(() => new Set(bundledPets.map((pet) => pet.slug)), [bundledPets]);

  const [query, setQuery] = useState("");
  const [browseOpen, setBrowseOpen] = useState(false);
  const [manifest, setManifest] = useState<PetManifestEntryInfo[]>([]);
  const [manifestState, setManifestState] = useState<ManifestState>("idle");
  const [manifestError, setManifestError] = useState<string | null>(null);
  const [downloadStates, setDownloadStates] = useState<Record<string, DownloadState>>({});
  const [downloadedSlugs, setDownloadedSlugs] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (open) {
      setQuery("");
      setBrowseOpen(false);
    }
  }, [open]);

  const loadManifest = async () => {
    if (manifestState === "loading" || manifestState === "ready") return;
    setManifestState("loading");
    setManifestError(null);
    try {
      const result = await commands.listPetManifest();
      if (result.status === "error") {
        setManifestError(result.error.message);
        setManifestState("error");
        return;
      }
      setManifest(result.data);
      setManifestState("ready");
    } catch (error) {
      setManifestError(error instanceof Error ? error.message : String(error));
      setManifestState("error");
    }
  };

  const expandBrowse = () => {
    setBrowseOpen(true);
    void loadManifest();
  };

  const select = (slug: string | null) => {
    onSelect(slug);
    onClose();
  };

  const download = async (entry: PetManifestEntryInfo) => {
    setDownloadStates((state) => ({ ...state, [entry.slug]: "downloading" }));
    try {
      const result = await commands.downloadPet(entry.slug, entry.spritesheetUrl);
      if (result.status === "error") {
        setDownloadStates((state) => ({ ...state, [entry.slug]: "error" }));
        return;
      }
      setDownloadStates((state) => ({ ...state, [entry.slug]: "idle" }));
      setDownloadedSlugs((slugs) => new Set(slugs).add(entry.slug));
    } catch {
      setDownloadStates((state) => ({ ...state, [entry.slug]: "error" }));
    }
  };

  if (!open) return null;

  const normalizedQuery = query.trim().toLowerCase();
  const filteredBundled = bundledPets.filter((pet) => matches(normalizedQuery, pet.displayName, pet.slug));
  // Anything already offered in the bundled grid is excluded from the
  // browse results — no point offering a second, download-gated copy of a
  // pet that's already one click away above.
  const browseMatches = manifest.filter(
    (entry) => !bundledSlugs.has(entry.slug) && matches(normalizedQuery, entry.displayName, entry.slug),
  );
  const visibleBrowseMatches = browseMatches.slice(0, BROWSE_RESULTS_CAP);

  return (
    <Modal onClose={onClose} width={480}>
      <ModalHeader title="Choose a pet" description="Pick a bundled pet, or browse petdex.dev for more." />
      <ModalBody className="flex flex-col gap-3">
        <div className="relative">
          <Search aria-hidden size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
          <Input
            className="pl-9"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search pets"
            aria-label="Search pets"
          />
        </div>

        <div className="grid grid-cols-4 gap-2">
          {filteredBundled.map((pet) => (
            <button
              key={pet.slug}
              type="button"
              onClick={() => select(pet.slug)}
              aria-pressed={currentPet === pet.slug}
              className={`flex flex-col items-center gap-1 rounded-md border px-2 py-2.5 text-center hover:bg-accent ${
                currentPet === pet.slug ? "border-primary/60 bg-primary/5" : "border-border"
              }`}
            >
              <PetSprite slug={pet.slug} bundled size={40} fallbackColor={NEUTRAL_AVATAR_COLOR} />
              <span className="w-full truncate text-[11px] font-medium">{pet.displayName}</span>
            </button>
          ))}
          {filteredBundled.length === 0 && (
            <p className="col-span-4 py-2 text-center text-[12px] text-muted-foreground">No bundled pets match your search.</p>
          )}
        </div>

        <div className="border-t border-border pt-3">
          {!browseOpen ? (
            <Button variant="outline" size="sm" onClick={expandBrowse} className="w-full">
              Browse petdex.dev
            </Button>
          ) : (
            <div className="flex flex-col gap-2">
              <span className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Browse petdex.dev</span>
              {manifestState === "loading" && <p className="py-3 text-center text-[12px] text-muted-foreground">Loading petdex.dev…</p>}
              {manifestState === "error" && (
                <div className="rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-[12px] text-destructive">
                  <div className="flex items-center justify-between">
                    Couldn't load petdex.dev.
                    <Button variant="ghost" size="xs" onClick={() => void loadManifest()}>
                      Retry
                    </Button>
                  </div>
                  {manifestError && <p className="m-0 mt-1 break-words text-[11px] text-destructive/80">{manifestError}</p>}
                </div>
              )}
              {manifestState === "ready" && (
                <div className="flex max-h-64 flex-col gap-1 overflow-y-auto">
                  {visibleBrowseMatches.map((entry) => {
                    const downloadState = downloadStates[entry.slug] ?? "idle";
                    const isDownloaded = downloadedSlugs.has(entry.slug);
                    return (
                      <div key={entry.slug} className="flex items-center gap-2.5 rounded-md border border-border px-2.5 py-2">
                        {isDownloaded ? (
                          <button
                            type="button"
                            onClick={() => select(entry.slug)}
                            aria-pressed={currentPet === entry.slug}
                            className="flex min-w-0 flex-1 items-center gap-2.5 text-left"
                          >
                            <PetSprite slug={entry.slug} bundled={false} size={32} fallbackColor={NEUTRAL_AVATAR_COLOR} />
                            <span className="min-w-0 flex-1">
                              <span className="block truncate text-[12.5px] font-medium">{entry.displayName}</span>
                              {entry.submittedBy && (
                                <span className="block truncate text-[10.5px] text-muted-foreground">by {entry.submittedBy}</span>
                              )}
                            </span>
                          </button>
                        ) : (
                          <span className="min-w-0 flex-1">
                            <span className="block truncate text-[12.5px] font-medium">{entry.displayName}</span>
                            {entry.submittedBy && (
                              <span className="block truncate text-[10.5px] text-muted-foreground">by {entry.submittedBy}</span>
                            )}
                          </span>
                        )}
                        {!isDownloaded && (
                          <Button
                            variant="outline"
                            size="xs"
                            onClick={() => void download(entry)}
                            disabled={downloadState === "downloading"}
                            className="shrink-0"
                          >
                            <Download aria-hidden size={12} strokeWidth={2} />
                            {downloadState === "downloading" ? "Downloading…" : downloadState === "error" ? "Retry" : "Download"}
                          </Button>
                        )}
                      </div>
                    );
                  })}
                  {browseMatches.length === 0 && (
                    <p className="py-3 text-center text-[12px] text-muted-foreground">No matches on petdex.dev.</p>
                  )}
                  {browseMatches.length > visibleBrowseMatches.length && (
                    <p className="py-1 text-center text-[11px] text-muted-foreground">
                      Showing first {visibleBrowseMatches.length} of {browseMatches.length} matches — refine your search.
                    </p>
                  )}
                </div>
              )}
            </div>
          )}
        </div>
      </ModalBody>
      <ModalFooter>
        {currentPet && (
          <Button variant="outline" onClick={() => select(null)} className="mr-auto">
            Clear (use color)
          </Button>
        )}
        <Button variant="outline" onClick={onClose}>
          Close
        </Button>
      </ModalFooter>
    </Modal>
  );
}
