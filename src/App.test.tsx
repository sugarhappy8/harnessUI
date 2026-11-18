// [0.0.7 P6] The app's own listeners: stopped even when they start after the window's effects
// were cleaned up, the chat whose reply runs read at start (a window that reloaded while a
// reply ran hears nothing more from it), and Esc stops the open chat's reply only.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { StrictMode } from "react";
import type { ApprovalRequest } from "./lib/types";
import type { Run } from "./components/studio/types";
import { RUN } from "./components/studio/fixtures/models";

type Started = { unlisten: ReturnType<typeof vi.fn>; ready: () => void; handler: (payload: unknown) => void };

const listeners = vi.hoisted(() => {
  const byName = new Map<string, Started[]>();
  return {
    byName,
    delayed: true,
    /** What the app sends: each listener of `name` hears it. */
    emit: (name: string, payload?: unknown) => (byName.get(name) ?? []).forEach((s) => s.handler(payload)),
  };
});

const ipcMock = vi.hoisted(() => {
  /** A listener that starts when the test says so, as Tauri's `listen` does (asynchronously). */
  const delayed = (name: string) =>
    vi.fn(
      (handler: (payload: unknown) => void) =>
        new Promise<() => void>((resolve) => {
          const unlisten = vi.fn();
          const list = listeners.byName.get(name) ?? [];
          list.push({ unlisten, handler, ready: () => resolve(unlisten) });
          listeners.byName.set(name, list);
          if (!listeners.delayed) resolve(unlisten);
        }),
    );
  return {
    isTauri: () => true,
    onApprovalRequest: delayed("approval-request"),
    onApprovalClosed: delayed("approval-closed"),
    onRuntimeStatus: delayed("runtime-status"),
    onTurnRunning: delayed("turn-running"),
    onStopRequested: delayed("stop-requested"),
    runningTurn: vi.fn(async () => null as number | null),
    pendingApprovals: vi.fn(async (): Promise<ApprovalRequest[]> => []),
    runtimeStatus: vi.fn(async () => null),
    listChats: vi.fn(async () => []),
    listModelOptions: vi.fn(async () => []),
    firstRunInfo: vi.fn(async () => ({ done: true })),
    getChat: vi.fn(async () => {
      throw new Error("no chat");
    }),
    stopGeneration: vi.fn<(chatId?: number | null) => Promise<void>>(async () => undefined),
  };
});
vi.mock("./lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./components/Sidebar", () => ({ Sidebar: () => null }));
vi.mock("./components/ChatView", () => ({ ChatView: () => <div data-testid="chat-view" /> }));
vi.mock("./components/Workbench", () => ({ Workbench: () => null }));
vi.mock("./components/studio/StudioView", () => ({ StudioView: () => <div data-testid="studio-view" /> }));
// Every Studio command and event wrapper, mocked as it is.
vi.mock("./components/studio/api");
vi.mock("./components/SettingsDialog", () => ({ SettingsDialog: () => null }));
vi.mock("./components/ModelsDialog", () => ({ ModelsDialog: () => null }));
vi.mock("./components/FirstRun", () => ({ FirstRun: () => null }));
vi.mock("./components/ShortcutsDialog", () => ({ ShortcutsDialog: () => null }));
vi.mock("./components/AppNotices", () => ({ AppNotices: () => null }));
vi.mock("./components/AllowAllButton", () => ({ FullAccessStartCheck: () => null }));

const { default: App } = await import("./App");
const { useApp } = await import("./store/app");
const { useStudio } = await import("./store/studio");
const studioApi = await import("./components/studio/api");

const NAMES = ["approval-request", "approval-closed", "runtime-status", "turn-running"];

