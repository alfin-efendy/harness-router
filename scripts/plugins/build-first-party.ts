// Usage:
//   bun scripts/plugins/build-first-party.ts            # build + sign every component
//   bun scripts/plugins/build-first-party.ts mimo       # build + sign one component
//   bun scripts/plugins/build-first-party.ts keygen      # generate a dev signing keypair
//
// Reproducibly builds the first-party provider *components* (plugins/mimo,
// plugins/opencode), then emits — per component — the seven release artifacts the
// engine's signed install pipeline fetches (crates/core/src/plugins/remote_catalog.rs
// `install_component_release`):
//
//   <id>.ryuzi-plugin.toml         (the committed manifest, verbatim)
//   <id>.release.json              (a `ryuzi_plugin_sdk::PluginRelease` descriptor)
//   <id>.release.json.sig          (the `plugin.sig` envelope: JSON {key_id, signature})
//   <id>.wasm                      (the compiled component, `component_url` points here)
//   <id>-<version>.ryuzi-plugin.toml (pinned-stem alias, byte-identical)
//   <id>-<version>.release.json    (pinned-stem alias, byte-identical)
//   <id>-<version>.release.json.sig (pinned-stem alias, byte-identical)
//
// Each of the three descriptor files is additionally published under the pinned stem
// `<id>-<version>.*` (byte-identical); the wasm is published once.
//
// The signature is an ed25519 signature over `release.json`'s EXACT raw bytes,
// base64url-no-pad, wrapped in the JSON envelope `plugins::bundle::verify_bundle`
// expects: {"key_id":"first-party","signature":"<b64url>"}. NOTE the encoding
// differs from the catalog feed (`scripts/catalog/build-feed.ts` writes a RAW
// 64-byte detached .sig; here the .sig is a JSON envelope with the signature
// base64url-encoded inside it).
//
// The signing seed comes from the `FIRST_PARTY_PRIVATE_KEY` env var (base64,
// exactly what `keygen` prints) — the private key is NEVER read from or written
// to a committed file. The matching PUBLIC key is pasted into
// crates/core/src/plugins/first_party_key.rs by a human at rollout; this script
// never touches that file.
import { readdir, rm } from "node:fs/promises";
import {
  exportPrivateKeySeedBase64,
  exportPublicKeyRaw,
  generateKeyPair,
  importSigningKeyFromSeedBase64,
  signBytes,
  toRustByteArrayLiteral,
} from "../catalog/ed25519.ts";

/** The `key_id` every first-party `plugin.sig` names — MUST match `first_party_key::FIRST_PARTY_KEY_ID`. */
export const FIRST_PARTY_KEY_ID = "first-party";

/**
 * The `key_id` written into each `plugin.sig`. Defaults to the first-party id
 * CI signs with; a LOCAL dev feed overrides it to `dev`
 * (`first_party_key::DEV_KEY_ID`) so a debug build trusts the bundle through
 * `RYUZI_DEV_PLUGIN_PUBKEY` without the dev key posing as the first-party
 * signer. Read at call time, not module load, so tests and callers can set it.
 */
export function signingKeyId(): string {
  const id = process.env.FIRST_PARTY_KEY_ID?.trim();
  return id !== undefined && id.length > 0 ? id : FIRST_PARTY_KEY_ID;
}

/**
 * Derive the concrete WIT contract version a component was built against from
 * its OWN manifest `[component] wit-api` RANGE. Unlike the manifest's range
 * (e.g. `>=0.1.0, <0.2.0`), a `PluginRelease.wit-api` must be a single semver
 * (`plugins::bundle::PluginRelease::validate`) — the host now supports MORE
 * THAN ONE contract version simultaneously (0.1.0 and the newer tool-carrying
 * 0.2.0), so there is no single shared constant to stamp into every release
 * the way there used to be when the host spoke exactly one ABI.
 *
 * Every `wit-api` range that ships in this repo (`plugins/*\/ryuzi-plugin.toml`)
 * has the shape `>=X.Y.Z, <X.Y'.Z'` — an inclusive lower bound and an
 * exclusive upper bound, produced by the manifest tooling as `>=<minor
 * floor>, <next minor>`. The lower bound IS the concrete version the
 * component was built and tested against (the range only exists to admit
 * future patch/compatible releases of that same contract), so it is the
 * correct `wit-api` to publish. Any other shape (no bound, a `^`/`~`
 * shorthand, an open-ended range, multiple comma-separated clauses beyond the
 * two this parses, ...) is NOT guessed at — it throws, so a manifest range
 * this derivation can't interpret fails the release build loudly instead of
 * silently publishing a wrong version.
 */
