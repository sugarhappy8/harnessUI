import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ModelBusy, ModelLoadInfo, ModelLoadSettings, OffloadPreview, RuntimeStatus } from "../lib/types";
import { FINE_LAUNCH_NONE, computerBar, fineFacts, gb, graphicsBar, picture, processorPicture } from "./fine.fixtures";

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
  offloadMeasure: vi.fn<(path: string, o: ModelLoadSettings) => Promise<OffloadPreview>>(),
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
      fine: FINE_LAUNCH_NONE,
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
    fineFacts: fineFacts(),
    picture: processorPicture(),
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
    placement: null,
  };
}

/** The info of the panel opened last (whether it is the loaded model). */
let opened: ModelLoadInfo = sample();

/**
 * What the backend answers for `overrides` (saved, or asked about unsaved): as with WP1b's
 * `same_launch`, the loaded copy is older when a launch setting differs, and How long it
 * may think is not one (it is a per-request limit).
 */
function answerFor(path: string, overrides: ModelLoadSettings): ModelLoadInfo {
  const launch = (Object.keys(overrides) as (keyof ModelLoadSettings)[]).some(
    (k) => k !== "thinkingLimit" && overrides[k] !== "default",
  );
  return { ...sample(path), overrides, loaded: opened.loaded, olderSettingsLoaded: opened.loaded && launch };
}

