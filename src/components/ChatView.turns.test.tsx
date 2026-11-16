// [0.0.7 P6] Turns across chats: why a turn stopped, and its Continue, stay with their chat; a
// reply running in another chat is named, with a way to it, instead of a Stop that would stop
// it; a tool call stored without a result shows as running only while its own turn runs.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ApprovalRequest, ChatDetail, ChatSummary, MessageDto, ToolCallDto, TurnStreamEvent } from "../lib/types";

const fixtures = vi.hoisted(() => ({
  details: new Map<number, unknown>(),
  /** What the backend's turn does for each message sent: the events it sends. */
  events: [] as { chatId: number; text: string; send: TurnStreamEvent[] }[],
}));

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  listChats: vi.fn(async () => [] as unknown[]),
  getChat: vi.fn(async (chatId: number) => fixtures.details.get(chatId)),
  sendMessage: vi.fn(async (opts: { chatId: number; text: string }, onEvent: (e: unknown) => void) => {
    const found = fixtures.events.find((e) => e.chatId === opts.chatId && e.text === opts.text);
    for (const e of found?.send ?? [{ type: "finished", status: "complete", message: null }]) onEvent(e);
  }),
  stopGeneration: vi.fn<(chatId?: number | null) => Promise<void>>(async () => undefined),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./ModelPicker", () => ({ ModelPicker: () => null }));
vi.mock("./RuntimeIndicator", () => ({ RuntimeIndicator: () => null }));
vi.mock("./ToolsPanel", () => ({ ToolsPanel: () => null }));
vi.mock("./AllowAllButton", () => ({ ApprovalModeMenu: () => null }));
vi.mock("./SystemPromptDialog", () => ({ SystemPromptDialog: () => null }));
vi.mock("./UsageLine", () => ({ UsageLine: () => null }));
vi.mock("./FolderChip", () => ({ FolderChip: () => null }));
vi.mock("./MessageItem", () => ({
  MessageItem: (p: {
    message: MessageDto;
    onEditResend: (m: MessageDto, text: string, attachmentIds: number[]) => void;
  }) => (
    <div data-message={p.message.id}>
      {p.message.text}
      {p.message.role === "user" && (
        <>
          <button type="button" onClick={() => p.onEditResend(p.message, "edited", [5, 7])}>
            Resend {p.message.id} with its attachments
          </button>
          <button type="button" onClick={() => p.onEditResend(p.message, "edited", [])}>
            Resend {p.message.id} without attachments
          </button>
        </>
      )}
    </div>
  ),
}));
// The message box: what the chat tells it (Stop, or why it cannot send).
vi.mock("./Composer", () => ({
  Composer: (p: { busy: boolean; disabledReason: string | null; onStop: () => void }) => (
    <div data-testid="composer" data-disabled={p.disabledReason ?? ""}>
      {p.busy && <button type="button" aria-label="Stop" onClick={p.onStop} />}
    </div>
  ),
}));

// jsdom does not scroll.
Element.prototype.scrollTo = () => undefined;

const { useApp, CONTINUE_MESSAGE } = await import("../store/app");
const { ChatView } = await import("./ChatView");

const STEP_LIMIT = "The model reached the step limit for one turn.";

function message(id: number, role: "user" | "assistant", text: string, toolCalls: ToolCallDto[] = []): MessageDto {
  return {
    id,
    parentId: id > 1 ? id - 1 : null,
    role,
    text,
    reasoning: null,
    status: "complete",
    modelRef: null,
    promptTokens: null,
    completionTokens: null,
    tokensPerSecond: null,
    error: null,
    createdAt: 0,
    siblingIds: [id],
    imageIds: [],
    documents: [],
    toolCalls,
    toolResult: null,
  };
}

function chat(id: number, title: string, messages: MessageDto[] = []): ChatDetail {
  return {
    chat: {
      id,
      title,
      modelRef: "remote:1:m",
      systemPrompt: null,
      leafMessageId: null,
      createdAt: 0,
      updatedAt: 0,
      thinking: true,
      workingFolder: null,
    },
    messages,
    summary: null,
  };
}

function summary(id: number, title: string): ChatSummary {
  return { id, title, updatedAt: 0, snippet: null } as ChatSummary;
}

function generation(chatId: number) {
  return {
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
  };
}

