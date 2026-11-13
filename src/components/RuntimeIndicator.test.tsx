import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ChatDetail, LoadFailure, ModelBusy, RuntimeStatus, ShareStatus } from "../lib/types";
import { placement } from "./fine.fixtures";

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

const { RuntimeIndicator, runtimeNotes, partOnProcessor } = await import("./RuntimeIndicator");
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
    placement: null,
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

  it("shortens the ready line by whole phrases only, never inside a word (header-layout)", async () => {
    await show(status());
    const button = screen.getByRole("button", { name: "Model menu" });
    const piece = (name: string) => button.querySelector(`[data-ready="${name}"]`) as HTMLElement;
    // Which form shows is up to the room the line gets (a container query on it), each with the
    // number whole: "Remembers about …" with its reason, "About …", then the number in words.
    expect(piece("remembers").textContent).toBe("Remembers about 24,000 words");
    expect(piece("reason").textContent).toBe(" (Automatic)");
    expect(piece("about").textContent).toBe("About 24,000 words");
    expect(piece("words").textContent).toBe("24,000 words");
    expect(button.closest(".\\@container\\/ready")).not.toBeNull();
    // The reason goes with "Remembers about …" only and drops whole: both sit on a row one line
    // high that moves what does not fit onto a hidden second line. Nothing is cut with an
    // ellipsis.
    const row = piece("reason").parentElement as HTMLElement;
    expect(row.contains(piece("remembers"))).toBe(true);
    for (const c of ["flex-wrap", "overflow-hidden", "h-4"]) expect(row.className.split(" ")).toContain(c);
    for (const name of ["remembers", "reason", "about", "words"]) {
      expect(piece(name).className).not.toMatch(/truncate|text-ellipsis/);
    }
    // "· Change" (and "· Used by another app") drop whole the same way, before the reason.
    const line = button.closest("[data-ready-line]") as HTMLElement;
    for (const c of ["flex-wrap", "overflow-hidden", "h-4"]) expect(line.className.split(" ")).toContain(c);
    expect(line.firstElementChild?.contains(button)).toBe(true);
    expect(line.lastElementChild?.textContent).toBe("·Change");
    // The whole line stays in the tooltip and the menu.
    expect(button.title.split("\n")[0]).toBe("Remembers about 24,000 words (Automatic)");
  });

  it("keeps 'Loading model' whole and drops the stage whole when the header is crowded (header-layout)", async () => {
    await show(status({ state: "starting", progress: 0.45, loadStage: { kind: "readingModel" } }));
    const line = document.querySelector("[data-loading-line]") as HTMLElement;
    // The whole line is its tooltip.
    expect(line.title).toBe("Loading model · Reading the model file 45%");
    const piece = (name: string) => line.querySelector(`[data-loading="${name}"]`) as HTMLElement;
    expect(piece("start").textContent).toBe("Loading model");
    expect(piece("stage").textContent).toBe(" · Reading the model file");
    expect(piece("progress").textContent).toBe(" 45%");
    const row = piece("stage").parentElement as HTMLElement;
    for (const c of ["flex-wrap", "overflow-hidden", "h-4"]) expect(row.className.split(" ")).toContain(c);
    for (const name of ["start", "stage", "progress"]) expect(piece(name).className).not.toMatch(/truncate|text-ellipsis/);
    expect(line.closest(".\\@container\\/loading")).not.toBeNull();
    // Without a percentage the line ends in "…".
    cleanup();
    await show(status({ state: "starting", progress: null, loadStage: null }));
    expect((document.querySelector("[data-loading-line]") as HTMLElement).title).toBe("Loading model…");
  });

  it("keeps both actions of a failed load in sight, never on a hidden line (header-layout r2)", async () => {
    await show(
      status({
        state: "failed",
        error: { code: "load_failed", message: "Could not read /models/a-very-long-name.gguf", nextStep: "", details: "" },
      }),
    );
    const box = document.querySelector("[data-header-state='failed']") as HTMLElement;
    // The line asks the header for "Didn't load · Model settings…" (jsdom has no layout: the
    // e2e scenario header-layout measures it at 720 pixels).
    expect(box.className.split(" ")).toContain("min-w-[10.5rem]");
    const settings = screen.getByRole("button", { name: "Model settings…" });
    const again = screen.getByRole("button", { name: "Try again" });
    const first = box.querySelector("[data-failed-line]") as HTMLElement;
    expect(first.textContent).toBe("Didn't load·Model settings…");
    expect(first.contains(settings)).toBe(true);
    // No row that clips what does not fit: nothing focusable can sit where it cannot be seen.
    for (const c of ["overflow-hidden", "h-4", "flex-wrap"]) expect(first.className.split(" ")).not.toContain(c);
    // Try again follows the reason on the second line, like a note's action.
    const reason = screen.getByRole("status");
    expect(reason.textContent).toBe("Could not read /models/a-very-long-name.gguf");
    expect(reason.parentElement).toBe(again.parentElement);
    expect(again.className.split(" ")).toContain("shrink-0");
    for (const b of [settings, again]) {
      expect(b.tabIndex).toBe(0);
      expect(b.closest(".overflow-hidden")).toBeNull();
    }
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

  it("says only Load in the narrowest header, where the usage icons keep their room (0.0.6 item 6)", async () => {
    // jsdom has no layout: the classes are checked here, the real widths in the fine-control e2e.
    for (const [unloadedPath, words] of [
      ["/models/a.gguf", "Unloaded to free memory"],
      [null, "Not loaded"],
    ] as const) {
      await show(status({ state: "stopped", modelPath: null, modelName: null, nCtx: null, unloadedPath }));
      const load = screen.getByRole("button", { name: "Load" });
      const line = load.parentElement as HTMLElement;
      expect(line.getAttribute("title")).toBe(words);
      const shown = screen.getByText(words);
      expect(shown.className).toContain("@max-xl:hidden");
      expect((shown.nextElementSibling as HTMLElement).className).toContain("@max-xl:hidden");
      expect(load.className).not.toContain("hidden");
      cleanup();
    }
  });
});

