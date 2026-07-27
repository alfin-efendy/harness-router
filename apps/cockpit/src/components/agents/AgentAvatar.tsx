import { Bot } from "lucide-react";
import { cn } from "@ryuzi/ui";
import { useBundledPetSlugs } from "@/lib/bundled-pets";
import { NEUTRAL_AVATAR_COLOR, type PetPose } from "@/lib/pet-sprite";
import { PetSprite } from "./PetSprite";

// Neutral no-avatar fallback. Keeps the historical
// "agent-avatar-color-tile" testid so existing roster/detail/e2e assertions
// keep working — the tile just isn't per-agent colored anymore.
function FallbackTile({ size, className }: { size: number; className?: string }) {
  return (
    <span
      aria-hidden
      data-testid="agent-avatar-color-tile"
      className={cn("flex shrink-0 items-center justify-center rounded-lg border border-white/10 bg-muted", className)}
      style={{ width: size, height: size }}
    >
      <Bot aria-hidden size={Math.max(12, Math.round(size * 0.55))} strokeWidth={2} className="text-muted-foreground" />
    </span>
  );
}

// Split into its own component so `useBundledPetSlugs` (and the
// `/pets/index.json` fetch it triggers) only ever runs for agents that
// actually have a pet configured. Bundled-ness resolves asynchronously:
// before the first fetch settles, a bundled pet is briefly treated as
// non-bundled and PetSprite issues one wasted `getPetSprite` IPC call,
// then self-corrects on the next render. Fail-soft, matches PetSprite's
// own fallback-before-load posture.
function PetTile({ pet, size, pose, animate }: { pet: string; size: number; pose?: PetPose; animate?: boolean }) {
  const bundledSlugs = useBundledPetSlugs();
  return (
    <PetSprite slug={pet} bundled={bundledSlugs.has(pet)} size={size} pose={pose} animate={animate} fallbackColor={NEUTRAL_AVATAR_COLOR} />
  );
}

export type AgentAvatarProps = {
  /** Bundled or downloaded pet slug; `null` renders the neutral fallback tile. */
  pet: string | null;
  size: number;
  pose?: PetPose;
  animate?: boolean;
  className?: string;
};

/** An agent's identity avatar: its pet sprite when it has one, otherwise a neutral Bot tile. */
export function AgentAvatar({ pet, size, pose, animate, className }: AgentAvatarProps) {
  if (!pet) return <FallbackTile size={size} className={className} />;
  return <PetTile pet={pet} size={size} pose={pose} animate={animate} />;
}
