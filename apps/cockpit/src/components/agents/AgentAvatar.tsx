import { cn } from "@ryuzi/ui";
import { useBundledPetSlugs } from "@/lib/bundled-pets";
import type { PetPose } from "@/lib/pet-sprite";
import { PetSprite } from "./PetSprite";

function ColorTile({ size, colorHex, className }: { size: number; colorHex: string; className?: string }) {
  return (
    <span
      aria-hidden
      data-testid="agent-avatar-color-tile"
      className={cn("shrink-0 rounded-lg border border-white/10", className)}
      style={{ width: size, height: size, backgroundColor: colorHex }}
    />
  );
}

// Split into its own component so `useBundledPetSlugs` (and the
// `/pets/index.json` fetch it triggers) only ever runs for agents that
// actually have a pet configured — every no-pet render (the overwhelming
// majority in most rosters) stays a plain, hook-free color tile.
//
// Bundled-ness resolves asynchronously: before that first fetch settles
// (once per app session — cached after), a bundled pet is briefly treated
// as non-bundled and PetSprite issues one wasted `getPetSprite` IPC call,
// then self-corrects on the next render. Fail-soft, matches PetSprite's own
// fallback-before-load posture; not worth the extra loading state.
function PetTile({
  pet,
  size,
  pose,
  animate,
  colorHex,
}: {
  pet: string;
  size: number;
  pose?: PetPose;
  animate?: boolean;
  colorHex: string;
}) {
  const bundledSlugs = useBundledPetSlugs();
  return <PetSprite slug={pet} bundled={bundledSlugs.has(pet)} size={size} pose={pose} animate={animate} fallbackColor={colorHex} />;
}

export type AgentAvatarProps = {
  /** Bundled or downloaded pet slug; `null` renders the plain color tile. */
  pet: string | null;
  /** Resolved hex color used as the no-pet look and as the pet sprite's loading/unavailable fallback. */
  colorHex: string;
  size: number;
  pose?: PetPose;
  animate?: boolean;
  className?: string;
};

/** An agent's identity avatar: its pet sprite when it has one, otherwise the existing plain color tile. */
export function AgentAvatar({ pet, colorHex, size, pose, animate, className }: AgentAvatarProps) {
  if (!pet) return <ColorTile size={size} colorHex={colorHex} className={className} />;
  return <PetTile pet={pet} size={size} pose={pose} animate={animate} colorHex={colorHex} />;
}
