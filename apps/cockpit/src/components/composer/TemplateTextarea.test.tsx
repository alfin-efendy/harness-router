import { useState } from "react";
import { afterEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { AgentInfo } from "@/bindings";

const reviewerAgent: AgentInfo = { name: "reviewer", description: "Reviews changes", mode: "subagent", builtin: true };

const nativeAgents = mock(async () => ({ status: "ok" as const, data: [reviewerAgent] }));
const searchFiles = mock(async () => ({ status: "ok" as const, data: [] }));

mock.module("@/bindings", () => ({
  commands: { nativeAgents, searchFiles },
  events: { coreEventMsg: { listen: async () => () => {} } },
}));

const { TemplateTextarea } = await import("./TemplateTextarea");
const { useNative } = await import("@/store-native");

function Harness({ projectId }: { projectId: string | null }) {
  const [value, setValue] = useState("");
  return <TemplateTextarea value={value} onChange={setValue} projectId={projectId} slashEntries={[]} aria-label="Template" />;
}

afterEach(() => {
  cleanup();
  useNative.setState({ agentsByProject: {} });
  nativeAgents.mockClear();
  searchFiles.mockClear();
});

test("loads the hint project's agents and lists them in the @ menu, inserting @name on pick", async () => {
  render(<Harness projectId="p1" />);
  await waitFor(() => expect(nativeAgents).toHaveBeenCalledWith("local", "p1"));
  await waitFor(() => expect(useNative.getState().agentsByProject.p1).toHaveLength(1));

  const textarea = screen.getByLabelText("Template") as HTMLTextAreaElement;
  fireEvent.change(textarea, { target: { value: "@rev", selectionStart: 4 } });

  const item = await screen.findByRole("button", { name: /reviewer/ });
  fireEvent.click(item);
  expect(textarea.value).toBe("@reviewer ");
});

test("does not query or list agents when there is no hint project", async () => {
  render(<Harness projectId={null} />);
  const textarea = screen.getByLabelText("Template") as HTMLTextAreaElement;
  fireEvent.change(textarea, { target: { value: "@rev", selectionStart: 4 } });

  // Give any stray microtask a chance to run before asserting the negative.
  await new Promise((resolve) => setTimeout(resolve, 10));
  expect(nativeAgents).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: /reviewer/ })).toBeNull();
});

test("Escape closes an open menu and does not let the keydown reach a parent listener", async () => {
  useNative.setState({ agentsByProject: { p1: [reviewerAgent] } });
  const parentKeyDown = mock(() => {});
  render(
    // biome-ignore lint/a11y/noStaticElementInteractions: test-only spy wrapper, not real UI
    <div onKeyDown={parentKeyDown}>
      <Harness projectId="p1" />
    </div>,
  );
  const textarea = screen.getByLabelText("Template") as HTMLTextAreaElement;
  fireEvent.change(textarea, { target: { value: "@rev", selectionStart: 4 } });
  expect(await screen.findByRole("button", { name: /reviewer/ })).toBeTruthy();

  fireEvent.keyDown(textarea, { key: "Escape" });

  expect(screen.queryByRole("button", { name: /reviewer/ })).toBeNull();
  expect(parentKeyDown).not.toHaveBeenCalled();
});
