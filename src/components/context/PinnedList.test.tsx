import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { Pin, PinList } from "./types";

const apiMock = vi.hoisted(() => ({
  listPins: vi.fn<(chatId: number) => Promise<PinList>>(),
  addPin: vi.fn<(chatId: number, text: string, everyChat: boolean) => Promise<PinList>>(),
  editPin: vi.fn<(chatId: number, id: number, text: string) => Promise<PinList>>(),
  removePin: vi.fn<(chatId: number, id: number) => Promise<PinList>>(),
  movePin: vi.fn<(chatId: number, id: number, up: boolean) => Promise<PinList>>(),
  setPinScope: vi.fn<(chatId: number, id: number, everyChat: boolean) => Promise<PinList>>(),
  onPinsChanged: vi.fn<(handler: (chatId: number | null) => void) => Promise<() => void>>(),
  inspectContext: vi.fn(() => new Promise(() => undefined)),
  onTaskChanged: vi.fn(async () => () => undefined),
  onToolServersChanged: vi.fn(async () => () => undefined),
}));
vi.mock("./api", () => apiMock);

const { PinnedList, charCount } = await import("./PinnedList");
const { ContextTab } = await import("../workbench/ContextTab");

function list(pins: Pin[]): PinList {
  return { pins, max: 20, maxChars: 1000 };
}

const EVERY: Pin = { id: 1, text: "Answer in German.", everyChat: true };
const OWN: Pin = { id: 2, text: "Use metric units.", everyChat: false };
const OWN2: Pin = { id: 3, text: "Keep it short.\nNo tables.", everyChat: false };

let changed: ((chatId: number | null) => void) | null = null;

async function show(pins: Pin[], chatId = 7) {
  apiMock.listPins.mockResolvedValue(list(pins));
  await act(async () => {
    render(<PinnedList chatId={chatId} />);
  });
}

function items() {
  return screen.queryAllByRole("listitem").map((li) => li.querySelector("[data-pin-text]")?.textContent);
}

function item(n: number) {
  return within(screen.getAllByRole("listitem")[n]!);
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
}

