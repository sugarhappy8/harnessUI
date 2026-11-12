import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ChatDetail, ErrorInfo, MessageDto } from "../lib/types";

// The header's own components talk to the backend; the chat's layout is what is tested here.
vi.mock("../lib/ipc", () => ({ isTauri: () => false }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./ModelPicker", () => ({ ModelPicker: () => null }));
vi.mock("./RuntimeIndicator", () => ({ RuntimeIndicator: () => null }));
vi.mock("./ToolsPanel", () => ({ ToolsPanel: () => null }));
vi.mock("./AllowAllButton", () => ({ AllowAllButton: () => null }));
vi.mock("./Composer", () => ({ Composer: () => null }));
vi.mock("./SystemPromptDialog", () => ({ SystemPromptDialog: () => null }));
vi.mock("./MessageItem", () => ({
  MessageItem: (p: { message: MessageDto }) => (
    <div data-message={p.message.id}>
      {p.message.text}
      {p.message.error && <p>{p.message.error.message}</p>}
    </div>
  ),
}));

// jsdom does not scroll.
Element.prototype.scrollTo = () => undefined;

const { useApp } = await import("../store/app");
const { ChatView, ContextMeter, ModelSettingsAction, SummaryDivider, contextMeterTitle, summaryDividerAfter } =
  await import("./ChatView");

function message(id: number, role: "user" | "assistant" | "tool", text: string, error: ErrorInfo | null = null): MessageDto {
  return {
    id,
    parentId: id > 1 ? id - 1 : null,
    role,
    text,
    reasoning: null,
    status: error ? "error" : "complete",
    modelRef: null,
    promptTokens: null,
    completionTokens: null,
    tokensPerSecond: null,
    error,
    createdAt: 0,
    siblingIds: [id],
    imageIds: [],
    documents: [],
    toolCalls: [],
    toolResult: null,
  };
}

const tooLong: ErrorInfo = {
  code: "context_full",
  message: "This chat is too long for the model to remember, even after summarising.",
  nextStep: "Start a new chat, or choose Model settings to let this model remember more.",
  details: "",
};

function detail(modelRef: string, summary: ChatDetail["summary"], messages: MessageDto[]): ChatDetail {
  return {
    chat: {
      id: 1,
      title: "t",
      modelRef,
      systemPrompt: null,
      leafMessageId: null,
      createdAt: 0,
      updatedAt: 0,
      thinking: true,
      workingFolder: null,
    },
    messages,
    summary,
  };
}

function show(d: ChatDetail) {
  useApp.setState({ activeChatId: 1, detail: d, generation: null, turnNotes: null, error: null, approvals: [] });
  return render(<ChatView />);
}

describe("the summary's line in the chat", () => {
  afterEach(cleanup);

  it("goes under the last shown message the summary stands for", () => {
    const shown = [message(1, "user", "a"), message(2, "assistant", "b"), message(5, "user", "c"), message(6, "assistant", "d")];
    // The summary ends at a tool result (4): its card is in the row of message 2.
    expect(summaryDividerAfter(shown, 4)).toBe(2);
    expect(summaryDividerAfter(shown, 5)).toBe(5);
    expect(summaryDividerAfter(shown, 100)).toBe(6);
    expect(summaryDividerAfter(shown, 0)).toBeNull();
  });

  it("says what it means and shows the summary as plain text on request", () => {
    render(<SummaryDivider text={"Goal: plan the trip.\n<b>Decisions</b>: <img src=x onerror=alert(1)>"} />);
    expect(screen.getByText(/Above this line, the model sees a summary instead of the messages\./)).toBeTruthy();
    const button = screen.getByRole("button", { name: "Show summary" });
    expect(button.getAttribute("aria-expanded")).toBe("false");
    expect(document.querySelector("[data-summary-text]")).toBeNull();
    fireEvent.click(button);
    const text = document.querySelector("[data-summary-text]")!;
    // Model output, never markup: the tags are text and nothing is drawn from them.
    expect(text.textContent).toBe("Goal: plan the trip.\n<b>Decisions</b>: <img src=x onerror=alert(1)>");
    expect(text.querySelector("b, img")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Hide summary" }));
    expect(document.querySelector("[data-summary-text]")).toBeNull();
  });

  it("is drawn once, in the chat, under the right message", () => {
    const messages = [
      message(1, "user", "first question"),
      message(2, "assistant", "first answer"),
      message(3, "user", "second question"),
      message(4, "assistant", "second answer"),
    ];
    show(detail("remote:1:m", { lastMessageId: 2, text: "Goal: answer two questions." }, messages));
    const lines = document.querySelectorAll("[data-summary-divider]");
    expect(lines).toHaveLength(1);
    const row = lines[0]!.closest(".message-row")!;
    expect(row.querySelector("[data-message]")!.getAttribute("data-message")).toBe("2");
    fireEvent.click(screen.getByRole("button", { name: "Show summary" }));
    expect(screen.getByText("Goal: answer two questions.")).toBeTruthy();
  });

  it("is not drawn for a chat without a summary", () => {
    show(detail("remote:1:m", null, [message(1, "user", "hi"), message(2, "assistant", "hello")]));
    expect(document.querySelector("[data-summary-divider]")).toBeNull();
    cleanup();
    // An older chat detail without the field.
    const old = detail("remote:1:m", null, [message(1, "user", "hi")]);
    delete old.summary;
    show(old);
    expect(document.querySelector("[data-summary-divider]")).toBeNull();
  });
});

describe("the Model settings button under a too-long error", () => {
  afterEach(cleanup);

  it("opens this chat's model settings when the model is local", () => {
    const opened = vi.fn();
    const listener = (e: Event) => opened((e as CustomEvent<string>).detail);
    window.addEventListener("app:open-model-settings", listener);
    show(detail("local:/models/small.gguf", null, [message(1, "user", "hi"), message(2, "assistant", "", tooLong)]));
    fireEvent.click(screen.getByRole("button", { name: "Model settings" }));
    expect(opened).toHaveBeenCalledWith("/models/small.gguf");
    window.removeEventListener("app:open-model-settings", listener);
  });

  it("is offered for every error about the chat no longer fitting, and only those", () => {
    for (const code of ["context_full", "context_overflow", "tools_do_not_fit"]) {
      show(detail("local:/m.gguf", null, [message(1, "user", "hi"), message(2, "assistant", "", { ...tooLong, code })]));
      expect(screen.getAllByRole("button", { name: "Model settings" })).toHaveLength(1);
      cleanup();
    }
    show(detail("local:/m.gguf", null, [message(1, "user", "hi"), message(2, "assistant", "", { ...tooLong, code: "network" })]));
    expect(screen.queryByRole("button", { name: "Model settings" })).toBeNull();
  });

  it("is not offered for a model on an endpoint, whose size the app does not choose", () => {
    show(detail("remote:1:m", null, [message(1, "user", "hi"), message(2, "assistant", "", tooLong)]));
    expect(screen.queryByRole("button", { name: "Model settings" })).toBeNull();
  });

  it("names the model it opens", () => {
    const opened = vi.fn();
    const listener = (e: Event) => opened((e as CustomEvent<string>).detail);
    window.addEventListener("app:open-model-settings", listener);
    render(<ModelSettingsAction path={"C:\\models\\a b.gguf"} />);
    fireEvent.click(screen.getByRole("button", { name: "Model settings" }));
    expect(opened).toHaveBeenCalledWith("C:\\models\\a b.gguf");
    window.removeEventListener("app:open-model-settings", listener);
  });
});

describe("the meter", () => {
  afterEach(cleanup);

  it("says how much it can remember, in tokens and words", () => {
    expect(contextMeterTitle(1_200, 8_192)).toBe(
      "How much it can remember: 1,200 of 8,192 tokens used (about 900 of 6,000 words)",
    );
    expect(contextMeterTitle(40, null)).toBe("How much it can remember: 40 tokens used");
    expect(contextMeterTitle(2_000, null)).toBe("How much it can remember: 2,000 tokens used (about 1,000 words)");
    render(<ContextMeter used={3_000} nCtx={4_096} />);
    expect(document.querySelector("[title^='How much it can remember: 3,000 of 4,096 tokens used']")).toBeTruthy();
  });
});
