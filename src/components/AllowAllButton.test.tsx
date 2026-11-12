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
  getFullAccessDefaultPending: vi.fn<() => Promise<boolean>>(async () => false),
  answerFullAccessDefault: vi.fn<(on: boolean) => Promise<void>>(async () => undefined),
  onApprovalModeChanged: vi.fn(async (handler: (chatId: number | null) => void) => {
    modeChanged = handler;
    return () => undefined;
  }),
}));
vi.mock("../lib/ipc", () => ipcMock);

const {
  APPROVAL_HINTS,
  APPROVAL_MODES,
  AllowAllConfirm,
  ApprovalHint,
  ApprovalModeMenu,
  CONFIRM_INPUT_DELAY_MS,
  FULL_ACCESS_ASKS_AGAIN,
  FULL_ACCESS_RISKS,
  FULL_ACCESS_SUMMARY,
  FullAccessConfirm,
  FullAccessStartCheck,
  LIMITS,
  NO_LONGER_ASKS,
  STILL_ASKS,
} = await import("./AllowAllButton");
const { useApprovalMode } = await import("../lib/approvalMode");
const { useApp } = await import("../store/app");

const CHAT = 7;
const FULL_ACCESS_CHAT_LINE = "It stays on in this chat until you switch it off or quit the app.";

async function show(chatId = CHAT) {
  await act(async () => {
    render(<ApprovalModeMenu chatId={chatId} />);
  });
}

