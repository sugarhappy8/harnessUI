import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { Tooltip } from "radix-ui";
import { SelectField } from "./SelectField";
import { SwitchField } from "./SwitchField";
import { SliderField } from "./SliderField";
import { Tip } from "./Tip";
import { ChipGroup } from "./ChipGroup";
import { MenuButton, MenuRadio } from "./MenuButton";
import { SlideOver } from "./SlideOver";
import { UiProviders } from "./UiProviders";
import { BOX } from "../studio/styles";
import { FOCUS_RING } from "./styles";

// jsdom has none of what Radix Select, Slider and the menus measure and capture with.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.scrollIntoView ??= () => {};

afterEach(cleanup);

describe("shared controls", () => {
  it("SelectField is a combobox named by its label, and choosing an option calls onChange", () => {
    const onChange = vi.fn();
    render(
      <SelectField
        label="Shape"
        value={"a" as string}
        options={[
          { value: "a", label: "Square" },
          { value: "b", label: "Wide" },
        ]}
        onChange={onChange}
      />,
    );
    const box = screen.getByRole("combobox", { name: "Shape" });
    fireEvent.pointerDown(box, {
      button: 0,
      ctrlKey: false,
      pointerType: "mouse",
    });
    fireEvent.click(screen.getByRole("option", { name: "Wide" }));
    expect(onChange).toHaveBeenCalledWith("b");
  });

  it("SwitchField has role switch with aria-checked, and toggles on click and Space", () => {
    function Host() {
      const [on, setOn] = useState(false);
      return <SwitchField label="Spend" description="Allows paid runs" checked={on} onChange={setOn} />;
    }
    render(<Host />);
    const sw = screen.getByRole("switch", { name: "Spend" });
    expect(sw.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(sw);
    expect(sw.getAttribute("aria-checked")).toBe("true");
    // A button activates on Space's keyup in a browser; jsdom does not, so send that click.
    fireEvent.keyDown(sw, { key: " " });
    fireEvent.keyUp(sw, { key: " " });
    fireEvent.click(sw);
    expect(sw.getAttribute("aria-checked")).toBe("false");
  });

  it("SliderField moves by its step on arrow keys and shows its value as text", () => {
    const onChange = vi.fn();
    render(
      <SliderField
        label="Strength"
        min={-1}
        max={2}
        step={0.05}
        value={1}
        onChange={onChange}
        format={(v) => `${v.toFixed(2)}x`}
      />,
    );
    expect(screen.getByText("1.00x")).toBeTruthy();
    const thumb = screen.getByRole("slider", { name: "Strength" });
    fireEvent.keyDown(thumb, { key: "ArrowRight" });
    expect(onChange).toHaveBeenCalledWith(1.05);
  });

  it("Tip shows its content on keyboard focus", async () => {
    render(
      <Tooltip.Provider delayDuration={0}>
        <Tip content="Runs on this computer">
          <button type="button">Dot</button>
        </Tip>
      </Tooltip.Provider>,
    );
    expect(screen.queryByRole("tooltip")).toBeNull();
    await act(async () => {
      screen.getByRole("button", { name: "Dot" }).focus();
    });
    expect((await screen.findByRole("tooltip")).textContent).toContain("Runs on this computer");
  });

  it("ChipGroup is a group of pressed buttons, one at a time", () => {
    function Host() {
      const [v, setV] = useState("a");
      return (
        <ChipGroup
          label="Filter"
          value={v}
          options={[
            { value: "a", label: "All" },
            { value: "b", label: "Cloud" },
          ]}
          onChange={setV}
        />
      );
    }
    render(<Host />);
    const group = screen.getByRole("group", { name: "Filter" });
    const all = within(group).getByRole("button", { name: "All" });
    const cloud = within(group).getByRole("button", { name: "Cloud" });
    expect(all.getAttribute("aria-pressed")).toBe("true");
    expect(cloud.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(cloud);
    expect(cloud.getAttribute("aria-pressed")).toBe("true");
    expect(all.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(cloud);
    expect(cloud.getAttribute("aria-pressed")).toBe("true");
  });

  it("MenuButton opens a menu, and its radio items report the choice", () => {
    const onChange = vi.fn();
    render(
      <MenuButton label="Count" trigger="×1">
        <MenuRadio
          value={"1"}
          options={[
            { value: "1", label: "One" },
            { value: "2", label: "Two" },
          ]}
          onChange={onChange}
        />
      </MenuButton>,
    );
    fireEvent.pointerDown(screen.getByRole("button", { name: "Count" }), {
      button: 0,
      ctrlKey: false,
    });
    expect(screen.getByRole("menuitemradio", { name: "One" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByRole("menuitemradio", { name: "Two" }));
    expect(onChange).toHaveBeenCalledWith("2");
  });

  it("SlideOver traps focus, closes on Esc and returns focus to its opener", async () => {
    function Host() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>
            Open
          </button>
          <SlideOver open={open} onOpenChange={setOpen} title="Settings" width="panel">
            <button type="button">Inside</button>
          </SlideOver>
        </>
      );
    }
    render(<Host />);
    const opener = screen.getByRole("button", { name: "Open" });
    opener.focus();
    fireEvent.click(opener);
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    // Focus moved inside, and the page behind is out of the way.
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(within(dialog).getByRole("button", { name: "Close" })).toBeTruthy();
    // Tab at the last control wraps inside the panel (Radix's focus scope).
    const inside = within(dialog).getByRole("button", { name: "Inside" });
    inside.focus();
    fireEvent.keyDown(inside, { key: "Tab" });
    expect(dialog.contains(document.activeElement)).toBe(true);
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });

  it("UiProviders lets a Tip render inside a dialog with no other provider, as App's first-run branch does", async () => {
    render(
      <UiProviders>
        <SlideOver open onOpenChange={() => {}} title="Welcome" width="panel">
          <Tip content="Why">
            <button type="button">Help</button>
          </Tip>
        </SlideOver>
      </UiProviders>,
    );
    await act(async () => {
      screen.getByRole("button", { name: "Help" }).focus();
    });
    expect((await screen.findByRole("tooltip")).textContent).toContain("Why");
  });

  it("Studio's text boxes carry the shared focus ring", () => {
    for (const cls of FOCUS_RING.split(" ")) expect(BOX.split(" ")).toContain(cls);
  });
});
