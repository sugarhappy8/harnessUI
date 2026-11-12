import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ApprovalRequest, KeepAsking } from "../lib/types";
import type { ApprovalModeName } from "../lib/ipc";

// The chat's approval mode is switched through these commands only.
const ipcMock = vi.hoisted(() => ({
  getChatApprovalMode: vi.fn<(chatId: number) => Promise<ApprovalModeName>>(async () => "ask"),
  setChatApprovalMode: vi.fn<(chatId: number, mode: ApprovalModeName) => Promise<ApprovalModeName>>(
    async (_chat, mode) => mode,
  ),
  getDefaultApprovalMode: vi.fn<() => Promise<ApprovalModeName>>(async () => "ask"),
  setDefaultApprovalMode: vi.fn(async () => undefined),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { ALLOW_ALL_AFTER_UNTRUSTED, ApprovalCard, CARD_INPUT_DELAY_MS, KEEP_ASKING_TEXT } = await import(
  "./ApprovalCard"
);
const { CONFIRM_INPUT_DELAY_MS, STILL_ASKS } = await import("./AllowAllButton");
const { useApprovalMode } = await import("../lib/approvalMode");
const { useApp } = await import("../store/app");

const base: ApprovalRequest = {
  id: "req-1",
  chatId: 1,
  callId: "call_1",
  server: "files",
  tool: "write_file",
  summary: "Create /home/u/proj/a.txt (5 bytes)",
  preview: "+hello",
  card: "grantable",
  reason: "normal",
};

describe("ApprovalCard safety", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("ignores clicks for a short time after it appears", () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    fireEvent.click(screen.getByText("Allow once"));
    expect(onDecide).not.toHaveBeenCalled();
    act(() => {
      vi.advanceTimersByTime(CARD_INPUT_DELAY_MS + 10);
    });
    fireEvent.click(screen.getByText("Allow once"));
    expect(onDecide).toHaveBeenCalledWith("allowOnce", null);
  });

  it("never gives Allow default focus and Enter does not approve", () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    act(() => {
      vi.advanceTimersByTime(CARD_INPUT_DELAY_MS + 10);
    });
    const allow = screen.getByText("Allow once");
    expect(document.activeElement).not.toBe(allow);
    expect(document.activeElement).not.toBe(screen.getByText("Allow for this chat"));
    fireEvent.keyDown(allow, { key: "Enter" });
    expect(onDecide).not.toHaveBeenCalled();
  });

  it("always-ask cards offer only Allow once and Deny", () => {
    render(<ApprovalCard request={{ ...base, card: "onceOnly", reason: "alwaysAsk" }} onDecide={() => {}} />);
    expect(screen.queryByText("Allow for this chat")).toBeNull();
    expect(screen.getByText(/always asks/)).toBeTruthy();
  });

  it("shows model text as text, including escaped controls, never as HTML", () => {
    const evil: ApprovalRequest = {
      ...base,
      summary: '<img src=x onerror="alert(1)">',
      preview: "evil\\u{202E}txt.exe <script>alert(2)</script>",
      reason: "recoveredFromText",
    };
    const { container } = render(<ApprovalCard request={evil} onDecide={() => {}} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.textContent).toContain("\\u{202E}");
    expect(container.textContent).toContain("recovered from text");
  });

  it("passes a deny note back", () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    act(() => {
      vi.advanceTimersByTime(CARD_INPUT_DELAY_MS + 10);
    });
    fireEvent.click(screen.getByText("Add a note"));
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "use the other folder" } });
    fireEvent.click(screen.getByText("Deny"));
    expect(onDecide).toHaveBeenCalledWith("deny", "use the other folder");
  });
});

