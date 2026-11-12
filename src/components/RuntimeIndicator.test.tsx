import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ChatDetail, LoadFailure, ModelBusy, RuntimeStatus, ShareStatus } from "../lib/types";

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
const { useApprovalMode } = await import("../lib/approvalMode");

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
    useWaiting.setState({ change: null, kind: null });
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
    window.addEventListener("app:open-model-settings", opened);
    await show(status());
    expect(text()).toContain("Remembers about 24,000 words (Automatic)");
    expect(text()).not.toMatch(/vulkan|Vulkan0|context/);
    // A pointer shortcut (the keyboard and screen readers use the menu's Model settings…).
    const change = screen.getByRole("button", { name: "Change", hidden: true });
    expect(change.tabIndex).toBe(-1);
    await click(change);
    expect((opened.mock.calls[0]?.[0] as CustomEvent).detail).toBe("/models/a.gguf");
    window.removeEventListener("app:open-model-settings", opened);
  });

  it("hosts the settings panel for anything that asks for it, even with an endpoint chat", async () => {
    await show(status(), chat("provider:1:x"));
    await act(async () => {
      window.dispatchEvent(new CustomEvent("app:open-model-settings", { detail: "/models/b.gguf" }));
    });
    expect(screen.getByText("Settings for this model")).toBeTruthy();
    expect(ipcMock.modelLoadInfo).toHaveBeenCalledWith("/models/b.gguf");
  });

  it("has a menu with Reload, Model settings… and Unload to free memory", async () => {
    await show(status());
    // The whole line, reason included, even when a narrow header shortens it.
    expect(screen.getByRole("button", { name: "Model menu" }).title).toBe(
      "Remembers about 24,000 words (Automatic)\n/models/a.gguf",
    );
    await openMenu();
    expect(text()).toContain("runs on Graphics card (Vulkan)");
    expect(screen.getByRole("menu").textContent).toContain("Remembers about 24,000 words (Automatic)");
    expect(screen.getByRole("menuitem", { name: /Reload/ })).toBeTruthy();
    expect(screen.getByRole("menuitem", { name: /Model settings/ })).toBeTruthy();
    await click(screen.getByRole("menuitem", { name: /Reload/ }));
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(true, true);
    await openMenu();
    await click(screen.getByRole("menuitem", { name: /Unload to free memory/ }));
    expect(ipcMock.unloadModel).toHaveBeenCalledWith(true);
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

  it("asks the D3 question the same way in Ask first, Allow all and Full access (it is the user's action, not a tool's)", async () => {
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 1, modelChecks: 0, otherApps: 0 });
    for (const mode of ["ask", "allowAll", "fullAccess"] as const) {
      vi.clearAllMocks();
      ipcMock.modelBusy.mockResolvedValue({ chatTurns: 1, modelChecks: 0, otherApps: 0 });
      ipcMock.shareStatus.mockResolvedValue(SHARE_OFF);
      useApprovalMode.setState({ modes: { 3: mode }, defaultMode: mode });
      await show(status());
      await openMenu();
      await click(screen.getByRole("menuitem", { name: /Reload/ }));
      expect(screen.getByText("A reply is being written. Stop it and restart the model?"), mode).toBeTruthy();
      expect(ipcMock.reloadLocalModel, mode).not.toHaveBeenCalled();
      await click(screen.getByRole("button", { name: "Restart when it ends" }));
      expect(ipcMock.reloadLocalModel, mode).toHaveBeenCalledWith(true, true);
      cleanup();
    }
    useApprovalMode.setState({ modes: {}, defaultMode: "ask" });
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

  it("words a wait the backend started on its own by the change that runs", async () => {
    // No question was asked (nothing used the model when checked), then a reply started:
    // the backend waits instead of stopping it, and the line names the change.
    useWaiting.setState({ change: null, kind: "switch" });
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 1, modelChecks: 0, otherApps: 0 });
    await show(status());
    expect(screen.queryByText(/when the reply ends/)).toBeNull();
    cleanup();
    await show(status({ reloadWaiting: true }));
    expect(screen.getByRole("status").textContent).toContain("Changes the model when the reply ends");
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
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(false, true);
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
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(true, true);
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

  it("after the whole step-down chain, says where the model runs now, in the specification's words", async () => {
    // What WP1a's supervisor reports for a graphics card that runs out of memory with Save
    // memory on: Save memory off, then As much as fits, then the processor.
    const chain: RuntimeStatus["adjustments"] = [
      { kind: "retryingWithoutSaveMemory", backend: "vulkan" },
      { kind: "retryingWithLessGpu", backend: "vulkan" },
      { kind: "steppingDown", from: "vulkan", to: "cpu", reason: { code: "out_of_memory" } },
    ];
    await show(status({ backend: "cpu", adjustments: chain }));
    expect(screen.getByRole("status").textContent).toBe(
      "Running on the processor because the graphics card ran out of memory. Reload to try the graphics card again.Reload",
    );
    // "Part of the model runs on the processor" is no longer true once all of it does.
    const notes = runtimeNotes(status({ backend: "cpu", adjustments: chain }), { restarted: false, hasMessages: false });
    expect(notes.map((n) => n.text)).toEqual([
      "Running on the processor because the graphics card ran out of memory. Reload to try the graphics card again.",
      "Save memory doesn't work with this graphics card, so it was turned off for this load.",
    ]);
    expect(notes.map((n) => n.action)).toEqual(["reload", "settings"]);
    await click(screen.getByRole("button", { name: "Reload" }));
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(true, true);

    // Two step-downs: only the last says where it runs now; the slow part-on-the-processor
    // line comes back when the lower graphics backend runs only part of the model.
    const twice = runtimeNotes(
      status({
        backend: "vulkan",
        offloadedLayers: 12,
        totalLayers: 30,
        adjustments: [
          { kind: "retryingWithLessGpu", backend: "rocm" },
          { kind: "steppingDown", from: "rocm", to: "vulkan", reason: { code: "out_of_memory" } },
        ],
      }),
      { restarted: false, hasMessages: false },
    );
    expect(twice.map((n) => n.text)).toEqual([
      "Running on the graphics card because the AMD graphics card ran out of memory. Reload to try the AMD graphics card again.",
      "Part of the model runs on the processor, so replies are slower. Save memory or a smaller memory size lets more fit on the graphics card.",
    ]);
    const chained = runtimeNotes(
      status({
        backend: "cpu",
        adjustments: [
          { kind: "steppingDown", from: "rocm", to: "vulkan", reason: { code: "missing_runtime_dll" } },
          { kind: "steppingDown", from: "vulkan", to: "cpu", reason: { code: "out_of_memory" } },
        ],
      }),
      { restarted: false, hasMessages: false },
    );
    expect(chained.map((n) => n.text)).toEqual([
      "Running on the processor because the graphics card ran out of memory. Reload to try the graphics card again.",
    ]);
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

  it("offers the action that can help after a step-down: Model settings… for a missing card, none for an unreadable model", () => {
    const down = (reason: LoadFailure) =>
      runtimeNotes(status({ backend: "cpu", adjustments: [{ kind: "steppingDown", from: "vulkan", to: "cpu", reason }] }), {
        restarted: false,
        hasMessages: false,
      });
    expect(down({ code: "invalid_device", device: "Vulkan1" })).toEqual([
      {
        text: "Running on the processor because the chosen graphics card was not found. Choose another in Model settings.",
        action: "settings",
      },
    ]);
    expect(down({ code: "model_file_invalid" })).toEqual([
      { text: "Running on the processor because the graphics card could not load this model.", action: null },
    ]);
    expect(down({ code: "out_of_memory" })[0]?.action).toBe("reload");
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

  it("says Used by another app from the sharing status, also in the tooltip and the menu", async () => {
    ipcMock.shareStatus.mockResolvedValue({ ...SHARE_OFF, enabled: true, port: 51234, listening: true, inUse: true });
    await show(status());
    expect(text()).toContain("Used by another app");
    const button = screen.getByRole("button", { name: "Model menu" });
    expect(button.title).toBe("Remembers about 24,000 words (Automatic)\nUsed by another app\n/models/a.gguf");
    await openMenu();
    expect(screen.getByRole("menu").textContent).toContain("Used by another app");
  });

  it("has the ready line's shorter forms for a crowded header, each with the number whole", async () => {
    await show(status());
    const button = screen.getByRole("button", { name: "Model menu" });
    const form = (name: string) => button.querySelector(`[data-ready="${name}"]`)?.textContent;
    // Which one shows is up to the room the header leaves (a container query on the line).
    expect(form("remembers")).toBe("Remembers about 24,000 words (Automatic)");
    expect(form("about")).toBe("About 24,000 words");
    expect(form("words")).toBe("24,000 words");
    expect(button.closest(".\\@container\\/ready")).not.toBeNull();
  });

  it("offers Model settings… after a failed load, and Load after Unload", async () => {
    await show(
      status({
        state: "failed",
        error: { code: "load_failed", message: "This model doesn't fit.", nextStep: "", details: "" },
      }),
    );
    expect(text()).toContain("Didn't load");
    // The reason is visible text, not only a tooltip.
    expect(screen.getByRole("status").textContent).toBe("This model doesn't fit.");
    expect(screen.getByRole("button", { name: "Model settings…" })).toBeTruthy();
    await click(screen.getByRole("button", { name: "Try again" }));
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/a.gguf", null, true);
    cleanup();
    vi.clearAllMocks();
    await show(status({ state: "stopped", modelPath: null, modelName: null, nCtx: null, unloadedPath: "/models/a.gguf" }));
    expect(text()).toContain("Unloaded to free memory");
    await click(screen.getByRole("button", { name: "Load" }));
    expect(ipcMock.loadLocalModel).toHaveBeenCalledWith("/models/a.gguf", null, true);
  });
});
