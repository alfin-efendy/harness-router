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
// crate has to run this first: `cockpit:dev`, `cockpit:build`, and CI's
// `cockpit-rust` job. By hand, `bun run cockpit:sidecar --debug` is enough to
// unblock `cargo check/clippy/test -p ryuzi-cockpit` and `cargo gen-bindings`.
//
// Usage: bun scripts/cockpit/stage-sidecar.ts [--debug] [--target <triple>]

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";

// Anchor on the repo root, not the caller's cwd: this runs from package.json
// scripts, from CI, and by hand from wherever the operator happens to be.
const root = resolve(import.meta.dir, "..", "..");

const argv = process.argv.slice(2);
const debug = argv.includes("--debug");
const profile = debug ? "debug" : "release";

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

/**
 * Ask cargo where it writes artifacts instead of assuming `<root>/target`.
 * Honours `CARGO_TARGET_DIR` and `build.target-dir`, which is what a shared
 * target dir (CI caches) or a RAM-disk build setup uses.
 */
function targetDirectory(): string {
  const meta = execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const parsed = JSON.parse(meta) as { target_directory?: string };
  if (!parsed.target_directory) throw new Error("`cargo metadata` reported no target_directory");
  return parsed.target_directory;
}

const explicitTriple = requestedTriple();
const triple = explicitTriple ?? hostTriple();
// The suffix follows the TARGET, not the host: a Windows-hosted cross build
// still produces a suffix-less binary for a Linux triple, and vice versa.
const exeSuffix = triple.includes("windows") ? ".exe" : "";

const cargoArgs = ["build", "-p", "ryuzi-control"];
if (!debug) cargoArgs.push("--release");
// Only pass --target when asked: an explicit --target is not a no-op even for
// the host triple (it changes the output path and disables some config).
if (explicitTriple) cargoArgs.push("--target", explicitTriple);
execFileSync("cargo", cargoArgs, { cwd: root, stdio: "inherit" });

// `--target` inserts a triple directory; a plain host build does not.
const from = join(targetDirectory(), ...(explicitTriple ? [explicitTriple] : []), profile, `ryuzi${exeSuffix}`);
if (!existsSync(from)) {
  throw new Error(`${from} does not exist — \`cargo ${cargoArgs.join(" ")}\` produced no control-plane binary there`);
}

const dir = join(root, "apps", "cockpit", "src-tauri", "binaries");
const to = join(dir, `ryuzi-${triple}${exeSuffix}`);

mkdirSync(dir, { recursive: true });
copyFileSync(from, to);
console.log(`staged ${from} -> ${to}`);
