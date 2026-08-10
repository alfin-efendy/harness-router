import { expect, test } from "bun:test";
import { readdir } from "node:fs/promises";
import { join } from "node:path";
import {
  artifactNames,
  buildSignatureEnvelope,
  COMPONENTS,
  deriveWitApiVersion,
  FIRST_PARTY_KEY_ID,
  readManifest,
  signingKeyId,
} from "./build-first-party.ts";

// Repo root relative to this file (scripts/plugins/), so the test is
// cwd-independent — it resolves plugins/ the same way regardless of where
// `bun test` is invoked from.
const REPO_ROOT = join(import.meta.dir, "..", "..");
const PLUGINS_DIR = join(REPO_ROOT, "plugins");

/**
 * Every `plugins/<id>/` that ships a `ryuzi-plugin.toml` is a first-party
 * component the release pipeline MUST build + sign. Sibling `plugins/*` dirs
 * without a manifest (the shared `openai-format` / `anthropic-format` wire
 * crates) are path dependencies, not bundles, so they are excluded here.
 */
async function shippedComponentIds(): Promise<string[]> {
  const entries = await readdir(PLUGINS_DIR, { withFileTypes: true });
  const ids: string[] = [];
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    if (await Bun.file(join(PLUGINS_DIR, entry.name, "ryuzi-plugin.toml")).exists()) {
      ids.push(entry.name);
    }
  }
  return ids.sort();
}

// The drift-guard. `build-first-party.ts`'s COMPONENTS list must cover EVERY
// shipped component. Without this, a new provider component added under
// `plugins/` would silently never be built or signed by the release pipeline —
// the exact gap that shipped `anthropic-oauth` and `qwen` unbuilt. It fails in
// BOTH directions: a missing entry (drift the guard exists to catch) and a
// stale entry whose `plugins/<id>/` manifest was removed.
test("COMPONENTS covers every shipped plugins/<id>/ that has a ryuzi-plugin.toml", async () => {
  const shipped = await shippedComponentIds();
  const listed = COMPONENTS.map((c) => c.id).sort();

  const missing = shipped.filter((id) => !listed.includes(id));
  const stale = listed.filter((id) => !shipped.includes(id));

  expect(missing).toEqual([]); // shipped component the release pipeline would never build/sign
  expect(stale).toEqual([]); // COMPONENTS entry whose plugins/<id>/ manifest no longer exists
});

// Each COMPONENTS entry must point at a real dir whose manifest declares the
// SAME id. `processComponent` already enforces this at build time; asserting it
// here makes a typo fail `bun test`, not a live release.
test("each COMPONENTS entry's dir exists and its manifest id matches", async () => {
  for (const spec of COMPONENTS) {
    const manifest = Bun.file(join(REPO_ROOT, spec.dir, "ryuzi-plugin.toml"));
    expect(await manifest.exists()).toBe(true);
    const parsed = Bun.TOML.parse(await manifest.text()) as { id?: unknown };
    expect(parsed.id).toBe(spec.id);
  }
});

// `crateWasmStem` is cargo's wasm output filename: the crate's `[package] name`
// with `-` -> `_`. A mismatch makes `processComponent` read a nonexistent
// `<stem>.wasm` and the release fails — so pin it to each crate's real name.
test("each COMPONENTS entry's crateWasmStem matches its crate's [package] name", async () => {
  for (const spec of COMPONENTS) {
    const parsed = Bun.TOML.parse(await Bun.file(join(REPO_ROOT, spec.dir, "Cargo.toml")).text()) as {
      package?: { name?: unknown };
    };
    const crateName = parsed.package?.name;
    expect(typeof crateName).toBe("string");
    expect(spec.crateWasmStem).toBe((crateName as string).replaceAll("-", "_"));
  }
});

// Every shipped bundle manifest must be manifest-v2 (`contract = 2`, wasm
// filename under `[component].file`) — `readManifest` is the release
// pipeline's own reader, so a passing call here means the same manifest will
// parse when `build-first-party.ts` actually signs a release. Catches a
// manifest left on v1 (top-level `contract`/`component` string) before it
// reaches CI's signing step.
test("readManifest accepts every shipped plugins/<id>/ryuzi-plugin.toml as contract 2", async () => {
  for (const spec of COMPONENTS) {
    const manifest = await readManifest(join(REPO_ROOT, spec.dir));
    expect(manifest.id).toBe(spec.id);
    expect(manifest.component.length).toBeGreaterThan(0);
    expect(manifest.witApiRange.length).toBeGreaterThan(0);
  }
});