function approval(chatId: number): ApprovalRequest {
  return {
    id: "card-1",
    chatId,
    callId: "c9",
    server: "terminal",
    tool: "run_command",
    summary: "Run a command",
    preview: "rm -rf build",
    card: "grantable",
    reason: "normal",
  } as ApprovalRequest;
}

const INSTALL = chat(1, "Install the toolchain");
const OTHER = chat(2, "Holiday plans");

function start(open: ChatDetail, patch: Partial<ReturnType<typeof useApp.getState>> = {}) {
  fixtures.details.set(1, INSTALL);
  fixtures.details.set(2, OTHER);
  useApp.setState({
    chats: [summary(1, INSTALL.chat.title), summary(2, OTHER.chat.title)],
    activeChatId: open.chat.id,
    detail: open,
    generation: null,
    runningTurn: null,
    turnNotes: null,
    stoppedTurns: {},
    error: null,
    approvals: [],
    ...patch,
  });
  return render(<ChatView />);
}

beforeEach(() => {
  vi.clearAllMocks();
  fixtures.events = [];
});
afterEach(cleanup);

describe("a turn that stopped before it was done", () => {
  it("keeps the step-limit notice and its Continue in its own chat, and Continue goes there", async () => {
    fixtures.events.push({
      chatId: 1,
      text: "Install the toolchain",
      send: [{ type: "finished", status: "stepCap", message: STEP_LIMIT }],
    });
    start(INSTALL);
    await act(async () => {
      await useApp.getState().send("Install the toolchain");
    });
    expect(screen.getByRole("status").textContent).toContain(STEP_LIMIT);
    expect(screen.getByRole("button", { name: "Continue" })).toBeTruthy();

    // Another chat is opened (the sidebar's openChat): the notice is not its.
    await act(async () => {
      await useApp.getState().openChat(2);
    });
    expect(screen.queryByText(STEP_LIMIT)).toBeNull();
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();

    // Back in the first chat it is still there, and Continue goes to that chat.
    await act(async () => {
      await useApp.getState().openChat(1);
    });
    expect(screen.getByText(STEP_LIMIT)).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    });
    const last = ipcMock.sendMessage.mock.calls.at(-1)![0];
    expect(last.chatId).toBe(1);
    expect(last.text).toBe(CONTINUE_MESSAGE);
    expect(CONTINUE_MESSAGE).toBe("Continue where you left off.");
    expect(screen.queryByText(STEP_LIMIT)).toBeNull();
  });

  it("keeps the notices about a repeated call and about invalid calls with their chat too", async () => {
    for (const [status, words] of [
      ["loop", "The model kept repeating the same tool call."],
      ["mistakes", "The model made too many invalid tool calls in a row."],
    ] as const) {
      fixtures.events = [{ chatId: 1, text: "go", send: [{ type: "finished", status, message: words }] }];
      start(INSTALL);
      await act(async () => {
        await useApp.getState().send("go");
      });
      expect(screen.getByRole("alert").textContent).toContain(words);
      await act(async () => {
        await useApp.getState().openChat(2);
      });
      expect(screen.queryByText(words)).toBeNull();
      expect(useApp.getState().error).toBeNull();
      await act(async () => {
        await useApp.getState().openChat(1);
      });
      expect(screen.getByText(words)).toBeTruthy();
      cleanup();
    }
  });

  it("never sends Continue to a chat that is not open, and a message in another chat leaves the notice", async () => {
    const notice = { code: "stepCap", message: STEP_LIMIT, nextStep: "Choose Continue.", details: "" };
    start(OTHER, { stoppedTurns: { 1: notice } });
    await act(async () => {
      await useApp.getState().continueTurn(1);
    });
    expect(ipcMock.sendMessage).not.toHaveBeenCalled();
    await act(async () => {
      await useApp.getState().send("Where shall we go?");
    });
    expect(ipcMock.sendMessage.mock.calls[0]![0].chatId).toBe(2);
    expect(useApp.getState().stoppedTurns[1]).toEqual(notice);
    // Dismiss forgets it.
    act(() => useApp.getState().dismissStoppedTurn(1));
    expect(useApp.getState().stoppedTurns).toEqual({});
  });
});

