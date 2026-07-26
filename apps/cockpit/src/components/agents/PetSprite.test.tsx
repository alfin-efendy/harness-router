import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { PET_COLS, PET_FRAME_H, PET_FRAME_W, PET_ROWS, POSE_FRAMES, POSE_ROW } from "@/lib/pet-sprite";

// Only `commands.getPetSprite` is exercised by PetSprite.tsx, so that's all
// this file's mock needs to provide — see the sibling agents/*.test.tsx
// files for the same narrow-mock convention. mock.module leaks process-wide
// across the whole bun test run (known repo issue), so this must be
// verified both standalone and alongside the rest of apps/cockpit/src/components/agents.
type GetPetSpriteResult = { status: "ok"; data: string | null } | { status: "error"; error: { message: string } };
const getPetSprite = mock(async (_slug: string): Promise<GetPetSpriteResult> => ({ status: "ok", data: null }));
mock.module("@/bindings", () => ({ commands: { getPetSprite }, events: {} }));

const { PetSprite } = await import("./PetSprite");

const SIZE = 96;
const SCALE = SIZE / PET_FRAME_W;
const FRAME_HEIGHT_PX = PET_FRAME_H * SCALE;

function expectedBackgroundSize(size: number): string {
  const scale = size / PET_FRAME_W;
  return `${PET_COLS * size}px ${PET_ROWS * PET_FRAME_H * scale}px`;
}

beforeEach(() => {
  getPetSprite.mockClear();
  getPetSprite.mockImplementation(async () => ({ status: "ok" as const, data: null }));
});
afterEach(() => {
  cleanup();
});

test("renders the fallback color tile before the sprite resolves", async () => {
  render(<PetSprite slug="sprout" bundled size={SIZE} fallbackColor="#8B5CF6" />);

  // Assert synchronously, before the resolveSrc() microtask has a chance to
  // settle — this is the pre-resolution render the test is about.
  const fallback = screen.getByTestId("pet-sprite-fallback");
  expect(fallback.style.backgroundColor).toBe("#8B5CF6");
  expect(fallback.className).toContain("rounded-lg");
  expect(screen.queryByTestId("pet-sprite")).toBeNull();

  // Drain the pending resolution inside `act()` so it doesn't land as a
  // stray update once the next test is running.
  await screen.findByTestId("pet-sprite");
});

test("bundled sprite resolves to the /pets/<slug>/sprite.webp background image", async () => {
  render(<PetSprite slug="sprout" bundled size={SIZE} fallbackColor="#8B5CF6" />);

  const sprite = await screen.findByTestId("pet-sprite");
  expect(sprite.style.backgroundImage).toContain("/pets/sprout/sprite.webp");
  expect(sprite.style.backgroundSize).toBe(expectedBackgroundSize(SIZE));
  expect(screen.queryByTestId("pet-sprite-fallback")).toBeNull();
});

test("pose selects the background-position-y offset using the exported frame constants", async () => {
  render(<PetSprite slug="sprout" bundled size={SIZE} pose="review" fallbackColor="#8B5CF6" />);

  const sprite = await screen.findByTestId("pet-sprite");
  const expectedY = -(POSE_ROW.review * FRAME_HEIGHT_PX);
  expect(sprite.style.backgroundPositionY).toBe(`${expectedY}px`);
});

test("a different pose yields a different background-position-y, still on the constants' grid", async () => {
  render(<PetSprite slug="sprout" bundled size={SIZE} pose="jump" fallbackColor="#8B5CF6" />);

  const sprite = await screen.findByTestId("pet-sprite");
  const expectedY = -(POSE_ROW.jump * FRAME_HEIGHT_PX);
  expect(sprite.style.backgroundPositionY).toBe(`${expectedY}px`);
});

test("animate=true applies a stepped keyframe animation over the pose's own frame count, not PET_COLS", async () => {
  // Default pose is "idle" (6 frames), which is narrower than PET_COLS (8)
  // — pinning this to POSE_FRAMES.idle (not the fixed grid width) catches a
  // regression back to animating past a short row's blank padding columns.
  render(<PetSprite slug="sprout" bundled size={SIZE} fallbackColor="#8B5CF6" />);

  const sprite = await screen.findByTestId("pet-sprite");
  expect(sprite.style.animation).toContain(`steps(${POSE_FRAMES.idle})`);
});

test("a pose with a different frame count steps a different number of times", async () => {
  render(<PetSprite slug="sprout" bundled size={SIZE} pose="run" fallbackColor="#8B5CF6" />);

  const sprite = await screen.findByTestId("pet-sprite");
  expect(sprite.style.animation).toContain(`steps(${POSE_FRAMES.run})`);
  expect(POSE_FRAMES.run).not.toBe(POSE_FRAMES.idle);
});

