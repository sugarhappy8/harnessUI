import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ApprovalModeName } from "../lib/ipc";

// Radix positions the menu with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

let mode: Record<number, ApprovalModeName> = {};
let modeChanged: (chatId: number | null) => void = () => undefined;

const ipcMock = vi.hoisted(() => ({
  getChatApprovalMode: vi.fn<(chatId: number) => Promise<ApprovalModeName>>(),
  setChatApprovalMode: vi.fn<(chatId: number, mode: ApprovalModeName) => Promise<ApprovalModeName>>(),
  getDefaultApprovalMode: vi.fn<() => Promise<ApprovalModeName>>(async () => "ask"),
  setDefaultApprovalMode: vi.fn(async () => undefined),
  onApprovalModeChanged: vi.fn(async (handler: (chatId: number | null) => void) => {
    modeChanged = handler;
    return () => undefined;
  }),
}));
vi.mock("../lib/ipc", () => ipcMock);

// [0.0.7] The app's "main window shown" event, sent by hand here.
const shown = vi.hoisted(() => ({ handlers: new Set<() => void>() }));
vi.mock("../lib/windowShown", () => ({
  MAIN_WINDOW_SHOWN: "main-window-shown",
  onMainWindowShown: (handler: () => void) => {
    shown.handlers.add(handler);
    return () => shown.handlers.delete(handler);
  },
}));

const {
  APPROVAL_HINTS,
  APPROVAL_MODES,
  ApprovalHint,
  ApprovalModeMenu,
} = await import("./AllowAllButton");
const { useApprovalMode } = await import("../lib/approvalMode");
const { useApp } = await import("../store/app");

const CHAT = 7;

async function show(chatId = CHAT) {
  await act(async () => {
    render(<ApprovalModeMenu chatId={chatId} />);
  });
}

/** Opens the header menu the way a mouse does (Radix opens it on pointer down). */
async function openMenu(current: string) {
  await act(async () => {
    fireEvent.pointerDown(screen.getByRole("button", { name: current }), { button: 0, ctrlKey: false });
  });
  return screen.getByRole("menu");
}

/** Chooses a mode from the header menu. */
async function pick(current: string, label: string) {
  const menu = await openMenu(current);
  await act(async () => {
    fireEvent.click(within(menu).getByRole("menuitemradio", { name: new RegExp(`^${label}`) }));
  });
}

