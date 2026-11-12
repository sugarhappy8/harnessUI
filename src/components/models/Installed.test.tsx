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
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/small.gguf", null, undefined);
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
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/big.gguf", null, undefined);
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
  });
});
