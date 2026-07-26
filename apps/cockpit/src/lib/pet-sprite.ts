// Bundled-pet spritesheet constants + pose math. Pure TS, no React.
//
// Sourcing/attribution for the bundled sheets under `public/pets/` lives in
// `public/pets/THIRD_PARTY_PETS.md`. The bundled roster is fetched at runtime
// from `/pets/index.json` (shape: BundledPet[]).
//
// Grid convention — the petdex README's stated row order (idle, wave, run,
// failed, review, jump) does NOT match the actual bundled art. Re-verified
// empirically (2026-07-26) by eyeballing every bundled sheet
// (sprout/boxcat/crystal in full, cloudlet/tennis-ball/docket-pin spot
// checked): each `public/pets/<slug>/sprite.webp` is a 1536x1872px WebP
// image laid out as an 8-column x 9-row grid of 192x208px frames, but each
// row only fills its own number of leading columns — the rest of the row is
// transparent padding out to column 8. Real top-to-bottom row content:
//   row 0: idle (6 frames, gentle blink/breathe loop)
//   row 1: walk (8 frames) — no PetPose maps here, unused
//   row 2: run (8 frames, brisker walk-style gait)
//   row 3: wave (4 frames, one arm/paw raised into a wave)
//   row 4: jump/cheer (5 frames, arms-up cheering)
//   row 5: sad/crying (8 frames, tears) — mapped to the "failed" pose
//   rows 6-8: three unused "extra" rows (6 frames each), not a single
//     coherent designed animation — see the `review` note below.
// See `.superpowers/sdd/final-fix-report.md` for the per-sheet crops used to
// re-derive this table.

export const PET_FRAME_W = 192;
export const PET_FRAME_H = 208;
export const PET_COLS = 8;
export const PET_ROWS = 9;

export type PetPose = "idle" | "wave" | "run" | "failed" | "review" | "jump";

// Row index (0-based, top-to-bottom) for each pose, per the real layout
// documented above (NOT the petdex README's row order, which is wrong).
export const POSE_ROW: Record<PetPose, number> = {
  idle: 0,
  run: 2,
  wave: 3,
  jump: 4,
  failed: 5,
  // Rows 6-8 are leftover "extra" frames, not a single designed pose — none
  // of the three reads cleanly as "reviewing". Row 8 was picked as the
  // least-wrong fit: across the inspected sheets it's the row that leans
  // toward a quiet, focused expression (e.g. tennis-ball's hand-to-face,
  // narrowed-eyes frame; cloudlet's closed-eyes paws-pressed-together
  // frame; boxcat's focused paw-licking frame), whereas rows 6-7 read as
  // uniformly relaxed/happy (or, on sprout, plainly annoyed) with nothing
  // resembling contemplation.
  review: 8,
};

// Frame count (non-blank columns) per pose's row — rows are NOT all 8
// columns wide; short rows leave the remaining columns blank. Verified
// against the same sheets as POSE_ROW above.
export const POSE_FRAMES: Record<PetPose, number> = {
  idle: 6,
  run: 8,
  wave: 4,
  jump: 5,
  failed: 8,
  review: 6,
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
