import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ContextView } from "./types";

const apiMock = vi.hoisted(() => ({
  inspectContext: vi.fn<(chatId: number) => Promise<ContextView>>(),
  pinMessage: vi.fn(),
  onPinsChanged: vi.fn<(h: (chatId: number | null) => void) => Promise<() => void>>(),
  onTaskChanged: vi.fn<(h: (chatId: number) => void) => Promise<() => void>>(),
  onToolServersChanged: vi.fn<(h: () => void) => Promise<() => void>>(),
}));
vi.mock("./api", () => apiMock);
const clipboard = vi.hoisted(() => ({ writeText: vi.fn<(text: string) => Promise<void>>(async () => undefined) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => clipboard);

const { ContextInspector, copyAllText, DRAWN_CHARS } = await import("./ContextInspector");
const { useApp } = await import("../../store/app");

function view(over: Partial<ContextView> = {}): ContextView {
  return {
    model: "This chat's model runs on this computer with a context of 8,192 tokens.",
    totals: {
      tokens: 1234,
      exact: false,
      nCtx: 8192,
      budget: 6048,
      trigger: 4838,
      tools: { used: 900, allowed: 2419 },
      pinned: { used: 40, allowed: 1512 },
    },
    sections: [
      { id: "system", title: "System prompt", detail: "The chat's own", tokens: 10, text: "You are a careful assistant." },
      { id: "pinned", title: "Pinned instructions", detail: "1 pin", tokens: 30, text: "1. Answer in German." },
      { id: "tools", title: "Tool definitions", detail: "2 shown: read_file, list_dir", tokens: 300, text: '{"name": "read_file"}' },
      {
        id: "kept",
        title: "Messages kept",
        detail: "2 messages",
        tokens: 50,
        text: "[user]\n<img src=x onerror=alert(1)> **not bold**",
      },
      { id: "leftOut", title: "Messages left out", detail: "None", tokens: 0, text: "" },
    ],
    nextDrop: "Nothing is left out now.",
    refusal: null,
    notes: [],
    ...over,
  };
}

const handlers: { pins?: (c: number | null) => void; task?: (c: number) => void; servers?: () => void } = {};

async function show(v: ContextView, chatId = 7) {
  apiMock.inspectContext.mockResolvedValue(v);
  await act(async () => {
    render(<ContextInspector chatId={chatId} />);
  });
}

function row(id: string) {
  return document.querySelector<HTMLElement>(`[data-section="${id}"]`)!;
}

describe("ContextInspector", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiMock.onPinsChanged.mockImplementation(async (h) => {
      handlers.pins = h;
      return () => undefined;
    });
    apiMock.onTaskChanged.mockImplementation(async (h) => {
      handlers.task = h;
      return () => undefined;
    });
    apiMock.onToolServersChanged.mockImplementation(async (h) => {
      handlers.servers = h;
      return () => undefined;
    });
    useApp.setState({ generation: null, detail: null });
  });
  afterEach(() => cleanup());

  it("shows the sections collapsed with their token estimates, the total and what goes next", async () => {
    await show(view());
    expect(apiMock.inspectContext).toHaveBeenCalledWith(7);
    const titles = [...document.querySelectorAll("[data-section] button .font-medium")].map((e) => e.textContent);
    expect(titles).toEqual(["System prompt", "Pinned instructions", "Tool definitions", "Messages kept", "Messages left out"]);
    expect(within(row("tools")).getByText("300 tokens")).toBeTruthy();
    expect(within(row("tools")).getByText("2 shown: read_file, list_dir")).toBeTruthy();
    expect(screen.getByText("About 1,234 of 8,192 tokens")).toBeTruthy();
    expect(screen.getByText(/Pinned instructions: 40 of 1,512 tokens allowed/)).toBeTruthy();
    expect(screen.getByText(/Fixed part without pins: 900 of 2,419/)).toBeTruthy();
    expect(screen.getByText("Nothing is left out now.")).toBeTruthy();
    // Collapsed: no text drawn until opened.
    expect(screen.queryByText("You are a careful assistant.")).toBeNull();
    const toggle = within(row("system")).getByRole("button");
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("You are a careful assistant.")).toBeTruthy();
    // An empty section cannot be opened.
    expect((within(row("leftOut")).getByRole("button") as HTMLButtonElement).disabled).toBe(true);
    expect(document.querySelector("[role=status], [role=alert]")).toBeNull();
  });

  it("draws text as text, never as markup", async () => {
    await show(view());
    fireEvent.click(within(row("kept")).getByRole("button"));
    expect(row("kept").querySelector("pre")!.textContent).toBe("[user]\n<img src=x onerror=alert(1)> **not bold**");
    expect(row("kept").querySelector("img, strong")).toBeNull();
  });

  it("cuts a large section for display and says Copy all has everything", async () => {
    const big = "y".repeat(DRAWN_CHARS + 500);
    await show(view({ sections: [{ id: "kept", title: "Messages kept", detail: "1", tokens: 9000, text: big }] }));
    fireEvent.click(within(row("kept")).getByRole("button"));
    expect(row("kept").querySelector("pre")!.textContent!.length).toBe(DRAWN_CHARS);
    expect(screen.getByText(/Cut here for display: 500 more characters. Copy all has everything./)).toBeTruthy();
    expect(copyAllText(view({ sections: [{ id: "kept", title: "Messages kept", detail: "1", tokens: 9000, text: big }] }))).toContain(big);
  });

  it("copies the whole text with Copy all", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await show(view());
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Copy all/ }));
    });
    const text = copyAllText(view());
    expect(text).toContain("You are a careful assistant.");
    expect(text).toContain("1. Answer in German.");
    expect(text).toContain("=== Tool definitions (about 300 tokens)");
    expect(writeText).toHaveBeenCalledWith(text);
    expect(clipboard.writeText).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /Copied/ })).toBeTruthy();
  });

  it("says plainly when there are no numbers, and shows a refusal", async () => {
    await show(
      view({
        model: "The model for this chat is not loaded. Token numbers appear once it is.",
        totals: null,
        sections: [{ id: "system", title: "System prompt", detail: "x", tokens: null, text: "p" }],
        nextDrop: "Shown once a model is ready for this chat.",
      }),
    );
    expect(screen.getByText(/not loaded. Token numbers appear once it is./)).toBeTruthy();
    expect(document.querySelector("[data-inspector-totals]")).toBeNull();
    expect(document.querySelector("[data-section-tokens]")).toBeNull();
    cleanup();
    await show(view({ refusal: "Your 20 pinned instructions need about 6,734 tokens." }));
    expect(screen.getByText(/The next message would not be sent: Your 20 pinned/)).toBeTruthy();
  });

  it("reads again on pins, task, tool and server changes, and when a turn ends; never while one runs", async () => {
    await show(view());
    expect(apiMock.inspectContext).toHaveBeenCalledTimes(1);
    await act(async () => handlers.pins!(null));
    expect(apiMock.inspectContext).toHaveBeenCalledTimes(2);
    await act(async () => handlers.pins!(9));
    await act(async () => handlers.task!(9));
    expect(apiMock.inspectContext).toHaveBeenCalledTimes(2);
    await act(async () => handlers.task!(7));
    expect(apiMock.inspectContext).toHaveBeenCalledTimes(3);
    await act(async () => handlers.servers!());
    expect(apiMock.inspectContext).toHaveBeenCalledTimes(4);
    await act(async () => {
      window.dispatchEvent(new CustomEvent("chat-tools-changed", { detail: 8 }));
    });
    expect(apiMock.inspectContext).toHaveBeenCalledTimes(4);
    await act(async () => {
      window.dispatchEvent(new CustomEvent("chat-tools-changed", { detail: 7 }));
    });
    expect(apiMock.inspectContext).toHaveBeenCalledTimes(5);
    // A turn runs: nothing is read; when it ends, the view is read again.
    await act(async () => {
      useApp.setState({ generation: { chatId: 7 } as never });
    });
    await act(async () => handlers.pins!(7));
    expect(apiMock.inspectContext).toHaveBeenCalledTimes(5);
    expect(screen.getByText(/A reply is being written/)).toBeTruthy();
    await act(async () => {
      useApp.setState({ generation: null });
    });
    expect(apiMock.inspectContext).toHaveBeenCalledTimes(6);
  });

  it("shows the app's error as text", async () => {
    apiMock.inspectContext.mockRejectedValue({ code: "not_found", message: "That chat no longer exists.", nextStep: "", details: "" });
    await act(async () => {
      render(<ContextInspector chatId={7} />);
    });
    expect(screen.getByText("That chat no longer exists.")).toBeTruthy();
  });
});
