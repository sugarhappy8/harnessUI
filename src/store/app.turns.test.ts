// [0.0.7 P6a review] The app writes one reply at a time. A send or regenerate while a reply ran
// (an edit box left open when the reply started, a click in the moment it started, a reply this
// window did not start) put a new empty reply in the running one's place before the app
// refused it, then cleared it: the running reply's text and tool cards vanished, the chat's
// notice went, and a busy error showed. Following which chat's reply runs, a follower that
// React's development mode had already stopped still applied a late answer.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatDetail, ErrorInfo } from "../lib/types";

type Listener = { handler: (chatId: number | null) => void; ready: () => void; unlisten: () => void };

const turnListeners = vi.hoisted(() => [] as Listener[]);
const runningAnswers = vi.hoisted(() => [] as ((chatId: number | null) => void)[]);

const ipcMock = vi.hoisted(() => ({
  isTauri: () => true,
  getChat: vi.fn(async (): Promise<unknown> => {
    throw new Error("not read in these tests");
  }),
  listChats: vi.fn(async () => [] as unknown[]),
  sendMessage: vi.fn(async (..._args: unknown[]) => undefined),
  regenerate: vi.fn(async (..._args: unknown[]) => undefined),
  /** Starts listening when the test says so, as Tauri's `listen` does (asynchronously). Tauri
   *  calls the handler from before `listen` resolves until the listener is removed. */
  onTurnRunning: vi.fn(
    (handler: (chatId: number | null) => void) =>
      new Promise<() => void>((resolve) => {
        const unlisten = vi.fn();
        turnListeners.push({ handler, unlisten, ready: () => resolve(unlisten) });
      }),
  ),
  runningTurn: vi.fn(() => new Promise<number | null>((resolve) => runningAnswers.push(resolve))),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { followRunningTurn, useApp } = await import("./app");

const detail = (id: number): ChatDetail => ({
  chat: {
    id,
    title: `Chat ${id}`,
    modelRef: "local:/m.gguf",
    systemPrompt: null,
    leafMessageId: null,
    createdAt: 0,
    updatedAt: 0,
    thinking: true,
    workingFolder: null,
  },
  messages: [],
});

const BUSY: ErrorInfo = {
  code: "busy",
  message: "A reply is already being generated.",
  nextStep: "Wait for it to finish or press Stop.",
  details: "",
};
const NOTICE: ErrorInfo = {
  code: "stepCap",
  message: "The model reached the step limit for one turn.",
  nextStep: "Choose Continue to let it carry on.",
  details: "",
};

/** A reply being written in `chatId`: half an answer and a command running. */
function running(chatId: number) {
  return {
    chatId,
    text: "Half of the answer",
    reasoning: "",
    tools: [
      {
        callId: "c1",
        name: "run_command",
        arguments: { command: "make" },
        recovered: false,
        status: "running",
        text: "",
        decision: null,
        durationMs: null,
        progress: "building\n",
      },
    ],
    notes: [],
    step: 2,
    tokensPerSecond: null,
    promptTokens: null,
    completionTokens: null,
    contextUsed: null,
    nCtx: null,
  };
}

async function flush() {
  for (let i = 0; i < 6; i++) await Promise.resolve();
}

beforeEach(() => {
  vi.clearAllMocks();
  turnListeners.length = 0;
  runningAnswers.length = 0;
});

describe("a send or regenerate while a reply runs", () => {
  const COPY = { messageId: 12, attachmentIds: [5] };

  it("leaves the running reply of the open chat as it is", async () => {
    const reply = running(1);
    useApp.setState({ activeChatId: 1, detail: detail(1), generation: reply, runningTurn: 1, error: null });
    await useApp.getState().send("edited", 11, undefined, undefined, undefined, COPY);
    await useApp.getState().regenerate(13);
    expect(ipcMock.sendMessage).not.toHaveBeenCalled();
    expect(ipcMock.regenerate).not.toHaveBeenCalled();
    expect(useApp.getState().generation).toBe(reply);
    expect(useApp.getState().error).toBeNull();
  });

  it("changes nothing in the open chat while a reply runs in another one", async () => {
    const notes = { chatId: 2, notes: ["The reply may be cut short."] };
    for (const [generation, runningTurn] of [
      // This window's own reply.
      [running(1), 1],
      // A reply this window did not start (it went on while the window reloaded).
      [null, 1],
    ] as const) {
      useApp.setState({
        activeChatId: 2,
        detail: detail(2),
        generation,
        runningTurn,
        turnNotes: notes,
        stoppedTurns: { 2: NOTICE },
        error: null,
      });
      await useApp.getState().send("edited", 11, undefined, undefined, undefined, COPY);
      await useApp.getState().regenerate(13);
      const s = useApp.getState();
      expect(ipcMock.sendMessage).not.toHaveBeenCalled();
      expect(ipcMock.regenerate).not.toHaveBeenCalled();
      expect(s.generation).toBe(generation);
      expect(s.runningTurn).toBe(1);
      expect(s.turnNotes).toBe(notes);
      expect(s.stoppedTurns).toEqual({ 2: NOTICE });
      expect(s.error).toBeNull();
    }
  });
});

describe("a send or regenerate the app refuses", () => {
  it("clears its own reply from view, and never a newer one", async () => {
    const newer = running(1);
    for (const [command, ask] of [
      [ipcMock.sendMessage, () => useApp.getState().send("hi", null)],
      [ipcMock.regenerate, () => useApp.getState().regenerate(13)],
    ] as const) {
      useApp.setState({ activeChatId: 1, detail: detail(1), generation: null, runningTurn: null, error: null });
      command.mockRejectedValueOnce(BUSY);
      await ask();
      expect(command).toHaveBeenCalledTimes(1);
      expect(useApp.getState().generation).toBeNull();
      expect(useApp.getState().error).toEqual(BUSY);

      // A newer reply's view took this one's place while the request was on its way.
      useApp.setState({ generation: null, error: null });
      command.mockImplementationOnce(async () => {
        useApp.setState({ generation: newer });
        throw BUSY;
      });
      await ask();
      expect(command).toHaveBeenCalledTimes(2);
      expect(useApp.getState().generation).toBe(newer);
      expect(useApp.getState().error).toEqual(BUSY);
    }
  });
});

describe("following which chat's reply runs", () => {
  it("ignores what a follower hears or is answered after it was stopped", async () => {
    useApp.setState({ activeChatId: null, detail: null, generation: null, runningTurn: null });
    // React's development mode starts every effect, stops it and starts it again, before any
    // listener has started.
    const stopFirst = followRunningTurn();
    stopFirst();
    const stopSecond = followRunningTurn();
    expect(turnListeners).toHaveLength(2);
    const [first, second] = turnListeners as [Listener, Listener];
    first.ready();
    second.ready();
    await flush();
    expect(first.unlisten).toHaveBeenCalledTimes(1);
    expect(second.unlisten).not.toHaveBeenCalled();
    // The follower that runs is answered: chat 3's reply is being written.
    runningAnswers.at(-1)!(3);
    await flush();
    expect(useApp.getState().runningTurn).toBe(3);
    // It ends, and the app says so.
    second.handler(null);
    await flush();
    expect(useApp.getState().runningTurn).toBeNull();
    // The stopped follower: whatever it asked is answered late (from while chat 3's reply ran),
    // and its handler, which Tauri calls until the listener is removed, hears an old event.
    for (const answer of runningAnswers.slice(0, -1)) answer(3);
    first.handler(3);
    await flush();
    expect(useApp.getState().runningTurn).toBeNull();
    // The follower that runs still follows.
    second.handler(5);
    expect(useApp.getState().runningTurn).toBe(5);
    stopSecond();
    expect(second.unlisten).toHaveBeenCalledTimes(1);
    second.handler(null);
    expect(useApp.getState().runningTurn).toBe(5);
  });
});
