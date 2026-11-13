import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { CatalogSuggestion, FirstRunInfo, ModelBusy, RuntimeStatus } from "../lib/types";

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

const ipcMock = vi.hoisted(() => ({
  firstRunInfo: vi.fn(async () => info),
  importModel: vi.fn(async (path: string) => ({ id: 7, path })),
  completeFirstRun: vi.fn(async () => undefined),
  modelBusy: vi.fn<() => Promise<ModelBusy>>(async () => ({ chatTurns: 0, modelChecks: 0, otherApps: 0 })),
  loadLocalModel: vi.fn<(path: string, s?: unknown, w?: boolean) => Promise<boolean>>(async () => true),
  setChatModel: vi.fn(async () => undefined),
  createChat: vi.fn(async (modelRef: string | null) => ({ id: 9, modelRef })),
  getChat: vi.fn(async (id: number) => ({ chat: { id, modelRef: null }, messages: [] })),
  listChats: vi.fn(async () => []),
  listModelOptions: vi.fn(async () => []),
}));
vi.mock("../lib/ipc", () => ipcMock);
const dialog = vi.hoisted(() => ({ open: vi.fn<() => Promise<string | null>>(async () => "/files/mine.gguf") }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

const { FirstRun } = await import("./FirstRun");
const { useApp } = await import("../store/app");

async function show(onDone: () => void = () => undefined) {
  await act(async () => {
    render(<FirstRun onDone={onDone} onOpenEndpoints={() => undefined} onOpenModels={() => undefined} />);
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

describe("FirstRun: I have a GGUF file", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    info = base(null, [suggestion("small", false)]);
  });
  afterEach(cleanup);

  const loaded: RuntimeStatus = {
    state: "ready",
    backend: "cpu",
    device: null,
    modelPath: "/models/other.gguf",
    modelName: "other.gguf",
    progress: 1,
    stage: null,
    nCtx: 4096,
    supportsTools: true,
    preserveReasoning: false,
    thinking: false,
    speedTps: null,
    slow: false,
    vision: false,
    offloadedLayers: null,
    totalLayers: null,
    error: null,
    loadStage: null,
    adjustments: [],
    ctxSource: null,
    settingsChanged: false,
    reloadWaiting: false,
    unloadedPath: null,
    placement: null,
  };

  async function importIt() {
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "I have a GGUF file" }));
    });
  }

  it("loads the file with its own settings and ends first run", async () => {
    useApp.setState({ runtime: null, activeChatId: null, detail: null });
    const onDone = vi.fn();
    await show(onDone);
    await importIt();
    expect(ipcMock.importModel).toHaveBeenCalledWith("/files/mine.gguf");
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/files/mine.gguf", null, true);
    expect(ipcMock.completeFirstRun).toHaveBeenCalled();
    expect(onDone).toHaveBeenCalled();
  });

  it("asks the D3 question when the loaded model is busy: Cancel keeps first run open, Stop and restart ends it", async () => {
    useApp.setState({ runtime: loaded, activeChatId: null, detail: null });
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 1, modelChecks: 0, otherApps: 0 });
    const onDone = vi.fn();
    await show(onDone);
    await importIt();
    expect(screen.getByText("A reply is being written. Stop it and restart the model?")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    });
    expect(ipcMock.loadLocalModel).not.toHaveBeenCalled();
    expect(ipcMock.completeFirstRun).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();

    await importIt();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Stop and restart" }));
    });
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/files/mine.gguf", null, false);
    expect(ipcMock.completeFirstRun).toHaveBeenCalled();
    expect(onDone).toHaveBeenCalled();
  });
});
