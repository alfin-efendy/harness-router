// Build the control-plane binary and place it in Cockpit's Tauri sidecar slot.
//
// Tauri resolves `bundle.externalBin: ["binaries/ryuzi"]` by looking for
// `apps/cockpit/src-tauri/binaries/ryuzi-<target-triple>[.exe]` at build time,
// and strips the suffix when installing the file next to the app executable —
// which is exactly what `engine::resolve_control_binary` looks for at runtime.

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const debug = process.argv.includes("--debug");
const profile = debug ? "debug" : "release";
const exeSuffix = process.platform === "win32" ? ".exe" : "";

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

const cargoArgs = ["build", "-p", "ryuzi-control"];
if (!debug) cargoArgs.push("--release");
execFileSync("cargo", cargoArgs, { stdio: "inherit" });

const triple = hostTriple();
const from = join("target", profile, `ryuzi${exeSuffix}`);
const dir = join("apps", "cockpit", "src-tauri", "binaries");
const to = join(dir, `ryuzi-${triple}${exeSuffix}`);

mkdirSync(dir, { recursive: true });
copyFileSync(from, to);
console.log(`staged ${from} -> ${to}`);