// The published `release.json` `wit-api` is derived from EACH component's own
// manifest range (Defect 2 fix) rather than one shared constant, since the
// host now supports two WIT contract versions simultaneously (0.1.0 and the
// newer tool-carrying 0.2.0) and different first-party components target
// different ones. This is the drift-guard: every shipped manifest's actual
// `wit-api` range must be a shape `deriveWitApiVersion` can interpret,
// otherwise a real release build would throw.
test("deriveWitApiVersion interprets every shipped component's manifest wit-api range", async () => {
  for (const spec of COMPONENTS) {
    const manifest = await readManifest(join(REPO_ROOT, spec.dir));
    expect(() => deriveWitApiVersion(manifest.witApiRange)).not.toThrow();
  }
});

// The ten provider components that moved to the tool-carrying interface
// publish `0.2.0`; the rest (mimo, opencode, connectors) still publish
// `0.1.0` — both paths must derive correctly, not just whichever one this
// script happened to hardcode before.
test("deriveWitApiVersion returns the range's lower bound for both host-supported shapes", () => {
  expect(deriveWitApiVersion(">=0.1.0, <0.2.0")).toBe("0.1.0");
  expect(deriveWitApiVersion(">=0.2.0, <0.3.0")).toBe("0.2.0");
  // Tolerate the lack of a space after the comma too.
  expect(deriveWitApiVersion(">=0.1.0,<0.2.0")).toBe("0.1.0");
});

// A range shape this derivation cannot interpret must fail loudly (throw) at
// build time rather than silently guessing a wrong `wit-api` — the whole
// point of Defect 2's fix over the old hardcoded constant.
test("deriveWitApiVersion throws on a range shape it cannot interpret", () => {
  expect(() => deriveWitApiVersion("^0.1.0")).toThrow();
  expect(() => deriveWitApiVersion("*")).toThrow();
  expect(() => deriveWitApiVersion(">=0.1.0")).toThrow();
  expect(() => deriveWitApiVersion("0.1.0")).toThrow();
  expect(() => deriveWitApiVersion("")).toThrow();
});

// The published-name contract: 3 descriptor files under BOTH stems (unversioned
// for latest installs, `<id>-<version>` for pinned `release_stem` fetches) and
// the wasm exactly ONCE — `component_url` names it absolutely, so a versioned
// wasm alias would be dead weight (and MBs of duplicate release assets).
test("artifactNames publishes descriptors under both stems and the wasm once", () => {
  expect(artifactNames("github", "0.1.1", "github.wasm")).toEqual([
    "github.ryuzi-plugin.toml",
    "github.release.json",
    "github.release.json.sig",
    "github.wasm",
    "github-0.1.1.ryuzi-plugin.toml",
    "github-0.1.1.release.json",
    "github-0.1.1.release.json.sig",
  ]);
});

// The default MUST stay `first-party` — CI signs real releases with this id and
// `first_party_key::FIRST_PARTY_KEY_ID` is what the installer looks up. A local
// dev feed opts out via FIRST_PARTY_KEY_ID=dev so the dev key can never pose as
// the first-party signer.
test("signingKeyId defaults to first-party and honors the dev override", async () => {
  const previous = process.env.FIRST_PARTY_KEY_ID;
  try {
    delete process.env.FIRST_PARTY_KEY_ID;
    expect(signingKeyId()).toBe(FIRST_PARTY_KEY_ID);
    expect(signingKeyId()).toBe("first-party");

    // Blank / whitespace is not an override — it falls back, so an exported-
    // but-empty shell var can't silently produce an unverifiable `""` key id.
    process.env.FIRST_PARTY_KEY_ID = "   ";
    expect(signingKeyId()).toBe("first-party");

    process.env.FIRST_PARTY_KEY_ID = "dev";
    expect(signingKeyId()).toBe("dev");

    // The id must actually reach the envelope the installer reads.
    const seed = Buffer.alloc(32, 3).toString("base64");
    const envelope = JSON.parse(await buildSignatureEnvelope(new TextEncoder().encode("{}\n"), seed));
    expect(envelope.key_id).toBe("dev");
  } finally {
    if (previous === undefined) delete process.env.FIRST_PARTY_KEY_ID;
    else process.env.FIRST_PARTY_KEY_ID = previous;
  }
});