describe("a reply running in another chat", () => {
  it("is named, with a way to it, instead of a Stop that would stop it", async () => {
    start(OTHER, { generation: generation(1) });
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    const line = document.querySelector("[data-turn-elsewhere]") as HTMLElement;
    expect(line.textContent).toBe("A reply is running in “Install the toolchain”. Open it");
    // The message box cannot send while it runs (the app writes one reply at a time).
    expect(screen.getByTestId("composer").getAttribute("data-disabled")).not.toBe("");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Open it" }));
    });
    expect(useApp.getState().activeChatId).toBe(1);
    // In its own chat: Stop, which stops that chat's reply only.
    expect(document.querySelector("[data-turn-elsewhere]")).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    });
    expect(ipcMock.stopGeneration).toHaveBeenCalledWith(1);
  });

  it("says when that reply waits for an approval", () => {
    start(OTHER, { generation: generation(1), approvals: [approval(1)] });
    const line = document.querySelector("[data-turn-elsewhere]") as HTMLElement;
    expect(line.textContent).toBe("A reply in “Install the toolchain” is waiting for your approval. Open it");
    // The card itself is drawn in its own chat only.
    expect(screen.queryByText("Run a command")).toBeNull();
  });

  it("counts a reply this window did not start (one that went on while the window reloaded)", async () => {
    start(OTHER, { runningTurn: 1 });
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    expect(document.querySelector("[data-turn-elsewhere]")?.textContent).toContain("Install the toolchain");
    await act(async () => {
      await useApp.getState().openChat(1);
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    });
    expect(ipcMock.stopGeneration).toHaveBeenCalledWith(1);
    // When it ends, its chat is read again: its messages were stored without events here.
    ipcMock.getChat.mockClear();
    await act(async () => {
      useApp.getState().setRunningTurn(null);
    });
    expect(ipcMock.getChat).toHaveBeenCalledWith(1);
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
  });

  it("names a chat it cannot find in the list as another chat", () => {
    start(OTHER, { generation: generation(9) });
    expect(document.querySelector("[data-turn-elsewhere]")?.textContent).toBe("A reply is running in another chat. Open it");
  });
});

describe("a tool call stored without a result (the app closed while it ran)", () => {
  const call: ToolCallDto = { id: "c1", name: "run_command", arguments: { command: "make" }, recovered: false };

  function card(): string {
    return (screen.getByText("run_command").closest("button") as HTMLElement).textContent ?? "";
  }

  it("shows as cancelled while a reply runs in another chat", () => {
    const old = chat(2, "Holiday plans", [message(1, "user", "build it"), message(2, "assistant", "", [call])]);
    start(old, { generation: generation(1) });
    expect(card()).toContain("Cancelled");
    expect(card()).not.toContain("Running");
  });

  it("shows as cancelled when it is older than this chat's running turn", () => {
    const old = chat(2, "Holiday plans", [
      message(1, "user", "build it"),
      message(2, "assistant", "", [call]),
      message(3, "user", "try again"),
    ]);
    start(old, { generation: generation(2) });
    expect(card()).toContain("Cancelled");
  });

  it("shows as running while it belongs to its chat's running turn", () => {
    const now = chat(2, "Holiday plans", [message(1, "user", "build it"), message(2, "assistant", "", [call])]);
    start(now, { generation: generation(2) });
    expect(card()).toContain("Running");
    cleanup();
    // Also for a reply this window did not start.
    start(now, { runningTurn: 2 });
    expect(card()).toContain("Running");
  });
});

// [0.0.7 P6] Edit and resend: the attachments left in the edit box go with the new message.
describe("edit and resend", () => {
  it("sends the new text on the edited message's branch with the attachments it keeps", async () => {
    const asked = chat(2, "Holiday plans", [
      message(1, "user", "hello"),
      message(2, "assistant", "hi"),
      message(3, "user", "What's wrong here?"),
    ]);
    start(asked);
    // The chat as it is read again after each reply.
    fixtures.details.set(2, asked);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Resend 3 with its attachments" }));
    });
    expect(ipcMock.sendMessage.mock.calls.at(-1)![0]).toMatchObject({
      chatId: 2,
      text: "edited",
      parentId: 2,
      copyAttachments: { messageId: 3, attachmentIds: [5, 7] },
    });
    // With every attachment removed, nothing is copied.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Resend 3 without attachments" }));
    });
    const plain = ipcMock.sendMessage.mock.calls.at(-1)![0] as { copyAttachments?: unknown };
    expect(plain.copyAttachments).toBeUndefined();
  });
});
