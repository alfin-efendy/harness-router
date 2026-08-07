import { afterEach, expect, test } from "bun:test";
import { cleanup, render, screen } from "@testing-library/react";
import { PluginContentsList } from "./PluginContentsList";

afterEach(cleanup);

test("renders commands as /name in mono, and skills by their plain name", () => {
  render(<PluginContentsList commands={["review", "deploy"]} skills={["release-notes"]} />);
  expect(screen.getByText("/review")).toBeTruthy();
  expect(screen.getByText("/deploy")).toBeTruthy();
  expect(screen.getByText("release-notes")).toBeTruthy();
  expect(screen.getByText("Commands")).toBeTruthy();
  expect(screen.getByText("Skills")).toBeTruthy();
});

test("renders only the Commands card when skills is empty", () => {
  render(<PluginContentsList commands={["review"]} skills={[]} />);
  expect(screen.getByText("/review")).toBeTruthy();
  expect(screen.queryByText("Skills")).toBeNull();
});

test("renders only the Skills card when commands is empty", () => {
  render(<PluginContentsList commands={[]} skills={["release-notes"]} />);
  expect(screen.getByText("release-notes")).toBeTruthy();
  expect(screen.queryByText("Commands")).toBeNull();
});

test("renders nothing when both are empty", () => {
  const { container } = render(<PluginContentsList commands={[]} skills={[]} />);
  expect(container.textContent).toBe("");
});