const arm = () =>
  act(() => {
    vi.advanceTimersByTime(CONFIRM_INPUT_DELAY_MS + 10);
  });

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

  it("asks before Allow all with the list of what still asks", async () => {
    await show();
    await pick("Ask first", "Allow all");
    // Choosing only opens the confirmation.
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Allow all in this chat?")).toBeTruthy();
    expect(dialog.textContent).toContain(NO_LONGER_ASKS);
    const still = within(dialog).getByRole("list", { name: "Still asks" });
    expect(within(still).getAllByRole("listitem").map((li) => li.textContent)).toEqual([...STILL_ASKS]);
    for (const line of LIMITS) expect(dialog.textContent).toContain(line);
    expect(dialog.textContent).toContain("until you switch it off or quit the app");

    const confirm = within(dialog).getByRole("button", { name: "Allow all in this chat" });
    expect(document.activeElement).not.toBe(confirm);
    // Too soon: ignored.
    fireEvent.click(confirm);
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
    arm();
    await act(async () => {
      fireEvent.click(confirm);
    });
    expect(ipcMock.setChatApprovalMode).toHaveBeenCalledWith(CHAT, "allowAll");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // The mode is always visible while it is on.
    const on = screen.getByRole("button", { name: "Allow all" });
    expect(on.textContent).toBe("Allow all");
    expect(on.className).toContain("text-warning");
    expect(on.querySelector("span")?.className ?? "").not.toContain("hidden");
  });

  it("asks before Full access with a plain warning, ignores an early click, then switches", async () => {
    await show();
    await pick("Ask first", "Full access");
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Turn on Full access in this chat?")).toBeTruthy();
    expect(dialog.className).toContain("border-danger");
    expect(dialog.querySelector("h2 svg")?.getAttribute("class") ?? "").toContain("text-danger");
    expect(dialog.textContent).toContain(FULL_ACCESS_SUMMARY);
    const risks = within(dialog).getByRole("list", { name: "Risks" });
    expect(within(risks).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      ...FULL_ACCESS_RISKS,
      FULL_ACCESS_CHAT_LINE,
    ]);
    for (const needle of ["Nothing asks first", "any command", "delete any file or folder", "trash", "browser", "logged in", "trick", "app's own data", "saved keys", "out of reach", "not a lock"]) {
      expect(dialog.textContent).toContain(needle);
    }
    // Allow all's list is not shown here: in Full access nothing still asks.
    expect(within(dialog).queryByRole("list", { name: "Still asks" })).toBeNull();

    const confirm = within(dialog).getByRole("button", { name: "Turn on Full access" });
    expect(confirm.className).toContain("text-danger");
    // Too soon: ignored.
    fireEvent.click(confirm);
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
    arm();
    // The closing menu left the focus on Cancel, never on the confirm button.
    expect(document.activeElement?.textContent).toBe("Cancel");
    await act(async () => {
      fireEvent.click(confirm);
    });
    expect(ipcMock.setChatApprovalMode).toHaveBeenCalledWith(CHAT, "fullAccess");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(useApprovalMode.getState().modes[CHAT]).toBe("fullAccess");
    const on = screen.getByRole("button", { name: "Full access" });
    expect(on.textContent).toBe("Full access");
    expect(on.className).toContain("text-danger");
    expect(on.className).toContain("bg-danger/10");
    expect(on.className).not.toContain("text-warning");
    expect(on.querySelector("span")?.className ?? "").not.toContain("hidden");
    // The focus goes back to the header button.
    act(() => {
      vi.advanceTimersByTime(10);
    });
    expect(document.activeElement).toBe(on);
  });

  it("changes nothing when the Full access confirmation is cancelled", async () => {
    await show();
    await pick("Ask first", "Full access");
    arm();
    await act(async () => {
      fireEvent.click(within(screen.getByRole("alertdialog")).getByText("Cancel"));
    });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
    expect(useApprovalMode.getState().modes[CHAT]).toBe("ask");
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
  });

  it("goes back to Ask first from Full access at once, with no confirmation", async () => {
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

  it("asks before going up from Allow all to Full access", async () => {
    mode[CHAT] = "allowAll";
    await show();
    await pick("Allow all", "Full access");
    expect(within(screen.getByRole("alertdialog")).getByText("Turn on Full access in this chat?")).toBeTruthy();
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
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
    arm();
    await act(async () => {
      fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Turn on Full access" }));
    });
    expect(ipcMock.setChatApprovalMode).toHaveBeenCalledWith(CHAT, "fullAccess");
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
    expect(useApprovalMode.getState().modes[CHAT]).toBe("ask");
    expect(useApprovalMode.getState().modes[8]).toBe("fullAccess");
    expect(useApp.getState().error?.message).toBe("That chat no longer exists.");
  });

  it("words the confirmations for every chat when it is the default that is changed", () => {
    const onAllowAll = vi.fn();
    const { unmount } = render(<AllowAllConfirm open scope="every" onOpenChange={() => {}} onConfirm={onAllowAll} />);
    let dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Allow all in every chat?")).toBeTruthy();
    expect(dialog.textContent).toContain("stays on after a restart");
    arm();
    fireEvent.click(within(dialog).getByRole("button", { name: "Allow all in every chat" }));
    expect(onAllowAll).toHaveBeenCalledTimes(1);
    unmount();

    const onFull = vi.fn();
    render(<FullAccessConfirm open scope="every" onOpenChange={() => {}} onConfirm={onFull} />);
    dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Turn on Full access in every chat?")).toBeTruthy();
    const risks = within(dialog).getByRole("list", { name: "Risks" });
    const lines = within(risks).getAllByRole("listitem").map((li) => li.textContent ?? "");
    expect(lines.slice(0, -1)).toEqual([...FULL_ACCESS_RISKS]);
    expect(lines.at(-1)).toContain("stays on after a restart");
    expect(lines.at(-1)).toContain("also the ones you switched by hand");
    const confirm = within(dialog).getByRole("button", { name: "Turn on Full access" });
    fireEvent.click(confirm);
    expect(onFull).not.toHaveBeenCalled();
    arm();
    fireEvent.click(confirm);
    expect(onFull).toHaveBeenCalledTimes(1);
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

  it("uses plain wording with no exclamation marks and no labels of other buttons", () => {
    const text = [
      NO_LONGER_ASKS,
      ...STILL_ASKS,
      ...LIMITS,
      FULL_ACCESS_SUMMARY,
      ...FULL_ACCESS_RISKS,
      ...APPROVAL_MODES.map((m) => m.description),
      ...Object.values(APPROVAL_HINTS),
    ].join(" ");
    expect(text).not.toContain("!");
    expect(STILL_ASKS.length).toBe(8);
    for (const label of ["Ask first", "Allow all", "Full access", "Allow all in this chat", "Turn on Full access"]) {
      expect(label).not.toContain("Allow once");
      expect(label).not.toContain("Deny");
    }
  });
});

describe("FullAccessStartCheck", () => {
  // What the app reports as the mode every chat starts in, before and after the answer.
  let defaultMode: ApprovalModeName = "ask";

  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    defaultMode = "ask";
    ipcMock.getFullAccessDefaultPending.mockImplementation(async () => true);
    ipcMock.answerFullAccessDefault.mockImplementation(async (on) => {
      defaultMode = on ? "fullAccess" : "ask";
    });
    ipcMock.getDefaultApprovalMode.mockImplementation(async () => defaultMode);
    useApprovalMode.setState({ modes: {}, defaultMode: "ask" });
    useApp.setState({ error: null });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  async function start() {
    await act(async () => {
      render(<FullAccessStartCheck />);
    });
  }

  it("shows nothing when the app is not waiting for an answer", async () => {
    ipcMock.getFullAccessDefaultPending.mockImplementation(async () => false);
    await start();
    expect(ipcMock.getFullAccessDefaultPending).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.answerFullAccessDefault).not.toHaveBeenCalled();
  });

  it("shows nothing when the question cannot be read: every chat stays in Ask first", async () => {
    ipcMock.getFullAccessDefaultPending.mockRejectedValueOnce(new Error("no app"));
    await start();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(useApp.getState().error).toBeNull();
  });

  it("asks with the risks, Ask first focused, and turns Full access on only after the delay", async () => {
    await start();
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Start every chat in Full access again?")).toBeTruthy();
    expect(dialog.className).toContain("border-danger");
    expect(dialog.textContent).toContain(FULL_ACCESS_ASKS_AGAIN);
    expect(FULL_ACCESS_ASKS_AGAIN).toContain("a program on this computer could have changed that setting");
    const risks = within(dialog).getByRole("list", { name: "Risks" });
    const lines = within(risks).getAllByRole("listitem").map((li) => li.textContent ?? "");
    expect(lines.slice(0, -1)).toEqual([...FULL_ACCESS_RISKS]);
    expect(lines.at(-1)).toContain("asks again the next time it starts");
    const askFirst = within(dialog).getByRole("button", { name: "Start in Ask first" });
    expect(document.activeElement).toBe(askFirst);
    const confirm = within(dialog).getByRole("button", { name: "Turn on Full access" });
    expect(confirm.className).toContain("text-danger");
    // Too soon (a double click, or a key held down): ignored.
    fireEvent.click(confirm);
    expect(ipcMock.answerFullAccessDefault).not.toHaveBeenCalled();
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    arm();
    await act(async () => {
      fireEvent.click(confirm);
    });
    // One answer: closing the dialog after confirming does not also answer Ask first.
    expect(ipcMock.answerFullAccessDefault).toHaveBeenCalledTimes(1);
    expect(ipcMock.answerFullAccessDefault).toHaveBeenCalledWith(true);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // Settings and the hint read the mode every chat starts in again.
    expect(ipcMock.getDefaultApprovalMode).toHaveBeenCalled();
    expect(useApprovalMode.getState().defaultMode).toBe("fullAccess");
  });

  it("starts in Ask first from its button", async () => {
    useApprovalMode.setState({ defaultMode: "fullAccess" });
    await start();
    await act(async () => {
      fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Start in Ask first" }));
    });
    expect(ipcMock.answerFullAccessDefault).toHaveBeenCalledTimes(1);
    expect(ipcMock.answerFullAccessDefault).toHaveBeenCalledWith(false);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(useApprovalMode.getState().defaultMode).toBe("ask");
  });

  it("answers Ask first when Escape closes it", async () => {
    await start();
    await act(async () => {
      fireEvent.keyDown(screen.getByRole("alertdialog"), { key: "Escape" });
    });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(ipcMock.answerFullAccessDefault).toHaveBeenCalledTimes(1);
    expect(ipcMock.answerFullAccessDefault).toHaveBeenCalledWith(false);
  });

  it("shows the error when the answer cannot be saved", async () => {
    ipcMock.answerFullAccessDefault.mockRejectedValueOnce({
      code: "internal",
      message: "The setting could not be saved.",
      nextStep: "",
      details: "",
    });
    await start();
    arm();
    await act(async () => {
      fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Turn on Full access" }));
    });
    expect(ipcMock.answerFullAccessDefault).toHaveBeenCalledWith(true);
    expect(useApp.getState().error?.message).toBe("The setting could not be saved.");
    // Nothing was turned on in the window's mirror.
    expect(ipcMock.getDefaultApprovalMode).not.toHaveBeenCalled();
    expect(useApprovalMode.getState().defaultMode).toBe("ask");
  });
});
