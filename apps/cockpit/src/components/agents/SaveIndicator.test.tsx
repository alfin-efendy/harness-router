import { afterEach, beforeEach, expect, jest, test } from "bun:test";
import { act, cleanup, render, screen } from "@testing-library/react";
import { useAgents } from "@/store-agents";
import { SaveIndicator } from "./SaveIndicator";

beforeEach(() => {
  useAgents.setState({ saving: false });
});
afterEach(() => {
  cleanup();
  jest.useRealTimers();
});

test("renders nothing before any save has started", () => {
  render(<SaveIndicator />);
  expect(screen.queryByText("Saving…")).toBeNull();
  expect(screen.queryByText("✓ Saved")).toBeNull();
});

test("shows Saving… while a save is in flight", () => {
  render(<SaveIndicator />);
  act(() => {
    useAgents.setState({ saving: true });
  });
  expect(screen.getByText("Saving…")).toBeTruthy();
  expect(screen.queryByText("✓ Saved")).toBeNull();
});

test("shows a Saved confirmation for ~1.5s after saving completes, then clears", () => {
  jest.useFakeTimers();
  render(<SaveIndicator />);
  act(() => {
    useAgents.setState({ saving: true });
  });
  act(() => {
    useAgents.setState({ saving: false });
  });
  expect(screen.getByText("✓ Saved")).toBeTruthy();
  expect(screen.queryByText("Saving…")).toBeNull();

  act(() => {
    jest.advanceTimersByTime(1499);
  });
  expect(screen.getByText("✓ Saved")).toBeTruthy();

  act(() => {
    jest.advanceTimersByTime(1);
  });
  expect(screen.queryByText("✓ Saved")).toBeNull();
});

test("a subsequent save re-enters Saving… from the Saved state", () => {
  jest.useFakeTimers();
  render(<SaveIndicator />);
  act(() => {
    useAgents.setState({ saving: true });
  });
  act(() => {
    useAgents.setState({ saving: false });
  });
  expect(screen.getByText("✓ Saved")).toBeTruthy();

  act(() => {
    useAgents.setState({ saving: true });
  });
  expect(screen.getByText("Saving…")).toBeTruthy();
  expect(screen.queryByText("✓ Saved")).toBeNull();
});
