import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const root = process.cwd();

type PackageConfig = {
  "package-name": string;
  "skip-github-release"?: boolean;
};

const config = JSON.parse(readFileSync(join(root, "release-please-config.json"), "utf8")) as {
  packages: Record<string, PackageConfig>;
};

const manifest = JSON.parse(readFileSync(join(root, ".release-please-manifest.json"), "utf8")) as Record<string, string>;

/** The `name` under the `[package]` table of `<dir>/Cargo.toml`. */
function cargoPackageName(dir: string): string {
  const toml = readFileSync(join(root, dir, "Cargo.toml"), "utf8");
  const section = toml.split(/^\[/m).find((s) => s.startsWith("package]"));
  if (!section) throw new Error(`no [package] section in ${dir}/Cargo.toml`);
  const match = section.match(/^\s*name\s*=\s*"([^"]+)"/m);
  if (!match) throw new Error(`no [package] name in ${dir}/Cargo.toml`);
  return match[1]!;
}

test("every release-please package-name matches its real Cargo package name", () => {
  const mismatches: string[] = [];
  for (const [path, pkg] of Object.entries(config.packages)) {
    const real = cargoPackageName(path);
    if (real !== pkg["package-name"]) {
      mismatches.push(`${path}: config says ${pkg["package-name"]}, Cargo says ${real}`);
    }
  }
  expect(mismatches).toEqual([]);
});

test("the manifest and the config describe the same set of packages", () => {
  expect(Object.keys(manifest).sort()).toEqual(Object.keys(config.packages).sort());
});

test("release.yml reads its outputs from the single release-creating package", () => {
  const anchors = Object.entries(config.packages)
    .filter(([, pkg]) => pkg["skip-github-release"] !== true)
    .map(([path]) => path);
  expect(anchors).toHaveLength(1);

  const anchor = anchors[0]!;
  const workflow = readFileSync(join(root, ".github/workflows/release.yml"), "utf8");
  expect(workflow).toContain(`steps.rp.outputs['${anchor}--release_created']`);
  expect(workflow).toContain(`steps.rp.outputs['${anchor}--tag_name']`);
});
