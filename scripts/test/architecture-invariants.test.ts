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
];

test("no file references the pre-S1 runner crate", () => {
  const offenders: string[] = [];
  for (const rel of walk(root, [])) {
    const text = readFileSync(join(root, rel), "utf8");
    for (const [label, pattern] of retired) {
      if (pattern.test(text)) offenders.push(`${rel}: ${label}`);
    }
  }
  expect(offenders).toEqual([]);
});
