// Build the control-plane binary and place it in Cockpit's Tauri sidecar slot.
//
// Tauri resolves `bundle.externalBin: ["binaries/ryuzi"]` by looking for
// `apps/cockpit/src-tauri/binaries/ryuzi-<target-triple>[.exe]`, and strips the
// suffix when installing the file next to the app executable — which is exactly
// what `engine::resolve_control_binary` looks for at runtime.
//
// `tauri-build` does that lookup inside `build.rs`, on EVERY compile of
// `ryuzi-cockpit` — dev profile included, not just `tauri build` — and the slot
// is a gitignored build artifact. So every automated path that compiles the
// crate has to run this first: `cockpit:dev`, `cockpit:build`, CI's
// `cockpit-rust` job, and `cockpit-release.yml`'s installer matrix. By hand,
// `bun run cockpit:sidecar --debug` (or `make sidecar`) is what unblocks
// `cargo check/clippy/test -p ryuzi-cockpit`, `cargo gen-bindings`, and a
// whole-workspace `cargo test`.
//
// Usage: bun scripts/cockpit/stage-sidecar.ts [--debug] [--target <triple>]

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";

// Anchor on the repo root, not the caller's cwd: this runs from package.json
// scripts, from CI, and by hand from wherever the operator happens to be.
const root = resolve(import.meta.dir, "..", "..");
const slotDir = join(root, "apps", "cockpit", "src-tauri", "binaries");

const argv = process.argv.slice(2);
const debug = argv.includes("--debug");
const profile = debug ? "debug" : "release";

/** Tauri's virtual macOS target. Not a real rustc triple — it is fused with `lipo`. */
const UNIVERSAL = "universal-apple-darwin";
/** The two real triples a `universal-apple-darwin` bundle is fused from. */
const UNIVERSAL_ARCHES = ["x86_64-apple-darwin", "aarch64-apple-darwin"];

/** `--target <triple>`, spelled like cargo's own flag. Defaults to the host. */
function requestedTriple(): string | undefined {
  const flag = argv.indexOf("--target");
  if (flag === -1) return undefined;
  const value = argv[flag + 1];
  if (!value) throw new Error("--target needs a triple, e.g. --target aarch64-apple-darwin");
  return value;
}

function hostTriple(): string {
  try {
    const printed = execFileSync("rustc", ["--print", "host-tuple"], {
      encoding: "utf8",
    }).trim();
    if (printed) return printed;
  } catch {
    // Older rustc has no --print host-tuple; fall through to -vV.
  }
  const verbose = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  const match = verbose.match(/^host:\s*(\S+)$/m);
  if (!match) throw new Error("could not determine the host target triple from `rustc -vV`");
  return match[1]!;
}

let cachedTargetDir: string | undefined;

/**
 * Ask cargo where it writes artifacts instead of assuming `<root>/target`.
 * Honours `CARGO_TARGET_DIR` and `build.target-dir`, which is what a shared
 * target dir (CI caches) or a RAM-disk build setup uses.
 */
function targetDirectory(): string {
  if (cachedTargetDir) return cachedTargetDir;
  const meta = execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const parsed = JSON.parse(meta) as { target_directory?: string };
  if (!parsed.target_directory) throw new Error("`cargo metadata` reported no target_directory");
  cachedTargetDir = parsed.target_directory;
  return cachedTargetDir;
}

/**
 * Build the control plane for one REAL triple and copy it into the slot.
 * `undefined` means "the host, with no `--target`" — an explicit `--target` is
 * not a no-op even for the host triple, since it changes the output path and
 * disables some config. Returns the staged path.
 */
function stage(triple: string | undefined): string {
  const target = triple ?? hostTriple();
  // The suffix follows the TARGET, not the host: a Windows-hosted cross build
  // still produces a suffix-less binary for a Linux triple, and vice versa.
  const exeSuffix = target.includes("windows") ? ".exe" : "";

  const cargoArgs = ["build", "-p", "ryuzi-control"];
  if (!debug) cargoArgs.push("--release");
  if (triple) cargoArgs.push("--target", triple);
  execFileSync("cargo", cargoArgs, { cwd: root, stdio: "inherit" });

  // `--target` inserts a triple directory; a plain host build does not.
  const from = join(targetDirectory(), ...(triple ? [triple] : []), profile, `ryuzi${exeSuffix}`);
  if (!existsSync(from)) {
    throw new Error(`${from} does not exist — \`cargo ${cargoArgs.join(" ")}\` produced no control-plane binary there`);
  }

  const to = join(slotDir, `ryuzi-${target}${exeSuffix}`);
  mkdirSync(slotDir, { recursive: true });
  copyFileSync(from, to);
  console.log(`staged ${from} -> ${to}`);
  return to;
}

const requested = requestedTriple();
if (requested === UNIVERSAL) {
  if (process.platform !== "darwin") {
    throw new Error(`--target ${UNIVERSAL} only works on macOS — the fat binary is fused with \`lipo\``);
  }
  // A universal bundle needs THREE files in the slot, not one. `tauri build
  // --target universal-apple-darwin` invokes cargo once per real arch, so
  // build.rs sees TARGET=x86_64-apple-darwin and then TARGET=aarch64-apple-darwin
  // and demands a sidecar for each; the bundler afterwards resolves the virtual
  // triple itself and demands a third, fat file. Stage all three.
  const slices = UNIVERSAL_ARCHES.map((arch) => stage(arch));
  const fat = join(slotDir, `ryuzi-${UNIVERSAL}`);
  execFileSync("lipo", ["-create", "-output", fat, ...slices], { stdio: "inherit" });
  console.log(`fused ${slices.join(" + ")} -> ${fat}`);
} else {
  stage(requested);
}
