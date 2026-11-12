import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ChatDetail, ModelBusy, RuntimeStatus, ShareStatus } from "../lib/types";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const IDLE: ModelBusy = { chatTurns: 0, modelChecks: 0, otherApps: 0 };
const SHARE_OFF: ShareStatus = {
  enabled: false,
  port: null,
  listening: false,
  error: null,
  refused: 0,
  inUse: false,
  modelCtx: null,
};

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  modelBusy: vi.fn<() => Promise<ModelBusy>>(),
  shareStatus: vi.fn<() => Promise<ShareStatus>>(),
  reloadLocalModel: vi.fn<(force: boolean, whenIdle: boolean) => Promise<boolean>>(async () => true),
  unloadModel: vi.fn<(whenIdle?: boolean) => Promise<boolean>>(async () => true),
  loadLocalModel: vi.fn<(path: string, s?: unknown, w?: boolean) => Promise<boolean>>(async () => true),
  cancelPendingReload: vi.fn(async () => undefined),
  modelLoadInfo: vi.fn(async () => new Promise(() => undefined)),
  setChatModel: vi.fn(async () => undefined),
  getChat: vi.fn(async (id: number) => ({ chat: { id, modelRef: "local:/models/a.gguf" }, messages: [] })),
  listModelOptions: vi.fn(async () => []),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { RuntimeIndicator, runtimeNotes } = await import("./RuntimeIndicator");
const { useApp } = await import("../store/app");
const { useRestartNote, useWaiting } = await import("../store/modelChange");

function status(patch: Partial<RuntimeStatus> = {}): RuntimeStatus {
  return {
    state: "ready",
    backend: "vulkan",
    device: "Vulkan0",
    modelPath: "/models/a.gguf",
    modelName: "a.gguf",
    progress: 1,
    stage: null,
    nCtx: 32_768,
    supportsTools: true,
    preserveReasoning: false,
    thinking: true,
    speedTps: 30,
    slow: false,
    vision: false,
    offloadedLayers: null,
    totalLayers: null,
    error: null,
    loadStage: null,
    adjustments: [],
    ctxSource: "automatic",
    settingsChanged: false,
    reloadWaiting: false,
    unloadedPath: null,
    ...patch,
  };
}

function chat(modelRef: string | null, messages = 0): ChatDetail {
  return {
    chat: {
      id: 3,
      title: "t",
      modelRef,
      systemPrompt: null,
      leafMessageId: null,
      createdAt: 0,
      updatedAt: 0,
      thinking: true,
      workingFolder: null,
    },
    messages: Array.from({ length: messages }, (_, i) => ({ id: i + 1 }) as ChatDetail["messages"][number]),
  };
}

async function show(rt: RuntimeStatus | null, detail: ChatDetail | null = chat("local:/models/a.gguf")) {
  useApp.setState({ runtime: rt, detail, activeChatId: detail?.chat.id ?? null, generation: null, error: null });
  await act(async () => {
    render(<RuntimeIndicator />);
  });
}

async function openMenu() {
  await act(async () => {
    fireEvent.keyDown(screen.getByRole("button", { name: "Model menu" }), { key: "Enter" });
  });
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
}

const text = () => document.body.textContent ?? "";