describe("PinnedList", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    changed = null;
    apiMock.onPinsChanged.mockImplementation(async (handler) => {
      changed = handler;
      return () => undefined;
    });
  });
  afterEach(() => cleanup());

  it("lists the pins for every chat first, with a counter and the plain-text warning", async () => {
    await show([EVERY, OWN, OWN2]);
    expect(apiMock.listPins).toHaveBeenCalledWith(7);
    expect(items()).toEqual(["Answer in German.", "Use metric units.", "Keep it short.\nNo tables."]);
    expect(screen.getByText("3 of 20")).toBeTruthy();
    expect(screen.getByText(/do not put passwords or keys in\s+a pin/)).toBeTruthy();
    const scope = within(item(0).getByRole("group", { name: "Applies to" }));
    expect(scope.getByRole("button", { name: "Every chat" }).getAttribute("aria-pressed")).toBe("true");
    expect(scope.getByRole("button", { name: "This chat" }).getAttribute("aria-pressed")).toBe("false");
    // Nothing that the end-to-end checks read as a notice or an error.
    expect(document.querySelector("[role=status], [role=alert]")).toBeNull();
  });

  it("says when nothing is pinned", async () => {
    await show([]);
    expect(screen.getByText("Nothing is pinned yet.")).toBeTruthy();
    expect(screen.getByText("0 of 20")).toBeTruthy();
  });

  it("pins typed text for this chat or for every chat", async () => {
    await show([]);
    const box = screen.getByLabelText("New pinned instruction");
    const pinIt = screen.getByRole("button", { name: "Pin it" });
    expect((pinIt as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(box, { target: { value: "  " } });
    expect((pinIt as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(box, { target: { value: "Use metric units." } });
    apiMock.addPin.mockResolvedValueOnce(list([OWN]));
    await click(pinIt);
    expect(apiMock.addPin).toHaveBeenCalledWith(7, "Use metric units.", false);
    expect(items()).toEqual(["Use metric units."]);
    expect((box as HTMLTextAreaElement).value).toBe("");

    fireEvent.click(screen.getByLabelText("Every chat"));
    fireEvent.change(box, { target: { value: "Answer in German." } });
    apiMock.addPin.mockResolvedValueOnce(list([EVERY, OWN]));
    await click(screen.getByRole("button", { name: "Pin it" }));
    expect(apiMock.addPin).toHaveBeenLastCalledWith(7, "Answer in German.", true);
    expect(screen.getByText("2 of 20")).toBeTruthy();
  });

  it("shows the character count while typing and refuses more than the limit", async () => {
    await show([]);
    const box = screen.getByLabelText("New pinned instruction");
    fireEvent.change(box, { target: { value: "🙂".repeat(1000) } });
    expect(screen.getByText("1,000 of 1,000 characters")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Pin it" }) as HTMLButtonElement).disabled).toBe(false);
    fireEvent.change(box, { target: { value: "x".repeat(1001) } });
    expect(screen.getByText(/1,001 of 1,000 characters: too long/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "Pin it" }) as HTMLButtonElement).disabled).toBe(true);
    expect(charCount("a🙂ä")).toBe(3);
  });

  it("shows the app's refusal and keeps what was typed", async () => {
    await show([]);
    fireEvent.change(screen.getByLabelText("New pinned instruction"), { target: { value: "One more." } });
    apiMock.addPin.mockRejectedValueOnce({
      code: "pinned_refused",
      message: "This chat already has 20 pinned instructions, counting those for every chat. Remove one first.",
      nextStep: "",
      details: "",
    });
    await click(screen.getByRole("button", { name: "Pin it" }));
    expect(screen.getByText(/already has 20 pinned instructions/)).toBeTruthy();
    expect((screen.getByLabelText("New pinned instruction") as HTMLTextAreaElement).value).toBe("One more.");
  });

  it("stops offering to add at the limit", async () => {
    const twenty = Array.from({ length: 20 }, (_, i) => ({ id: i + 1, text: `pin ${i}`, everyChat: false }));
    await show(twenty);
    expect(screen.getByText("20 of 20")).toBeTruthy();
    expect((screen.getByLabelText("New pinned instruction") as HTMLTextAreaElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Pin it" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("edits a pin in place, keeps or discards the change", async () => {
    await show([EVERY, OWN]);
    await click(item(1).getByRole("button", { name: "Edit pin" }));
    const editor = item(1).getByLabelText("Pinned instruction") as HTMLTextAreaElement;
    expect(editor.value).toBe("Use metric units.");
    fireEvent.change(editor, { target: { value: "Use SI units." } });
    expect(item(1).getByText("13 of 1,000 characters")).toBeTruthy();
    await click(item(1).getByRole("button", { name: "Discard changes" }));
    expect(apiMock.editPin).not.toHaveBeenCalled();
    expect(items()).toEqual(["Answer in German.", "Use metric units."]);

    await click(item(1).getByRole("button", { name: "Edit pin" }));
    fireEvent.change(item(1).getByLabelText("Pinned instruction"), { target: { value: "Use SI units." } });
    apiMock.editPin.mockResolvedValueOnce(list([EVERY, { ...OWN, text: "Use SI units." }]));
    await click(item(1).getByRole("button", { name: "Keep changes" }));
    expect(apiMock.editPin).toHaveBeenCalledWith(7, 2, "Use SI units.");
    expect(items()).toEqual(["Answer in German.", "Use SI units."]);
  });

  it("discards an edit with Escape and keeps one with Ctrl+Enter", async () => {
    await show([OWN]);
    await click(item(0).getByRole("button", { name: "Edit pin" }));
    fireEvent.keyDown(item(0).getByLabelText("Pinned instruction"), { key: "Escape" });
    expect(screen.queryByLabelText("Pinned instruction")).toBeNull();
    await click(item(0).getByRole("button", { name: "Edit pin" }));
    const editor = item(0).getByLabelText("Pinned instruction");
    fireEvent.change(editor, { target: { value: "Metric only." } });
    apiMock.editPin.mockResolvedValueOnce(list([{ ...OWN, text: "Metric only." }]));
    await act(async () => {
      fireEvent.keyDown(editor, { key: "Enter", ctrlKey: true });
    });
    expect(apiMock.editPin).toHaveBeenCalledWith(7, 2, "Metric only.");
  });

  it("moves a pin only within its own kind", async () => {
    await show([EVERY, OWN, OWN2]);
    const up = (n: number) => item(n).getByRole("button", { name: "Move up" }) as HTMLButtonElement;
    const down = (n: number) => item(n).getByRole("button", { name: "Move down" }) as HTMLButtonElement;
    // The one pin for every chat cannot move; the chat's own two can swap.
    expect([up(0).disabled, down(0).disabled]).toEqual([true, true]);
    expect([up(1).disabled, down(1).disabled]).toEqual([true, false]);
    expect([up(2).disabled, down(2).disabled]).toEqual([false, true]);
    apiMock.movePin.mockResolvedValueOnce(list([EVERY, OWN2, OWN]));
    await click(up(2));
    expect(apiMock.movePin).toHaveBeenCalledWith(7, 3, true);
    expect(items()).toEqual(["Answer in German.", "Keep it short.\nNo tables.", "Use metric units."]);
    apiMock.movePin.mockResolvedValueOnce(list([EVERY, OWN, OWN2]));
    await click(down(1));
    expect(apiMock.movePin).toHaveBeenLastCalledWith(7, 3, false);
  });

  it("switches a pin between this chat and every chat, and unpins", async () => {
    await show([EVERY, OWN]);
    apiMock.setPinScope.mockResolvedValueOnce(list([EVERY, { ...OWN, everyChat: true }]));
    await click(item(1).getByRole("button", { name: "Every chat" }));
    expect(apiMock.setPinScope).toHaveBeenCalledWith(7, 2, true);
    // Pressing the side that is already on does nothing.
    await click(item(0).getByRole("button", { name: "Every chat" }));
    expect(apiMock.setPinScope).toHaveBeenCalledTimes(1);
    apiMock.removePin.mockResolvedValueOnce(list([{ ...OWN, everyChat: true }]));
    await click(item(0).getByRole("button", { name: "Unpin" }));
    expect(apiMock.removePin).toHaveBeenCalledWith(7, 1);
    expect(items()).toEqual(["Use metric units."]);
  });

  it("reads the list again when pins change for this chat or for every chat", async () => {
    await show([OWN]);
    expect(changed).not.toBeNull();
    apiMock.listPins.mockResolvedValue(list([EVERY, OWN]));
    await act(async () => changed!(8));
    expect(apiMock.listPins).toHaveBeenCalledTimes(1);
    await act(async () => changed!(null));
    expect(apiMock.listPins).toHaveBeenCalledTimes(2);
    expect(items()).toEqual(["Answer in German.", "Use metric units."]);
    await act(async () => changed!(7));
    expect(apiMock.listPins).toHaveBeenCalledTimes(3);
  });

  it("shows a pin's text as text, never as markup", async () => {
    await show([{ id: 9, text: "<img src=x onerror=alert(1)> **bold**", everyChat: false }]);
    expect(items()).toEqual(["<img src=x onerror=alert(1)> **bold**"]);
    expect(document.querySelector("li img")).toBeNull();
  });

  it("is what the Context tab shows", async () => {
    apiMock.listPins.mockResolvedValue(list([EVERY]));
    await act(async () => {
      render(<ContextTab chatId={4} />);
    });
    expect(apiMock.listPins).toHaveBeenCalledWith(4);
    expect(screen.getByRole("heading", { name: "Pinned instructions" })).toBeTruthy();
    expect(items()).toEqual(["Answer in German."]);
  });
});
