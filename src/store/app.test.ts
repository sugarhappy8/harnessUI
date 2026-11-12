import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatDetail, TurnStreamEvent } from "../lib/types";

// Capture the stream callback the store hands to sendMessage, and serve a chat detail.
let emit: (e: TurnStreamEvent) => void = () => undefined;
let finishSend: () => void = () => undefined;
const detail: ChatDetail = {
  chat: {
    id: 1,
    title: "t",
    modelRef: "local:/m.gguf",
    systemPrompt: null,
    leafMessageId: null,
    createdAt: 0,
    updatedAt: 0,
    thinking: true,
    workingFolder: null,
  },
  messages: [],
};

vi.mock("../lib/ipc", () => ({
  getChat: vi.fn(async () => detail),
  listChats: vi.fn(async () => []),
  sendMessage: vi.fn((_opts: unknown, onEvent: (e: TurnStreamEvent) => void) => {
    emit = onEvent;
    return new Promise<void>((resolve) => {
      finishSend = resolve;
    });
  }),
  resolveApproval: vi.fn(async () => undefined),
  createChat: vi.fn(async (modelRef: string | null) => ({ ...detail.chat, id: 7, modelRef })),
  setChatModel: vi.fn(async () => undefined),
}));

const { useApp } = await import("./app");