export function deriveWitApiVersion(range: string): string {
  // Read the captured lower bound through `?.[1]` and test THAT, rather than
  // testing the match and asserting the group: under `noUncheckedIndexedAccess`
  // an index read is `string | undefined`, and folding both cases into the one
  // throw keeps the loud-failure contract without a non-null assertion.
  const lowerBound = /^>=(\d+\.\d+\.\d+),\s*<\d+\.\d+\.\d+$/.exec(range.trim())?.[1];
  if (lowerBound === undefined) {
    throw new Error(
      `cannot derive a concrete wit-api version from manifest range ${JSON.stringify(range)}: ` +
        `expected the shape ">=X.Y.Z, <X.Y'.Z'" (an inclusive lower bound + exclusive upper bound)`,
    );
  }
  return lowerBound;
}

/**
 * Base URL the seven release artifacts (3 descriptors × 2 stems + 1 wasm —
 * see `artifactNames`) are published under. `component_url` is built as
 * `<base>/<id>.wasm`; the installer's `require_same_origin` check requires
 * the wasm URL to share scheme+host+port with this base, and the 11a default
 * (`DEFAULT_COMPONENT_RELEASE_BASE_URL`) is this same GitHub host, so a
 * same-host asset URL always passes. Override with `FIRST_PARTY_RELEASE_BASE_URL`.
 */
export const DEFAULT_RELEASE_BASE_URL = "https://github.com/alfin-efendy/ryuzi/releases/latest/download";

/** The SDK WIT source the components' `wit/deps/` is materialized from (mirrors `crates/core/tests/fixtures/build-components.sh`). */
const SDK_WIT_DIR = "crates/plugin-sdk/wit";
const WASM_TARGET = "wasm32-wasip2";

/**
 * One first-party release to build + sign. `crateWasmStem` is cargo's output
 * name (crate name with `-`→`_`) — present for a WASM-component plugin,
 * ABSENT for a declarative-only plugin (no `[component]` at all, e.g.
 * `atlassian-rovo`, a remote-MCP-over-HTTP manifest with no wasm). Nothing
 * is built or hashed for a declarative-only entry, and its published
 * `release.json` carries no component fields — the signature over it is
 * still mandatory either way.
 */
export interface ComponentSpec {
  id: string;
  dir: string;
  crateWasmStem?: string;
}