test("the keyframe end offset custom property is the pose's frame count times the rendered frame width, not the full row width", async () => {
  render(<PetSprite slug="sprout" bundled size={SIZE} pose="wave" fallbackColor="#8B5CF6" />);

  const sprite = await screen.findByTestId("pet-sprite");
  expect(sprite.style.getPropertyValue("--pet-sprite-total-w")).toBe(`${POSE_FRAMES.wave * SIZE}px`);
  // Sanity: wave's row is shorter than the full PET_COLS-wide grid, so this
  // must differ from a PET_COLS-based offset.
  expect(POSE_FRAMES.wave * SIZE).not.toBe(PET_COLS * SIZE);
});

test("animate=false yields no animation style", async () => {
  render(<PetSprite slug="sprout" bundled size={SIZE} animate={false} fallbackColor="#8B5CF6" />);

  const sprite = await screen.findByTestId("pet-sprite");
  expect(sprite.style.animation).toBe("");
});

test("prefers-reduced-motion disables the animation even when animate defaults to true", async () => {
  const original = window.matchMedia;
  window.matchMedia = mock((query: string) => ({
    matches: query.includes("reduce"),
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  }));
  try {
    render(<PetSprite slug="sprout" bundled size={SIZE} fallbackColor="#8B5CF6" />);
    const sprite = await screen.findByTestId("pet-sprite");
    expect(sprite.style.animation).toBe("");
  } finally {
    window.matchMedia = original;
  }
});

test("a missing window.matchMedia is treated as no motion preference (animates normally)", async () => {
  const original = window.matchMedia;
  // @ts-expect-error -- test-only deletion of the global
  window.matchMedia = undefined;
  try {
    render(<PetSprite slug="sprout" bundled size={SIZE} fallbackColor="#8B5CF6" />);
    const sprite = await screen.findByTestId("pet-sprite");
    expect(sprite.style.animation).toContain(`steps(${POSE_FRAMES.idle})`);
  } finally {
    window.matchMedia = original;
  }
});

test("an unknown slug (getPetSprite resolves null) falls back to the color tile", async () => {
  getPetSprite.mockImplementation(async () => ({ status: "ok" as const, data: null }));
  render(<PetSprite slug="ghost-pet" bundled={false} size={SIZE} fallbackColor="#F43F5E" />);

  await waitFor(() => expect(getPetSprite).toHaveBeenCalledWith("ghost-pet"));
  expect(screen.getByTestId("pet-sprite-fallback")).toBeTruthy();
  expect(screen.queryByTestId("pet-sprite")).toBeNull();
});

test("a failed fetch (getPetSprite rejects) falls back to the color tile", async () => {
  getPetSprite.mockImplementation(async () => {
    throw new Error("network error");
  });
  render(<PetSprite slug="broken-pet" bundled={false} size={SIZE} fallbackColor="#F43F5E" />);

  await waitFor(() => expect(getPetSprite).toHaveBeenCalledWith("broken-pet"));
  expect(screen.getByTestId("pet-sprite-fallback")).toBeTruthy();
  expect(screen.queryByTestId("pet-sprite")).toBeNull();
});

test("a command-level error result falls back to the color tile", async () => {
  getPetSprite.mockImplementation(async () => ({ status: "error" as const, error: { message: "boom" } }));
  render(<PetSprite slug="broken-pet-2" bundled={false} size={SIZE} fallbackColor="#F43F5E" />);

  await waitFor(() => expect(getPetSprite).toHaveBeenCalledWith("broken-pet-2"));
  expect(screen.getByTestId("pet-sprite-fallback")).toBeTruthy();
  expect(screen.queryByTestId("pet-sprite")).toBeNull();
});

test("non-bundled sprites build a data URL from the base64 payload and cache it across mounts", async () => {
  getPetSprite.mockImplementation(async () => ({ status: "ok" as const, data: "QUJD" }));
  const { unmount } = render(<PetSprite slug="custom-pet" bundled={false} size={SIZE} fallbackColor="#F43F5E" />);

  const sprite = await screen.findByTestId("pet-sprite");
  expect(sprite.style.backgroundImage).toContain("data:image/webp;base64,QUJD");
  expect(getPetSprite).toHaveBeenCalledTimes(1);
  unmount();

  render(<PetSprite slug="custom-pet" bundled={false} size={SIZE} fallbackColor="#F43F5E" />);
  await screen.findByTestId("pet-sprite");
  // Second mount for the same slug reuses the module-level cache instead of
  // calling the command again.
  expect(getPetSprite).toHaveBeenCalledTimes(1);
});
