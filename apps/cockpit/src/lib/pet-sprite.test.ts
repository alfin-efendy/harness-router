import { describe, expect, test } from "bun:test";
import { PET_COLS, PET_FRAME_H, PET_FRAME_W, PET_ROWS, POSE_ROW, type PetPose, poseForRunStatus } from "./pet-sprite";

describe("grid constants", () => {
  test("match the verified spritesheet layout (1536x1872, 192x208 frames, 8x9 grid)", () => {
    expect(PET_FRAME_W).toBe(192);
    expect(PET_FRAME_H).toBe(208);
    expect(PET_COLS).toBe(8);
    expect(PET_ROWS).toBe(9);
    expect(PET_COLS * PET_FRAME_W).toBe(1536);
    expect(PET_ROWS * PET_FRAME_H).toBe(1872);
  });
});

describe("POSE_ROW", () => {
  const poses: PetPose[] = ["idle", "wave", "run", "failed", "review", "jump"];

  test("has an entry for every PetPose", () => {
    for (const pose of poses) {
      expect(POSE_ROW[pose]).toBeDefined();
    }
  });

  test("every row index is a distinct, in-bounds integer", () => {
    const rows = poses.map((pose) => POSE_ROW[pose]);
    for (const row of rows) {
      expect(Number.isInteger(row)).toBe(true);
      expect(row).toBeGreaterThanOrEqual(0);
      expect(row).toBeLessThan(PET_ROWS);
    }
    expect(new Set(rows).size).toBe(rows.length);
  });

  test("matches the documented top-to-bottom README row order", () => {
    expect(POSE_ROW.idle).toBe(0);
    expect(POSE_ROW.wave).toBe(1);
    expect(POSE_ROW.run).toBe(2);
    expect(POSE_ROW.failed).toBe(3);
    expect(POSE_ROW.review).toBe(4);
    expect(POSE_ROW.jump).toBe(5);
  });
});

describe("poseForRunStatus", () => {
  test("running -> run", () => {
    expect(poseForRunStatus("running")).toBe("run");
  });

  test("failed -> failed", () => {
    expect(poseForRunStatus("failed")).toBe("failed");
  });

  test("completed -> jump", () => {
    expect(poseForRunStatus("completed")).toBe("jump");
  });

  test("queued -> idle", () => {
    expect(poseForRunStatus("queued")).toBe("idle");
  });

  test("cancelled -> idle", () => {
    expect(poseForRunStatus("cancelled")).toBe("idle");
  });

  test("interrupted -> idle", () => {
    expect(poseForRunStatus("interrupted")).toBe("idle");
  });

  test("unknown/other statuses fall back to idle", () => {
    expect(poseForRunStatus("some-future-status")).toBe("idle");
    expect(poseForRunStatus("")).toBe("idle");
  });

  test("every returned pose is a valid PetPose with a POSE_ROW entry", () => {
    const statuses = ["running", "failed", "completed", "queued", "cancelled", "interrupted", "unknown"];
    for (const status of statuses) {
      const pose = poseForRunStatus(status);
      expect(POSE_ROW[pose]).toBeDefined();
    }
  });
});
