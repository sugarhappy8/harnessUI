import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ModelBusy, ModelLoadInfo, ModelLoadSettings, RuntimeStatus } from "../lib/types";

const IDLE: ModelBusy = { chatTurns: 0, modelChecks: 0, otherApps: 0 };

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  modelLoadInfo: vi.fn<(path: string) => Promise<ModelLoadInfo>>(),
  previewModelLoadSettings: vi.fn<(path: string, o: ModelLoadSettings) => Promise<ModelLoadInfo>>(),
  setModelLoadSettings: vi.fn<(path: string, o: ModelLoadSettings) => Promise<ModelLoadInfo>>(),
  resetModelLoadSettings: vi.fn<(path: string) => Promise<ModelLoadInfo>>(),
  reloadLocalModel: vi.fn<(force: boolean, whenIdle: boolean) => Promise<boolean>>(async () => true),
  loadLocalModel: vi.fn<(path: string, s?: unknown, w?: boolean) => Promise<boolean>>(async () => true),
  modelBusy: vi.fn<() => Promise<ModelBusy>>(),
  setChatModel: vi.fn(async () => undefined),
  getChat: vi.fn(async (id: number) => ({ chat: { id, modelRef: null }, messages: [] })),
  listModelOptions: vi.fn(async () => []),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { ModelSettings, QuestionHost } = await import("./ModelSettings");
const { useApp } = await import("../store/app");

const DEFAULTS: ModelLoadSettings = {
  context: "default",
  gpu: "default",
  saveMemory: "default",
  keepInMemory: "default",
  device: "default",
  thinkingLimit: "default",
};

/** The contract's `sample_info` (report-WP0-fix.md), for a model at `path`. */
function sample(path = "/models/model.gguf"): ModelLoadInfo {
  return {
    path,
    fileName: path.split("/").pop() ?? path,
    sizeBytes: 4_920_000_000,
    trainedCtx: 40_960,
    thinking: true,
    vision: false,
    overrides: { ...DEFAULTS },
    defaults: {
      ctx: 8192,
      backend: null,
      device: null,
      contextAuto: false,
      gpu: "all",
      saveMemory: false,
      keepInMemory: false,
      thinkingLimit: "noLimit",
    },
    resolved: {
      ctx: 8192,
      ctxChoice: { fixed: 8192 },
      ctxSource: "defaultSetting",
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
    contextOptions: [
      { choice: "auto", tokens: 24_576, fit: "green" },
      { choice: { fixed: 4096 }, tokens: 4096, fit: "green" },
      { choice: { fixed: 8192 }, tokens: 8192, fit: "green" },
      { choice: { fixed: 16_384 }, tokens: 16_384, fit: "green" },
      { choice: { fixed: 32_768 }, tokens: 32_768, fit: "yellow" },
      { choice: { fixed: 40_960 }, tokens: 40_960, fit: "red" },
    ],
    gpuOptions: [
      { gpu: "all", fit: "green" },
      { gpu: "fit", fit: "green" },
      { gpu: "off", fit: "yellow" },
    ],
    devices: [{ name: "Vulkan0", description: "Sample graphics card", totalMib: 16_384, freeMib: 15_000, recommended: true }],
    loaded: false,
    olderSettingsLoaded: false,
    autoCtx: null,
  };
}

function ready(path: string): RuntimeStatus {
  return {
    state: "ready",
    backend: "vulkan",
    device: "Vulkan0",
    modelPath: path,
    modelName: path.split("/").pop() ?? null,
    progress: 1,
    stage: null,
    nCtx: 8192,
    supportsTools: true,
    preserveReasoning: false,
    thinking: true,
    speedTps: null,
    slow: false,
    vision: false,
    offloadedLayers: null,
    totalLayers: null,
    error: null,
    loadStage: null,
    adjustments: [],
    ctxSource: "defaultSetting",
    settingsChanged: false,
    reloadWaiting: false,
    unloadedPath: null,
  };
}

async function openPanel(info: ModelLoadInfo, onClose = vi.fn()) {
  ipcMock.modelLoadInfo.mockResolvedValue(info);
  await act(async () => {
    render(
      <>
        <ModelSettings path={info.path} onClose={onClose} />
        <QuestionHost />
      </>,
    );
  });
  return onClose;
}

function select(label: string): HTMLSelectElement {
  return screen.getByLabelText(label) as HTMLSelectElement;
}

function optionTexts(label: string): string[] {
  return [...select(label).options].map((o) => o.textContent ?? "");
}

async function choose(label: string, value: string) {
  await act(async () => {
    fireEvent.change(select(label), { target: { value } });
  });
}

async function click(name: string) {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name }));
  });
}

