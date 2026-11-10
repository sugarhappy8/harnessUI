import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import type { CatalogSuggestion, FirstRunInfo } from "../lib/types";

const suggestion = (id: string, tested: boolean): CatalogSuggestion => ({
  id,
  name: id,
  repo: `org/${id}`,
  quant: "Q4_K_M",
  capabilities: ["tools"],
  license: "apache-2.0",
  totalBytes: 2e9,
  memoryNeeded: 3e9,
  recommendedCtx: 8192,
  tierGb: 8,
  fits: true,
  tested,
  installed: false,
});

let info: FirstRunInfo;

vi.mock("../lib/ipc", () => ({
  firstRunInfo: vi.fn(async () => info),
}));

const { FirstRun } = await import("./FirstRun");

async function show() {
  await act(async () => {
    render(<FirstRun onDone={() => undefined} onOpenEndpoints={() => undefined} onOpenModels={() => undefined} />);
  });
}

function base(pick: string | null, suggestions: CatalogSuggestion[]): FirstRunInfo {
  return {
    done: false,
    hardware: {
      target: "cpu",
      gpuName: null,
      gpuFreeBytes: null,
      ramTotalBytes: 16e9,
      ramAvailableBytes: 12e9,
      usableBytes: 12e9,
      backend: null,
    },
    pick,
    suggestions,
    catalogSequence: 1,
    refreshDisabledReason: null,
  };
}

describe("FirstRun preselection", () => {
  afterEach(cleanup);

  it("preselects nothing when no tested entry fits, even if untested ones do", async () => {
    info = base(null, [suggestion("small-untested", false), suggestion("other-untested", false)]);
    await show();
    const radios = screen.getAllByRole("radio") as HTMLInputElement[];
    expect(radios).toHaveLength(2);
    expect(radios.some((r) => r.checked)).toBe(false);
    expect((screen.getByRole("button", { name: "Download" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/none is\s+preselected automatically/)).toBeTruthy();
  });

  it("preselects the tested pick", async () => {
    info = base("tested-one", [suggestion("untested-first", false), suggestion("tested-one", true)]);
    await show();
    const radios = screen.getAllByRole("radio") as HTMLInputElement[];
    expect(radios.map((r) => r.checked)).toEqual([false, true]);
    expect(screen.getByRole("button", { name: "Download tested-one" })).toBeTruthy();
  });
});
