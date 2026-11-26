import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import type { ApprovalRequest, KeepAsking } from "../lib/types";
import type { ApprovalModeName } from "../lib/ipc";

// The chat's approval mode is switched through these commands only.
const ipcMock = vi.hoisted(() => ({
  getChatApprovalMode: vi.fn<(chatId: number) => Promise<ApprovalModeName>>(
    async () => "ask",
  ),
  setChatApprovalMode: vi.fn<
    (chatId: number, mode: ApprovalModeName) => Promise<ApprovalModeName>
  >(async (_chat, mode) => mode),
  getDefaultApprovalMode: vi.fn<() => Promise<ApprovalModeName>>(
    async () => "ask",
  ),
  setDefaultApprovalMode: vi.fn(async () => undefined),
}));
vi.mock("../lib/ipc", () => ipcMock);

// [0.0.7] The app's "main window shown" event, sent by hand here.
const shown = vi.hoisted(() => ({
  handlers: new Set<() => void>(),
  stopped: 0,
}));
vi.mock("../lib/windowShown", () => ({
  MAIN_WINDOW_SHOWN: "main-window-shown",
  onMainWindowShown: (handler: () => void) => {
    shown.handlers.add(handler);
    return () => {
      shown.handlers.delete(handler);
      shown.stopped += 1;
    };
  },
}));

const {
  ALLOW_ALL_AFTER_UNTRUSTED,
  ApprovalCard,
  CARD_INPUT_DELAY_MS,
  KEEP_ASKING_TEXT,
} = await import("./ApprovalCard");
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
    expect(document.activeElement).not.toBe(
      screen.getByText("Allow for this chat"),
    );
    fireEvent.keyDown(allow, { key: "Enter" });
    expect(onDecide).not.toHaveBeenCalled();
  });

  it("always-ask cards offer only Allow once and Deny", () => {
    render(
      <ApprovalCard
        request={{ ...base, card: "onceOnly", reason: "alwaysAsk" }}
        onDecide={() => {}}
      />,
    );
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
    const { container } = render(
      <ApprovalCard request={evil} onDecide={() => {}} />,
    );
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
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "use the other folder" },
    });
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

  it("is a third choice on cards that can be granted, and only there", () => {
    const { unmount } = render(
      <ApprovalCard request={base} onDecide={() => {}} />,
    );
    expect(screen.getByText("Allow once")).toBeTruthy();
    expect(screen.getByText("Allow for this chat")).toBeTruthy();
    expect(screen.getByText("Allow all in this chat")).toBeTruthy();
    expect(screen.getByText("Deny")).toBeTruthy();
    unmount();
    render(
      <ApprovalCard
        request={{ ...base, card: "onceOnly", reason: "alwaysAsk" }}
        onDecide={() => {}}
      />,
    );
    expect(screen.queryByText("Allow all in this chat")).toBeNull();
  });

  it("ignores the click until the card has been on screen for a moment", async () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    fireEvent.click(screen.getByText("Allow all in this chat"));
    expect(ipcMock.setChatApprovalMode).not.toHaveBeenCalled();
    expect(onDecide).not.toHaveBeenCalled();
  });

  it("Allow all in this chat applies at once: the chat is switched, then this card is approved", async () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    arm(CARD_INPUT_DELAY_MS);
    await act(async () => {
      fireEvent.click(screen.getByText("Allow all in this chat"));
    });
    expect(
      screen.queryByRole("alertdialog", { name: /Allow all in this chat\?/ }),
    ).toBeNull();
    expect(ipcMock.setChatApprovalMode).toHaveBeenCalledTimes(1);
    expect(ipcMock.setChatApprovalMode).toHaveBeenCalledWith(1, "allowAll");
    expect(onDecide).toHaveBeenCalledWith("allowOnce", null);
    const switched =
      ipcMock.setChatApprovalMode.mock.invocationCallOrder[0] ?? Infinity;
    const approved = onDecide.mock.invocationCallOrder[0] ?? -1;
    expect(switched).toBeLessThan(approved);
    expect(useApprovalMode.getState().modes[1]).toBe("allowAll");
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
    arm(CARD_INPUT_DELAY_MS);
    await act(async () => {
      fireEvent.click(screen.getByText("Allow all in this chat"));
    });
    expect(onDecide).not.toHaveBeenCalled();
    expect(useApprovalMode.getState().modes[1]).toBeUndefined();
    expect(useApp.getState().error?.message).toBe(
      "That chat no longer exists.",
    );
  });

  it("is not offered while the chat is already in Allow all, and the card says why it shows", () => {
    const request: ApprovalRequest = {
      ...base,
      reason: "untrustedContentThenOutbound",
    };
    // In Ask mode the wording is the one it always had.
    const { unmount } = render(
      <ApprovalCard request={request} onDecide={() => {}} />,
    );
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
    render(
      <ApprovalCard
        request={{ ...base, reason: "untrustedContentThenOutbound" }}
        onDecide={() => {}}
      />,
    );
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
          request={{
            ...base,
            card: "onceOnly",
            reason: "stillAsksInAllowAll",
            keepAsking,
          }}
          onDecide={() => {}}
        />,
      );
      expect(container.textContent).toContain(
        "Allow all is on in this chat, but this still asks:",
      );
      expect(container.textContent).toContain(KEEP_ASKING_TEXT[keepAsking]);
      // Allow once and Deny only.
      expect(screen.queryByText("Allow for this chat")).toBeNull();
      expect(screen.queryByText("Allow all in this chat")).toBeNull();
      unmount();
    }
    expect(KEEP_ASKING_TEXT.delete).toContain("deletes files");
    // Without a kind (an older card), it still says the mode is why.
    const { container } = render(
      <ApprovalCard
        request={{ ...base, card: "onceOnly", reason: "stillAsksInAllowAll" }}
        onDecide={() => {}}
      />,
    );
    expect(container.textContent).toContain("this kind of action always asks");
  });

  it("keeps its labels apart from Allow once and Deny", () => {
    render(<ApprovalCard request={base} onDecide={() => {}} />);
    const labels = [...document.querySelectorAll("button")].map(
      (b) => b.textContent ?? "",
    );
    expect(labels.filter((l) => l.includes("Allow once"))).toEqual([
      "Allow once",
    ]);
    expect(labels.filter((l) => l.includes("Deny"))).toEqual(["Deny"]);
  });
});

