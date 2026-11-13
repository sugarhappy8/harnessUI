import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ChatDetail, ErrorInfo, MessageDto, RuntimeStatus } from "../lib/types";

// The header's own components talk to the backend; the chat's layout is what is tested here.
vi.mock("../lib/ipc", () => ({ isTauri: () => false }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./ModelPicker", () => ({ ModelPicker: () => null }));
vi.mock("./RuntimeIndicator", () => ({ RuntimeIndicator: () => null }));
vi.mock("./ToolsPanel", () => ({ ToolsPanel: () => null }));
vi.mock("./AllowAllButton", () => ({ ApprovalModeMenu: () => null }));
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
    // [0.0.6] Pinned instructions that do not fit get more room with a larger memory too.
    for (const code of ["context_full", "context_overflow", "tools_do_not_fit", "pinned_do_not_fit"]) {
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
    // [0.0.6] Fewer words than the rounding shows: never "about 0".
    expect(contextMeterTitle(120, 8_192)).toBe(
      "How much it can remember: 120 of 8,192 tokens used (fewer than 100 of about 6,000 words)",
    );
    expect(contextMeterTitle(134, 4_096)).toBe(
      "How much it can remember: 134 of 4,096 tokens used (about 100 of 3,000 words)",
    );
    expect(contextMeterTitle(40, null)).toBe("How much it can remember: 40 tokens used");
    expect(contextMeterTitle(2_000, null)).toBe("How much it can remember: 2,000 tokens used (about 1,000 words)");
    render(<ContextMeter used={3_000} nCtx={4_096} />);
    expect(document.querySelector("[title^='How much it can remember: 3,000 of 4,096 tokens used']")).toBeTruthy();
  });
});

