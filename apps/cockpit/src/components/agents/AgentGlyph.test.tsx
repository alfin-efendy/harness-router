import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { __resetBundledPetsCacheForTests } from "@/lib/bundled-pets";

type GetPetSpriteResult = { status: "ok"; data: string | null } | { status: "error"; error: { message: string } };
const getPetSprite = mock(async (_slug: string): Promise<GetPetSpriteResult> => ({ status: "ok", data: null }));
mock.module("@/bindings", () => ({ commands: { getPetSprite }, events: {} }));

const { AgentGlyph } = await import("./AgentGlyph");

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

test("a null pet renders the generic Bot icon at botSize, never touching the bundled-pet fetch", () => {
  const fetchSpy = mock(() => Promise.resolve(jsonResponse([])));
  globalThis.fetch = fetchSpy as unknown as typeof fetch;

  render(<AgentGlyph pet={null} petSize={20} botSize={15} botClassName="mt-0.5 shrink-0 text-muted-foreground" />);

  const icon = document.querySelector("svg.lucide-bot");
  expect(icon).toBeTruthy();
  expect(icon?.getAttribute("width")).toBe("15");
  expect(icon?.classList.contains("mt-0.5")).toBe(true);
  expect(screen.queryByTestId("pet-sprite")).toBeNull();
  expect(fetchSpy).not.toHaveBeenCalled();
});

test("a pet renders PetSprite at petSize, posed as requested", async () => {
  globalThis.fetch = mock(() =>
    Promise.resolve(jsonResponse([{ slug: "sprout", displayName: "Sprout", submittedBy: null }])),
  ) as unknown as typeof fetch;

  render(<AgentGlyph pet="sprout" pose="run" petSize={20} botSize={15} />);

  await waitFor(() => expect(screen.getByTestId("pet-sprite").style.backgroundImage).toContain("/pets/sprout/sprite.webp"));
  expect(document.querySelector("svg.lucide-bot")).toBeNull();
});