export const COMPONENTS: ComponentSpec[] = [
  { id: "mimo", dir: "plugins/mimo", crateWasmStem: "ryuzi_plugin_mimo" },
  { id: "opencode", dir: "plugins/opencode", crateWasmStem: "ryuzi_plugin_opencode" },
  // OpenAI-chat provider components. All share the `plugins/openai-format`
  // crate (which is NOT a bundle and so is not listed here — it has no manifest
  // and produces no .wasm; it is pulled in as a path dependency of each).
  { id: "openai", dir: "plugins/openai", crateWasmStem: "ryuzi_plugin_openai" },
  { id: "openrouter", dir: "plugins/openrouter", crateWasmStem: "ryuzi_plugin_openrouter" },
  { id: "groq", dir: "plugins/groq", crateWasmStem: "ryuzi_plugin_groq" },
  { id: "deepseek", dir: "plugins/deepseek", crateWasmStem: "ryuzi_plugin_deepseek" },
  { id: "mistral", dir: "plugins/mistral", crateWasmStem: "ryuzi_plugin_mistral" },
  { id: "xai", dir: "plugins/xai", crateWasmStem: "ryuzi_plugin_xai" },
  { id: "nvidia", dir: "plugins/nvidia", crateWasmStem: "ryuzi_plugin_nvidia" },
  { id: "huggingface", dir: "plugins/huggingface", crateWasmStem: "ryuzi_plugin_huggingface" },
  { id: "google", dir: "plugins/google", crateWasmStem: "ryuzi_plugin_google" },
  // Qwen also speaks OpenAI-chat (shares `plugins/openai-format`); its egress is
  // host-managed OAuth (the world imports `ryuzi:oauth`, not provider-auth), but
  // it builds and signs exactly like the other OpenAI-chat components.
  { id: "qwen", dir: "plugins/qwen", crateWasmStem: "ryuzi_plugin_qwen" },
  // Anthropic speaks the Messages wire format, not OpenAI-chat, so its bundle
  // does NOT depend on `plugins/openai-format`; it is built identically all the
  // same.
  { id: "anthropic", dir: "plugins/anthropic", crateWasmStem: "ryuzi_plugin_anthropic" },
  // Anthropic-OAuth speaks the same Messages wire format (shares
  // `plugins/anthropic-format`); its egress is host-managed OAuth rather than an
  // API key, but it builds and signs identically.
  { id: "anthropic-oauth", dir: "plugins/anthropic-oauth", crateWasmStem: "ryuzi_plugin_anthropic_oauth" },
  { id: "github", dir: "plugins/github", crateWasmStem: "ryuzi_plugin_github" },
  { id: "discord", dir: "plugins/discord", crateWasmStem: "ryuzi_plugin_discord" },
  { id: "atlassian", dir: "plugins/atlassian", crateWasmStem: "ryuzi_plugin_atlassian" },
  { id: "bitbucket", dir: "plugins/bitbucket", crateWasmStem: "ryuzi_plugin_bitbucket" },
  // Declarative-only: a remote-MCP-over-HTTP manifest with no `[component]`
  // at all (Task 10 made that genuinely optional end to end) — no
  // `crateWasmStem`, so `processComponent` builds and hashes nothing and
  // publishes a `release.json` with no component fields, signed exactly
  // like every other first-party release.
  { id: "atlassian-rovo", dir: "plugins/atlassian-rovo" },
];

/**
 * The `PluginRelease` JSON shape (crates/plugin-sdk/src/bundle.rs). `wit-api`
 * is kebab in the wire form. The three component fields are OMITTED
 * entirely (not written as `null`/`""`) for a declarative-only release —
 * `crates/plugin-sdk/src/bundle.rs`'s `#[serde(default)]` on all three
 * accepts the keys being absent, and Rust's `PluginRelease::validate()`
 * treats "all three absent" as the legitimate component-less shape.
 */
export interface PluginReleaseJson {
  id: string;
  version: string;
  "wit-api"?: string;
  component_url?: string;
  component_sha256?: string;
  size_bytes?: number;
  published_at?: string;
}

/**
 * The artifact filenames one signed release publishes. The three descriptor
 * files exist under BOTH stems — `<id>.*` (what an unversioned
 * `release_stem` fetch resolves) and `<id>-<version>.*` (what a pinned fetch
 * resolves) — as byte-identical copies of the same signed bytes. The wasm is
 * published ONCE under `component`: both stems' release.json point at it
 * absolutely via `component_url`, so it needs no versioned alias.
 *
 * `component` is omitted for a declarative-only release (no `[component]` at
 * all) — there is no wasm to publish, so the returned list has six names
 * instead of seven.
 */
export function artifactNames(id: string, version: string, component?: string): string[] {
  const names = [`${id}.ryuzi-plugin.toml`, `${id}.release.json`, `${id}.release.json.sig`];
  if (component !== undefined) {
    names.push(component);
  }
  names.push(
    `${id}-${version}.ryuzi-plugin.toml`,
    `${id}-${version}.release.json`,
    `${id}-${version}.release.json.sig`,
  );
  return names;
}

