import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, renderHook, waitFor } from "@testing-library/react";
import { __resetBundledPetsCacheForTests, useBundledPetSlugs, useBundledPets } from "./bundled-pets";

const originalFetch = globalThis.fetch;

function jsonResponse(body: unknown, ok = true): Response {
  return { ok, json: () => Promise.resolve(body) } as Response;
}

beforeEach(() => {
  __resetBundledPetsCacheForTests();
});

afterEach(() => {
  cleanup();
  globalThis.fetch = originalFetch;
});

test("fetches /pets/index.json once and shares the resolved roster across every hook instance", async () => {
  const fetchMock = mock(() => Promise.resolve(jsonResponse([{ slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." }])));
  globalThis.fetch = fetchMock as unknown as typeof fetch;

  const first = renderHook(() => useBundledPets());
  const second = renderHook(() => useBundledPets());

  await waitFor(() => expect(first.result.current).toEqual([{ slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." }]));
  await waitFor(() => expect(second.result.current).toEqual([{ slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." }]));
  expect(fetchMock).toHaveBeenCalledTimes(1);
  expect(fetchMock).toHaveBeenCalledWith("/pets/index.json");

  // A hook mounted after the roster has already resolved reuses the cache
  // synchronously (no extra fetch, no render-then-update flicker needed).
  const third = renderHook(() => useBundledPets());
  expect(third.result.current).toEqual([{ slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." }]);
  expect(fetchMock).toHaveBeenCalledTimes(1);
});

test("useBundledPetSlugs indexes the roster by slug", async () => {
  globalThis.fetch = mock(() =>
    Promise.resolve(
      jsonResponse([
        { slug: "sprout", displayName: "Sprout", submittedBy: null },
        { slug: "boxcat", displayName: "Boxcat", submittedBy: "railly" },
      ]),
    ),
  ) as unknown as typeof fetch;

  const { result } = renderHook(() => useBundledPetSlugs());

  await waitFor(() => expect(result.current.has("sprout")).toBe(true));
  expect(result.current.has("boxcat")).toBe(true);
  expect(result.current.has("crystal")).toBe(false);
});

test("a non-ok response resolves to an empty roster instead of throwing", async () => {
  globalThis.fetch = mock(() => Promise.resolve(jsonResponse(null, false))) as unknown as typeof fetch;

  const { result } = renderHook(() => useBundledPets());

  await waitFor(() => expect(result.current).toEqual([]));
});

test("a rejected fetch resolves to an empty roster instead of throwing", async () => {
  globalThis.fetch = mock(() => Promise.reject(new Error("network unavailable"))) as unknown as typeof fetch;

  const { result } = renderHook(() => useBundledPets());

  await waitFor(() => expect(result.current).toEqual([]));
});
