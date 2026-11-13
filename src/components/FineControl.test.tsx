import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { FineControl as FineSettings, FineFacts, GpuUse, OffloadPreview } from "../lib/types";
import { FineControl, fineIsAuto, hasCustomPlacement } from "./FineControl";
import { FINE_AUTO, fineFacts, gb, picture } from "./fine.fixtures";

interface Opts {
  facts?: Partial<FineFacts>;
  fine?: Partial<FineSettings>;
  gpuOff?: boolean;
  gpu?: GpuUse;
  saveMemory?: boolean;
  picture?: OffloadPreview | null;
  open?: boolean;
}

/** Renders the section open, keeping the choices it writes, like the panel does. */
function show(opts: Opts = {}) {
  const writes: FineSettings[] = [];
  const onToggle = vi.fn();
  let fine: FineSettings = { ...FINE_AUTO, ...opts.fine };
  const view = (f: FineSettings) => (
    <FineControl
      facts={fineFacts(opts.facts)}
      fine={f}
      gpuOff={opts.gpuOff ?? false}
      gpu={opts.gpu ?? "fit"}
      saveMemory={opts.saveMemory ?? false}
      picture={opts.picture === undefined ? picture({ custom: false }) : opts.picture}
      open={opts.open ?? true}
      onToggle={onToggle}
      disabled={false}
      onChange={(next) => {
        writes.push(next);
        fine = next;
        rerender(view(next));
      }}
    />
  );
  const { rerender } = render(view(fine));
  return { writes, onToggle, last: () => writes[writes.length - 1] };
}

function select(label: string): HTMLSelectElement {
  return screen.getByLabelText(label) as HTMLSelectElement;
}

function choose(label: string, value: string) {
  act(() => {
    fireEvent.change(select(label), { target: { value } });
  });
}

function slide(label: string, value: number) {
  act(() => {
    fireEvent.change(screen.getByLabelText(label), { target: { value: String(value) } });
  });
}

function options(label: string): string[] {
  return [...select(label).options].map((o) => o.textContent ?? "");
}

const text = () => document.body.textContent ?? "";

