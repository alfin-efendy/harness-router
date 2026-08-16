import { expect, test } from "bun:test";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";

const root = process.cwd();

const skippedDirs = new Set([
  ".git",
  ".agents",
  ".claude",
  ".cache",
  ".superpowers",
  "coverage",
  "dist",
  "logs",
  "node_modules",
  "out",
  "target",
  "target-clippy",
  "target-plugins",
]);

// Local-only working artifacts (all gitignored) plus the released changelog,
// which legitimately records the pre-rename package name in its history.
const skippedSubtrees = new Set(["docs/design", "docs/plans", "docs/superpowers"]);
const skippedFiles = new Set(["crates/control/CHANGELOG.md", "scripts/test/architecture-invariants.test.ts"]);

const textExtensions = new Set([".json", ".lock", ".md", ".rs", ".sh", ".toml", ".ts", ".tsx", ".yaml", ".yml"]);
const textFilenames = new Set(["Dockerfile", "Makefile"]);

function walk(dir: string, out: string[]): string[] {
  for (const entry of readdirSync(dir)) {
    const abs = join(dir, entry);
    const rel = relative(root, abs).split(sep).join("/");
    let isDir = false;
    try {
      isDir = statSync(abs).isDirectory();
    } catch {
      // Device names (this repo has a stray `nul` at the root) and broken
      // symlinks are not source files; skip rather than fail the whole walk.
      continue;
    }
    if (isDir) {
      if (skippedDirs.has(entry) || skippedSubtrees.has(rel)) continue;
      walk(abs, out);
      continue;
    }
    if (skippedFiles.has(rel)) continue;
    const dot = entry.lastIndexOf(".");
    const ext = dot === -1 ? "" : entry.slice(dot);
    if (!textExtensions.has(ext) && !textFilenames.has(entry)) continue;
    out.push(rel);
  }
  return out;
}

// SCOPE: the three `pre-S1` entries below exist only to prove the S1 rename
// sweep was complete. S2 reintroduces ALL THREE names for the new thin
// executor — `crates/runner`, package `ryuzi-runner`, lib `ryuzi_runner` — so
// S2's first step is to delete exactly those three entries. Entries added by
// later tasks (Task 4 adds the retired Cockpit daemon mode) are permanent
// invariants and must survive.
const retired: Array<[string, RegExp]> = [
  ["pre-S1 cargo package name", /ryuzi-runner/],
  ["pre-S1 crate lib name", /ryuzi_runner/],
  ["pre-S1 crate path", /crates\/runner/],
  ["retired in-process cockpit daemon mode", /--engine-daemon/],
  ["retired cockpit daemon module", /engine_daemon/],
];

test("no file references a pre-S1 architecture element", () => {
  const offenders: string[] = [];
  for (const rel of walk(root, [])) {
    const text = readFileSync(join(root, rel), "utf8");
    for (const [label, pattern] of retired) {
      if (pattern.test(text)) offenders.push(`${rel}: ${label}`);
    }
  }
  expect(offenders).toEqual([]);
});

// Constants two crates must spell identically, with nothing at build time
// linking them. A typo on either side fails silently and in the direction that
// looks fine (the daemon just keeps its self-updater on under Cockpit — the
// exact drift S1 exists to prevent), so pin the real call sites, not a bare
// substring: prose mentioning the name in a comment must not satisfy the
// guard.
const pairedConstants: Array<[string, Array<[string, RegExp]>]> = [
  [
    "RYUZI_MANAGED_HOST: Cockpit sets it on the control-plane spawn, the control plane reads it to disable self-update",
    [
      ["apps/cockpit/src-tauri/src/engine.rs", /\.env\("RYUZI_MANAGED_HOST",\s*"1"\)/],
      ["crates/control/src/daemon_cmd.rs", /env::var\("RYUZI_MANAGED_HOST"\)/],
    ],
  ],
];

test("both sides of a cross-crate string constant spell it the same way", () => {
  const missing: string[] = [];
  for (const [label, sides] of pairedConstants) {
    for (const [rel, pattern] of sides) {
      if (!pattern.test(readFileSync(join(root, rel), "utf8"))) {
        missing.push(`${rel}: ${label}`);
      }
    }
  }
  expect(missing).toEqual([]);
});
