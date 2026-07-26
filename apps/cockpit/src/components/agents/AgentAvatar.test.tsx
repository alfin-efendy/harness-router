import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { __resetBundledPetsCacheForTests } from "@/lib/bundled-pets";

// AgentAvatar's pet branch reaches PetSprite (which calls
// `commands.getPetSprite` for non-bundled slugs) — narrow mock per the
// sibling agents/*.test.tsx convention.
type GetPetSpriteResult = { status: "ok"; data: string | null } | { status: "error"; error: { message: string } };
const getPetSprite = mock(async (_slug: string): Promise<GetPetSpriteResult> => ({ status: "ok", data: null }));
mock.module("@/bindings", () => ({ commands: { getPetSprite }, events: {} }));

const { AgentAvatar } = await import("./AgentAvatar");

const originalFetch = globalThis.fetch;

function jsonResponse(body: unknown): Response {
  return { ok: true, json: () => Promise.resolve(body) } as Response;
}

beforeEach(() => {
  __resetBundledPetsCacheForTests();
  getPetSprite.mockClear();
});

afterEach(() => {
  cleanup();
  globalThis.fetch = originalFetch;
});

test("a null pet renders the plain color tile and never touches the bundled-pet fetch", () => {
  const fetchSpy = mock(() => Promise.resolve(jsonResponse([])));
  globalThis.fetch = fetchSpy as unknown as typeof fetch;

  render(<AgentAvatar pet={null} colorHex="#8B5CF6" size={36} />);

  const tile = screen.getByTestId("agent-avatar-color-tile");
  expect(tile.style.backgroundColor).toBe("#8B5CF6");
  expect(tile.style.width).toBe("36px");
  expect(tile.style.height).toBe("36px");
  expect(screen.queryByTestId("pet-sprite")).toBeNull();
  expect(screen.queryByTestId("pet-sprite-fallback")).toBeNull();
  expect(fetchSpy).not.toHaveBeenCalled();
});

test("a bundled pet settles on PetSprite's bundled asset URL once /pets/index.json resolves", async () => {
  globalThis.fetch = mock(() =>
    Promise.resolve(jsonResponse([{ slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." }])),
  ) as unknown as typeof fetch;

  render(<AgentAvatar pet="sprout" colorHex="#8B5CF6" size={32} />);

  // Bundled-ness resolves asynchronously (the same /pets/index.json fetch),
  // so the very first render before it resolves may briefly treat the pet
  // as non-bundled; assert the settled state, not the transient one.
  await waitFor(() => expect(screen.getByTestId("pet-sprite").style.backgroundImage).toContain("/pets/sprout/sprite.webp"));
});

test("a downloaded (non-bundled) pet resolves through commands.getPetSprite", async () => {
  globalThis.fetch = mock(() => Promise.resolve(jsonResponse([]))) as unknown as typeof fetch;
  getPetSprite.mockImplementation(async () => ({ status: "ok" as const, data: "QUJD" }));

  // A slug unique to this file — PetSprite.test.tsx's own data-URL-caching
  // test reserves "custom-pet" for the same purpose, and the resolved data
  // URL is cached at PetSprite.tsx module scope across every test file in
  // the same bun test run, so reusing that slug here would let this test's
  // resolution satisfy that file's `getPetSprite` call-count assertion for
  // free (or vice versa, depending on run order).
  render(<AgentAvatar pet="agent-avatar-fixture-pet" colorHex="#F43F5E" size={32} />);

  const sprite = await screen.findByTestId("pet-sprite");
  expect(sprite.style.backgroundImage).toContain("data:image/webp;base64,QUJD");
  expect(getPetSprite).toHaveBeenCalledWith("agent-avatar-fixture-pet");
});

test("a pet that resolves to nothing (neither bundled nor downloaded) falls back to the color tile", async () => {
  globalThis.fetch = mock(() => Promise.resolve(jsonResponse([]))) as unknown as typeof fetch;
  getPetSprite.mockImplementation(async () => ({ status: "ok" as const, data: null }));

  render(<AgentAvatar pet="avatar-unresolvable-fixture-pet" colorHex="#F43F5E" size={32} />);

  await screen.findByTestId("pet-sprite-fallback");
  expect(screen.getByTestId("pet-sprite-fallback").style.backgroundColor).toBe("#F43F5E");
});