describe("the header (header-layout)", () => {
  afterEach(() => {
    cleanup();
    useApp.setState({ runtime: null });
  });

  it("keeps the model and its lines in one group and the chat's buttons in another that wraps under it", () => {
    useApp.setState({ runtime: { state: "ready", nCtx: 4_096, thinking: true, supportsTools: true } as RuntimeStatus });
    const answered: MessageDto = { ...message(2, "assistant", "ok"), promptTokens: 50, completionTokens: 10 };
    show(detail("local:/models/a.gguf", null, [message(1, "user", "hi"), answered]));
    const header = document.querySelector("main header") as HTMLElement;
    // jsdom has no layout: the classes are checked here, the real widths (720 to 1,440 pixels)
    // in the e2e scenario header-layout.
    expect(header.className.split(" ")).toContain("flex-wrap");
    const model = header.querySelector("[data-header-model]") as HTMLElement;
    const controls = header.querySelector("[data-header-controls]") as HTMLElement;
    expect([...header.children]).toEqual([model, controls]);
    // The model's group asks for its parts at their shortest and takes the rest of the row; the
    // buttons keep their width, right-aligned, and wrap among themselves only when a row of
    // their own is too narrow.
    for (const c of ["w-min", "grow", "min-w-0"]) expect(model.className.split(" ")).toContain(c);
    for (const c of ["ml-auto", "flex-wrap", "max-w-full", "justify-end"]) {
      expect(controls.className.split(" ")).toContain(c);
    }
    // The meter and Thinking go with the model; the folder chip, System prompt and Panel with
    // the buttons.
    expect(model.contains(document.querySelector("[title^='How much it can remember']"))).toBe(true);
    expect(model.contains(screen.getByRole("button", { name: "Thinking on" }))).toBe(true);
    expect(controls.querySelector("[data-folder-chip]")).toBeTruthy();
    expect(controls.contains(screen.getByRole("button", { name: "System prompt" }))).toBe(true);
    expect(controls.contains(screen.getByRole("button", { name: "Panel" }))).toBe(true);
    // [r2] System prompt says its name from @5xl (a window from about 1,300 pixels); below, the
    // icon, with the name as its accessible name and tooltip.
    const prompt = screen.getByRole("button", { name: "System prompt" }).querySelector("span") as HTMLElement;
    expect(prompt.className).toBe("hidden @5xl:inline");
  });

  it("says whether Thinking is on in words at every width, never by its colour alone (header-layout r2)", () => {
    const answered: MessageDto = { ...message(2, "assistant", "ok"), promptTokens: 50, completionTokens: 10 };
    for (const on of [true, false]) {
      for (const messages of [[], [message(1, "user", "hi"), answered]]) {
        useApp.setState({ runtime: { state: "ready", nCtx: 4_096, thinking: true, supportsTools: true } as RuntimeStatus });
        const d = detail("local:/models/a.gguf", null, messages);
        show({ ...d, chat: { ...d.chat, thinking: on } });
        const button = screen.getByRole("button", { name: on ? "Thinking on" : "Thinking off" });
        expect(button.getAttribute("aria-pressed")).toBe(String(on));
        // "On" or "Off" beside the icon in every header (no class hides it below a width), and
        // "Thinking on" or "Thinking off" in a wide one; one of the two shows at any width.
        // Beside the meter (a chat with messages) the long form waits for a wider header.
        const meter = document.querySelector("[title^='How much it can remember']");
        expect(!!meter).toBe(messages.length > 0);
        const wide = meter ? "@7xl" : "@5xl";
        const short = button.querySelector("[data-thinking='short']") as HTMLElement;
        const full = button.querySelector("[data-thinking='full']") as HTMLElement;
        expect(short.textContent).toBe(on ? "On" : "Off");
        expect(short.className).toBe(`${wide}:hidden`);
        expect(full.textContent).toBe(on ? "Thinking on" : "Thinking off");
        expect(full.className).toBe(`hidden ${wide}:inline`);
        cleanup();
      }
    }
  });

  it("places the model's parts by the header's width only, so nothing moves while a reply streams (header-layout r2)", () => {
    useApp.setState({ runtime: { state: "ready", nCtx: 4_096, thinking: true, supportsTools: true } as RuntimeStatus });
    const answered: MessageDto = { ...message(2, "assistant", "ok"), promptTokens: 50, completionTokens: 10 };
    show(detail("local:/models/a.gguf", null, [message(1, "user", "hi"), answered]));
    act(() =>
      useApp.setState({
        generation: {
          chatId: 1,
          text: "",
          reasoning: "",
          tools: [],
          notes: [],
          step: 0,
          tokensPerSecond: 12.5,
          promptTokens: 60,
          completionTokens: 5,
          contextUsed: null,
          nCtx: 4_096,
        },
      }),
    );
    const header = document.querySelector("main header") as HTMLElement;
    const model = header.querySelector("[data-header-model]") as HTMLElement;
    const classes = (e: Element | null) => (e as HTMLElement).className.split(" ");
    // Below @2xl a column: the name's row, then the usage line's row with the meter, speed and
    // Thinking at its right. From @2xl a grid: the name's row (`auto`, the rest of the row) with
    // the meter, speed and Thinking (`min-content`) on the first row, the usage line under both.
    // No `fr` column and no part across both that widens one.
    for (const c of ["flex", "flex-col", "@2xl:grid", "@2xl:grid-cols-[auto_min-content]"]) {
      expect(classes(model)).toContain(c);
    }
    const line = model.querySelector("[data-model-line]") as HTMLElement;
    const below = model.querySelector("[data-model-below]") as HTMLElement;
    expect([...model.children]).toEqual([line, below]);
    const usage = below.firstElementChild as HTMLElement;
    const status = below.querySelector("[data-model-status]") as HTMLElement;
    expect([...below.children]).toEqual([usage, status]);
    // (The usage line keeps its row's height while its first reading is on its way.)
    expect(usage.querySelector("[data-usage]")).toBeTruthy();
    // The name and the model's state on the first row; under 22.25rem the state goes under the
    // name.
    for (const c of ["@2xl:col-start-1", "@2xl:row-start-1", "@max-[22.25rem]:flex-wrap"]) {
      expect(classes(line)).toContain(c);
    }
    // The usage line's row: a flex row below @2xl (the meter, speed and Thinking go under the
    // usage line only under @2xs), its parts the grid's own from @2xl.
    for (const c of ["flex", "@max-2xs:flex-wrap", "@2xl:contents"]) expect(classes(below)).toContain(c);
    // The usage line takes the row's room, and from @2xl spans both columns without widening one.
    for (const c of ["grow", "min-w-0", "@2xl:col-span-2", "@2xl:row-start-2", "@2xl:w-0", "@2xl:min-w-full"]) {
      expect(classes(usage)).toContain(c);
    }
    // The meter, speed and Thinking: right-aligned beside the usage line, on the name's row from
    // @2xl.
    for (const c of ["ml-auto", "shrink-0", "@2xl:col-start-2", "@2xl:row-start-1"]) expect(classes(status)).toContain(c);
    expect(status.querySelector("[title^='How much it can remember']")).toBeTruthy();
    expect(status.contains(screen.getByRole("button", { name: "Thinking on" }))).toBe(true);
    // The reply's speed, left out under 22.25rem (the reply shows it when it ends).
    const speed = status.querySelector("[data-speed]") as HTMLElement;
    expect(speed.textContent).toBe("12.5 tokens/s");
    expect(classes(speed)).toContain("hidden");
    expect(classes(speed)).toContain("@min-[22.25rem]:inline");
  });

  it("shows the meter's numbers by the header's width, every one of them in its tooltip", () => {
    render(<ContextMeter used={3_000} nCtx={4_096} />);
    const meter = document.querySelector("[title^='How much it can remember']") as HTMLElement;
    expect(meter.title).toBe("How much it can remember: 3,000 of 4,096 tokens used (about 2,000 of 3,000 words)");
    const full = meter.querySelector("[data-meter='full']") as HTMLElement;
    const short = meter.querySelector("[data-meter='short']") as HTMLElement;
    expect(full.textContent).toBe("3.0k / 4.1k tokens (73%)");
    expect(short.textContent).toBe("73%");
    // All numbers in a wide header, the share in a middle one, the bar alone in the narrowest.
    expect(full.className).toBe("hidden @6xl:inline");
    expect(short.className).toBe("hidden @md:inline @6xl:hidden");
  });
});

describe("the chat's end while a reply runs", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  // [0.0.6 final pass] A note under the reply (the server's prompt size, a summary) can arrive
  // after the approval card is drawn. At the chat's end the chat follows it, so the card's
  // buttons stay in view instead of half under the message box, where the first click on
  // Deny or Allow scrolled the card and missed (found by the end-to-end run).
  it("follows a note that arrives while it is at the end", () => {
    const scrolled = vi.spyOn(Element.prototype, "scrollTo");
    const generation = {
      chatId: 1,
      text: "",
      reasoning: "",
      tools: [],
      notes: [] as string[],
      step: 1,
      tokensPerSecond: null,
      promptTokens: null,
      completionTokens: null,
      contextUsed: null,
      nCtx: null,
    };
    useApp.setState({
      activeChatId: 1,
      detail: detail("remote:1:m", null, [message(1, "user", "delete")]),
      generation,
      turnNotes: null,
      error: null,
      approvals: [],
    });
    render(<ChatView />);
    scrolled.mockClear();
    act(() => useApp.setState({ generation: { ...generation, notes: ["The server reported a prompt of 50 tokens."] } }));
    expect(screen.getByText("The server reported a prompt of 50 tokens.")).toBeTruthy();
    expect(scrolled).toHaveBeenCalled();
  });
});
