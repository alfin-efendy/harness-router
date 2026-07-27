import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { PetManifestEntryInfo } from "@/bindings";
import { __resetBundledPetsCacheForTests } from "@/lib/bundled-pets";

type Result<T> = { status: "ok"; data: T } | { status: "error"; error: { message: string } };

const listPetManifest = mock(async (): Promise<Result<PetManifestEntryInfo[]>> => ({ status: "ok", data: [] }));
const downloadPet = mock(async (_slug: string, _spritesheetUrl: string): Promise<Result<null>> => ({ status: "ok", data: null }));
const getPetSprite = mock(async (_slug: string): Promise<Result<string | null>> => ({ status: "ok", data: null }));
mock.module("@/bindings", () => ({ commands: { listPetManifest, downloadPet, getPetSprite }, events: {} }));

const { PetPicker } = await import("./PetPicker");

const originalFetch = globalThis.fetch;

function jsonResponse(body: unknown): Response {
  return { ok: true, json: () => Promise.resolve(body) } as Response;
}

const bundledRoster = [
  { slug: "sprout", displayName: "Sprout", submittedBy: "Chen W." },
  { slug: "boxcat", displayName: "Boxcat", submittedBy: "railly" },
];

function manifestEntry(overrides: Partial<PetManifestEntryInfo> = {}): PetManifestEntryInfo {
  return {
    slug: "paperclip",
    displayName: "Paperclip",
    kind: "community",
    submittedBy: "Ada",
    spritesheetUrl: "https://assets.petdex.dev/paperclip/sprite.webp",
    ...overrides,
  };
}

beforeEach(() => {
  __resetBundledPetsCacheForTests();
  listPetManifest.mockClear();
  downloadPet.mockClear();
  getPetSprite.mockClear();
  listPetManifest.mockResolvedValue({ status: "ok", data: [] });
  downloadPet.mockResolvedValue({ status: "ok", data: null });
  getPetSprite.mockResolvedValue({ status: "ok", data: null });
  globalThis.fetch = mock(() => Promise.resolve(jsonResponse(bundledRoster))) as unknown as typeof fetch;
});

afterEach(() => {
  cleanup();
  globalThis.fetch = originalFetch;
});

// Every bundled tile mounts a `<PetSprite bundled />`, whose src resolves
// asynchronously even on the (synchronous) bundled path — draining both
// before the test proceeds keeps state updates from landing outside `act()`
// and bleeding into whatever renders next (same idiom as PetSprite.test.tsx).
async function awaitBundledGridReady(): Promise<void> {
  await screen.findByText("Sprout");
  await waitFor(() => expect(screen.getAllByTestId("pet-sprite")).toHaveLength(2));
}

test("renders the bundled grid from /pets/index.json; selecting a tile reports the slug and closes", async () => {
  const onSelect = mock(() => {});
  const onClose = mock(() => {});
  render(<PetPicker open onClose={onClose} onSelect={onSelect} currentPet={null} />);

  await awaitBundledGridReady();
  expect(screen.getByText("Boxcat")).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: /Sprout/i }));
  expect(onSelect).toHaveBeenCalledWith("sprout");
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("search filters the bundled grid", async () => {
  render(<PetPicker open onClose={() => {}} onSelect={() => {}} currentPet={null} />);
  await awaitBundledGridReady();

  fireEvent.change(screen.getByRole("textbox", { name: "Search avatars" }), { target: { value: "box" } });

  expect(screen.getByText("Boxcat")).toBeTruthy();
  expect(screen.queryByText("Sprout")).toBeNull();
});

test("no clear affordance exists even with a current avatar — avatars can be changed, not removed", async () => {
  render(<PetPicker open onClose={() => {}} onSelect={() => {}} currentPet="sprout" />);
  await awaitBundledGridReady();
  expect(screen.queryByRole("button", { name: /Clear/ })).toBeNull();
});

test("Browse petdex.dev is lazy: listPetManifest only fires once expanded", async () => {
  listPetManifest.mockResolvedValue({ status: "ok", data: [manifestEntry()] });
  render(<PetPicker open onClose={() => {}} onSelect={() => {}} currentPet={null} />);
  await awaitBundledGridReady();

  expect(listPetManifest).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Browse petdex.dev" }));

  await screen.findByText("Paperclip");
  expect(listPetManifest).toHaveBeenCalledTimes(1);
  expect(screen.getByText("by Ada")).toBeTruthy();
});

