import { afterEach, expect, test } from "bun:test";
import { cleanup, render } from "@testing-library/react";
import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { MenuItem } from "../../index";

afterEach(cleanup);

test("MenuItem can be imported and exported from ui package", () => {
  expect(typeof MenuItem).toBe("function");
});

test("MenuItem renders a menu item element with styling", () => {
  const { container } = render(
    <MenuPrimitive.Root>
      <MenuItem>Test Item</MenuItem>
    </MenuPrimitive.Root>,
  );

  const menuItem = container.querySelector('[role="menuitem"]');
  expect(menuItem).toBeTruthy();
  expect(menuItem?.textContent).toBe("Test Item");
});

test("MenuItem merges custom className with default styles", () => {
  const { container } = render(
    <MenuPrimitive.Root>
      <MenuItem className="custom-class">Test Item</MenuItem>
    </MenuPrimitive.Root>,
  );

  const menuItem = container.querySelector('[role="menuitem"]');
  const classList = menuItem?.className || "";
  expect(classList.includes("custom-class")).toBe(true);
});