describe("ApprovalCard after the window comes back (0.0.7)", () => {
  let visibility: DocumentVisibilityState = "visible";
  beforeEach(() => {
    vi.useFakeTimers();
    visibility = "visible";
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => visibility,
    });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    // Back to jsdom's own.
    Reflect.deleteProperty(document, "visibilityState");
  });

  const wait = (ms: number) =>
    act(() => {
      vi.advanceTimersByTime(ms);
    });
  const allowDisabled = () =>
    screen.getByText("Allow once").getAttribute("aria-disabled");
  const windowShown = () =>
    act(() => {
      for (const h of [...shown.handlers]) h();
    });
  const setVisibility = (v: DocumentVisibilityState) =>
    act(() => {
      visibility = v;
      document.dispatchEvent(new Event("visibilitychange"));
    });

  it("ignores clicks again for a moment each time the app brings the window back", () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    wait(CARD_INPUT_DELAY_MS + 10);
    expect(allowDisabled()).toBe("false");
    // The app restored its window for this card, in front of where the user was clicking.
    windowShown();
    expect(allowDisabled()).toBe("true");
    fireEvent.click(screen.getByText("Allow once"));
    expect(onDecide).not.toHaveBeenCalled();
    wait(CARD_INPUT_DELAY_MS - 100);
    fireEvent.click(screen.getByText("Allow once"));
    expect(onDecide).not.toHaveBeenCalled();
    wait(110);
    fireEvent.click(screen.getByText("Allow once"));
    expect(onDecide).toHaveBeenCalledWith("allowOnce", null);
  });

  it("does not count the time while the page is hidden", () => {
    const onDecide = vi.fn();
    // The card arrives while the window is minimised.
    visibility = "hidden";
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    wait(5000);
    expect(allowDisabled()).toBe("true");
    setVisibility("visible");
    fireEvent.click(screen.getByText("Deny"));
    expect(onDecide).not.toHaveBeenCalled();
    wait(CARD_INPUT_DELAY_MS + 10);
    expect(allowDisabled()).toBe("false");
    // Hidden again (minimised by hand): disarmed until it is back.
    setVisibility("hidden");
    expect(allowDisabled()).toBe("true");
  });

  it("ignores a click in the same moment the window comes back, before the card is drawn again", () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    wait(CARD_INPUT_DELAY_MS + 10);
    const allow = screen.getByText("Allow once");
    act(() => {
      for (const h of [...shown.handlers]) h();
      // A plain DOM click: React has not drawn the card again yet.
      allow.click();
    });
    expect(onDecide).not.toHaveBeenCalled();
  });

  it("keeps working when the window merely gets the focus", () => {
    const onDecide = vi.fn();
    render(<ApprovalCard request={base} onDecide={onDecide} />);
    wait(CARD_INPUT_DELAY_MS + 10);
    // The click that focuses a card shown in front is the user's own.
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    fireEvent.click(screen.getByText("Allow once"));
    expect(onDecide).toHaveBeenCalledWith("allowOnce", null);
  });

  it("stops listening when it goes, and a late event starts nothing", () => {
    const { unmount } = render(
      <ApprovalCard request={base} onDecide={() => {}} />,
    );
    const handler = [...shown.handlers][0];
    expect(handler).toBeDefined();
    const before = shown.stopped;
    unmount();
    expect(shown.stopped).toBe(before + 1);
    expect(shown.handlers.size).toBe(0);
    // Tauri can still call a listener once after its cleanup ran.
    act(() => handler?.());
    expect(vi.getTimerCount()).toBe(0);
  });
});