describe("ApprovalCard: Allow all in this chat", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    ipcMock.setChatApprovalMode.mockImplementation(async (_chat, mode) => mode);
    useApprovalMode.setState({ modes: {} });
    useApp.setState({ error: null });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  const arm = (ms: number) =>
    act(() => {
      vi.advanceTimersByTime(ms + 10);
    });
  const dialog = () => screen.getByText("Allow all in this chat?").closest('[role="alertdialog"]') as HTMLElement;
  const openConfirm = () => {
    arm(CARD_INPUT_DELAY_MS);
    fireEvent.click(screen.getByText("Allow all in this chat"));
  };

  it("is a third choice on cards that can be granted, and only there", () => {
    const { unmount } = render(<ApprovalCard request={base} onDecide={() => {}} />);
    expect(screen.getByText("Allow once")).toBeTruthy();
    expect(screen.getByText("Allow for this chat")).toBeTruthy();
    expect(screen.getByText("Allow all in this chat")).toBeTruthy();
    expect(screen.getByText("Deny")).toBeTruthy();
    unmount();
    render(<ApprovalCard request={{ ...base, card: "onceOnly", reason: "alwaysAsk" }} onDecide={() => {}} />);
    expect(screen.queryByText("Allow all in this chat")).toBeNull();
  });

  it("ignores the click until the card has been on screen for a moment", () => {
    render(<ApprovalCard request={base} onDecide={() => {}} />);
    fireEvent.click(screen.getByText("Allow all in this chat"));
    expect(screen.queryByText("Allow all in this chat?")).toBeNull();
    openConfirm();
    expect(screen.getByText("Allow all in this chat?")).toBeTruthy();
  });

  it("asks first with the list of what still asks, then switches the chat and approves this card", async () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    openConfirm();
    const d = dialog();
    // Nothing has happened yet: opening the dialog changes nothing.
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
    expect(onDecide).not.toHaveBeenCalled();
    const still = within(d).getByRole("list", { name: "Still asks" });
    expect(within(still).getAllByRole("listitem").map((li) => li.textContent)).toEqual([...STILL_ASKS]);
    expect(d.textContent).toContain("Deleting files or folders");
    expect(d.textContent).toContain("git push");
    // Cancel has the focus, never the confirm button.
    const confirm = within(d).getByRole("button", { name: "Allow all in this chat" });
    expect(document.activeElement).not.toBe(confirm);
    // The confirm button ignores a click that comes too soon (a double click).
    fireEvent.click(confirm);
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
    arm(CONFIRM_INPUT_DELAY_MS);
    await act(async () => {
      fireEvent.click(confirm);
    });
    expect(ipcMock.setChatApprovalMode).toHaveBeenCalledWith(1, "allowAll");
    expect(onDecide).toHaveBeenCalledWith("allowOnce", null);
    // The chat is switched before the card is approved.
    const switched = ipcMock.setChatApprovalMode.mock.invocationCallOrder[0] ?? Infinity;
    const approved = onDecide.mock.invocationCallOrder[0] ?? -1;
    expect(switched).toBeLessThan(approved);
    expect(useApprovalMode.getState().modes[1]).toBe("allowAll");
  });

  it("approves nothing when the confirmation is cancelled", async () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    openConfirm();
    arm(CONFIRM_INPUT_DELAY_MS);
    await act(async () => {
      fireEvent.click(within(dialog()).getByText("Cancel"));
    });
    expect(screen.queryByText("Allow all in this chat?")).toBeNull();
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
    expect(onDecide).not.toHaveBeenCalled();
    expect(useApprovalMode.getState().modes[1]).toBeUndefined();
  });

  it("approves nothing when the app refuses the switch", async () => {
    ipcMock.setChatApprovalMode.mockRejectedValueOnce({
      code: "bad_request",
      message: "That chat no longer exists.",
      nextStep: "",
      details: "",
    });
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    openConfirm();
    arm(CONFIRM_INPUT_DELAY_MS);
    await act(async () => {
      fireEvent.click(within(dialog()).getByRole("button", { name: "Allow all in this chat" }));
    });
    expect(onDecide).not.toHaveBeenCalled();
    expect(useApprovalMode.getState().modes[1]).toBeUndefined();
    expect(useApp.getState().error?.message).toBe("That chat no longer exists.");
  });

  it("is not offered while the chat is already in Allow all, and the card says why it shows", () => {
    const request: ApprovalRequest = { ...base, reason: "untrustedContentThenOutbound" };
    // In Ask mode the wording is the one it always had.
    const { unmount } = render(<ApprovalCard request={request} onDecide={() => {}} />);
    expect(screen.getByText(/this action sends data out/)).toBeTruthy();
    expect(screen.queryByText(ALLOW_ALL_AFTER_UNTRUSTED)).toBeNull();
    unmount();
    useApprovalMode.setState({ modes: { 1: "allowAll" } });
    render(<ApprovalCard request={request} onDecide={() => {}} />);
    expect(screen.queryByText("Allow all in this chat")).toBeNull();
    expect(screen.getByText("Allow for this chat")).toBeTruthy();
    expect(screen.getByText(ALLOW_ALL_AFTER_UNTRUSTED)).toBeTruthy();
    expect(ALLOW_ALL_AFTER_UNTRUSTED).toContain("Allow all is on in this chat");
  });

  it("is not offered in Full access either, where the card reads as in Allow all", () => {
    // A card should not appear in Full access at all; if one does, it offers no more than
    // in Allow all, and no Full access button.
    useApprovalMode.setState({ modes: { 1: "fullAccess" } });
    render(<ApprovalCard request={{ ...base, reason: "untrustedContentThenOutbound" }} onDecide={() => {}} />);
    expect(screen.queryByText("Allow all in this chat")).toBeNull();
    expect(screen.queryByText(/Full access/)).toBeNull();
    expect(screen.getByText("Allow once")).toBeTruthy();
    expect(screen.getByText("Allow for this chat")).toBeTruthy();
    expect(screen.getByText("Deny")).toBeTruthy();
    expect(screen.getByText(ALLOW_ALL_AFTER_UNTRUSTED)).toBeTruthy();
  });

  it("says why an action still asks in Allow all", () => {
    const kinds: KeepAsking[] = [
      "delete",
      "destroy",
      "sensitive",
      "publish",
      "protectedApp",
      "markedDestructive",
      "hidden",
    ];
    expect(Object.keys(KEEP_ASKING_TEXT).sort()).toEqual([...kinds].sort());
    for (const keepAsking of kinds) {
      const { container, unmount } = render(
        <ApprovalCard
          request={{ ...base, card: "onceOnly", reason: "stillAsksInAllowAll", keepAsking }}
          onDecide={() => {}}
        />,
      );
      expect(container.textContent).toContain("Allow all is on in this chat, but this still asks:");
      expect(container.textContent).toContain(KEEP_ASKING_TEXT[keepAsking]);
      // Allow once and Deny only.
      expect(screen.queryByText("Allow for this chat")).toBeNull();
      expect(screen.queryByText("Allow all in this chat")).toBeNull();
      unmount();
    }
    expect(KEEP_ASKING_TEXT.delete).toContain("deletes files");
    // Without a kind (an older card), it still says the mode is why.
    const { container } = render(
      <ApprovalCard request={{ ...base, card: "onceOnly", reason: "stillAsksInAllowAll" }} onDecide={() => {}} />,
    );
    expect(container.textContent).toContain("this kind of action always asks");
  });

  it("keeps its labels apart from Allow once and Deny", () => {
    render(<ApprovalCard request={base} onDecide={() => {}} />);
    openConfirm();
    const labels = [...document.querySelectorAll("button")].map((b) => b.textContent ?? "");
    expect(labels.filter((l) => l.includes("Allow once"))).toEqual(["Allow once"]);
    expect(labels.filter((l) => l.includes("Deny"))).toEqual(["Deny"]);
  });
});