async function flush() {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

function started(name: string): Started[] {
  return listeners.byName.get(name) ?? [];
}

beforeEach(() => {
  vi.clearAllMocks();
  listeners.byName.clear();
  listeners.delayed = true;
  useApp.setState({ activeChatId: null, detail: null, generation: null, runningTurn: null, approvals: [] });
  useStudio.setState({ view: "chat", tab: "image", live: {}, readyNotice: null });
});
afterEach(cleanup);

/** A reply being written in chat `chatId`. */
const turn = (chatId: number) => ({
  chatId,
  text: "",
  reasoning: "",
  tools: [],
  notes: [],
  step: 1,
  tokensPerSecond: null,
  promptTokens: null,
  completionTokens: null,
  contextUsed: null,
  nCtx: null,
});

/** The app, with its listeners started. */
async function renderApp() {
  listeners.delayed = false;
  await act(async () => {
    render(<App />);
    await flush();
  });
}

describe("the app's listeners (0.0.7 P6)", () => {
  it("stops each listener that starts after its effect was cleaned up, and keeps the newest", async () => {
    render(
      <StrictMode>
        <App />
      </StrictMode>,
    );
    // Development mode runs every effect, cleans it up and runs it again, before any
    // listener has started.
    for (const name of NAMES) expect(started(name).length, name).toBe(2);
    await act(async () => {
      for (const name of NAMES) for (const s of started(name)) s.ready();
      await flush();
    });
    for (const name of NAMES) {
      const [first, second] = started(name);
      expect(first!.unlisten, name).toHaveBeenCalledTimes(1);
      expect(second!.unlisten, name).not.toHaveBeenCalled();
    }
    cleanup();
    for (const name of NAMES) expect(started(name)[1]!.unlisten, name).toHaveBeenCalledTimes(1);
  });
});

describe("the chat whose reply runs", () => {
  it("is read at start, so a window that reloaded while a reply ran knows of it", async () => {
    listeners.delayed = false;
    ipcMock.runningTurn.mockResolvedValueOnce(3);
    await act(async () => {
      render(<App />);
      await flush();
    });
    expect(useApp.getState().runningTurn).toBe(3);
    // The app says when it ends, and when another one starts.
    await act(async () => started("turn-running")[0]!.handler(null));
    expect(useApp.getState().runningTurn).toBeNull();
    await act(async () => started("turn-running")[0]!.handler(5));
    expect(useApp.getState().runningTurn).toBe(5);
  });

  it("follows what the app says over an older answer to the read at start", async () => {
    let answer: (chatId: number | null) => void = () => undefined;
    ipcMock.runningTurn.mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)));
    render(<App />);
    await act(async () => {
      started("turn-running")[0]!.ready();
      await flush();
    });
    await act(async () => started("turn-running")[0]!.handler(null));
    await act(async () => {
      answer(3);
      await flush();
    });
    expect(useApp.getState().runningTurn).toBeNull();
  });
});

describe("approval cards waiting when the window loads (0.0.7 P6)", () => {
  const card = (id: string, chatId = 2): ApprovalRequest => ({
    id,
    chatId,
    callId: `call-${id}`,
    server: "builtin",
    tool: "run_command",
    summary: "Run a command",
    preview: "ls",
    card: "onceOnly",
    reason: "normal",
    keepAsking: null,
  });

  it("are read once both listeners started, so a reply that waits for one can go on", async () => {
    ipcMock.pendingApprovals.mockResolvedValueOnce([card("a"), card("b", 3)]);
    render(<App />);
    await act(async () => {
      started("approval-request")[0]!.ready();
      await flush();
    });
    expect(ipcMock.pendingApprovals).not.toHaveBeenCalled();
    await act(async () => {
      started("approval-closed")[0]!.ready();
      await flush();
      await flush();
    });
    expect(ipcMock.pendingApprovals).toHaveBeenCalledTimes(1);
    expect(useApp.getState().approvals.map((a) => a.id)).toEqual(["a", "b"]);
  });

  it("keeps a card closed while the answer was on its way, and adds none twice", async () => {
    let answer: (cards: ApprovalRequest[]) => void = () => undefined;
    ipcMock.pendingApprovals.mockImplementationOnce(() => new Promise((resolve) => (answer = resolve)));
    listeners.delayed = false;
    await act(async () => {
      render(<App />);
      await flush();
    });
    expect(ipcMock.pendingApprovals).toHaveBeenCalledTimes(1);
    // While the answer is on its way, card b arrives by its event and card a is answered.
    await act(async () => started("approval-request")[0]!.handler(card("b", 3)));
    await act(async () => started("approval-closed")[0]!.handler("a"));
    await act(async () => {
      answer([card("a"), card("b", 3), card("c")]);
      await flush();
    });
    expect(useApp.getState().approvals.map((a) => a.id)).toEqual(["b", "c"]);
  });
});