describe("ModelSettings: one model's load settings in plain words", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    ipcMock.setModelLoadSettings.mockImplementation(async (path, overrides) => ({ ...sample(path), overrides }));
    ipcMock.resetModelLoadSettings.mockImplementation(async (path) => sample(path));
    ipcMock.previewModelLoadSettings.mockImplementation(async (path, overrides) => ({ ...sample(path), overrides }));
    useApp.setState({ runtime: null, activeChatId: 3, detail: null, error: null });
    Object.defineProperty(navigator, "platform", { value: "Linux x86_64", configurable: true });
  });
  afterEach(cleanup);

  it("starts every setting as Same as default, with help lines and fit marks", async () => {
    await openPanel(sample());
    expect(screen.getByText("Settings for this model")).toBeTruthy();
    expect(screen.getByText("model.gguf")).toBeTruthy();
    expect(select("How much it can remember").value).toBe("default");
    expect(optionTexts("How much it can remember")).toEqual([
      "Same as default (8,192 tokens, about 6,000 words)",
      "Automatic: 24,576 tokens, about 18,000 words · Fits",
      "4,096 tokens, about 3,000 words · Fits",
      "8,192 tokens, about 6,000 words · Fits",
      "16,384 tokens, about 12,000 words · Fits",
      "32,768 tokens, about 24,000 words · Fits with limits",
      "40,960 tokens, about 30,000 words · Too large",
    ]);
    expect(optionTexts("Use the graphics card")).toEqual([
      "Same as default (All of it)",
      "All of it · Fits",
      "As much as fits · Fits",
      "Don't use it · Fits with limits",
    ]);
    expect(optionTexts("Save memory")).toEqual(["Same as default (Off)", "On", "Off"]);
    expect(optionTexts("Keep the model in memory")).toEqual(["Same as default (Off)", "On", "Off"]);
    expect(optionTexts("How long it may think")).toEqual(["Same as default (no limit)", "Default", "Short", "Medium", "Long"]);
    const text = document.body.textContent ?? "";
    expect(text).toContain(
      "How much of the chat the model can see at once. When a chat gets longer than this, older messages are summarised automatically so it can keep going.",
    );
    expect(text).toContain("The graphics card is much faster than the processor.");
    expect(text).toContain("Lets the model remember more in the same memory.");
    expect(text).toContain("Not recommended with 16 GB or less.");
    expect(text).toContain("Longer thinking can give better answers but takes more time.");
    expect(text).toContain("The most this model can remember is 40,960 tokens, about 30,000 words.");
    // One graphics card: no "Which graphics card", and never a device id on screen.
    expect(screen.queryByLabelText("Which graphics card")).toBeNull();
    expect(text).not.toMatch(/Vulkan0|vulkan/);
    // Not loaded: it offers to load with these settings, and Save waits for a change.
    expect(screen.getByRole("button", { name: "Load with these settings" })).toBeTruthy();
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Reset to defaults" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("saves only what changed, as typed choices", async () => {
    await openPanel(sample());
    await choose("How much it can remember", "fixed:16384");
    await choose("Save memory", "on");
    await click("Save");
    expect(ipcMock.setModelLoadSettings).toHaveBeenCalledWith("/models/model.gguf", {
      ...DEFAULTS,
      context: { fixed: 16_384 },
      saveMemory: "on",
    });
    expect(screen.getByRole("status").textContent).toContain("Saved.");
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("says what it is doing while a slow command runs, with the buttons off", async () => {
    let finish: (info: ModelLoadInfo) => void = () => undefined;
    ipcMock.setModelLoadSettings.mockImplementation(
      (path, overrides) => new Promise((resolve) => (finish = () => resolve({ ...sample(path), overrides }))),
    );
    await openPanel(sample());
    await choose("Save memory", "on");
    await click("Save");
    expect(screen.getByRole("status").textContent).toBe("Saving…");
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Load with these settings" }) as HTMLButtonElement).disabled).toBe(true);
    expect((select("Save memory") as HTMLSelectElement).disabled).toBe(true);
    await act(async () => finish(sample()));
    expect(screen.getByRole("status").textContent).toContain("Saved.");
    expect((select("Save memory") as HTMLSelectElement).disabled).toBe(false);

    let reset: (info: ModelLoadInfo) => void = () => undefined;
    ipcMock.resetModelLoadSettings.mockImplementation((path) => new Promise((resolve) => (reset = () => resolve(sample(path)))));
    await click("Reset to defaults");
    expect(screen.getByRole("status").textContent).toBe("Going back to the defaults…");
    await act(async () => reset(sample()));
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("shows what Automatic chose, the too-large line and the one-time upgrade note", async () => {
    const info = sample();
    info.defaults.upgradeNote = 16_384;
    info.tooLarge = false;
    await openPanel(info);
    const text = () => document.body.textContent ?? "";
    expect(text()).not.toContain("Automatic: 24,576 tokens, about 18,000 words, on this computer right now");
    expect(text()).toContain(
      "Every model now uses the size you chose before (16,384 tokens, about 12,000 words). Choose Automatic to let the app pick the largest size that fits.",
    );
    await choose("How much it can remember", "auto");
    expect(text()).toContain("Automatic: 24,576 tokens, about 18,000 words, on this computer right now");
    expect(text()).not.toContain("Every model now uses the size you chose before");
    cleanup();

    const big = sample();
    big.tooLarge = true;
    big.fit = "red";
    await openPanel(big);
    expect(screen.getByRole("alert").textContent).toBe(
      "Too large for this computer, even with the shortest memory. Try Save memory, As much as fits, or a smaller download.",
    );
  });

  it("Apply and restart saves, then restarts the loaded model without force", async () => {
    const info = sample();
    info.loaded = true;
    const onClose = await openPanel(info);
    const apply = screen.getByRole("button", { name: "Apply and restart" }) as HTMLButtonElement;
    expect(apply.disabled).toBe(true);
    await choose("Keep the model in memory", "on");
    await click("Apply and restart");
    expect(ipcMock.setModelLoadSettings).toHaveBeenCalledWith("/models/model.gguf", { ...DEFAULTS, keepInMemory: "on" });
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(false, false);
    expect(onClose).toHaveBeenCalled();
  });

  it("asks the D3 question when the model is busy: Cancel keeps the panel, Restart when it ends waits", async () => {
    const info = sample();
    info.olderSettingsLoaded = true;
    useApp.setState({ runtime: ready(info.path) });
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 1, modelChecks: 0, otherApps: 0 });
    const onClose = await openPanel(info);
    expect(document.body.textContent).toContain("Settings changed. Restart the model to use them.");
    await click("Apply and restart");
    expect(screen.getByText("A reply is being written. Stop it and restart the model?")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Restart when it ends" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Stop and restart" })).toBeTruthy();
    await click("Cancel");
    expect(ipcMock.reloadLocalModel).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByText("Settings for this model")).toBeTruthy();

    await click("Apply and restart");
    await click("Restart when it ends");
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(false, true);
    expect(onClose).toHaveBeenCalled();
  });

  it("keeps the D3 question current while it is open, asking again what uses the model", async () => {
    vi.useFakeTimers();
    try {
      const info = sample();
      info.olderSettingsLoaded = true;
      useApp.setState({ runtime: ready(info.path) });
      ipcMock.modelBusy.mockResolvedValue({ chatTurns: 1, modelChecks: 0, otherApps: 0 });
      await openPanel(info);
      await click("Apply and restart");
      expect(screen.getByText("A reply is being written. Stop it and restart the model?")).toBeTruthy();
      ipcMock.modelBusy.mockResolvedValue({ chatTurns: 0, modelChecks: 1, otherApps: 0 });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1600);
      });
      expect(screen.getByText("A model check is running. Stop it and restart the model?")).toBeTruthy();
      expect(screen.queryByText("A reply is being written. Stop it and restart the model?")).toBeNull();
      await click("Cancel");
      // Closed: no more asking.
      const calls = ipcMock.modelBusy.mock.calls.length;
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5000);
      });
      expect(ipcMock.modelBusy.mock.calls.length).toBe(calls);
    } finally {
      vi.useRealTimers();
    }
  });

  it("Reset to defaults forgets the model's own settings", async () => {
    const info = sample();
    info.overrides = { ...DEFAULTS, context: { fixed: 4096 }, gpu: "fit" };
    await openPanel(info);
    expect(select("How much it can remember").value).toBe("fixed:4096");
    expect(select("Use the graphics card").value).toBe("fit");
    await click("Reset to defaults");
    expect(ipcMock.resetModelLoadSettings).toHaveBeenCalledWith("/models/model.gguf");
    expect(select("How much it can remember").value).toBe("default");
    expect(select("Use the graphics card").value).toBe("default");
  });

  it("Load with these settings saves, makes it the chat's model and loads it with no settings", async () => {
    const onClose = await openPanel(sample("/models/other.gguf"));
    await choose("How long it may think", "short");
    await click("Load with these settings");
    expect(ipcMock.setModelLoadSettings).toHaveBeenCalledWith("/models/other.gguf", { ...DEFAULTS, thinkingLimit: "short" });
    expect(ipcMock.setChatModel).toHaveBeenCalledWith(3, "local:/models/other.gguf");
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/other.gguf", null, undefined);
    expect(onClose).toHaveBeenCalled();
  });

  /** Lets the panel ask for the fit of unsaved choices (it waits a moment after a change). */
  async function settle() {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 400));
    });
  }

  /** The facts with Save memory on: more fits, and Automatic picks more. */
  function withSaveMemory(path: string, overrides: ModelLoadSettings): ModelLoadInfo {
    const info = { ...sample(path), overrides, tooLarge: false, fit: "green" as const };
    info.contextOptions = [
      { choice: "auto", tokens: 32_768, fit: "green" },
      { choice: { fixed: 4096 }, tokens: 4096, fit: "green" },
      { choice: { fixed: 8192 }, tokens: 8192, fit: "green" },
      { choice: { fixed: 16_384 }, tokens: 16_384, fit: "green" },
      { choice: { fixed: 32_768 }, tokens: 32_768, fit: "green" },
      { choice: { fixed: 40_960 }, tokens: 40_960, fit: "yellow" },
    ];
    info.gpuOptions = [
      { gpu: "all", fit: "green" },
      { gpu: "fit", fit: "green" },
      { gpu: "off", fit: "green" },
    ];
    return info;
  }

  it("moves the fit marks with unsaved choices, saves nothing, and goes back when they are undone", async () => {
    const saved = sample();
    saved.tooLarge = true;
    saved.fit = "red";
    ipcMock.previewModelLoadSettings.mockImplementation(async (path, overrides) =>
      overrides.saveMemory === "on" ? withSaveMemory(path, overrides) : { ...saved, overrides },
    );
    await openPanel(saved);
    expect(screen.getByRole("alert").textContent).toContain("Too large for this computer");
    expect(optionTexts("How much it can remember")[5]).toBe("32,768 tokens, about 24,000 words · Fits with limits");

    await choose("Save memory", "on");
    await settle();
    expect(ipcMock.previewModelLoadSettings).toHaveBeenLastCalledWith("/models/model.gguf", { ...DEFAULTS, saveMemory: "on" });
    expect(optionTexts("How much it can remember")).toEqual([
      "Same as default (8,192 tokens, about 6,000 words)",
      "Automatic: 32,768 tokens, about 24,000 words · Fits",
      "4,096 tokens, about 3,000 words · Fits",
      "8,192 tokens, about 6,000 words · Fits",
      "16,384 tokens, about 12,000 words · Fits",
      "32,768 tokens, about 24,000 words · Fits",
      "40,960 tokens, about 30,000 words · Fits with limits",
    ]);
    expect(optionTexts("Use the graphics card")[3]).toBe("Don't use it · Fits");
    // With these choices it fits: the too-large line goes, and Automatic says what it would pick.
    expect(screen.queryByRole("alert")).toBeNull();
    await choose("How much it can remember", "auto");
    await settle();
    expect(document.body.textContent).toContain("Automatic: 32,768 tokens, about 24,000 words, on this computer right now");
    expect(ipcMock.setModelLoadSettings).not.toHaveBeenCalled();

    // Undone: the saved marks again, without asking.
    const asked = ipcMock.previewModelLoadSettings.mock.calls.length;
    await choose("Save memory", "default");
    await choose("How much it can remember", "default");
    await settle();
    expect(ipcMock.previewModelLoadSettings.mock.calls.length).toBe(asked);
    expect(optionTexts("How much it can remember")[5]).toBe("32,768 tokens, about 24,000 words · Fits with limits");
    expect(screen.getByRole("alert").textContent).toContain("Too large for this computer");
  });

  it("ignores a fit answer for choices that changed since it was asked", async () => {
    let slow: (info: ModelLoadInfo) => void = () => undefined;
    ipcMock.previewModelLoadSettings.mockImplementation((path, overrides) =>
      overrides.saveMemory === "on"
        ? new Promise((resolve) => (slow = resolve))
        : Promise.resolve({ ...sample(path), overrides }),
    );
    await openPanel(sample());
    await choose("Save memory", "on");
    await settle();
    await choose("Save memory", "off");
    await settle();
    // The answer for "on" arrives after "off" was chosen: the marks stay those of "off".
    await act(async () => slow(withSaveMemory("/models/model.gguf", { ...DEFAULTS, saveMemory: "on" })));
    expect(optionTexts("How much it can remember")[5]).toBe("32,768 tokens, about 24,000 words · Fits with limits");
  });

  it("keeps the saved marks when the fit of unsaved choices cannot be read", async () => {
    ipcMock.previewModelLoadSettings.mockRejectedValue({ code: "bad_request", message: "No.", nextStep: "", details: "" });
    await openPanel(sample());
    await choose("Save memory", "on");
    await settle();
    expect(optionTexts("How much it can remember")[5]).toBe("32,768 tokens, about 24,000 words · Fits with limits");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says what the loaded model's Automatic chose until a choice changes, then what it would choose", async () => {
    const info = sample();
    info.loaded = true;
    info.autoCtx = 20_480;
    info.overrides = { ...DEFAULTS, context: "auto" };
    ipcMock.previewModelLoadSettings.mockImplementation(async (path, overrides) => withSaveMemory(path, overrides));
    await openPanel(info);
    expect(document.body.textContent).toContain("Automatic: 20,480 tokens, about 15,000 words, on this computer right now");
    await choose("Save memory", "on");
    await settle();
    expect(document.body.textContent).toContain("Automatic: 32,768 tokens, about 24,000 words, on this computer right now");
  });

  it("names graphics cards by name and memory, and says Use Apple graphics on a Mac", async () => {
    Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
    const info = sample();
    info.devices = [
      { name: "Vulkan0", description: "AMD Radeon(TM) Graphics", totalMib: 2048, freeMib: 1000, recommended: false },
      { name: "Vulkan1", description: "AMD Radeon RX 9070 XT", totalMib: 16_384, freeMib: 15_000, recommended: true },
    ];
    await openPanel(info);
    expect(screen.getByLabelText("Use Apple graphics")).toBeTruthy();
    expect(optionTexts("Which graphics card")).toEqual([
      "Same as default (Automatic)",
      "Automatic",
      "AMD Radeon(TM) Graphics · 2 GB",
      "AMD Radeon RX 9070 XT · 16 GB (recommended)",
    ]);
    await choose("Which graphics card", "named:Vulkan1");
    await click("Save");
    expect(ipcMock.setModelLoadSettings).toHaveBeenCalledWith("/models/model.gguf", {
      ...DEFAULTS,
      device: { named: "Vulkan1" },
    });
  });
});