describe("FineControl: the finer offloading settings of one model", () => {
  afterEach(cleanup);

  it("writes each setting as its typed value", () => {
    const { last } = show({ facts: { expertLayers: 48 } });
    expect(options("Layers on the graphics card")).toEqual(["Automatic (follows Use the graphics card)", "Choose how many"]);
    choose("Layers on the graphics card", "count");
    // The slider starts where the model goes now (30 of 41 in the picture).
    expect(last().layers).toEqual({ count: 30 });
    expect(text()).toContain("30 of 41 layers");
    slide("Layers on the graphics card: how many", 20);
    expect(last().layers).toEqual({ count: 20 });
    expect(text()).toContain("20 of 41 layers");

    choose("Expert parts on the processor", "all");
    expect(last().experts).toBe("all");
    choose("Expert parts on the processor", "some");
    expect(last().experts).toEqual({ firstLayers: 24 });
    slide("Expert parts on the processor: how many", 20);
    expect(last().experts).toEqual({ firstLayers: 20 });
    expect(text()).toContain("the first 20 of 48 layers");

    choose("Conversation memory on the graphics card", "computerMemory");
    expect(last().kv).toBe("computerMemory");
    expect(options("Conversation memory on the graphics card")).toEqual(["Yes, faster (Automatic)", "No, keep it in computer memory"]);

    choose("Processor threads", "count");
    expect(last().threads).toEqual({ count: 8 });
    slide("Processor threads: how many", 2);
    expect(last().threads).toEqual({ count: 2 });
    expect(text()).toContain("2 of 16 threads");

    choose("Fast attention", "off");
    expect(last().fastAttention).toBe("off");
    expect(options("Fast attention")).toEqual(["Automatic", "On", "Off"]);

    expect(options("Leave free on the graphics card")).toEqual(["Automatic (1 GB)", "0.5 GB", "2 GB", "4 GB"]);
    // A custom placement greys Leave free out; back to Automatic layers and experts to set it.
    choose("Layers on the graphics card", "auto");
    choose("Expert parts on the processor", "auto");
    choose("Leave free on the graphics card", "4096");
    expect(last().leaveFree).toEqual({ mib: 4096 });
    expect(last()).toEqual({
      layers: "auto",
      experts: "auto",
      kv: "computerMemory",
      threads: { count: 2 },
      fastAttention: "off",
      leaveFree: { mib: 4096 },
    });
  });

  it("keeps a slider's number within its range and steps with the keyboard's native range input", () => {
    const { last } = show({ fine: { layers: { count: 5 } } });
    const slider = screen.getByLabelText("Layers on the graphics card: how many") as HTMLInputElement;
    expect(slider.type).toBe("range");
    expect(slider.min).toBe("0");
    expect(slider.max).toBe("41");
    expect(slider.step).toBe("1");
    expect(slider.getAttribute("aria-valuetext")).toBe("5 of 41 layers");
    slide("Layers on the graphics card: how many", 400);
    expect(last().layers).toEqual({ count: 41 });
  });

  it("greys out what the graphics card setting makes useless, with a visible note", () => {
    show({ gpuOff: true, gpu: "off", facts: { expertLayers: 48 }, fine: { layers: { count: 10 } } });
    for (const label of [
      "Layers on the graphics card",
      "Expert parts on the processor",
      "Conversation memory on the graphics card",
      "Leave free on the graphics card",
    ]) {
      expect(select(label).disabled, label).toBe(true);
    }
    expect(screen.getAllByText("Not used while Use the graphics card is Don't use it.").length).toBe(4);
    // Threads and fast attention still matter on the processor.
    expect(select("Processor threads").disabled).toBe(false);
    expect(select("Fast attention").disabled).toBe(false);
  });

  it("shows the layer and graphics settings only when a graphics device is known", () => {
    show({ facts: { graphics: false, expertLayers: 48 } });
    expect(screen.queryByLabelText("Layers on the graphics card")).toBeNull();
    expect(screen.queryByLabelText("Expert parts on the processor")).toBeNull();
    expect(screen.queryByLabelText("Conversation memory on the graphics card")).toBeNull();
    expect(screen.queryByLabelText("Leave free on the graphics card")).toBeNull();
    expect(screen.getByLabelText("Processor threads")).toBeTruthy();
    expect(screen.getByLabelText("Fast attention")).toBeTruthy();
  });

  it("fixes fast attention on while Save memory is on", () => {
    show({ saveMemory: true, fine: { fastAttention: "off" } });
    expect(options("Fast attention")).toEqual(["On (Save memory needs it)"]);
    expect(select("Fast attention").disabled).toBe(true);
  });

  it("offers expert parts only for a mixture of experts, with its note when only some layers are on the card", () => {
    show();
    expect(screen.queryByLabelText("Expert parts on the processor")).toBeNull();
    cleanup();
    show({ facts: { expertLayers: 40 }, fine: { layers: { count: 32 } } });
    expect(text()).toContain("This model is a mixture of experts");
    expect(text()).toContain(
      "The first 9 layers already run on the processor. Only expert parts of later layers free graphics memory.",
    );
    expect(select("Expert parts on the processor").disabled).toBe(false);
    cleanup();
    show({ facts: { expertLayers: 40 }, fine: { layers: { count: 0 } } });
    expect(text()).toContain("Not used while no layers are on the graphics card.");
    expect(select("Expert parts on the processor").disabled).toBe(true);
  });

  it("says when Leave free is not used: with All of it, and with a custom placement", () => {
    show({ gpu: "all" });
    expect(text()).toContain("Only used with As much as fits.");
    expect(select("Leave free on the graphics card").disabled).toBe(true);
    cleanup();
    show({ gpu: "fit", fine: { experts: "all" }, facts: { expertLayers: 40 } });
    expect(text()).toContain("Not used while you choose the layers or expert parts yourself.");
    expect(select("Leave free on the graphics card").disabled).toBe(true);
    cleanup();
    show({ gpu: "fit" });
    expect(select("Leave free on the graphics card").disabled).toBe(false);
    expect(text()).not.toContain("Only used with As much as fits.");
  });

  it("says Apple graphics on a Mac, in labels, help lines and notes", () => {
    show({ facts: { apple: true, graphicsLimitBytes: gb(10.7) }, gpuOff: true, gpu: "off" });
    expect(screen.getByLabelText("Layers on Apple graphics")).toBeTruthy();
    expect(screen.getByLabelText("Conversation memory with Apple graphics")).toBeTruthy();
    expect(screen.getByLabelText("Memory to leave free")).toBeTruthy();
    expect(options("Layers on Apple graphics")[0]).toBe("Automatic (follows Use Apple graphics)");
    expect(options("Conversation memory with Apple graphics")[1]).toBe("No, keep it with the processor");
    expect(text()).toContain("as long as they fit in Apple graphics' share of memory (up to 10.7 GB here).");
    expect(text()).toContain("Not used while Use Apple graphics is Don't use it.");
    expect(text()).toContain("Automatic turns it on where Apple graphics supports it.");
    expect(text()).not.toMatch(/graphics card/);
  });

  it("Reset fine control sets all six back to Automatic", () => {
    const { last } = show({ fine: { threads: { count: 2 }, fastAttention: "off", kv: "computerMemory" } });
    fireEvent.click(screen.getByRole("button", { name: "Reset fine control" }));
    expect(last()).toEqual(FINE_AUTO);
    expect((screen.getByRole("button", { name: "Reset fine control" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("sums itself up when closed: the layers on the card, and whether anything is changed", () => {
    const { onToggle } = show({ open: false, fine: { threads: { count: 2 } } });
    const summary = document.querySelector("summary") as HTMLElement;
    expect(summary.textContent).toBe("Fine control · 30 of 41 layers on the graphics card · changed");
    expect((document.querySelector("details") as HTMLDetailsElement).open).toBe(false);
    const details = document.querySelector("details") as HTMLDetailsElement;
    act(() => {
      details.open = true;
      details.dispatchEvent(new Event("toggle"));
    });
    expect(onToggle).toHaveBeenCalledWith(true);
    cleanup();
    show({ open: false, picture: picture({ graphics: null, layersOnGraphics: 0 }) });
    expect(document.querySelector("summary")?.textContent).toBe("Fine control · On the processor only");
  });

  it("knows Automatic and a custom placement", () => {
    expect(fineIsAuto(undefined)).toBe(true);
    expect(fineIsAuto(FINE_AUTO)).toBe(true);
    expect(fineIsAuto({ ...FINE_AUTO, leaveFree: { mib: 512 } })).toBe(false);
    expect(hasCustomPlacement({ ...FINE_AUTO, threads: { count: 2 } })).toBe(false);
    expect(hasCustomPlacement({ ...FINE_AUTO, layers: { count: 2 } })).toBe(true);
    expect(hasCustomPlacement({ ...FINE_AUTO, experts: "all" })).toBe(true);
  });
});
