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
      intelMac: false,
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

describe("FirstRun hardware line", () => {
  afterEach(cleanup);

  it("says an Intel Mac runs models on the processor", async () => {
    info = base(null, [suggestion("small", false)]);
    info.hardware = { ...info.hardware, backend: "cpu", intelMac: true };
    await show();
    expect(screen.getByText("Intel Mac: models run on the processor, with 12.0 GB of free memory.")).toBeTruthy();
    expect(screen.queryByText(/No supported GPU/)).toBeNull();
  });

  it("keeps the other lines as they were", async () => {
    info = base(null, [suggestion("small", false)]);
    await show();
    expect(
      screen.getByText("No supported GPU found; models will run on the CPU with 12.0 GB of free memory."),
    ).toBeTruthy();
    cleanup();

    info.hardware = { ...info.hardware, target: "apple", gpuName: "Apple M2", usableBytes: 10e9, backend: "metal" };
    await show();
    expect(screen.getByText("Apple silicon; the GPU can use about 10.0 GB.")).toBeTruthy();
    cleanup();

    info.hardware = { ...info.hardware, target: "gpu", gpuName: "Radeon RX", gpuFreeBytes: 15e9, backend: "vulkan" };
    await show();
    expect(screen.getByText("Radeon RX with 15.0 GB free (vulkan).")).toBeTruthy();
  });
});
