import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import type { OffloadPreview, PictureSource } from "../lib/types";
import { OffloadPicture } from "./OffloadPicture";
import { computerBar, gb, graphicsBar, picture } from "./fine.fixtures";

function show(p: OffloadPreview, opts: Partial<{ measuring: boolean; fineOpen: boolean; apple: boolean; mac: boolean }> = {}) {
  return render(
    <OffloadPicture picture={p} measuring={opts.measuring ?? false} fineOpen={opts.fineOpen ?? false} apple={opts.apple ?? false} mac={opts.mac ?? false} />,
  );
}

const text = () => document.body.textContent ?? "";

/** The numbers of a legend, in GB, in order. */
function legendNumbers(bar: HTMLElement): number[] {
  return [...bar.querySelectorAll("li")].map((li) => Number(/([\d.]+) GB$/.exec(li.textContent ?? "")?.[1]));
}

describe("OffloadPicture: where the model goes, before it loads", () => {
  afterEach(cleanup);

  it("draws the worked example: two bars whose legends add up, the needs line, the layers and the source", () => {
    show(picture());
    expect(screen.getByRole("heading", { name: "Where the model goes" })).toBeTruthy();
    expect(text()).toContain("Graphics card · AMD Radeon RX 9070 XT · 16 GB");
    expect(text()).toContain("Computer memory · 32 GB");
    const graphics = document.querySelector('[data-bar="graphics"]') as HTMLElement;
    const computer = document.querySelector('[data-bar="computer"]') as HTMLElement;
    expect([...graphics.querySelectorAll("li")].map((li) => li.textContent)).toEqual([
      "Model 9.9 GB",
      "Conversation memory 1.2 GB",
      "Working space 0.6 GB",
      "Other programs 0.9 GB",
      "Free after loading 3.3 GB",
    ]);
    // The parts add up to the card (15.9 GB) and to the computer's memory (31.9 GB here).
    expect(legendNumbers(graphics).reduce((a, b) => a + b, 0)).toBeCloseTo(15.9, 5);
    expect(legendNumbers(computer).reduce((a, b) => a + b, 0)).toBeCloseTo(31.9, 5);
    expect(text()).toContain("Needs about 11.7 GB of the 15 GB free now.");
    expect(text()).toContain("30 of 41 layers on the graphics card, 11 on the processor.");
    expect(text()).toContain("Worked out for these settings.");
    expect(text()).toContain("Memory is counted the way Task Manager counts it, so it can differ from the download size.");
    // Never colour alone: each bar is an image with its whole sentence.
    const bars = screen.getAllByRole("img");
    expect(bars.map((b) => b.getAttribute("aria-label"))).toEqual([
      "Graphics card · AMD Radeon RX 9070 XT · 16 GB: model 9.9 GB, conversation memory 1.2 GB, working space 0.6 GB, other programs 0.9 GB, free after loading 3.3 GB.",
      "Computer memory · 32 GB: model 4.1 GB, conversation memory 0.4 GB, working space 0.3 GB, other programs 12 GB, free after loading 15.1 GB.",
    ]);
    // No warning in the picture: the panel has one warning line of its own.
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says each source in its own words, and keeps the estimate while exact figures are worked out", () => {
    const cases: [PictureSource, { measuring?: boolean; fineOpen?: boolean }, string][] = [
      [{ kind: "measured" }, {}, "Worked out for these settings."],
      [{ kind: "estimate", why: "quick" }, {}, "Rough estimate from the model file. Open Fine control to work out exact figures."],
      [{ kind: "estimate", why: "quick" }, { measuring: true, fineOpen: true }, "Working out exact figures…"],
      [{ kind: "estimate", why: "failed" }, { fineOpen: true }, "Rough estimate from the model file. Exact figures couldn't be worked out for this file."],
      [{ kind: "estimate", why: "timedOut" }, { fineOpen: true }, "Rough estimate from the model file. Working them out took too long."],
      [
        { kind: "estimate", why: "noTool", backend: "rocm" },
        { fineOpen: true },
        "Rough estimate from the model file. The AMD graphics card (ROCm) download can't work out exact figures.",
      ],
      [
        { kind: "estimate", why: "noTool", backend: "cuda" },
        { fineOpen: true },
        "Rough estimate from the model file. Exact figures can't be worked out with this download.",
      ],
      [{ kind: "estimate", why: "notReadable" }, {}, "Rough estimate. The model file's details couldn't be read."],
    ];
    for (const [source, opts, words] of cases) {
      show(picture({ source }), opts);
      expect(document.querySelector('[data-picture="source"]')?.textContent).toBe(words);
      // The bars stay shown while the figures are worked out.
      expect(screen.getAllByRole("img").length).toBe(2);
      cleanup();
    }
  });

  it("has a line for every placement", () => {
    const cases: [Partial<OffloadPreview>, string][] = [
      [{ layersOnGraphics: 41 }, "All 41 layers on the graphics card."],
      [{}, "30 of 41 layers on the graphics card, 11 on the processor."],
      [
        { layersAbout: true, custom: false },
        "About 30 of 41 layers on the graphics card. The rest run on the processor. The exact number is decided when the model loads.",
      ],
      [{ graphics: null, layersOnGraphics: 0 }, "On the processor only."],
      [{ layersOnGraphics: 41, expertLayersOnProcessor: 20 }, "All 41 layers on the graphics card. Expert parts of 20 layers stay on the processor."],
      [
        { units: 49, layersOnGraphics: 49, layersAbout: true, expertLayersOnProcessor: 19, custom: false },
        "All 49 layers on the graphics card. Expert parts of about 19 layers stay on the processor. The exact split is decided when the model loads.",
      ],
    ];
    for (const [patch, words] of cases) {
      show(picture(patch));
      expect(document.querySelector('[data-picture="layers"]')?.textContent).toBe(words);
      cleanup();
    }
    // On the processor only there is no graphics bar.
    show(picture({ graphics: null, layersOnGraphics: 0 }));
    expect(document.querySelector('[data-bar="graphics"]')).toBeNull();
    expect(document.querySelector('[data-bar="computer"]')).toBeTruthy();
  });

  it("says when the free figure counts the loaded model as unloaded, and shows an image reader", () => {
    show(
      picture({
        graphics: graphicsBar({ freeAfterUnload: true, need: { model: gb(9.9), context: gb(1.2), compute: gb(0.6), imageReader: gb(0.9) } }),
      }),
    );
    expect(text()).toContain("Needs about 12.6 GB of the 15 GB free once the loaded model is unloaded.");
    expect(text()).toContain("Image reader 0.9 GB");
  });

  it("draws one shared bar on a Mac, with the most Apple graphics may use as a marker", () => {
    show(
      picture({
        unified: true,
        graphics: null,
        graphicsLimitBytes: gb(10.7),
        computer: computerBar({ totalBytes: gb(16), freeBytes: gb(10), otherBytes: gb(6) }),
      }),
      { apple: true, mac: true },
    );
    expect(document.querySelectorAll("[data-bar]").length).toBe(1);
    expect(text()).toContain("Memory · shared by the processor and Apple graphics · 16 GB");
    expect(text()).toContain("Apple graphics can use up to 10.7 GB");
    const marker = document.querySelector('[data-marker="apple-limit"]') as HTMLElement;
    expect(marker.style.left).toBe(`${((10.7 / 16) * 100).toFixed(2)}%`);
    expect(text()).toContain("30 of 41 layers on Apple graphics, 11 on the processor.");
    expect(text()).toContain("Memory is counted the way Activity Monitor counts it");
    expect(text()).not.toContain("graphics card");
  });

  it("shows a graphics card name with hidden and right-to-left characters as codes", () => {
    show(picture({ graphics: graphicsBar({ name: "Card\u202Eevil\u200B" }) }));
    expect(text()).toContain("Graphics card · Card\\u{202E}evil\\u{200B} · 16 GB");
    const bar = document.querySelector('[data-bar="graphics"] [role="img"]') as HTMLElement;
    expect(bar.getAttribute("aria-label")).toMatch(/^Graphics card · Card\\u\{202E\}evil\\u\{200B\} · 16 GB: /);
    expect(document.body.innerHTML).not.toMatch(/[\u202E\u200B]/);
  });
});
