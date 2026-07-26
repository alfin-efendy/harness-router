// Bundled-pet spritesheet constants + pose math. Pure TS, no React.
//
// Sourcing/attribution for the bundled sheets under `public/pets/` lives in
// `public/pets/THIRD_PARTY_PETS.md`. The bundled roster is fetched at runtime
// from `/pets/index.json` (shape: BundledPet[]).
//
// Grid convention — verified empirically (2026-07-26) against every bundled
// sheet: each `public/pets/<slug>/sprite.webp` is a 1536x1872px WebP image
// laid out as an 8-column x 9-row grid of 192x208px frames. Per the petdex
// README, rows run top-to-bottom as: idle, wave, run, failed, review, jump,
// then 3 unused "extra" rows. See `.superpowers/sdd/task-3-report.md` for the
// per-sheet measurements and the manual eyeball check of the run row.

export const PET_FRAME_W = 192;
export const PET_FRAME_H = 208;
export const PET_COLS = 8;
export const PET_ROWS = 9;

export type PetPose = "idle" | "wave" | "run" | "failed" | "review" | "jump";

// Row index (0-based, top-to-bottom) for each pose, per the README convention above.
export const POSE_ROW: Record<PetPose, number> = {
  idle: 0,
  wave: 1,
  run: 2,
  failed: 3,
  review: 4,
  jump: 5,
};

/**
 * Maps an agent run status to the pet pose that should be shown for it.
 * running -> run, failed -> failed, completed -> jump; queued, cancelled,
 * interrupted, and any other/unknown status fall back to idle.
 */
export function poseForRunStatus(status: string): PetPose {
  switch (status) {
    case "running":
      return "run";
    case "failed":
      return "failed";
    case "completed":
      return "jump";
    default:
      return "idle";
  }
}

/** One entry of `public/pets/index.json`, fetched at runtime from "/pets/index.json". */
export interface BundledPet {
  slug: string;
  displayName: string;
  submittedBy: string | null;
}

/** Slug of the bundled pet assigned to the Fresh Agent's default avatar. */
export const FRESH_AGENT_PET = "sprout";
