import { afterEach, expect, test } from "bun:test";
import { cleanup, render, screen } from "@testing-library/react";
import type { AgentImportResultInfo } from "@/bindings";
import { AgentImportModal } from "./AgentImportModal";

function result(overrides: Partial<AgentImportResultInfo> = {}): AgentImportResultInfo {
  return {
    agentId: "agent-imported",
    agentName: "Reviewer",
    renamed: false,
    knowledgeFilesWritten: 3,
    projectMemoryFilesSkipped: 0,
    tolerated: [],
    ...overrides,
  };
}

afterEach(() => {
  cleanup();
});

test("renders nothing when there is no import result", () => {
  const { container } = render(<AgentImportModal result={null} onClose={() => {}} />);
  expect(container.textContent).toBe("");
  expect(screen.queryByRole("button", { name: "Open agent" })).toBeNull();
});

test("a clean import names the agent and reports it as ready", () => {
  render(<AgentImportModal result={result()} onClose={() => {}} />);
  expect(screen.getByText("Imported Reviewer")).toBeTruthy();
  expect(screen.getByText("3 knowledge files imported.")).toBeTruthy();
  expect(screen.getByText("Ready to use.")).toBeTruthy();
  expect(screen.queryByText(/not executable yet/)).toBeNull();
  expect(screen.queryByText("Renamed to avoid a name collision.")).toBeNull();
});

test("tolerated references are listed verbatim under the not-executable line", () => {
  render(
    <AgentImportModal
      result={result({
        tolerated: [
          { field: "model.name", message: "model `unknown/missing` is not served by an enabled connection" },
          { field: "skills", message: "unknown skill `deep-review`" },
        ],
      })}
      onClose={() => {}}
    />,
  );
  expect(screen.getByText(/not executable yet/)).toBeTruthy();
  expect(screen.getByText("model.name:")).toBeTruthy();
  expect(screen.getByText(/model `unknown\/missing` is not served by an enabled connection/)).toBeTruthy();
  expect(screen.getByText("skills:")).toBeTruthy();
  expect(screen.getByText(/unknown skill `deep-review`/)).toBeTruthy();
  expect(screen.queryByText("Ready to use.")).toBeNull();
});

test("a renamed import says so", () => {
  render(<AgentImportModal result={result({ renamed: true, agentName: "Reviewer Copy", knowledgeFilesWritten: 1 })} onClose={() => {}} />);
  expect(screen.getByText("Imported Reviewer Copy")).toBeTruthy();
  expect(screen.getByText("1 knowledge file imported.")).toBeTruthy();
  expect(screen.getByText("Renamed to avoid a name collision.")).toBeTruthy();
});
