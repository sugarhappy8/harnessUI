import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type {
  DownloadGroup,
  ImageFileOffer,
  InstalledModel,
  ModelBusy,
  ModelLoadInfo,
  RuntimeStatus,
  UpdateStatus,
} from "../../lib/types";
import { FINE_LAUNCH_NONE, fineFacts, processorPicture } from "../fine.fixtures";
import type { LocalModels } from "../studio/types";
import { LOCAL } from "../studio/fixtures/models";
import { formatBytes } from "../../lib/format";

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  listInstalledModels: vi.fn<() => Promise<InstalledModel[]>>(),
  diskUsage: vi.fn(async () => ({ modelsDir: "/models", totalBytes: 2_000_000_000 })),
  modelLoadInfo: vi.fn<(path: string) => Promise<ModelLoadInfo>>(),
  modelBusy: vi.fn<() => Promise<ModelBusy>>(async () => ({ chatTurns: 0, modelChecks: 0, otherApps: 0 })),
  loadLocalModel: vi.fn<(path: string, s?: unknown, w?: boolean) => Promise<boolean>>(async () => true),
  unloadModel: vi.fn<(w?: boolean) => Promise<boolean>>(async () => true),
  deleteModel: vi.fn<(id: number, files: boolean) => Promise<string | null | void>>(async () => undefined),
  getLocalModelSettings: vi.fn(async () => {
    throw new Error("Use must not pass the defaults for all models");
  }),
  setChatModel: vi.fn(async () => undefined),
  getChat: vi.fn(async (id: number) => ({ chat: { id, modelRef: null }, messages: [] })),
  listModelOptions: vi.fn(async () => []),
  // [0.0.7 P2] The image file line of a row with a repository and no image file.
  imageFileOffer: vi.fn<(path: string) => Promise<ImageFileOffer>>(async () => ({ kind: "none" })),
  downloadImageFile: vi.fn<(path: string) => Promise<string>>(async () => "group"),
  onDownloadProgress: vi.fn<(h: (g: DownloadGroup) => void) => Promise<() => void>>(async () => () => undefined),
  // A model this version can't open yet: its row's "Check for updates".
  checkForUpdate: vi.fn<() => Promise<unknown>>(async () => null),
  updateStatus: vi.fn<() => Promise<UpdateStatus>>(),
  onUpdateState: vi.fn(async () => () => undefined),
}));
vi.mock("../../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
// 0.1.0 Studio: the picture and video models installed here, for the line that opens Studio.
const studioApi = vi.hoisted(() => ({ studioLocalModels: vi.fn<() => Promise<LocalModels>>() }));
vi.mock("../studio/api", () => studioApi);

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
      fine: FINE_LAUNCH_NONE,
    },
    fit: "green",
    tooLarge: false,
    contextOptions: [{ choice: "auto", tokens: 24_576, fit: "green" }],
    gpuOptions: [],
    devices: [],
    loaded: false,
    olderSettingsLoaded: false,
    autoCtx: null,
    fineFacts: fineFacts(),
    picture: processorPicture(),
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
    placement: null,
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
    studioApi.studioLocalModels.mockResolvedValue(LOCAL);
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
      placement: null,
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

  it("[0.0.7 P6] says when Delete left a model's files in place, and the model leaves the list", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    await show();
    const note = "Removed from the list. Its file is outside the models folder, so it was left where it is.";
    ipcMock.deleteModel.mockResolvedValueOnce(note);
    ipcMock.listInstalledModels.mockResolvedValue([BIG]);
    await click(screen.getByRole("button", { name: "Delete small.gguf" }));
    expect(ipcMock.deleteModel).toHaveBeenCalledWith(1, true);
    expect(screen.getByText(note)).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete small.gguf" })).toBeNull();
    // The next Delete says nothing more.
    ipcMock.deleteModel.mockResolvedValueOnce(null);
    ipcMock.listInstalledModels.mockResolvedValue([]);
    await click(screen.getByRole("button", { name: "Delete big.gguf" }));
    expect(screen.queryByText(note)).toBeNull();
    confirm.mockRestore();
  });

  it("[0.0.7 P6b] asks about what Delete really removes: its image file too, and not files outside the folder", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    ipcMock.listInstalledModels.mockResolvedValue([
      { ...SMALL, deleteBytes: 1_300_000_000, keptOutside: 0 },
      { ...BIG, deleteBytes: 1_000_000_000, keptOutside: 1 },
      { ...model(3, "away.gguf"), deleteBytes: 0, keptOutside: 2 },
      { ...model(4, "gone.gguf"), deleteBytes: 0, keptOutside: 0 },
      { ...model(5, "far.gguf"), deleteBytes: 0, keptOutside: 1 },
      { ...model(6, "lone.gguf"), deleteBytes: 0, keptOutside: 1, onlyImageOutside: true },
      { ...model(7, "twin.gguf"), deleteBytes: 0, keptOutside: 0, keptInUse: 1 },
      { ...model(8, "parts.gguf"), deleteBytes: 0, keptOutside: 0, keptInUse: 2 },
    ]);
    await show();
    await click(screen.getByRole("button", { name: "Delete small.gguf" }));
    expect(confirm).toHaveBeenLastCalledWith("Delete small.gguf? This removes 1.3 GB from disk.");
    await click(screen.getByRole("button", { name: "Delete big.gguf" }));
    expect(confirm).toHaveBeenLastCalledWith(
      "Delete big.gguf? This removes 1.0 GB from disk. Files outside the models folder stay where they are.",
    );
    await click(screen.getByRole("button", { name: "Delete away.gguf" }));
    expect(confirm).toHaveBeenLastCalledWith(
      "Remove away.gguf from the list? Its files are outside the models folder, so they stay where they are.",
    );
    await click(screen.getByRole("button", { name: "Delete gone.gguf" }));
    expect(confirm).toHaveBeenLastCalledWith("Remove gone.gguf from the list? Its file is no longer on disk.");
    await click(screen.getByRole("button", { name: "Delete far.gguf" }));
    expect(confirm).toHaveBeenLastCalledWith(
      "Remove far.gguf from the list? Its file is outside the models folder, so it stays where it is.",
    );
    // Round 2: as the note after Delete says it, and never "no longer on disk" for a file
    // another model on the list uses.
    await click(screen.getByRole("button", { name: "Delete lone.gguf" }));
    expect(confirm).toHaveBeenLastCalledWith(
      "Remove lone.gguf from the list? Its image file is outside the models folder, so it stays where it is.",
    );
    await click(screen.getByRole("button", { name: "Delete twin.gguf" }));
    expect(confirm).toHaveBeenLastCalledWith(
      "Remove twin.gguf from the list? Another model on the list uses its file, so it stays where it is.",
    );
    await click(screen.getByRole("button", { name: "Delete parts.gguf" }));
    expect(confirm).toHaveBeenLastCalledWith(
      "Remove parts.gguf from the list? Another model on the list uses its files, so they stay where they are.",
    );
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

  it("[0.0.7] marks vision only for an image file the model can read, and says when it couldn't be read", async () => {
    const seeing = { ...model(3, "seeing.gguf"), projectorPath: "/models/mmproj-seeing-F16.gguf" };
    const broken = { ...model(4, "broken.gguf"), projectorPath: null, imageFileUnreadable: true };
    ipcMock.listInstalledModels.mockResolvedValue([seeing, broken]);
    await show();
    expect(screen.getAllByText("vision")).toHaveLength(1);
    expect(screen.getByText("The image file couldn't be read, so this model can't see images.")).toBeTruthy();
    // No repository: nothing to ask Hugging Face.
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
  });

  it("[0.0.7] downloads the image file from the row and reads the list again when it is done", async () => {
    // [P2 review M2] What its repository offers was saved when it was downloaded: nothing is asked.
    const fromHub: InstalledModel = {
      ...model(5, "hub.gguf"),
      repo: "someone/Hub-GGUF",
      imageFileOffer: { kind: "available", file: "mmproj-F16.gguf", sizeBytes: 800_000_000 },
    };
    ipcMock.listInstalledModels.mockResolvedValue([fromHub]);
    const listener: { progress: ((g: DownloadGroup) => void) | null } = { progress: null };
    ipcMock.onDownloadProgress.mockImplementation(async (h) => {
      listener.progress = h;
      return () => undefined;
    });
    await show();
    await click(screen.getByRole("button", { name: "Download the image file (800 MB)" }));
    expect(ipcMock.downloadImageFile).toHaveBeenCalledWith("/models/hub.gguf");
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
    const reads = ipcMock.listInstalledModels.mock.calls.length;
    await act(async () => {
      listener.progress?.({
        id: "group",
        repo: "someone/Hub-GGUF",
        label: "Image file",
        fileCount: 1,
        status: "done",
        bytesDone: 800_000_000,
        bytesTotal: 800_000_000,
        error: null,
        bytesPerSecond: null,
        unverified: false,
      });
    });
    expect(ipcMock.listInstalledModels.mock.calls.length).toBe(reads + 1);
    // Not the model loaded now (P2 review m7).
    expect(screen.getByText("Image file downloaded. It will be used the next time the model loads.")).toBeTruthy();
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

  it("shows file names, folders and repos with hidden and right-to-left characters as codes", async () => {
    const odd = { ...model(3, "X-\u202Efuug.Q4_K_M.gguf"), repo: "someone/\u202Eevil-GGUF" };
    ipcMock.listInstalledModels.mockResolvedValue([odd]);
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    await show();
    const name = screen.getByText("X-\\u{202E}fuug.Q4_K_M.gguf");
    expect(name.tagName).toBe("BDI");
    expect(name.getAttribute("dir")).toBe("ltr");
    expect(name.parentElement?.getAttribute("title")).toBe("/models/X-\\u{202E}fuug.Q4_K_M.gguf");
    expect(screen.getByText("someone/\\u{202E}evil-GGUF")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Settings for this model: X-\\u{202E}fuug.Q4_K_M.gguf" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add to comparison: X-\\u{202E}fuug.Q4_K_M.gguf" })).toBeTruthy();
    await click(screen.getByRole("button", { name: "Delete X-\\u{202E}fuug.Q4_K_M.gguf" }));
    expect(confirm).toHaveBeenCalledWith("Delete X-\\u{202E}fuug.Q4_K_M.gguf? This removes 1.0 GB from disk.");
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("\u202E");
    confirm.mockRestore();
  });

  it("shows the models folder with hidden and right-to-left characters as codes", async () => {
    ipcMock.diskUsage.mockResolvedValueOnce({ modelsDir: "/home/u/X-\u202Esledom", totalBytes: 2_000_000_000 });
    await show();
    const folder = screen.getByText("/home/u/X-\\u{202E}sledom");
    expect(folder.tagName).toBe("BDI");
    expect(folder.getAttribute("dir")).toBe("ltr");
    expect(folder.parentElement?.textContent).toBe("2.0 GB in /home/u/X-\\u{202E}sledom");
    expect(folder.parentElement?.getAttribute("title")).toBe("/home/u/X-\\u{202E}sledom");
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("\u202E");
  });

  it("Installed has one line for picture and video models that opens Studio", async () => {
    const opened: unknown[] = [];
    const onOpen = (e: Event) => opened.push((e as CustomEvent<unknown>).detail);
    window.addEventListener("app:open-studio", onOpen);
    try {
      const onClose = await show();
      const line = screen.getByText(/^Picture and video models:/);
      expect(line.textContent).toBe(
        `Picture and video models: ${LOCAL.installed.length} (${formatBytes(LOCAL.installedBytes)}) · Manage in Studio`,
      );
      // The list never offers Use for them: its rows are the chat models.
      expect(useButtons()).toHaveLength(2);
      await click(screen.getByRole("button", { name: "Manage in Studio" }));
      expect(opened).toEqual(["local"]);
      expect(onClose).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("app:open-studio", onOpen);
    }
  });

  it("with no picture or video model installed, the Studio line says none yet and still opens Studio", async () => {
    studioApi.studioLocalModels.mockResolvedValue({ ...LOCAL, installed: [], installedBytes: 0 });
    const opened: unknown[] = [];
    const onOpen = (e: Event) => opened.push((e as CustomEvent<unknown>).detail);
    window.addEventListener("app:open-studio", onOpen);
    try {
      const onClose = await show();
      const line = screen.getByText(/^Picture and video models:/);
      expect(line.textContent).toBe("Picture and video models: none yet · Manage in Studio");
      await click(screen.getByRole("button", { name: "Manage in Studio" }));
      expect(opened).toEqual(["local"]);
      expect(onClose).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("app:open-studio", onOpen);
    }
  });

  it("has no Studio line when Studio's models could not be read (Studio says why)", async () => {
    studioApi.studioLocalModels.mockRejectedValue({ code: "io", message: "x", nextStep: "", details: "" });
    await show();
    expect(screen.queryByText(/^Picture and video models:/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Manage in Studio" })).toBeNull();
  });
});

// ---- A model this version can't open yet ---------------------------------------------
//
// The owner's Installed row said "The bundled llama.cpp cannot load inkling2_loop." in red,
// then, in the same row, "Fits with limits", an enabled "Download the image file (639 MB)" and
// "Use" dimmed with no word of why.

const FUTURE: InstalledModel = {
  id: 7,
  path: "/models/someone/inkling-next-GGUF/next-Q6_K.gguf",
  repo: "someone/inkling-next-GGUF",
  fileName: "next-Q6_K.gguf",
  sizeBytes: 23_490_083_680,
  source: "download",
  projectorPath: null,
  summary: {
    architecture: "futurearch",
    quantization: "Q6_K",
    sizeLabel: "26B",
    contextLength: 1_048_576,
    templateTools: true,
    templateThinking: false,
    isMoe: true,
  },
  architectureSupported: false,
  imageFileOffer: { kind: "available", file: "mmproj-F16.gguf", sizeBytes: 639_348_160 },
};

const UPDATES: UpdateStatus = {
  configured: true,
  autoCheck: true,
  currentVersion: "0.2.1",
  checking: false,
  checked: true,
  downloading: null,
  available: null,
  ready: null,
  error: null,
};

async function rowOf(m: InstalledModel) {
  await show();
  const row = screen.getByText(m.fileName).closest("li");
  if (!row) throw new Error("no row");
  return row;
}

describe("Models → Installed: a model this version can't open yet", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listInstalledModels.mockResolvedValue([SMALL, FUTURE]);
    // What the memory check says is a guess for a kind it does not know: yellow here.
    ipcMock.modelLoadInfo.mockImplementation(async (path) =>
      path === FUTURE.path ? info(path, { fit: "yellow" }) : info(path),
    );
    ipcMock.updateStatus.mockResolvedValue(UPDATES);
    useApp.setState({ runtime: null, activeChatId: 3, detail: null, error: null });
    useCompare.setState({ entries: [] });
    studioApi.studioLocalModels.mockResolvedValue(LOCAL);
  });
  afterEach(cleanup);

  it("says in plain words that it can't open yet, and that a newer version may", async () => {
    const row = await rowOf(FUTURE);
    expect(row.textContent).toContain(
      "Can't open yet: this version of harnessUI does not know the “futurearch” kind of model. A newer version may open it.",
    );
    expect(row.textContent).not.toMatch(/llama|architecture|bundled/i);
  });

  it("gives no memory verdict for it, and keeps the verdict of the model that can open", async () => {
    const row = await rowOf(FUTURE);
    expect(row.textContent).not.toMatch(/Fits|Too large/);
    expect(screen.getAllByText("Fits")).toHaveLength(1);
  });

  it("offers no image file for it, not even one its repository has", async () => {
    const row = await rowOf(FUTURE);
    expect(row.textContent).not.toMatch(/image file/i);
    expect(ipcMock.imageFileOffer).not.toHaveBeenCalled();
    expect(ipcMock.downloadImageFile).not.toHaveBeenCalled();
  });

  it("turns Use off with the reason beside it, readable by a screen reader", async () => {
    const row = await rowOf(FUTURE);
    const use = Array.from(row.querySelectorAll("button")).find((b) => b.textContent === "Use") as HTMLButtonElement;
    expect(use.disabled).toBe(true);
    expect(use.getAttribute("title")).toBe("This version can't open this model yet");
    const why = row.querySelector(`[id="${use.getAttribute("aria-describedby")}"]`);
    expect(why?.textContent).toContain("Can't open yet: this version of harnessUI");
    // The model that can open keeps a plain Use.
    const other = screen.getByText("small.gguf").closest("li")!;
    const useOther = Array.from(other.querySelectorAll("button")).find((b) => b.textContent === "Use") as HTMLButtonElement;
    expect(useOther.disabled).toBe(false);
    expect(useOther.getAttribute("aria-describedby")).toBeNull();
  });

  it("keeps Add to comparison, Settings and Delete, so a model can be compared or removed", async () => {
    const row = await rowOf(FUTURE);
    for (const name of [`Add to comparison: ${FUTURE.fileName}`, `Settings for this model: ${FUTURE.fileName}`, `Delete ${FUTURE.fileName}`]) {
      const b = row.querySelector(`button[aria-label="${name}"]`) as HTMLButtonElement | null;
      expect(b, name).not.toBeNull();
      expect(b!.disabled).toBe(false);
    }
  });

  it("checks for updates from the row, and says what it found", async () => {
    const row = await rowOf(FUTURE);
    const link = Array.from(row.querySelectorAll("button")).find((b) => b.textContent === "Check for updates")!;
    await click(link);
    expect(ipcMock.checkForUpdate).toHaveBeenCalledTimes(1);
    expect(row.textContent).toContain("You have the latest version (0.2.1). It can't open this model yet, so check again later.");
  });

  it("says plainly when the file names no kind, with no update to wait for", async () => {
    const kindless = { ...FUTURE, summary: { ...FUTURE.summary!, architecture: "unknown" } };
    ipcMock.listInstalledModels.mockResolvedValue([kindless]);
    const row = await rowOf(kindless);
    expect(row.textContent).toContain("This file does not say what kind of model it is, so this version can't open it.");
    expect(row.textContent).not.toContain("A newer version may open it");
    expect(Array.from(row.querySelectorAll("button")).some((b) => b.textContent === "Check for updates")).toBe(false);
  });

  it("shows the kind's own text with its invisible characters made visible", async () => {
    const odd = { ...FUTURE, summary: { ...FUTURE.summary!, architecture: "fu\u202Eture" } };
    ipcMock.listInstalledModels.mockResolvedValue([odd]);
    const row = await rowOf(odd);
    expect(row.textContent).toContain("the “fu[U+202E]ture” kind of model");
  });

  it("leaves a row whose kind nobody knows (null) as it was", async () => {
    const unknown = { ...FUTURE, architectureSupported: null, fileName: "maybe.gguf" };
    ipcMock.listInstalledModels.mockResolvedValue([unknown]);
    const row = await rowOf(unknown);
    expect(row.textContent).not.toContain("Can't open yet");
    const use = Array.from(row.querySelectorAll("button")).find((b) => b.textContent === "Use") as HTMLButtonElement;
    expect(use.disabled).toBe(false);
  });
});
