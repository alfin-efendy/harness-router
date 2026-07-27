import { useRef, useState } from "react";
import { cn } from "@ryuzi/ui";

export type IdentityFieldProps = {
  value: string;
  ariaLabel: string;
  commit: (next: string) => void;
  className?: string;
};

/**
 * Seamless inline-editable text for the agent detail header (name,
 * description). Renders as plain text until hovered/focused; commits the
 * trimmed draft on blur or Enter; Escape or an empty/unchanged draft
 * reverts without saving (same no-op-guard posture as
 * AgentPersonalityCard's blur-commit).
 */
export function IdentityField({ value, ariaLabel, commit, className }: IdentityFieldProps) {
  // null = not editing; the input shows the live store value so optimistic
  // updates and rollbacks flow straight through when unfocused.
  const [draft, setDraft] = useState<string | null>(null);
  // Escape must revert even though `blur()` fires synchronously inside the
  // keydown handler (before the `setDraft(null)` re-render lands) — a ref,
  // not state, is the only signal onBlur can trust at that moment.
  const cancelled = useRef(false);
  return (
    <input
      aria-label={ariaLabel}
      value={draft ?? value}
      onFocus={() => setDraft(value)}
      onChange={(event) => setDraft(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") event.currentTarget.blur();
        if (event.key === "Escape") {
          cancelled.current = true;
          event.currentTarget.blur();
        }
      }}
      onBlur={() => {
        const wasCancelled = cancelled.current;
        cancelled.current = false;
        const next = (draft ?? "").trim();
        setDraft(null);
        if (wasCancelled || draft === null) return;
        if (next.length === 0 || next === value) return;
        commit(next);
      }}
      className={cn(
        "-mx-1 w-full min-w-0 truncate rounded-md border border-transparent bg-transparent px-1 outline-none",
        "hover:border-border/60 focus-visible:border-border focus-visible:bg-background",
        className,
      )}
    />
  );
}
