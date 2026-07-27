import { Bot } from "lucide-react";
import { useBundledPetSlugs } from "@/lib/bundled-pets";
import { NEUTRAL_AVATAR_COLOR, type PetPose } from "@/lib/pet-sprite";
import { PetSprite } from "./PetSprite";

// Same "only pay for the bundled-pet lookup when there's a pet" split as
// AgentAvatar's PetTile — see that file's comment for the transient
// bundled-ness caveat, which applies here too.
function PetGlyph({ pet, pose, size, animate }: { pet: string; pose?: PetPose; size: number; animate?: boolean }) {
  const bundledSlugs = useBundledPetSlugs();
  return (
    <PetSprite slug={pet} bundled={bundledSlugs.has(pet)} size={size} pose={pose} animate={animate} fallbackColor={NEUTRAL_AVATAR_COLOR} />
  );
}

export type AgentGlyphProps = {
  /** Bundled or downloaded pet slug; `null` renders the generic Bot icon (today's look, unchanged). */
  pet: string | null;
  pose?: PetPose;
  /** Rendered PetSprite width when `pet` is set. */
  petSize: number;
  /** Bot icon size when `pet` is null. */
  botSize: number;
  botClassName?: string;
  animate?: boolean;
};

/**
 * Bot-icon fallback for surfaces that show an agent's identity but have no
 * plain color tile to fall back to (run cards, the roster, the run detail
 * header, the Home agent picker trigger) — renders the agent's pet, posed,
 * when it has one; otherwise the exact Bot icon these surfaces always showed.
 */
export function AgentGlyph({ pet, pose, petSize, botSize, botClassName, animate }: AgentGlyphProps) {
  if (!pet) return <Bot aria-hidden size={botSize} strokeWidth={2} className={botClassName} />;
  return <PetGlyph pet={pet} pose={pose} size={petSize} animate={animate} />;
}
