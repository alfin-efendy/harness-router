import { afterEach, expect, mock, test } from "bun:test";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ManualMcpOauthClient } from "@/bindings";
import { ManualOauthClientModal } from "./ManualOauthClientModal";

const recorded = [{ issuer: "https://as.example", clientId: "manual-client" }] satisfies ManualMcpOauthClient[];

afterEach(cleanup);

test("renders the recorded client ids and disables Save until both fields are filled", () => {
  render(
    <ManualOauthClientModal
      serverName="Rovo"
      clients={recorded}
      onClose={() => {}}
      onSave={async () => true}
      onDelete={async () => true}
    />,
  );
  expect(screen.getByRole("dialog", { name: "Client ID" })).toBeTruthy();
  expect(screen.getByText("https://as.example")).toBeTruthy();

  const save = screen.getByRole("button", { name: "Save" }) as HTMLButtonElement;
  expect(save.disabled).toBe(true);

  fireEvent.change(screen.getByRole("textbox", { name: "Issuer URL" }), { target: { value: "https://other.example" } });
  expect(save.disabled).toBe(true);
  fireEvent.change(screen.getByRole("textbox", { name: "Client ID" }), { target: { value: "c-1" } });
  expect(save.disabled).toBe(false);
});

test("saves trimmed values and clears the form without closing", async () => {
  const onClose = mock(() => {});
  const onSave = mock(async (_issuer: string, _clientId: string) => true);
  render(<ManualOauthClientModal serverName="Rovo" clients={[]} onClose={onClose} onSave={onSave} onDelete={async () => true} />);

  const issuer = screen.getByRole("textbox", { name: "Issuer URL" }) as HTMLInputElement;
  const clientId = screen.getByRole("textbox", { name: "Client ID" }) as HTMLInputElement;
  fireEvent.change(issuer, { target: { value: "  https://as.example  " } });
  fireEvent.change(clientId, { target: { value: "  manual-client  " } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));

  await waitFor(() => expect(onSave).toHaveBeenCalledWith("https://as.example", "manual-client"));
  // A client id is per authorization server, so recording one must leave the
  // modal open and ready for the next.
  expect(onClose).not.toHaveBeenCalled();
  await waitFor(() => expect(issuer.value).toBe(""));
  expect(clientId.value).toBe("");
});

test("Remove calls onDelete with that row's issuer", () => {
  const onDelete = mock(async (_issuer: string) => true);
  render(<ManualOauthClientModal serverName="Rovo" clients={recorded} onClose={() => {}} onSave={async () => true} onDelete={onDelete} />);
  fireEvent.click(screen.getByRole("button", { name: "Remove client id for https://as.example" }));
  expect(onDelete).toHaveBeenCalledWith("https://as.example");
});
