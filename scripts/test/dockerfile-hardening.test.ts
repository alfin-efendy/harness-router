import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// The published container runs the agent daemon, which executes tools and shell
// commands. As uid 0 any harness escape acts as root inside the container -- and
// as root on the host through a bind-mounted workspace. These assertions are the
// regression guard for the hardening in the repo-root Dockerfile.
const dockerfile = readFileSync(join(process.cwd(), "Dockerfile"), "utf8");
const lines = dockerfile.split("\n").map((line) => line.trim());

function firstIndexOfInstruction(name: string): number {
  return lines.findIndex((line) => line.toUpperCase().startsWith(`${name} `));
}

test("the image drops to a non-root user", () => {
  const index = firstIndexOfInstruction("USER");
  expect(index).toBeGreaterThanOrEqual(0);
  const argument = (lines[index] ?? "").slice("USER ".length).trim();
  expect(argument).not.toBe("root");
  expect(argument).not.toBe("0");
  expect(argument.startsWith("10001")).toBe(true);
});

test("HOME is set explicitly so the state dir resolves under the non-root home", () => {
  expect(dockerfile).toContain("ENV HOME=/home/ryuzi");
});

test("the persistent dirs are created and chowned before VOLUME", () => {
  const chown = lines.findIndex((line) => line.includes("chown -R 10001:10001 /home/ryuzi"));
  const volume = firstIndexOfInstruction("VOLUME");
  expect(chown).toBeGreaterThanOrEqual(0);
  expect(volume).toBeGreaterThan(chown);
});

test("both persistent dirs are declared volumes", () => {
  const volume = lines[firstIndexOfInstruction("VOLUME")] ?? "";
  expect(volume).toContain("/home/ryuzi/.local/share/ryuzi");
  expect(volume).toContain("/home/ryuzi/.config/ryuzi");
});

test("the control-API port is documented with EXPOSE", () => {
  expect(dockerfile).toContain("EXPOSE 4483");
});
