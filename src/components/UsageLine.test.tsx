import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ChatDetail, RuntimeStatus, UsageNow } from "../lib/types";
import { gb, placement, usage } from "./fine.fixtures";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  usageNow: vi.fn<() => Promise<UsageNow>>(),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { UsageLine } = await import("./UsageLine");
const { useApp } = await import("../store/app");
const { useApprovalMode } = await import("../lib/approvalMode");

function ready(patch: Partial<RuntimeStatus> = {}): RuntimeStatus {
  return {
    state: "ready",
    backend: "vulkan",
    device: "Vulkan0",
    modelPath: "/models/a.gguf",
    modelName: "a.gguf",
    progress: 1,
    stage: null,
    nCtx: 16_384,
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
    placement: placement(),
    ...patch,
  };
}

function chat(modelRef: string | null): ChatDetail {
  return {
    chat: { id: 3, title: "t", modelRef, systemPrompt: null, leafMessageId: null, createdAt: 0, updatedAt: 0, thinking: false, workingFolder: null },
    messages: [],
  };
}

async function show(now: UsageNow, opts: { runtime?: RuntimeStatus | null; modelRef?: string | null; noChat?: boolean } = {}) {
  ipcMock.usageNow.mockResolvedValue(now);
  useApp.setState({
    runtime: opts.runtime === undefined ? ready() : opts.runtime,
    detail: opts.noChat ? null : chat(opts.modelRef === undefined ? "local:/models/a.gguf" : opts.modelRef),
  });
  await act(async () => {
    render(<UsageLine />);
  });
}

function form(name: "wide" | "medium" | "small" | "icons"): HTMLElement {
  return document.querySelector(`[data-usage="${name}"]`) as HTMLElement;
}

async function openMenu() {
  await act(async () => {
    fireEvent.keyDown(screen.getByRole("button", { name: "Computer usage" }), { key: "Enter" });
  });
}

const menu = () => document.querySelector('[role="menu"]')?.textContent ?? "";

function setVisibility(state: "visible" | "hidden") {
  Object.defineProperty(document, "visibilityState", { value: state, configurable: true });
  document.dispatchEvent(new Event("visibilitychange"));
}