describe("Esc", () => {
  it("stops the open chat's reply, never another chat's", async () => {
    listeners.delayed = false;
    await act(async () => {
      render(<App />);
      await flush();
    });
    useApp.setState({ activeChatId: 2, generation: turn(1) });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(ipcMock.stopGeneration).not.toHaveBeenCalled();
    useApp.setState({ activeChatId: 1 });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(ipcMock.stopGeneration).toHaveBeenCalledWith(1);
    // Also a reply this window did not start.
    ipcMock.stopGeneration.mockClear();
    useApp.setState({ activeChatId: 4, generation: null, runningTurn: 4 });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(ipcMock.stopGeneration).toHaveBeenCalledWith(4);
  });
});

// 0.1.0 Studio: a view beside the chat, which keeps running (and keeps its draft) while
// Studio is shown. Esc never cancels a Studio job, and the stop hotkey stops chat turns only.
describe("Studio", () => {
  function run(patch: Partial<Run>): Run {
    return { ...RUN, ...patch };
  }

  it("Ctrl+G opens Studio and Esc there stops nothing", async () => {
    await renderApp();
    useApp.setState({ activeChatId: 1, generation: turn(1) }); // Esc would stop this reply in the chat view
    fireEvent.keyDown(window, { key: "g", ctrlKey: true });
    expect(useStudio.getState().view).toBe("studio");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(ipcMock.stopGeneration).not.toHaveBeenCalled();
    act(() => useStudio.getState().showChat());
    fireEvent.keyDown(window, { key: "Escape" });
    expect(ipcMock.stopGeneration).toHaveBeenCalledWith(1); // the seed was live
  });

  it("the stop hotkey leaves Studio runs alone", async () => {
    await renderApp();
    useStudio.setState({ live: { r1: run({ status: "waiting", watching: true }) } });
    listeners.emit("stop-requested"); // what hotkey::stop_now sends
    expect(studioApi.studioCancel).not.toHaveBeenCalled();
    expect(useStudio.getState().live.r1?.status).toBe("waiting");
  });

  it("app:open-studio opens Studio on the tab it names", async () => {
    await renderApp();
    act(() => {
      window.dispatchEvent(new CustomEvent("app:open-studio", { detail: "video" }));
    });
    expect(useStudio.getState()).toMatchObject({ view: "studio", tab: "video" });
    // Without a tab it opens on the one shown last.
    act(() => useStudio.getState().showChat());
    act(() => {
      window.dispatchEvent(new CustomEvent("app:open-studio"));
    });
    expect(useStudio.getState()).toMatchObject({ view: "studio", tab: "video" });
  });

  it("the chat stays mounted, hidden, while Studio is shown", async () => {
    await renderApp();
    expect(screen.queryByTestId("studio-view")).toBeNull(); // mounted on first open
    const chat = screen.getByTestId("chat-view");
    expect(chat.closest("[hidden]")).toBeNull();
    act(() => useStudio.getState().showStudio());
    expect(screen.getByTestId("chat-view")).toBe(chat);
    expect(chat.closest("[hidden]")).not.toBeNull();
    const studio = screen.getByTestId("studio-view");
    expect(studio.closest("[hidden]")).toBeNull();
    // Back in the chat, Studio stays mounted (hidden) too.
    act(() => useStudio.getState().showChat());
    expect(chat.closest("[hidden]")).toBeNull();
    expect(screen.getByTestId("studio-view")).toBe(studio);
    expect(studio.closest("[hidden]")).not.toBeNull();
  });

  it("Ctrl+N goes back to the chat", async () => {
    const original = useApp.getState().newChat;
    const newChat = vi.fn(async () => undefined);
    useApp.setState({ newChat });
    try {
      await renderApp();
      act(() => useStudio.getState().showStudio());
      fireEvent.keyDown(window, { key: "n", ctrlKey: true });
      expect(useStudio.getState().view).toBe("chat");
      expect(newChat).toHaveBeenCalledTimes(1);
    } finally {
      useApp.setState({ newChat: original });
    }
  });

  it("Ctrl+L goes back to the chat and focuses the message box", async () => {
    await renderApp();
    const focus = vi.fn();
    window.addEventListener("app:focus-input", focus);
    try {
      act(() => useStudio.getState().showStudio());
      fireEvent.keyDown(window, { key: "l", ctrlKey: true });
      expect(useStudio.getState().view).toBe("chat");
      expect(focus).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("app:focus-input", focus);
    }
  });
});