describe("RuntimeIndicator: where the model went (0.0.6 item 6)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.modelBusy.mockResolvedValue(IDLE);
    ipcMock.shareStatus.mockResolvedValue(SHARE_OFF);
    useWaiting.setState({ change: null, kind: null });
    useRestartNote.setState({ shown: false });
  });
  afterEach(cleanup);

  const PARTIAL =
    "Part of the model runs on the processor, so replies are slower. Save memory or a smaller memory size lets more fit on the graphics card.";
  const notes = (patch: Partial<RuntimeStatus>) =>
    runtimeNotes(status(patch), { restarted: false, hasMessages: false }).map((n) => n.text);

  it("says part of the model runs on the processor from the ready line's report, expert parts included", async () => {
    // As much as fits left 11 of 41 layers on the processor.
    expect(notes({ placement: placement({ custom: false, layersAbout: true }) })).toEqual([PARTIAL]);
    // All layers on the card, but the expert parts of some on the processor.
    expect(notes({ placement: placement({ custom: false, layersOnGraphics: 41, expertLayersOnProcessor: 19 }) })).toEqual([PARTIAL]);
    // Only amounts known after As much as fits: the rest is on the processor.
    expect(notes({ placement: placement({ custom: false, layersAbout: true, layersOnGraphics: null }) })).toEqual([PARTIAL]);
    expect(notes({ placement: placement({ custom: false, layersAbout: true, layersOnGraphics: null, units: null }) })).toEqual([
      PARTIAL,
    ]);
    // Everything on the card: no note.
    expect(notes({ placement: placement({ custom: false, layersOnGraphics: 41 }) })).toEqual([]);
    // The report wins over the old layer counts.
    expect(notes({ offloadedLayers: 10, totalLayers: 20, placement: placement({ custom: false, layersOnGraphics: 41 }) })).toEqual([]);
    await show(status({ placement: placement({ custom: false, layersAbout: true }) }));
    expect(screen.getByRole("status").textContent).toContain(PARTIAL);
    expect(screen.getByRole("button", { name: "Model settings…" })).toBeTruthy();
  });

  it("says nothing of the kind for a placement chosen in Fine control, or with nothing on the card", () => {
    expect(notes({ placement: placement({ custom: true }) })).toEqual([]);
    expect(notes({ placement: placement({ custom: true, layersOnGraphics: 41, expertLayersOnProcessor: 20 }) })).toEqual([]);
    expect(notes({ placement: placement({ custom: false, graphics: null, layersOnGraphics: 0 }) })).toEqual([]);
    // Only amounts known: All of it puts every layer on the card, so no note...
    expect(notes({ placement: placement({ custom: false, layersOnGraphics: null }) })).toEqual([]);
    expect(partOnProcessor(status({ backend: "cpu", placement: placement({ custom: false, layersAbout: true }) }))).toBe(true);
    // Without a report (a backend without item 6) the old layer counts still work.
    const old = status({ offloadedLayers: 10, totalLayers: 20 }) as Partial<RuntimeStatus>;
    delete old.placement;
    expect(partOnProcessor(old as RuntimeStatus)).toBe(true);
  });

  it("words the two new retries in the loading line and after the load", async () => {
    await show(status({ state: "starting", progress: null, loadStage: { kind: "retryingWithoutFastAttention", backend: "vulkan" } }));
    expect(document.body.textContent).toContain("Loading model · Trying again without Fast attention");
    cleanup();
    expect(
      notes({ adjustments: [{ kind: "retryingWithLessGpu", backend: "vulkan", custom: true, chosenLayers: 40 }], placement: placement({ custom: false, layersAbout: true }) }),
    ).toEqual([
      "The 40 layers you chose didn't fit on the graphics card, so As much as fits was used for this load. Choose fewer layers in Fine control.",
    ]);
    const after = runtimeNotes(status({ adjustments: [{ kind: "retryingWithoutFastAttention", backend: "vulkan" }] }), {
      restarted: false,
      hasMessages: false,
    });
    expect(after.map((n) => [n.text, n.action])).toEqual([
      ["Fast attention doesn't work with this graphics card, so it was turned off for this load.", "settings"],
    ]);
  });
});
