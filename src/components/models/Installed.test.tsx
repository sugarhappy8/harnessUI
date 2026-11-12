import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { InstalledModel, ModelBusy, ModelLoadInfo, RuntimeStatus } from "../../lib/types";

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  listInstalledModels: vi.fn<() => Promise<InstalledModel[]>>(),
  diskUsage: vi.fn(async () => ({ modelsDir: "/models", totalBytes: 2_000_000_000 })),
  modelLoadInfo: vi.fn<(path: string) => Promise<ModelLoadInfo>>(),
  modelBusy: vi.fn<() => Promise<ModelBusy>>(async () => ({ chatTurns: 0, modelChecks: 0, otherApps: 0 })),
  loadLocalModel: vi.fn<(path: string, s?: unknown, w?: boolean) => Promise<boolean>>(async () => true),
  unloadModel: vi.fn<(w?: boolean) => Promise<boolean>>(async () => true),
  deleteModel: vi.fn<(id: number, files: boolean) => Promise<void>>(async () => undefined),
  getLocalModelSettings: vi.fn(async () => {
    throw new Error("Use must not pass the defaults for all models");
  }),
  setChatModel: vi.fn(async () => undefined),
  getChat: vi.fn(async (id: number) => ({ chat: { id, modelRef: null }, messages: [] })),
  listModelOptions: vi.fn(async () => []),
}));
vi.mock("../../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const { Installed } = await import("./Installed");
const { QuestionHost } = await import("../ModelSettings");
const { useApp } = await import("../../store/app");
const { useCompare } = await import("../../store/compare");

function model(id: number, name: string): InstalledModel {
  return {
    id,
    path: `/models/${name}`,
    repo: null,
    fileName: name,
    sizeBytes: 1_000_000_000,
    source: "download",
    projectorPath: null,
    summary: null,
    architectureSupported: true,
  };
}

function info(path: string, patch: Partial<ModelLoadInfo> = {}): ModelLoadInfo {
  return {
    path,
    fileName: path.split("/").pop() ?? path,
    sizeBytes: 1_000_000_000,
    trainedCtx: 40_960,
    thinking: false,
    vision: false,
    overrides: { context: "default", gpu: "default", saveMemory: "default", keepInMemory: "default", device: "default", thinkingLimit: "default" },
    defaults: { ctx: 8192, backend: null, device: null, contextAuto: true, gpu: "all", saveMemory: false, keepInMemory: false, thinkingLimit: "noLimit" },
    resolved: {
      ctx: 24_576,
      ctxChoice: "auto",
      ctxSource: "automatic",
      gpu: "all",
      saveMemory: false,
      keepInMemory: false,
      thinkingLimit: "noLimit",
      backend: null,
      device: null,
      mmproj: null,
    },
    fit: "green",
    tooLarge: false,
    contextOptions: [{ choice: "auto", tokens: 24_576, fit: "green" }],
    gpuOptions: [],
    devices: [],
    loaded: false,
    olderSettingsLoaded: false,
    autoCtx: null,
    ...patch,
  };
}

function readyOn(path: string): RuntimeStatus {
  return {
    state: "ready",
    backend: "cpu",
    device: null,
    modelPath: path,
    modelName: path.split("/").pop() ?? null,
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
  };
}

const SMALL = model(1, "small.gguf");
const BIG = model(2, "big.gguf");

async function show(onClose = vi.fn()) {
  await act(async () => {
    render(
      <>
        <Installed onClose={onClose} />
        <QuestionHost />
      </>,
    );
  });
  return onClose;
}

function useButtons(): HTMLButtonElement[] {
  return screen.getAllByRole("button", { name: "Use" }) as HTMLButtonElement[];
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
}

describe("Models → Installed", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listInstalledModels.mockResolvedValue([SMALL, BIG]);
    ipcMock.modelLoadInfo.mockImplementation(async (path) =>
      path === BIG.path ? info(path, { fit: "red" }) : info(path),
    );
    useApp.setState({ runtime: null, activeChatId: 3, detail: null, error: null });
    useCompare.setState({ entries: [] });
  });
  afterEach(cleanup);

  it("shows each model's fit at its own settings", async () => {
    await show();
    expect(screen.getByText("Fits")).toBeTruthy();
    expect(screen.getByText("Too large")).toBeTruthy();
    expect(ipcMock.modelLoadInfo).toHaveBeenCalledWith(SMALL.path);
    expect(ipcMock.modelLoadInfo).toHaveBeenCalledWith(BIG.path);
  });

  it("Use loads with the model's own settings and closes the dialog", async () => {
    const onClose = await show();
    await click(useButtons()[0] as HTMLButtonElement);
    expect(ipcMock.getLocalModelSettings).not.toHaveBeenCalled();
    expect(ipcMock.setChatModel).toHaveBeenCalledWith(3, "local:/models/small.gguf");
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/small.gguf", null, true);
    expect(onClose).toHaveBeenCalled();
    expect(useApp.getState().error).toBeNull();
  });

  it("asks before loading a model marked Too large", async () => {
    const onClose = await show();
    await click(useButtons()[1] as HTMLButtonElement);
    expect(screen.getByText("This model probably needs more memory than this computer has. Load it anyway?")).toBeTruthy();
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(ipcMock.loadLocalModel).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    await click(useButtons()[1] as HTMLButtonElement);
    await click(screen.getByRole("button", { name: "Load anyway" }));
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/big.gguf", null, true);
    expect(onClose).toHaveBeenCalled();
  });

  it("asks the D3 question when the loaded model is busy, and stays open after Cancel", async () => {
    const loaded: RuntimeStatus = {
      state: "ready",
      backend: "cpu",
      device: null,
      modelPath: BIG.path,
      modelName: "big.gguf",
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
    };
    useApp.setState({ runtime: loaded });
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 0, modelChecks: 1, otherApps: 0 });
    const onClose = await show();
    await click(useButtons()[0] as HTMLButtonElement);
    expect(screen.getByText("A model check is running. Stop it and restart the model?")).toBeTruthy();
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).not.toHaveBeenCalled();
    expect(ipcMock.loadLocalModel).not.toHaveBeenCalled();
    await click(useButtons()[0] as HTMLButtonElement);
    await click(screen.getByRole("button", { name: "Restart when it ends" }));
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/small.gguf", null, true);
  });

  it("asks the D3 question before deleting the loaded model while it is busy, and unloads it first", async () => {
    useApp.setState({ runtime: readyOn(SMALL.path) });
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 1, modelChecks: 0, otherApps: 0 });
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    await show();
    await click(screen.getByRole("button", { name: "Delete small.gguf" }));
    expect(confirm.mock.calls[0]?.[0]).toMatch(/^Delete small\.gguf\? This removes .+ from disk\.$/);
    expect(screen.getByText("A reply is being written. Stop it and restart the model?")).toBeTruthy();
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(ipcMock.unloadModel).not.toHaveBeenCalled();
    expect(ipcMock.deleteModel).not.toHaveBeenCalled();

    // Restart when it ends: the unload waits for the reply; a cancelled wait deletes nothing.
    ipcMock.unloadModel.mockResolvedValueOnce(false);
    await click(screen.getByRole("button", { name: "Delete small.gguf" }));
    await click(screen.getByRole("button", { name: "Restart when it ends" }));
    expect(ipcMock.unloadModel).toHaveBeenLastCalledWith(true);
    expect(ipcMock.deleteModel).not.toHaveBeenCalled();

    const order: string[] = [];
    ipcMock.unloadModel.mockImplementationOnce(async () => {
      order.push("unload");
      return true;
    });
    ipcMock.deleteModel.mockImplementationOnce(async () => {
      order.push("delete");
    });
    await click(screen.getByRole("button", { name: "Delete small.gguf" }));
    await click(screen.getByRole("button", { name: "Stop and restart" }));
    expect(ipcMock.unloadModel).toHaveBeenLastCalledWith(false);
    expect(order).toEqual(["unload", "delete"]);
    expect(ipcMock.deleteModel).toHaveBeenCalledWith(1, true);
    confirm.mockRestore();
  });

  it("deletes a model that is not loaded without asking about the loaded one", async () => {
    useApp.setState({ runtime: readyOn(BIG.path) });
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    await show();
    await click(screen.getByRole("button", { name: "Delete small.gguf" }));
    expect(ipcMock.modelBusy).not.toHaveBeenCalled();
    expect(ipcMock.unloadModel).not.toHaveBeenCalled();
    expect(ipcMock.deleteModel).toHaveBeenCalledWith(1, true);
    // The loaded model while nothing uses it: unloaded (told to wait, which acts at once), then deleted.
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 0, modelChecks: 0, otherApps: 0 });
    await click(screen.getByRole("button", { name: "Delete big.gguf" }));
    expect(ipcMock.unloadModel).toHaveBeenCalledWith(true);
    expect(ipcMock.deleteModel).toHaveBeenLastCalledWith(2, true);
    // Cancel on the delete question itself does nothing at all.
    confirm.mockReturnValue(false);
    ipcMock.deleteModel.mockClear();
    await click(screen.getByRole("button", { name: "Delete big.gguf" }));
    expect(ipcMock.deleteModel).not.toHaveBeenCalled();
    confirm.mockRestore();
  });

  it("says what the model was made for in plain words: tokens with words, no jargon badges", async () => {
    ipcMock.listInstalledModels.mockResolvedValue([
      {
        ...SMALL,
        summary: {
          architecture: "qwen3moe",
          quantization: "Q4_K_M",
          sizeLabel: null,
          contextLength: 40_960,
          templateTools: true,
          templateThinking: false,
          isMoe: true,
        },
      },
    ]);
    await show();
    expect(screen.getByText("remembers up to 40,960 tokens, about 30,000 words")).toBeTruthy();
    expect(screen.getByText("uses part of itself at a time")).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/\bcontext\b|MoE|40k/);
  });

  it("opens the model's settings from its gear", async () => {
    await show();
    await click(screen.getByRole("button", { name: "Settings for this model: small.gguf" }));
    expect(screen.getByText("Settings for this model")).toBeTruthy();
    expect(screen.getByLabelText("How much it can remember")).toBeTruthy();
  });

  it("adds and removes a model for comparison, up to four", async () => {
    await show();
    await click(screen.getByRole("button", { name: "Add to comparison: small.gguf" }));
    expect(useCompare.getState().entries).toEqual([{ installedPath: "/models/small.gguf" }]);
    await click(screen.getByRole("button", { name: "Remove from comparison: small.gguf" }));
    expect(useCompare.getState().entries).toEqual([]);
    await act(async () => {
      useCompare.setState({ entries: [{ catalogId: "a" }, { catalogId: "b" }, { catalogId: "c" }, { catalogId: "d" }] });
    });
    const add = screen.getByRole("button", { name: "Add to comparison: big.gguf" }) as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    expect(add.title).toBe("You can compare up to 4 models. Remove one first.");
    // Also as visible text, once for the list, not only as a tooltip.
    expect(screen.getAllByText("You can compare up to 4 models. Remove one first.")).toHaveLength(1);
  });
});