async function openPanel(info: ModelLoadInfo, onClose = vi.fn()) {
  opened = info;
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
    ipcMock.setModelLoadSettings.mockImplementation(async (path, overrides) => answerFor(path, overrides));
    ipcMock.resetModelLoadSettings.mockImplementation(async (path) => sample(path));
    ipcMock.previewModelLoadSettings.mockImplementation(async (path, overrides) => answerFor(path, overrides));
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
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(false, true);
    expect(onClose).toHaveBeenCalled();
  });

  it("restarts nothing for a change that needs no restart: How long it may think is saved for the next reply", async () => {
    vi.useFakeTimers();
    try {
      const info = sample();
      info.loaded = true;
      useApp.setState({ runtime: ready(info.path) });
      ipcMock.modelBusy.mockResolvedValue({ chatTurns: 1, modelChecks: 0, otherApps: 0 });
      const onClose = await openPanel(info);
      await choose("How long it may think", "short");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(200);
      });
      const apply = screen.getByRole("button", { name: "Apply and restart" }) as HTMLButtonElement;
      expect(apply.disabled).toBe(true);
      expect(apply.title).toBe("This change needs no restart. Save keeps it.");
      await click("Save");
      expect(ipcMock.setModelLoadSettings).toHaveBeenCalledWith("/models/model.gguf", { ...DEFAULTS, thinkingLimit: "short" });
      expect(screen.getByRole("status").textContent).toBe("Saved. They are used from the next reply.");
      expect(ipcMock.modelBusy).not.toHaveBeenCalled();
      expect(ipcMock.reloadLocalModel).not.toHaveBeenCalled();
      expect(onClose).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("Apply and restart before the fit answer came: saves, and restarts only when the backend says the loaded copy is older", async () => {
    const info = sample();
    info.loaded = true;
    useApp.setState({ runtime: ready(info.path) });
    const onClose = await openPanel(info);
    // "Same as default (Off)" changed to "Off" resolves to what is loaded: nothing to restart.
    ipcMock.setModelLoadSettings.mockImplementationOnce(async (path, overrides) => ({
      ...answerFor(path, overrides),
      olderSettingsLoaded: false,
    }));
    await choose("Save memory", "off");
    await click("Apply and restart");
    expect(ipcMock.setModelLoadSettings).toHaveBeenCalledWith("/models/model.gguf", { ...DEFAULTS, saveMemory: "off" });
    expect(ipcMock.reloadLocalModel).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("status").textContent).toBe("Saved. They are used from the next reply.");
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
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/other.gguf", null, true);
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

describe("ModelSettings: Fine control and where the model goes (0.0.6 item 6)", () => {
  /** A model on a graphics card: 41 layers, the picture an instant estimate that fits. */
  function onCard(path = "/models/model.gguf"): ModelLoadInfo {
    return {
      ...sample(path),
      fineFacts: fineFacts(),
      picture: picture({ source: { kind: "estimate", why: "quick" }, custom: false, layersOnGraphics: 41 }),
    };
  }

  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    ipcMock.setModelLoadSettings.mockImplementation(async (path, overrides) => ({ ...onCard(path), overrides }));
    ipcMock.resetModelLoadSettings.mockImplementation(async (path) => onCard(path));
    ipcMock.previewModelLoadSettings.mockImplementation(async (path, overrides) => ({ ...opened, path, overrides }));
    ipcMock.offloadMeasure.mockImplementation(async () => picture({ custom: false, layersOnGraphics: 41 }));
    useApp.setState({ runtime: null, activeChatId: 3, detail: null, error: null });
    Object.defineProperty(navigator, "platform", { value: "Linux x86_64", configurable: true });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  const text = () => document.body.textContent ?? "";

  function fineSection(): HTMLDetailsElement {
    return document.querySelector('details[data-section="fine"]') as HTMLDetailsElement;
  }

  async function openFine() {
    await act(async () => {
      const d = fineSection();
      d.open = true;
      d.dispatchEvent(new Event("toggle"));
    });
  }

  async function wait(ms: number) {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  }

  it("shows the picture after the plain settings, and Fine control closed after More options", async () => {
    await openPanel(onCard());
    const heading = screen.getByRole("heading", { name: "Where the model goes" });
    const keep = select("Keep the model in memory");
    const more = [...document.querySelectorAll("summary")].find((x) => x.textContent === "More options") as HTMLElement;
    const fine = fineSection();
    const before = (a: Node, b: Node) => !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    expect(before(keep, heading)).toBe(true);
    expect(before(heading, more)).toBe(true);
    expect(before(more, fine)).toBe(true);
    expect(fine.open).toBe(false);
    expect(fine.querySelector("summary")?.textContent).toBe("Fine control · All 41 layers on the graphics card");
    expect(text()).toContain("Rough estimate from the model file. Open Fine control to work out exact figures.");
    // Closed: nothing is measured.
    expect(ipcMock.offloadMeasure).not.toHaveBeenCalled();
  });

  it("works out exact figures only while Fine control is open, a moment after the last change, for the choices on screen", async () => {
    vi.useFakeTimers();
    await openPanel(onCard());
    let answer: (p: OffloadPreview) => void = () => undefined;
    ipcMock.offloadMeasure.mockImplementation(() => new Promise((resolve) => (answer = resolve)));
    await openFine();
    expect(text()).toContain("Working out exact figures…");
    await wait(500);
    expect(ipcMock.offloadMeasure).not.toHaveBeenCalled();
    await wait(200);
    expect(ipcMock.offloadMeasure).toHaveBeenCalledWith("/models/model.gguf", DEFAULTS);
    await act(async () => answer(picture({ custom: false, layersOnGraphics: 41 })));
    expect(text()).toContain("Worked out for these settings.");

    // A change: the estimate shows again until the new choices are measured; an answer for
    // the old choices that arrives late is not used.
    let late: (p: OffloadPreview) => void = () => undefined;
    ipcMock.offloadMeasure.mockImplementation(() => new Promise((resolve) => (late = resolve)));
    await choose("Processor threads", "count");
    expect(text()).toContain("Working out exact figures…");
    expect(text()).not.toContain("Worked out for these settings.");
    await wait(700);
    expect(ipcMock.offloadMeasure).toHaveBeenLastCalledWith("/models/model.gguf", {
      ...DEFAULTS,
      fine: { layers: "auto", experts: "auto", kv: "auto", threads: { count: 8 }, fastAttention: "auto", leaveFree: "auto" },
    });
    await choose("Fast attention", "off");
    await act(async () => late(picture({ custom: false, layersOnGraphics: 41 })));
    expect(text()).toContain("Working out exact figures…");
    expect(fineSection().querySelector("summary")?.textContent).toContain(" · changed");
  });

  it("drops a measurement whose key is not the key of the choices on screen", async () => {
    vi.useFakeTimers();
    ipcMock.offloadMeasure.mockResolvedValue(picture({ key: "another draft", custom: false, layersOnGraphics: 41 }));
    await openPanel(onCard());
    await openFine();
    await wait(700);
    expect(ipcMock.offloadMeasure).toHaveBeenCalled();
    expect(text()).toContain("Working out exact figures…");
    expect(text()).not.toContain("Worked out for these settings.");
  });

  it("says why when exact figures could not be asked for", async () => {
    vi.useFakeTimers();
    ipcMock.offloadMeasure.mockRejectedValue({ code: "internal", message: "No.", nextStep: "", details: "" });
    await openPanel(onCard());
    await openFine();
    await wait(700);
    expect(text()).toContain("Rough estimate from the model file. Exact figures couldn't be worked out for this file.");
  });

  it("lets the measurement replace the marks of the selected choices only", async () => {
    vi.useFakeTimers();
    ipcMock.offloadMeasure.mockResolvedValue(picture({ custom: false, layersOnGraphics: 41, mark: "yellow", fits: "tight" }));
    await openPanel(onCard());
    expect(optionTexts("How much it can remember")[3]).toBe("8,192 tokens, about 6,000 words · Fits");
    expect(optionTexts("Use the graphics card")[1]).toBe("All of it · Fits");
    await openFine();
    await wait(700);
    // The resolved choices (8,192 tokens, All of it) take the measured mark; the others keep theirs.
    expect(optionTexts("How much it can remember")[3]).toBe("8,192 tokens, about 6,000 words · Fits with limits");
    expect(optionTexts("How much it can remember")[4]).toBe("16,384 tokens, about 12,000 words · Fits");
    expect(optionTexts("Use the graphics card")[1]).toBe("All of it · Fits with limits");
    expect(optionTexts("Use the graphics card")[2]).toBe("As much as fits · Fits");
    // ...and its warning replaces the estimate's.
    expect(screen.getByRole("alert").textContent).toBe(
      "It only just fits. Programs that use the graphics card, such as games or video, may stop it from loading.",
    );
  });

  it("shows a Custom option while Fine control chooses the layers, and any plain choice goes back to it", async () => {
    await openPanel(onCard());
    await openFine();
    await choose("Layers on the graphics card", "count");
    const gpu = select("Use the graphics card");
    expect(gpu.value).toBe("custom");
    const custom = [...gpu.options].find((o) => o.value === "custom") as HTMLOptionElement;
    expect(custom.textContent).toBe("Custom: 41 of 41 layers (set in Fine control)");
    expect(custom.disabled).toBe(true);
    expect(text()).toContain("Fine control sets where the model goes. Choose another option here to go back to it.");
    await act(async () => {
      fireEvent.change(screen.getByLabelText("Layers on the graphics card: how many"), { target: { value: "30" } });
    });
    expect(custom.textContent).toBe("Custom: 30 of 41 layers (set in Fine control)");

    await choose("Use the graphics card", "all");
    expect(select("Use the graphics card").value).toBe("all");
    expect(select("Layers on the graphics card").value).toBe("auto");
    expect([...select("Use the graphics card").options].some((o) => o.value === "custom")).toBe(false);
    await click("Save");
    // Fine control all Automatic is not sent at all.
    expect(ipcMock.setModelLoadSettings).toHaveBeenLastCalledWith("/models/model.gguf", { ...DEFAULTS, gpu: "all" });

    // Expert parts only, then "Same as default (…)": back to the plain placement too.
    cleanup();
    await openPanel({ ...onCard(), fineFacts: fineFacts({ expertLayers: 40 }) });
    await openFine();
    await choose("Expert parts on the processor", "all");
    expect(select("Use the graphics card").value).toBe("custom");
    expect(optionTexts("Use the graphics card")).toContain("Custom: expert parts on the processor (set in Fine control)");
    await choose("Use the graphics card", "default");
    expect(select("Use the graphics card").value).toBe("default");
    expect(select("Expert parts on the processor").value).toBe("auto");
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows no Custom option when the defaults for all models say Don't use it, and greys the placement out", async () => {
    const info = onCard();
    info.defaults.gpu = "off";
    info.overrides = { ...DEFAULTS, fine: { layers: { count: 20 }, experts: "auto", kv: "auto", threads: "auto", fastAttention: "auto", leaveFree: "auto" } };
    await openPanel(info);
    await openFine();
    expect(select("Use the graphics card").value).toBe("default");
    expect([...select("Use the graphics card").options].some((o) => o.value === "custom")).toBe(false);
    expect(select("Layers on the graphics card").disabled).toBe(true);
    expect(screen.getAllByText("Not used while Use the graphics card is Don't use it.").length).toBeGreaterThan(0);
  });

  it("shows one warning line, the most important first", async () => {
    const cases: [Partial<ModelLoadInfo>, Partial<OffloadPreview>, string | null][] = [
      [{ tooLarge: true }, { fits: "wontFitAtAll" }, "Too large for this computer, even with the shortest memory."],
      [
        {},
        {
          fits: "wontFitAtAll",
          graphics: graphicsBar({ need: { model: gb(16), context: gb(1), compute: gb(1), imageReader: 0 } }),
          computer: computerBar({ freeBytes: gb(15.2), need: { model: gb(18), context: gb(1), compute: gb(1), imageReader: 0 } }),
        },
        "This probably needs more memory than this computer has free: about 38 GB, with 30.2 GB free.",
      ],
      [
        {},
        { fits: "wontFitGraphics", custom: false, graphics: graphicsBar({ need: { model: gb(13.8), context: gb(1.2), compute: gb(0.6), imageReader: 0 } }) },
        "This probably won't fit in the graphics card's memory: it needs about 15.6 GB and 15 GB is free. Choose As much as fits, or turn on Save memory.",
      ],
      [{}, { fits: "tight" }, "It only just fits."],
      [{}, { fits: "partial" }, null],
      [{}, { fits: "fits" }, null],
    ];
    for (const [patch, pic, words] of cases) {
      const info = { ...onCard(), ...patch };
      info.picture = { ...info.picture, ...pic };
      await openPanel(info);
      const alerts = screen.queryAllByRole("alert");
      if (words === null) expect(alerts.length).toBe(0);
      else {
        expect(alerts.length).toBe(1);
        expect(alerts[0]?.textContent).toContain(words);
      }
      cleanup();
    }
  });

  it("asks before loading what probably won't fit on the graphics card, with Cancel focused", async () => {
    const info = onCard("/models/other.gguf");
    info.picture = { ...info.picture, fits: "wontFitGraphics" };
    await openPanel(info);
    await click("Load with these settings");
    const question =
      "This probably won't fit in the graphics card's memory. If it doesn't, the model won't load, or part of it moves to the processor, which is slower.";
    expect(screen.getByText(question)).toBeTruthy();
    expect(document.activeElement?.textContent).toBe("Cancel");
    await click("Cancel");
    expect(ipcMock.loadLocalModel).not.toHaveBeenCalled();
    expect(ipcMock.setChatModel).not.toHaveBeenCalled();
    await click("Load with these settings");
    await click("Load anyway");
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/other.gguf", null, true);
  });

  it("asks WP2's question before loading what probably won't fit at all", async () => {
    const info = onCard("/models/other.gguf");
    info.picture = { ...info.picture, fits: "wontFitAtAll" };
    await openPanel(info);
    await click("Load with these settings");
    expect(screen.getByText("This model probably needs more memory than this computer has. Load it anyway?")).toBeTruthy();
    await click("Cancel");
    expect(ipcMock.loadLocalModel).not.toHaveBeenCalled();
  });

  it("asks before Apply and restart too, and loads nothing on Cancel", async () => {
    const info = onCard();
    info.loaded = true;
    info.olderSettingsLoaded = true;
    info.picture = { ...info.picture, fits: "wontFitGraphics" };
    useApp.setState({ runtime: ready(info.path) });
    await openPanel(info);
    await click("Apply and restart");
    expect(screen.getByText(/This probably won't fit in the graphics card's memory\. If it doesn't/)).toBeTruthy();
    await click("Cancel");
    expect(ipcMock.reloadLocalModel).not.toHaveBeenCalled();
    await click("Apply and restart");
    await click("Load anyway");
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(false, true);
  });

  it("counts Fine control all Automatic the same as none", async () => {
    const info = onCard();
    // The backend leaves an all-Automatic Fine control out; here it is sent back in full.
    info.overrides = { ...DEFAULTS, fine: { layers: "auto", experts: "auto", kv: "auto", threads: "auto", fastAttention: "auto", leaveFree: "auto" } };
    await openPanel(info);
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    await openFine();
    await choose("Processor threads", "count");
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(false);
    await click("Reset fine control");
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Reset to defaults" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("is hidden for a backend without item 6 (no picture, no Fine control)", async () => {
    const old = sample() as Partial<ModelLoadInfo>;
    delete old.picture;
    delete old.fineFacts;
    await openPanel(old as ModelLoadInfo);
    expect(screen.queryByText("Where the model goes")).toBeNull();
    expect(fineSection()).toBeNull();
    expect(select("How much it can remember").value).toBe("default");
  });
});
