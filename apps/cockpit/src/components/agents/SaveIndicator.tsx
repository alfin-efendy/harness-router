import { useEffect, useRef, useState } from "react";
import { useAgents } from "@/store-agents";

// How long "✓ Saved" stays visible after a save completes, before the
// indicator clears itself back to nothing.
const SAVED_VISIBLE_MS = 1500;

type Phase = "idle" | "saving" | "saved";

/**
 * Ambient autosave status for the agent detail header: silent until the
 * first save starts, "Saving…" for the duration of any in-flight mutation
 * (`useAgents().saving`, which stays true across a queue of mutations — see
 * `enqueueMutation` in store-agents.ts), then "✓ Saved" for a beat once it
 * settles back to false, then nothing again.
 */
export function SaveIndicator() {
  const saving = useAgents((state) => state.saving);
  const [phase, setPhase] = useState<Phase>("idle");
  const observedSaving = useRef(false);

  useEffect(() => {
    if (saving) {
      observedSaving.current = true;
      setPhase("saving");
      return;
    }
    if (!observedSaving.current) return;
    observedSaving.current = false;
    setPhase("saved");
    const timer = setTimeout(() => setPhase("idle"), SAVED_VISIBLE_MS);
    return () => clearTimeout(timer);
  }, [saving]);

  if (phase === "saving") return <span className="text-[11px] text-muted-foreground">Saving…</span>;
  if (phase === "saved") return <span className="text-[11px] text-emerald-500">✓ Saved</span>;
  return null;
}
