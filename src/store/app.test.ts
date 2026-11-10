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
}));

const { useApp } = await import("./app");

async function flush() {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

describe("streamed turn events", () => {
  beforeEach(async () => {
    useApp.setState({ activeChatId: 1, detail, generation: null, error: null, approvals: [] });
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
