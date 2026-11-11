import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";

let mode: Record<number, boolean> = {};
let modeChanged: (chatId: number | null) => void = () => undefined;

const ipcMock = vi.hoisted(() => ({
  getApprovalMode: vi.fn<(chatId: number) => Promise<boolean>>(),
  setApprovalMode: vi.fn<(chatId: number, allowAll: boolean) => Promise<boolean>>(),
  getAllowAllDefault: vi.fn(async () => false),
  setAllowAllDefault: vi.fn(async () => undefined),
  onApprovalModeChanged: vi.fn(async (handler: (chatId: number | null) => void) => {
    modeChanged = handler;
    return () => undefined;
  }),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { AllowAllButton, AllowAllConfirm, ApprovalHint, CONFIRM_INPUT_DELAY_MS, LIMITS, NO_LONGER_ASKS, STILL_ASKS } =
  await import("./AllowAllButton");
const { useApprovalMode } = await import("../lib/approvalMode");
const { useApp } = await import("../store/app");

const CHAT = 7;

async function show(chatId = CHAT) {
  await act(async () => {
    render(<AllowAllButton chatId={chatId} />);
  });
}

const arm = () =>
  act(() => {
    vi.advanceTimersByTime(CONFIRM_INPUT_DELAY_MS + 10);
  });

describe("AllowAllButton", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    mode = {};
    ipcMock.getApprovalMode.mockImplementation(async (chatId) => mode[chatId] ?? false);
    ipcMock.setApprovalMode.mockImplementation(async (chatId, on) => {
      mode[chatId] = on;
      return on;
    });
    useApprovalMode.setState({ modes: {}, defaultOn: false });
    useApp.setState({ error: null });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("reads Ask first until the chat is switched, and asks before switching on", async () => {
    await show();
    expect(ipcMock.getApprovalMode).toHaveBeenCalledWith(CHAT);
    const button = screen.getByRole("button", { name: "Ask first" });
    expect(button.textContent).toBe("Ask first");
    await act(async () => {
      fireEvent.click(button);
    });
    // Clicking only opens the confirmation.
    expect(ipcMock.setApprovalMode).not.toHaveBeenCalled();
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
    expect(ipcMock.setApprovalMode).not.toHaveBeenCalled();
    arm();
    await act(async () => {
      fireEvent.click(confirm);
    });
    expect(ipcMock.setApprovalMode).toHaveBeenCalledWith(CHAT, true);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // The mode is always visible while it is on.
    const on = screen.getByRole("button", { name: "Allow all" });
    expect(on.textContent).toBe("Allow all");
    expect(on.className).toContain("text-warning");
    expect(on.querySelector("span")?.className ?? "").not.toContain("hidden");
  });

  it("switches back to asking at once, with no confirmation", async () => {
    mode[CHAT] = true;
    await show();
    const on = screen.getByRole("button", { name: "Allow all" });
    await act(async () => {
      fireEvent.click(on);
    });
    expect(ipcMock.setApprovalMode).toHaveBeenCalledWith(CHAT, false);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
  });

  it("changes nothing when the confirmation is cancelled", async () => {
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Ask first" }));
    });
    arm();
    await act(async () => {
      fireEvent.click(within(screen.getByRole("alertdialog")).getByText("Cancel"));
    });
    expect(ipcMock.setApprovalMode).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
  });

  it("keeps each chat's mode apart and shows what the app answered", async () => {
    mode[8] = true;
    await show(8);
    expect(screen.getByRole("button", { name: "Allow all" })).toBeTruthy();
    cleanup();
    await show(CHAT);
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
    // The app refuses (the chat is gone): the button stays as it was and the error shows.
    ipcMock.setApprovalMode.mockRejectedValueOnce({
      code: "bad_request",
      message: "That chat no longer exists.",
      nextStep: "",
      details: "",
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Ask first" }));
    });
    arm();
    await act(async () => {
      fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Allow all in this chat" }));
    });
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
    expect(useApp.getState().error?.message).toBe("That chat no longer exists.");
  });

  it("words the confirmation for every chat when it is the default that is switched", () => {
    const onConfirm = vi.fn();
    render(<AllowAllConfirm open scope="every" onOpenChange={() => {}} onConfirm={onConfirm} />);
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText("Allow all in every chat?")).toBeTruthy();
    expect(dialog.textContent).toContain("stays on after a restart");
    arm();
    fireEvent.click(within(dialog).getByRole("button", { name: "Allow all in every chat" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("follows a change made elsewhere in the app, such as forgetting what was allowed", async () => {
    mode[CHAT] = true;
    await show();
    expect(screen.getByRole("button", { name: "Allow all" })).toBeTruthy();
    // "Forget everything I allowed in this chat" put the chat back to asking.
    mode[CHAT] = false;
    await act(async () => {
      modeChanged(CHAT);
    });
    expect(screen.getByRole("button", { name: "Ask first" })).toBeTruthy();
    // The switch in Settings: every chat read so far is read again.
    mode[CHAT] = true;
    await act(async () => {
      modeChanged(null);
    });
    expect(screen.getByRole("button", { name: "Allow all" })).toBeTruthy();
  });

  it("words the hint under the message box for the chat on screen", async () => {
    const ASK = "the model asks before it changes anything";
    const ALL = "Allow all is on: the model asks only before deleting and a few other things";
    useApp.setState({ activeChatId: CHAT });
    const { container } = render(<ApprovalHint />);
    await act(async () => {});
    expect(container.textContent).toBe(ASK);
    await act(async () => {
      await useApprovalMode.getState().setMode(CHAT, true);
    });
    expect(container.textContent).toBe(ALL);
    // Another chat on screen is in its own mode.
    await act(async () => {
      useApp.setState({ activeChatId: 8 });
    });
    expect(container.textContent).toBe(ASK);
    // With no chat open, it says what a new chat will start in.
    await act(async () => {
      useApp.setState({ activeChatId: null });
    });
    expect(container.textContent).toBe(ASK);
    await act(async () => {
      useApprovalMode.setState({ defaultOn: true });
    });
    expect(container.textContent).toBe(ALL);
    useApp.setState({ activeChatId: null });
  });

  it("uses plain wording with no exclamation marks and no labels of other buttons", () => {
    const text = [NO_LONGER_ASKS, ...STILL_ASKS, ...LIMITS].join(" ");
    expect(text).not.toContain("!");
    expect(STILL_ASKS.length).toBe(8);
    for (const label of ["Ask first", "Allow all", "Allow all in this chat", "Allow all in every chat"]) {
      expect(label).not.toContain("Allow once");
      expect(label).not.toContain("Deny");
    }
  });
});