describe("UsageLine: this computer's usage under the model name", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setVisibility("visible");
  });
  afterEach(cleanup);

  it("has a form for each header width, chosen by the header's container", async () => {
    await show(usage());
    expect(form("wide").textContent).toBe("Graphics card 63% · 12.6 of 16 GB · Processor 12% · Memory 16.8 of 32 GB");
    expect(form("medium").textContent).toBe("Graphics card 63% · 12.6/16 GB · Processor 12% · Memory 16.8/32 GB");
    expect(form("small").textContent).toBe("Graphics card 63% · Processor 12% · Memory 53%");
    expect(form("icons").textContent).toBe("63%12%53%");
    // Only one form shows at a time, by the header's width (jsdom has no layout; e2e measures).
    expect(form("wide").className).toContain("hidden @5xl:inline");
    expect(form("medium").className).toContain("hidden @3xl:inline @5xl:hidden");
    expect(form("small").className).toContain("hidden @xl:inline @3xl:hidden");
    expect(form("icons").className).toContain("@xl:hidden");
    // Each icon says what it is in words.
    for (const label of ["Graphics card 63% busy", "Processor 12% busy", "Memory 53% in use"]) {
      const icon = screen.getByRole("img", { name: label });
      expect(icon.getAttribute("title")).toBe(label);
    }
    const button = screen.getByRole("button", { name: "Computer usage" });
    expect(button.title).toBe("Graphics card 63% · 12.6 of 16 GB · Processor 12% · Memory 16.8 of 32 GB");
    expect(button.className).toContain("text-[11px]");
    expect(button.className).toContain("tabular-nums");
    expect(button.className).toContain("truncate");
    expect(button.className).toContain("px-2.5");
  });

  it("says Apple graphics on a Mac, with graphics memory in the menu", async () => {
    await show(
      usage({
        unified: true,
        graphics: {
          kind: "value",
          value: { name: "Apple M2", busyPercent: { kind: "value", value: 40 }, memoryUsedBytes: { kind: "value", value: gb(6.1) }, memoryTotalBytes: gb(10.7) },
        },
        memory: { kind: "value", value: { usedBytes: gb(11.2), totalBytes: gb(16) } },
      }),
    );
    expect(form("wide").textContent).toBe("Apple graphics 40% · Processor 12% · Memory 11.2 of 16 GB");
    await openMenu();
    expect(menu()).toContain("Apple graphics · Apple M2");
    expect(menu()).toContain("Apple graphics: 6.1 GB of the computer's memory");
    expect(menu()).toContain("30 of 41 layers on Apple graphics, 11 on the processor.");
    expect(menu()).not.toContain("graphics card");
  });

  it("shows … on the first sample and – for half a reading, with the reason on its row", async () => {
    const now = usage({ processorPercent: { kind: "measuring" } });
    if (now.graphics.kind === "value") now.graphics.value.busyPercent = { kind: "notAvailable", why: "systemGivesNone" };
    await show(now);
    expect(form("wide").textContent).toBe("Graphics card – · 12.6 of 16 GB · Processor … · Memory 16.8 of 32 GB");
    expect(form("icons").textContent).toBe("–…53%");
    await openMenu();
    expect(menu()).toContain("Busy: –");
    expect(menu()).toContain("This computer doesn't tell apps how busy its graphics card is.");
    expect(menu()).toContain("Memory: 12.6 of 16 GB in use by all programs");
    expect(menu()).toContain("Processor: … · 16 threads");
  });

  it("names each missing figure's reason", async () => {
    const now = usage({ processorPercent: { kind: "notAvailable", why: "readFailed" } });
    if (now.graphics.kind === "value") now.graphics.value.memoryUsedBytes = { kind: "notAvailable", why: "notReadHere" };
    await show(now);
    expect(form("wide").textContent).toBe("Graphics card 63% · – of 16 GB · Processor – · Memory 16.8 of 32 GB");
    await openMenu();
    expect(menu()).toContain("Memory: – of 16 GB in use by all programs");
    expect(menu()).toContain("harnessUI can't read this graphics card on this system yet.");
    expect(menu()).toContain("Processor: – · 16 threads");
    expect(menu()).toContain("Couldn't read this just now.");
  });

  it("leaves the graphics card out when there is none", async () => {
    await show(usage({ graphics: { kind: "notAvailable", why: "noGraphicsCard" } }), { runtime: ready({ placement: placement({ graphics: null, layersOnGraphics: 0 }) }) });
    expect(form("wide").textContent).toBe("Processor 12% · Memory 16.8 of 32 GB");
    expect(form("icons").querySelectorAll("[data-usage-icon]").length).toBe(2);
    await openMenu();
    expect(menu()).not.toMatch(/Graphics card|Busy:/);
    expect(menu()).toContain("Where the model went: On the processor only.");
  });

  it("asks every 1.5 seconds while the window is visible, and stops while it is hidden", async () => {
    vi.useFakeTimers();
    try {
      await show(usage());
      expect(ipcMock.usageNow).toHaveBeenCalledTimes(1);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1500);
      });
      expect(ipcMock.usageNow).toHaveBeenCalledTimes(2);
      await act(async () => {
        setVisibility("hidden");
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(10_000);
      });
      expect(ipcMock.usageNow).toHaveBeenCalledTimes(2);
      await act(async () => {
        setVisibility("visible");
      });
      expect(ipcMock.usageNow).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it("is shown for a model on an endpoint and with no model chosen, without the model's rows", async () => {
    await show(usage(), { modelRef: "endpoint:1:some-model" });
    expect(screen.getByRole("button", { name: "Computer usage" })).toBeTruthy();
    await openMenu();
    expect(menu()).toContain("This computer right now");
    expect(menu()).toContain("Processor: 12% busy · 16 threads");
    expect(menu()).toContain("Memory: 16.8 of 32 GB in use");
    expect(menu()).not.toContain("the model");
    expect(menu()).not.toContain("Where the model went");
    expect(screen.queryByRole("menuitem", { name: /Model settings/ })).toBeNull();
    cleanup();

    await show(usage(), { noChat: true });
    expect(screen.getByRole("button", { name: "Computer usage" })).toBeTruthy();
    await openMenu();
    expect(menu()).not.toContain("Where the model went");
    cleanup();

    await show(usage(), { modelRef: null });
    await openMenu();
    expect(menu()).not.toContain("Where the model went");
  });

  it("lists every figure and where the loaded local model went, with Model settings…", async () => {
    await show(usage());
    await openMenu();
    const rows = [...document.querySelectorAll('[role="menu"] p')].map((p) => p.textContent);
    expect(rows).toEqual([
      "Graphics card · AMD Radeon RX 9070 XT",
      "Busy: 63%",
      "Memory: 12.6 of 16 GB in use by all programs",
      "The model uses 11.7 GB: 9.9 GB for itself, 1.2 GB conversation memory, 0.6 GB working space",
      "Processor: 12% busy · 16 threads",
      "Memory: 16.8 of 32 GB in use · the model: 4.8 GB",
      "Where the model went: 30 of 41 layers on the graphics card, 11 on the processor.",
      "Read on this computer only while you can see it. Nothing is sent anywhere.",
    ]);
    expect(menu()).toContain("This computer right now");
    const opened: string[] = [];
    const listener = (e: Event) => opened.push(String((e as CustomEvent<unknown>).detail));
    window.addEventListener("app:open-model-settings", listener);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: /Model settings/ }));
    });
    window.removeEventListener("app:open-model-settings", listener);
    expect(opened).toEqual(["/models/a.gguf"]);
  });

  it("leaves the model's rows out while the local model is not loaded, but offers its settings", async () => {
    await show(usage(), { runtime: ready({ state: "stopped", placement: null }) });
    await openMenu();
    expect(menu()).not.toContain("The model uses");
    expect(menu()).not.toContain("the model:");
    expect(menu()).not.toContain("Where the model went");
    expect(screen.getByRole("menuitem", { name: /Model settings/ })).toBeTruthy();
  });

  it("is the same in Ask first, Allow all and Full access: it is not a tool action (I6-17)", async () => {
    const seen: string[] = [];
    for (const mode of ["ask", "allowAll", "fullAccess"] as const) {
      useApprovalMode.setState({ modes: { 3: mode }, defaultMode: mode });
      await show(usage());
      await openMenu();
      seen.push(`${form("wide").textContent} | ${menu()}`);
      cleanup();
    }
    expect(new Set(seen).size).toBe(1);
    useApprovalMode.setState({ modes: {}, defaultMode: "ask" });
  });

  it("keeps its height and shows nothing until the first reading answers (or when it cannot be read)", async () => {
    ipcMock.usageNow.mockRejectedValue(new Error("no such command"));
    useApp.setState({ runtime: null, detail: null });
    await act(async () => {
      render(<UsageLine />);
    });
    expect(screen.queryByRole("button", { name: "Computer usage" })).toBeNull();
    expect(document.querySelector('[data-usage="waiting"]')).toBeTruthy();
  });
});