test("search also filters browsed entries, excluding slugs already offered in the bundled grid", async () => {
  listPetManifest.mockResolvedValue({
    status: "ok",
    data: [manifestEntry({ slug: "sprout", displayName: "Sprout" }), manifestEntry({ slug: "paperclip", displayName: "Paperclip" })],
  });
  render(<PetPicker open onClose={() => {}} onSelect={() => {}} currentPet={null} />);
  await awaitBundledGridReady();
  fireEvent.click(screen.getByRole("button", { name: "Browse petdex.dev" }));

  // "sprout" is already bundled — the browse list only ever shows the
  // remaining "paperclip" entry, with no duplicate download-gated Sprout row.
  await waitFor(() => expect(screen.getAllByText("Paperclip")).toHaveLength(1));
  expect(screen.getAllByText("Sprout")).toHaveLength(1); // only the bundled-grid tile

  fireEvent.change(screen.getByRole("textbox", { name: "Search avatars" }), { target: { value: "paper" } });
  expect(screen.getByText("Paperclip")).toBeTruthy();
  expect(screen.queryByText("Sprout")).toBeNull();
});

test("downloading a browsed entry makes it selectable, then reports its slug on click", async () => {
  listPetManifest.mockResolvedValue({ status: "ok", data: [manifestEntry()] });
  const onSelect = mock(() => {});
  const onClose = mock(() => {});
  render(<PetPicker open onClose={onClose} onSelect={onSelect} currentPet={null} />);
  await awaitBundledGridReady();
  fireEvent.click(screen.getByRole("button", { name: "Browse petdex.dev" }));
  await screen.findByText("Paperclip");

  expect(screen.getByRole("button", { name: /Download/i })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /Download/i }));
  await waitFor(() => expect(downloadPet).toHaveBeenCalledWith("paperclip", "https://assets.petdex.dev/paperclip/sprite.webp"));

  await waitFor(() => expect(screen.queryByRole("button", { name: /Download/i })).toBeNull());
  // The now-downloaded entry mounts a non-bundled PetSprite (an async
  // `getPetSprite` resolution) — let it settle before interacting further.
  await screen.findByTestId("pet-sprite-fallback");
  fireEvent.click(screen.getByRole("button", { name: /Paperclip/i }));
  expect(onSelect).toHaveBeenCalledWith("paperclip");
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("a manifest load failure shows the backend detail and a retry affordance", async () => {
  listPetManifest.mockResolvedValueOnce({ status: "error", error: { message: "pet manifest fetch failed: HTTP 502" } });
  render(<PetPicker open onClose={() => {}} onSelect={() => {}} currentPet={null} />);
  await awaitBundledGridReady();
  fireEvent.click(screen.getByRole("button", { name: "Browse petdex.dev" }));

  await screen.findByText("Couldn't load petdex.dev.");
  expect(screen.getByText("pet manifest fetch failed: HTTP 502")).toBeTruthy();

  listPetManifest.mockResolvedValueOnce({ status: "ok", data: [manifestEntry()] });
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await screen.findByText("Paperclip");
  expect(listPetManifest).toHaveBeenCalledTimes(2);
  expect(screen.queryByText("pet manifest fetch failed: HTTP 502")).toBeNull();
});

test("a thrown IPC error also surfaces its message", async () => {
  listPetManifest.mockRejectedValueOnce(new Error("engine transport closed"));
  render(<PetPicker open onClose={() => {}} onSelect={() => {}} currentPet={null} />);
  await awaitBundledGridReady();
  fireEvent.click(screen.getByRole("button", { name: "Browse petdex.dev" }));

  await screen.findByText("Couldn't load petdex.dev.");
  expect(screen.getByText("engine transport closed")).toBeTruthy();
});

test("caps visible browse results and hints at the remaining match count", async () => {
  const entries = Array.from({ length: 45 }, (_, index) => manifestEntry({ slug: `pet-${index}`, displayName: `Pet ${index}` }));
  listPetManifest.mockResolvedValue({ status: "ok", data: entries });
  render(<PetPicker open onClose={() => {}} onSelect={() => {}} currentPet={null} />);
  await awaitBundledGridReady();
  fireEvent.click(screen.getByRole("button", { name: "Browse petdex.dev" }));

  await screen.findByText("Pet 0");
  expect(screen.getAllByText(/^Pet \d+$/)).toHaveLength(40);
  expect(screen.getByText(/Showing first 40 of 45 matches/)).toBeTruthy();
});

test("returns null and closes without a network round trip when closed", () => {
  const onClose = mock(() => {});
  const { container } = render(<PetPicker open={false} onClose={onClose} onSelect={() => {}} currentPet={null} />);
  expect(container.firstChild).toBeNull();
  expect(listPetManifest).not.toHaveBeenCalled();
});