describe("ApprovalModeMenu", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    mode = {};
    ipcMock.getChatApprovalMode.mockImplementation(async (chatId) => mode[chatId] ?? "ask");
    ipcMock.setChatApprovalMode.mockImplementation(async (chatId, next) => {
      mode[chatId] = next;
      return next;
    });
    useApprovalMode.setState({ modes: {}, defaultMode: "ask" });
    useApp.setState({ error: null });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("shows Ask first until the chat is switched, and its menu lists the three modes", async () => {
    await show();
    expect(ipcMock.getChatApprovalMode).toHaveBeenCalledWith(CHAT);
    const button = screen.getByRole("button", { name: "Ask first" });
    expect(button.textContent).toBe("Ask first");
    // Narrow windows show only the shield while it asks first.
    expect(button.querySelector("span")?.className ?? "").toContain("hidden");
    // The keyboard opens it too.
    await act(async () => {
      fireEvent.keyDown(button, { key: "Enter" });
    });
    const menu = screen.getByRole("menu");
    const items = within(menu).getAllByRole("menuitemradio");
    expect(items.map((i) => i.textContent)).toEqual(APPROVAL_MODES.map((m) => m.label + m.description));
    expect(items.map((i) => i.getAttribute("aria-checked"))).toEqual(["true", "false", "false"]);
    expect(APPROVAL_MODES.map((m) => m.description)).toEqual([
      "Asks before it changes anything.",
      "Asks only before deleting and a few other things.",
      "Never asks. Can change or delete anything on this computer.",
    ]);
    // Opening the menu changes nothing.
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
  });

  it("switching to Allow all or Full access applies at once", async () => {
    await show();
    await pick("Ask first", "Allow all");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.setChatApprovalMode).toHaveBeenCalledTimes(1);
    expect(ipcMock.setChatApprovalMode).toHaveBeenLastCalledWith(CHAT, "allowAll");
    expect(screen.getByRole("button", { name: "Allow all" })).toBeTruthy();
    await pick("Allow all", "Full access");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.setChatApprovalMode).toHaveBeenCalledTimes(2);
    expect(ipcMock.setChatApprovalMode).toHaveBeenLastCalledWith(CHAT, "fullAccess");
    expect(screen.getByRole("button", { name: "Full access" })).toBeTruthy();
  });

  it("goes back to Ask first from Full access at once, at once", async () => {
    mode[CHAT] = "fullAccess";
    await show();
    // Choosing the mode it is already in does nothing.
    await pick("Full access", "Full access");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
    await pick("Full access", "Ask first");
    expect(ipcMock.setChatApprovalMode).toHaveBeenCalledWith(CHAT, "ask");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
  });

  it("steps down from Full access to Allow all at once, and from Allow all to Ask first", async () => {
    mode[CHAT] = "fullAccess";
    await show();
    await pick("Full access", "Allow all");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.setChatApprovalMode).toHaveBeenLastCalledWith(CHAT, "allowAll");
    await pick("Allow all", "Ask first");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.setChatApprovalMode).toHaveBeenLastCalledWith(CHAT, "ask");
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
  });

  it("keeps each chat's mode apart, and a refused switch leaves the mirror as it was", async () => {
    mode[8] = "fullAccess";
    await show(8);
    expect(screen.getByRole("button", { name: "Full access" })).toBeTruthy();
    cleanup();
    await show(CHAT);
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
    // The app refuses (the chat is gone): the button stays as it was and the error shows.
    ipcMock.setChatApprovalMode.mockRejectedValueOnce({
      code: "bad_request",
      message: "That chat no longer exists.",
      nextStep: "",
      details: "",
    });
    await pick("Ask first", "Full access");
    expect(ipcMock.setChatApprovalMode).toHaveBeenCalledWith(CHAT, "fullAccess");
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
    expect(useApprovalMode.getState().modes[CHAT]).toBe("ask");
    expect(useApprovalMode.getState().modes[8]).toBe("fullAccess");
    expect(useApp.getState().error?.message).toBe("That chat no longer exists.");
  });

  it("follows a change made elsewhere in the app, such as forgetting what was allowed", async () => {
    mode[CHAT] = "allowAll";
    await show();
    expect(screen.getByRole("button", { name: "Allow all" })).toBeTruthy();
    // "Forget everything I allowed in this chat" put the chat back to asking.
    mode[CHAT] = "ask";
    await act(async () => {
      modeChanged(CHAT);
    });
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
    // The choice in Settings: every chat read so far is read again.
    mode[CHAT] = "fullAccess";
    await act(async () => {
      modeChanged(null);
    });
    expect(screen.getByRole("button", { name: "Full access" })).toBeTruthy();
  });

  it("words the hint under the message box for the chat on screen", async () => {
    const ASK = "the model asks before it changes anything";
    const ALL = "Allow all is on: the model asks only before deleting and a few other things";
    const FULL = "Full access is on: the model does everything without asking";
    expect(APPROVAL_HINTS).toEqual({ ask: ASK, allowAll: ALL, fullAccess: FULL });
    useApp.setState({ activeChatId: CHAT });
    const { container } = render(<ApprovalHint />);
    await act(async () => {});
    expect(container.textContent).toBe(ASK);
    await act(async () => {
      await useApprovalMode.getState().setMode(CHAT, "allowAll");
    });
    expect(container.textContent).toBe(ALL);
    await act(async () => {
      await useApprovalMode.getState().setMode(CHAT, "fullAccess");
    });
    expect(container.textContent).toBe(FULL);
    // Full access stays in sight in the danger colour.
    expect(container.querySelector(".text-danger")?.textContent).toBe(FULL);
    // Another chat on screen is in its own mode.
    await act(async () => {
      useApp.setState({ activeChatId: 8 });
    });
    expect(container.textContent).toBe(ASK);
    expect(container.querySelector(".text-danger")).toBeNull();
    // With no chat open, it says what a new chat will start in.
    await act(async () => {
      useApp.setState({ activeChatId: null });
    });
    expect(container.textContent).toBe(ASK);
    await act(async () => {
      useApprovalMode.setState({ defaultMode: "allowAll" });
    });
    expect(container.textContent).toBe(ALL);
    await act(async () => {
      useApprovalMode.setState({ defaultMode: "fullAccess" });
    });
    expect(container.textContent).toBe(FULL);
    useApp.setState({ activeChatId: null });
  });

  it("uses plain wording with no exclamation marks", () => {
    const text = [...APPROVAL_MODES.map((m) => m.description), ...Object.values(APPROVAL_HINTS)].join(" ");
    expect(text).not.toContain("!");
  });
});
