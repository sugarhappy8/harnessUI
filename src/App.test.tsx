// [0.0.7 P6] The app's own listeners: stopped even when they start after the window's effects
// were cleaned up, the chat whose reply runs read at start (a window that reloaded while a
// reply ran hears nothing more from it), and Esc stops the open chat's reply only.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { StrictMode } from "react";
import type { ApprovalRequest } from "./lib/types";

type Started = { unlisten: ReturnType<typeof vi.fn>; ready: () => void; handler: (payload: unknown) => void };

const listeners = vi.hoisted(() => ({ byName: new Map<string, Started[]>(), delayed: true }));

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
vi.mock("./components/ChatView", () => ({ ChatView: () => null }));
vi.mock("./components/Workbench", () => ({ Workbench: () => null }));
vi.mock("./components/SettingsDialog", () => ({ SettingsDialog: () => null }));
vi.mock("./components/ModelsDialog", () => ({ ModelsDialog: () => null }));
vi.mock("./components/FirstRun", () => ({ FirstRun: () => null }));
vi.mock("./components/ShortcutsDialog", () => ({ ShortcutsDialog: () => null }));
vi.mock("./components/AppNotices", () => ({ AppNotices: () => null }));
vi.mock("./components/AllowAllButton", () => ({ FullAccessStartCheck: () => null }));

const { default: App } = await import("./App");
const { useApp } = await import("./store/app");

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
});
afterEach(cleanup);

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