/** Lowercase-hex SHA-256 of `bytes` (matches `plugins::bundle`'s `format!("{:x}", Sha256::digest(..))`). */
export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", bytes as Uint8Array<ArrayBuffer>);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/**
 * Strip the `world plugin { ... }` block from the SDK's `plugin.wit`, leaving
 * its `interface` definitions (types/lifecycle) — wit-bindgen 0.57 can't parse
 * the production world's named imports, but its interfaces remain the canonical
 * contract. A faithful port of `build-components.sh`'s `awk` filter.
 */
export function stripPluginWorld(pluginWit: string): string {
  const out: string[] = [];
  let depth = 0;
  let skipping = false;
  for (const line of pluginWit.split("\n")) {
    if (!skipping && /^world plugin\s*\{/.test(line)) {
      skipping = true;
      depth = 1;
      continue;
    }
    if (skipping) {
      depth += (line.match(/\{/g)?.length ?? 0) - (line.match(/\}/g)?.length ?? 0);
      if (depth === 0) skipping = false;
      continue;
    }
    out.push(line);
  }
  return out.join("\n");
}

/** Materialize `<dir>/wit/deps/` from the SDK (stripped plugin.wit + every dep interface), mirroring `materialize_deps`. */
export async function materializeDeps(dir: string): Promise<void> {
  const depsDir = `${dir}/wit/deps`;
  await rm(depsDir, { recursive: true, force: true });
  await Bun.write(`${depsDir}/plugin.wit`, stripPluginWorld(await Bun.file(`${SDK_WIT_DIR}/plugin.wit`).text()));
  for (const entry of await readdir(`${SDK_WIT_DIR}/deps`)) {
    if (entry.endsWith(".wit")) {
      await Bun.write(`${depsDir}/${entry}`, await Bun.file(`${SDK_WIT_DIR}/deps/${entry}`).arrayBuffer());
    }
  }
}

/** `cargo build --target wasm32-wasip2 --release` in `dir`; throws on a non-zero exit. */
export function buildComponent(dir: string): void {
  const result = Bun.spawnSync(["cargo", "build", "--target", WASM_TARGET, "--release"], {
    cwd: dir,
    stdout: "inherit",
    stderr: "inherit",
  });
  if (result.exitCode !== 0) {
    throw new Error(`cargo build failed for ${dir} (exit ${result.exitCode})`);
  }
}

/**
 * Minimal fields the signer needs from a release's manifest. `component`/
 * `witApiRange` are ABSENT when the manifest declares no `[component]` at
 * all (a declarative-only plugin, e.g. `atlassian-rovo`) — there is no wasm
 * filename or WIT range to read.
 */
interface ManifestFields {
  id: string;
  version: string;
  component?: string;
  /** The `[component] wit-api` RANGE (e.g. `>=0.2.0, <0.3.0`) — see `deriveWitApiVersion`. */
  witApiRange?: string;
}

/**
 * Read + minimally validate `<dir>/ryuzi-plugin.toml`, returning the
 * id/version the release descriptor mirrors, plus component/wit-api-range
 * WHEN the manifest declares a `[component]` table at all — a manifest with
 * no `[component]` (declarative-only) is a valid shape and returns those two
 * fields `undefined` rather than throwing.
 */
export async function readManifest(dir: string): Promise<ManifestFields> {
  const path = `${dir}/ryuzi-plugin.toml`;
  const parsed = Bun.TOML.parse(await Bun.file(path).text()) as Record<string, unknown>;
  const id = parsed.id;
  const version = parsed.version;
  const contract = parsed.contract;
  if (typeof id !== "string" || id.length === 0) throw new Error(`${path}: missing 'id'`);
  if (typeof version !== "string" || version.length === 0) throw new Error(`${path}: missing 'version'`);
  if (contract !== 2) throw new Error(`${path}: 'contract' must be 2`);

  // v2 manifests nest the wasm filename + wit-api range under `[component]`
  // (the top-level `component = "<name>.wasm"` string was a v1-only field —
  // see Task 2/17's manifest-v2 conversion). The table itself is optional —
  // a declarative-only manifest omits it entirely.
  const componentTable = parsed.component as Record<string, unknown> | undefined;
  if (componentTable === undefined) {
    return { id, version };
  }
  const component = componentTable.file;
  const witApiRange = componentTable["wit-api"];
  if (typeof component !== "string" || component.length === 0) throw new Error(`${path}: missing '[component].file'`);
  if (typeof witApiRange !== "string" || witApiRange.length === 0) {
    throw new Error(`${path}: missing '[component].wit-api'`);
  }
  return { id, version, component, witApiRange };
}

/**
 * Assemble the `PluginRelease` object — pure, no I/O. `published_at` is only
 * set when given (omitted keeps release.json byte-reproducible for a given
 * wasm). `witApi` is the CONCRETE version (see `deriveWitApiVersion`), not
 * the manifest's range.
 *
 * `witApi`/`componentUrl`/`sha256`/`sizeBytes` are ALL omitted together for a
 * declarative-only release (no `[component]` at all) — there is no wasm to
 * name a WIT contract, a download URL, a checksum, or a size for. Passing
 * some but not all of them is a caller bug (every call site derives them
 * together from `manifest.component.is_some()`), so this does not attempt to
 * validate that combination itself — `PluginRelease::validate()` on the Rust
 * side is the source of truth for what shapes a release may take.
 */
export function buildReleaseObject(args: {
  id: string;
  version: string;
  witApi?: string;
  componentUrl?: string;
  sha256?: string;
  sizeBytes?: number;
  publishedAt?: string;
}): PluginReleaseJson {
  const release: PluginReleaseJson = {
    id: args.id,
    version: args.version,
  };
  if (args.witApi !== undefined) {
    release["wit-api"] = args.witApi;
  }
  if (args.componentUrl !== undefined) {
    release.component_url = args.componentUrl;
  }
  if (args.sha256 !== undefined) {
    release.component_sha256 = args.sha256;
  }
  if (args.sizeBytes !== undefined) {
    release.size_bytes = args.sizeBytes;
  }
  if (args.publishedAt !== undefined && args.publishedAt !== "") {
    release.published_at = args.publishedAt;
  }
  return release;
}

/**
 * Serialize a release to its EXACT signed-and-published bytes — call ONCE per
 * component. The result is both the bytes signed and the bytes written to
 * `<id>.release.json`; verification is byte-for-byte
 * (`plugins::bundle::verify_bundle`), so these must never diverge.
 */
export function serializeRelease(release: PluginReleaseJson): Uint8Array {
  return new TextEncoder().encode(`${JSON.stringify(release, null, 2)}\n`);
}

/** Base64url without padding (matches Rust's `URL_SAFE_NO_PAD`). */
export function base64UrlNoPad(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("base64url");
}

/** Build the `plugin.sig` envelope over `releaseBytes`: `{"key_id":"first-party","signature":"<b64url ed25519 sig>"}`. */
export async function buildSignatureEnvelope(releaseBytes: Uint8Array, privateKeySeedBase64: string): Promise<string> {
  const signingKey = await importSigningKeyFromSeedBase64(privateKeySeedBase64);
  const signature = await signBytes(releaseBytes, signingKey);
  return `${JSON.stringify({ key_id: signingKeyId(), signature: base64UrlNoPad(signature) }, null, 2)}\n`;
}

/**
 * Build + sign one release, writing its artifacts (3 descriptors × 2 stems,
 * plus 1 wasm when there is a component) into `outDir`. Returns the release
 * descriptor for logging.
 *
 * `spec.crateWasmStem` decides which of the two shapes this release takes:
 * present, this builds+hashes a wasm exactly as before; absent, this is a
 * declarative-only release (e.g. `atlassian-rovo`) — nothing is built, and
 * the published `release.json` carries no component fields. Either way the
 * SAME `buildSignatureEnvelope` call below signs `release.json`'s exact
 * bytes — the signature is never conditional on whether there is a
 * component.
 */
async function processComponent(
  spec: ComponentSpec,
  privateKeySeedBase64: string,
  baseUrl: string,
  outDir: string,
  publishedAt: string | undefined,
): Promise<PluginReleaseJson> {
  const manifest = await readManifest(spec.dir);
  if (manifest.id !== spec.id) {
    throw new Error(`${spec.dir}/ryuzi-plugin.toml declares id ${JSON.stringify(manifest.id)}, expected ${JSON.stringify(spec.id)}`);
  }

  let release: PluginReleaseJson;
  let wasmBytes: Uint8Array | undefined;
  let componentFilename: string | undefined;

  if (spec.crateWasmStem !== undefined) {
    const { component, witApiRange } = manifest;
    if (component === undefined || witApiRange === undefined) {
      throw new Error(
        `${spec.dir}/ryuzi-plugin.toml: COMPONENTS entry ${JSON.stringify(spec.id)} names a crateWasmStem, but the manifest declares no [component]`,
      );
    }

    await materializeDeps(spec.dir);
    buildComponent(spec.dir);

    // CI shares one target dir across all 18 standalone bundle workspaces via
    // CARGO_TARGET_DIR (dep crates compile once, wasm stems never collide);
    // cargo honors it over the per-workspace `target/`, so the output path must too.
    const targetRoot = process.env.CARGO_TARGET_DIR ?? `${spec.dir}/target`;
    const wasmPath = `${targetRoot}/${WASM_TARGET}/release/${spec.crateWasmStem}.wasm`;
    wasmBytes = new Uint8Array(await Bun.file(wasmPath).arrayBuffer());
    componentFilename = component;

    release = buildReleaseObject({
      id: manifest.id,
      version: manifest.version,
      witApi: deriveWitApiVersion(witApiRange),
      componentUrl: `${baseUrl}/${component}`,
      sha256: await sha256Hex(wasmBytes),
      sizeBytes: wasmBytes.byteLength,
      publishedAt,
    });
  } else {
    if (manifest.component !== undefined) {
      throw new Error(
        `${spec.dir}/ryuzi-plugin.toml declares a [component], but COMPONENTS entry ${JSON.stringify(spec.id)} names no crateWasmStem to build it from`,
      );
    }
    release = buildReleaseObject({ id: manifest.id, version: manifest.version, publishedAt });
  }

  const releaseBytes = serializeRelease(release);
  const signatureEnvelope = await buildSignatureEnvelope(releaseBytes, privateKeySeedBase64);

  const manifestBytes = new Uint8Array(await Bun.file(`${spec.dir}/ryuzi-plugin.toml`).arrayBuffer());
  // `artifactNames` always puts the 3 unversioned descriptors first and the 3
  // pinned-stem descriptors last, with the (optional) component name sandwiched
  // between them — so the first/last three positions are stable regardless of
  // whether a component name was passed, and there is no positional footgun to
  // reason about when it is omitted.
  const names = artifactNames(spec.id, manifest.version, componentFilename);
  const [manifestName, releaseName, sigName] = names;
  const [pinnedManifestName, pinnedReleaseName, pinnedSigName] = names.slice(-3);

  await Bun.write(`${outDir}/${manifestName}`, manifestBytes);
  await Bun.write(`${outDir}/${releaseName}`, releaseBytes);
  await Bun.write(`${outDir}/${sigName}`, signatureEnvelope);
  if (componentFilename !== undefined && wasmBytes !== undefined) {
    await Bun.write(`${outDir}/${componentFilename}`, wasmBytes);
  }
  // Pinned-stem aliases: BYTE-IDENTICAL copies of the same signed bytes — the
  // signature is over release.json's exact bytes, so aliasing (not
  // re-serializing) is load-bearing.
  await Bun.write(`${outDir}/${pinnedManifestName}`, manifestBytes);
  await Bun.write(`${outDir}/${pinnedReleaseName}`, releaseBytes);
  await Bun.write(`${outDir}/${pinnedSigName}`, signatureEnvelope);

  return release;
}

/** `keygen` mode: print a dev signing keypair (private base64 seed + Rust pubkey literal). */
async function keygen(): Promise<void> {
  const keyPair = await generateKeyPair();
  const publicKeyRaw = await exportPublicKeyRaw(keyPair.publicKey);
  const privateKeySeedBase64 = await exportPrivateKeySeedBase64(keyPair.privateKey);

  console.log("=== ed25519 keypair for FIRST-PARTY component-bundle signing ===\n");
  console.log("1) Paste into crates/core/src/plugins/first_party_key.rs");
  console.log("   (replaces the all-zero FIRST_PARTY_PUBKEY placeholder — PUBLIC, safe to commit):\n");
  console.log(`    pub const FIRST_PARTY_PUBKEY: [u8; 32] = ${toRustByteArrayLiteral(publicKeyRaw)};\n`);
  console.log("2) Store as the CI secret / local env var FIRST_PARTY_PRIVATE_KEY");
  console.log("   (a gitignored path or the shell env — NEVER commit it):\n");
  console.log(`    ${privateKeySeedBase64}\n`);
  console.log("Run this exactly once per key (or rotation); a second run is an unrelated keypair.\n");
  console.log("--- Local dev feed (DEBUG builds only) ---");
  console.log("To install locally-signed bundles without touching the live first-party key,");
  console.log("sign under the `dev` key id and let the debug build trust this pubkey:\n");
  console.log(`    export FIRST_PARTY_PRIVATE_KEY='${privateKeySeedBase64}'`);
  console.log("    export FIRST_PARTY_KEY_ID=dev");
  console.log(`    export RYUZI_DEV_PLUGIN_PUBKEY='${Buffer.from(publicKeyRaw).toString("base64")}'\n`);
  console.log("A dev-signed bundle installs and runs, but does NOT receive the first-party-only");
  console.log("grants (allow_self_auth / allow_gateway) — those key off the first-party id.");
}

async function main(argv: string[]): Promise<void> {
  if (argv[0] === "keygen") {
    await keygen();
    return;
  }

  const privateKeySeedBase64 = process.env.FIRST_PARTY_PRIVATE_KEY;
  if (!privateKeySeedBase64) {
    console.error(
      "FIRST_PARTY_PRIVATE_KEY is not set. Generate a keypair with " +
        "`bun scripts/plugins/build-first-party.ts keygen`, store the private seed as this env var " +
        "(or the CI secret of the same name), and re-run. Never commit the private key.",
    );
    process.exit(1);
  }

  const baseUrl = (process.env.FIRST_PARTY_RELEASE_BASE_URL ?? DEFAULT_RELEASE_BASE_URL).replace(/\/+$/, "");
  const outDir = process.env.FIRST_PARTY_OUT_DIR ?? "dist/plugins";
  const publishedAt = process.env.FIRST_PARTY_PUBLISHED_AT;

  const requested = argv.filter((a) => !a.startsWith("-"));
  const specs = requested.length > 0 ? COMPONENTS.filter((c) => requested.includes(c.id)) : COMPONENTS;
  if (specs.length === 0) {
    throw new Error(`no matching components for ${JSON.stringify(requested)} (known: ${COMPONENTS.map((c) => c.id).join(", ")})`);
  }

  for (const spec of specs) {
    const release = await processComponent(spec, privateKeySeedBase64, baseUrl, outDir, publishedAt);
    const artifacts =
      spec.crateWasmStem !== undefined
        ? `${spec.id}.{ryuzi-plugin.toml,wasm,release.json,release.json.sig} (sha256 ${release.component_sha256})`
        : `${spec.id}.{ryuzi-plugin.toml,release.json,release.json.sig} (no component)`;
    console.log(
      `signed ${spec.id} ${release.version} -> ${outDir}/${artifacts} ` +
        `+ pinned ${spec.id}-${release.version}.{ryuzi-plugin.toml,release.json,release.json.sig} aliases`,
    );
  }
}

if (import.meta.main) {
  await main(Bun.argv.slice(2));
}