async function flush() {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

describe("streamed turn events", () => {
  beforeEach(async () => {
    useApp.setState({ activeChatId: 1, detail, generation: null, turnNotes: null, error: null, approvals: [] });
    void useApp.getState().send("hi", null);
    await flush();
  });

  it("accumulates text and thinking, and clears them when a message is stored", async () => {
    emit({ type: "reasoning", text: "Let me " });
    emit({ type: "reasoning", text: "think." });
    emit({ type: "content", text: "Hello" });
    emit({ type: "content", text: " there" });
    let g = useApp.getState().generation!;
    expect(g.reasoning).toBe("Let me think.");
    expect(g.text).toBe("Hello there");
    emit({ type: "messageSaved", messageId: 5, role: "assistant" });
    g = useApp.getState().generation!;
    expect(g.text).toBe("");
    expect(g.reasoning).toBe("");
  });

  it("tracks a tool call from start to finish, with progress", () => {
    emit({ type: "toolStarted", callId: "c1", name: "run_command", arguments: { command: "ls" }, recovered: false });
    emit({ type: "toolProgress", callId: "", text: "a.txt\n" });
    emit({ type: "toolProgress", callId: "", text: "b.txt\n" });
    let t = useApp.getState().generation!.tools[0]!;
    expect(t.status).toBe("running");
    expect(t.progress).toBe("a.txt\nb.txt\n");
    emit({
      type: "toolFinished",
      callId: "c1",
      name: "run_command",
      status: "ok",
      text: "a.txt\nb.txt",
      decision: "allowed once",
      durationMs: 120,
      ui: null,
    });
    t = useApp.getState().generation!.tools[0]!;
    expect(t.status).toBe("ok");
    expect(t.decision).toBe("allowed once");
  });

  it("puts a helper's progress on its own card, and keeps it out of the chat's text and numbers", () => {
    const finished = { type: "toolFinished", durationMs: 5, ui: null } as const;
    emit({
      type: "usage",
      promptTokens: 900,
      completionTokens: 20,
      tokensPerSecond: null,
      contextUsed: 900,
      nCtx: 8192,
    });
    emit({ type: "toolStarted", callId: "c1", name: "read_file", arguments: { path: "a.txt" }, recovered: false });
    emit({ ...finished, callId: "c1", name: "read_file", status: "ok", text: "a", decision: "allowed once" });
    emit({ type: "toolStarted", callId: "c2", name: "run_subagent", arguments: { task: "Read." }, recovered: false });
    // The helper's lines name the call they belong to.
    emit({ type: "toolProgress", callId: "c2", text: "read_file notes.txt" });
    emit({ type: "toolProgress", callId: "c2", text: " — done\n" });
    emit({ type: "toolProgress", callId: "c2", text: "the secret word is heron\n" });
    let g = useApp.getState().generation!;
    expect(g.tools.map((t) => t.name)).toEqual(["read_file", "run_subagent"]);
    expect(g.tools[0]!.progress).toBe("");
    expect(g.tools[1]!.progress).toBe("read_file notes.txt — done\nthe secret word is heron\n");
    expect(g.tools[1]!.status).toBe("running");
    // Nothing of the helper is the chat's answer text or its context numbers.
    expect(g.text).toBe("");
    expect(g.contextUsed).toBe(900);
    expect(g.promptTokens).toBe(900);
    emit({
      ...finished,
      callId: "c2",
      name: "run_subagent",
      status: "ok",
      text: "the secret word is heron",
      decision: "no approval needed",
    });
    g = useApp.getState().generation!;
    expect(g.tools[1]!.text).toBe("the secret word is heron");
    expect(g.tools[1]!.decision).toBe("no approval needed");
  });

  it("drops a discarded attempt's text", () => {
    emit({ type: "content", text: "<tool_call>{" });
    emit({ type: "discarded" });
    expect(useApp.getState().generation!.text).toBe("");
  });

  it("reports a step-limit finish and ends the generation", async () => {
    emit({ type: "finished", status: "stepCap", message: "The step limit for one turn was reached." });
    await flush();
    expect(useApp.getState().error?.code).toBe("stepCap");
    expect(useApp.getState().generation).toBeNull();
    finishSend();
  });

  it("keeps a turn's notes readable after it ends, until the next message", async () => {
    emit({ type: "content", text: "Half an ans" });
    emit({ type: "note", text: "The reply may be cut short." });
    emit({ type: "finished", status: "complete", message: null });
    await flush();
    expect(useApp.getState().generation).toBeNull();
    expect(useApp.getState().turnNotes).toEqual({ chatId: 1, notes: ["The reply may be cut short."] });
    finishSend();
    void useApp.getState().send("next", null);
    await flush();
    expect(useApp.getState().turnNotes).toBeNull();
    // A turn without notes leaves none behind.
    emit({ type: "finished", status: "complete", message: null });
    await flush();
    expect(useApp.getState().turnNotes).toBeNull();
    finishSend();
  });

  it("ends the generation even when the chat cannot be reloaded (deleted mid-reply)", async () => {
    const ipc = await import("../lib/ipc");
    vi.mocked(ipc.getChat).mockRejectedValueOnce({ code: "not_found", message: "gone", nextStep: "", details: "" });
    emit({ type: "content", text: "still writ" });
    emit({ type: "finished", status: "stopped", message: null });
    await flush();
    expect(useApp.getState().generation).toBeNull();
    finishSend();
  });

  it("keeps approval cards by id and removes them when answered", async () => {
    const r = {
      id: "a1",
      chatId: 1,
      callId: "c1",
      server: "files",
      tool: "write_file",
      summary: "s",
      preview: "p",
      card: "grantable" as const,
      reason: "normal" as const,
    };
    useApp.getState().addApproval(r);
    useApp.getState().addApproval(r);
    expect(useApp.getState().approvals).toHaveLength(1);
    await useApp.getState().decideApproval("a1", "deny", "no");
    expect(useApp.getState().approvals).toHaveLength(0);
  });
});

// Reported from a real PC: a model picked with no chat open loaded in the background, but
// nothing on screen changed, so it was picked again and the first load "was cancelled".
describe("picking a model", () => {
  it("opens a new chat for it when no chat is open", async () => {
    const ipc = await import("../lib/ipc");
    useApp.setState({ activeChatId: null, detail: null, error: null });
    await useApp.getState().setChatModel("local:/models/a.gguf");
    expect(ipc.createChat).toHaveBeenCalledWith("local:/models/a.gguf");
    expect(ipc.setChatModel).not.toHaveBeenCalled();
    expect(useApp.getState().activeChatId).toBe(7);
  });

  it("changes the open chat's model when there is one", async () => {
    const ipc = await import("../lib/ipc");
    vi.mocked(ipc.createChat).mockClear();
    useApp.setState({ activeChatId: 1, detail, error: null });
    await useApp.getState().setChatModel("local:/models/b.gguf");
    expect(ipc.setChatModel).toHaveBeenCalledWith(1, "local:/models/b.gguf");
    expect(ipc.createChat).not.toHaveBeenCalled();
  });
});

// [0.0.5 memory] When a reply has to make room, the chat says what was done, in plain words.
describe("making room in a long chat", () => {
  beforeEach(async () => {
    useApp.setState({ activeChatId: 1, detail, generation: null, turnNotes: null, error: null, approvals: [] });
    void useApp.getState().send("hi", null);
    await flush();
  });

  it("names each stage once, and keeps the notes after the reply", async () => {
    const { compactionNote } = await import("./app");
    emit({ type: "compacted", stage: 1 });
    emit({ type: "compacted", stage: 2 });
    // A stage that runs again (several summaries, several steps shortened) is said once.
    emit({ type: "compacted", stage: 2 });
    emit({ type: "compacted", stage: 3 });
    emit({ type: "compacted", stage: 3 });
    const notes = [
      "Older tool results were shortened to make room.",
      "Earlier messages were summarised to make room.",
      "Older steps of this reply were shortened to make room.",
    ];
    expect(useApp.getState().generation!.notes).toEqual(notes);
    expect([1, 2, 3].map(compactionNote)).toEqual(notes);
    // A stage this version does not know still says something true.
    expect(compactionNote(9)).toBe("Earlier parts of this chat were shortened to make room.");
    emit({ type: "finished", status: "complete", message: null });
    await flush();
    expect(useApp.getState().turnNotes).toEqual({ chatId: 1, notes });
    finishSend();
  });

  it("replaces the note shown while summarising with what happened", async () => {
    const summarising = "Summarising earlier messages to make room.";
    const done = "Earlier messages were summarised to make room.";
    const failed = "Could not summarise earlier messages. The reply goes on, but this chat may run out of room soon.";
    emit({ type: "note", text: summarising });
    expect(useApp.getState().generation!.notes).toEqual([summarising]);
    emit({ type: "compacted", stage: 2 });
    expect(useApp.getState().generation!.notes).toEqual([done]);
    // A second summary in the same reply: shown while it is written, then said once.
    emit({ type: "note", text: summarising });
    expect(useApp.getState().generation!.notes).toEqual([done, summarising]);
    emit({ type: "compacted", stage: 2 });
    expect(useApp.getState().generation!.notes).toEqual([done]);
    // One that fails takes the place of its "Summarising" note too.
    emit({ type: "note", text: summarising });
    emit({ type: "note", text: failed });
    expect(useApp.getState().generation!.notes).toEqual([done, failed]);
    // Other notes and other stages leave it alone.
    emit({ type: "note", text: summarising });
    emit({ type: "compacted", stage: 1 });
    emit({ type: "note", text: "Not offered to the model: x." });
    expect(useApp.getState().generation!.notes).toEqual([
      done,
      failed,
      summarising,
      "Older tool results were shortened to make room.",
      "Not offered to the model: x.",
    ]);
    emit({ type: "finished", status: "complete", message: null });
    await flush();
    finishSend();
  });

  it("shows a summary that failed as the loop words it, and the reply goes on", async () => {
    const failed = "Could not summarise earlier messages. The reply goes on, but this chat may run out of room soon.";
    emit({ type: "note", text: failed });
    emit({ type: "content", text: "Here is the answer." });
    const g = useApp.getState().generation!;
    expect(g.notes).toEqual([failed]);
    expect(g.text).toBe("Here is the answer.");
    emit({ type: "finished", status: "complete", message: null });
    await flush();
    expect(useApp.getState().turnNotes?.notes).toEqual([failed]);
    finishSend();
  });
});