describe("RuntimeIndicator: the local model's state in plain words", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    ipcMock.shareStatus.mockResolvedValue(SHARE_OFF);
    useWaiting.setState({ change: null });
    useRestartNote.setState({ shown: false });
  });
  afterEach(cleanup);

  it("shows nothing for a chat with an endpoint model", async () => {
    await show(status(), chat("provider:1:x"));
    expect(text()).toBe("");
  });

  it("words the loading stages, keeping the 'Loading model' start", async () => {
    await show(status({ state: "starting", progress: 0.45, loadStage: { kind: "readingModel" }, stage: "text_model" }));
    expect(text()).toContain("Loading model · Reading the model file 45%");
    expect(text()).not.toMatch(/text.model/);
    cleanup();
    await show(
      status({
        state: "starting",
        progress: null,
        loadStage: { kind: "steppingDown", from: "vulkan", to: "cpu", reason: { code: "out_of_memory" } },
      }),
    );
    expect(text()).toContain("Loading model · The graphics card ran out of memory, so trying the processor…");
  });

  it("shows the ready line with its reason and opens the panel from Change", async () => {
    const opened = vi.fn();
    window.addEventListener("app:model-settings", opened);
    await show(status());
    expect(text()).toContain("Remembers about 24,000 words (Automatic)");
    expect(text()).not.toMatch(/vulkan|Vulkan0|context/);
    await click(screen.getByRole("button", { name: "Change" }));
    expect((opened.mock.calls[0]?.[0] as CustomEvent).detail).toBe("/models/a.gguf");
    window.removeEventListener("app:model-settings", opened);
  });

  it("has a menu with Reload, Model settings… and Unload to free memory", async () => {
    await show(status());
    await openMenu();
    expect(text()).toContain("runs on Graphics card (Vulkan)");
    expect(screen.getByRole("menuitem", { name: /Reload/ })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: /Model settings/ })).toBeTruthy();
    await click(screen.getByRole("menuitem", { name: /Reload/ }));
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(true, false);
    await openMenu();
    await click(screen.getByRole("menuitem", { name: /Unload to free memory/ }));
    expect(ipcMock.unloadModel).toHaveBeenCalledWith(undefined);
  });

  it("asks the D3 question from the menu while a reply is being written", async () => {
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 1, modelChecks: 0, otherApps: 0 });
    await show(status());
    await openMenu();
    await click(screen.getByRole("menuitem", { name: /Reload/ }));
    expect(screen.getByText("A reply is being written. Stop it and restart the model?")).toBeTruthy();
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(ipcMock.reloadLocalModel).not.toHaveBeenCalled();
    await openMenu();
    await click(screen.getByRole("menuitem", { name: /Unload to free memory/ }));
    await click(screen.getByRole("button", { name: "Stop and restart" }));
    expect(ipcMock.unloadModel).toHaveBeenCalledWith(false);
  });

  it("shows the waiting line with Cancel while a change waits", async () => {
    await show(status({ reloadWaiting: true }));
    expect(screen.getByRole("status").textContent).toContain("Restarts when the reply ends");
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(ipcMock.cancelPendingReload).toHaveBeenCalled();
    expect(ipcMock.modelBusy).toHaveBeenCalled();
    cleanup();
    useWaiting.setState({ change: "unload" });
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 0, modelChecks: 1, otherApps: 0 });
    await show(status({ reloadWaiting: true }));
    expect(screen.getByRole("status").textContent).toContain("Unloads when the model check ends");
  });

  it("does not poll the busy counts when nothing waits", async () => {
    await show(status());
    expect(ipcMock.modelBusy).not.toHaveBeenCalled();
    expect(ipcMock.shareStatus).toHaveBeenCalled();
  });

  it("says when the settings changed, and Apply and restart restarts without force", async () => {
    await show(status({ settingsChanged: true }));
    expect(screen.getByRole("status").textContent).toContain("Settings changed. Restart the model to use them.");
    await click(screen.getByRole("button", { name: "Apply and restart" }));
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(false, false);
  });

  it("explains a step-down with Reload, and a partial load and slowness with Model settings…", async () => {
    await show(
      status({
        backend: "cpu",
        adjustments: [{ kind: "steppingDown", from: "vulkan", to: "cpu", reason: { code: "out_of_memory" } }],
      }),
    );
    expect(screen.getByRole("status").textContent).toContain(
      "Running on the processor because the graphics card ran out of memory. Reload to try the graphics card again.",
    );
    await click(screen.getByRole("button", { name: "Reload" }));
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(true, false);
    cleanup();
    await show(status({ offloadedLayers: 20, totalLayers: 33 }));
    expect(screen.getByRole("status").textContent).toContain(
      "Part of the model runs on the processor, so replies are slower. Save memory or a smaller memory size lets more fit on the graphics card.",
    );
    expect(screen.getByRole("button", { name: "Model settings…" })).toBeTruthy();
    cleanup();
    await show(status({ slow: true, speedTps: 2.7 }));
    expect(screen.getByRole("status").textContent).toContain("Replies are slow here (about 2 words a second).");
  });

  it("lists every note in order, most important first", () => {
    const notes = runtimeNotes(
      status({
        settingsChanged: true,
        offloadedLayers: 10,
        totalLayers: 20,
        slow: true,
        speedTps: 4,
        adjustments: [{ kind: "retryingWithoutSaveMemory", backend: "vulkan" }],
      }),
      { restarted: true, hasMessages: true },
    );
    expect(notes.map((n) => n.action)).toEqual(["apply", "settings", "settings", "settings", null]);
    expect(notes[1]?.text).toBe("Save memory doesn't work with this graphics card, so it was turned off for this load.");
    expect(notes[4]?.text).toBe("The first reply after a restart takes longer while the model rereads the chat.");
    expect(runtimeNotes(status(), { restarted: true, hasMessages: false })).toEqual([]);
  });

  it("notes the slower first reply after a restart until a reply ends", async () => {
    useRestartNote.setState({ shown: true });
    await show(status(), chat("local:/models/a.gguf", 2));
    expect(screen.getByRole("status").textContent).toContain("The first reply after a restart takes longer");
    await act(async () => {
      useApp.setState({ generation: { chatId: 3 } as never });
    });
    await act(async () => {
      useApp.setState({ generation: null });
    });
    expect(useRestartNote.getState().shown).toBe(false);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("says Used by another app from the sharing status", async () => {
    ipcMock.shareStatus.mockResolvedValue({ ...SHARE_OFF, enabled: true, port: 51234, listening: true, inUse: true });
    await show(status());
    expect(text()).toContain("Used by another app");
  });

  it("offers Model settings… after a failed load, and Load after Unload", async () => {
    await show(
      status({
        state: "failed",
        error: { code: "load_failed", message: "This model doesn't fit.", nextStep: "", details: "" },
      }),
    );
    expect(text()).toContain("Didn't load");
    expect(screen.getByRole("button", { name: "Model settings…" })).toBeTruthy();
    await click(screen.getByRole("button", { name: "Try again" }));
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/a.gguf", null, undefined);
    cleanup();
    vi.clearAllMocks();
    await show(status({ state: "stopped", modelPath: null, modelName: null, nCtx: null, unloadedPath: "/models/a.gguf" }));
    expect(text()).toContain("Unloaded to free memory");
    await click(screen.getByRole("button", { name: "Load" }));
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/a.gguf", null, undefined);
  });
});
