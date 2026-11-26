// [0.0.7 P6] Edit and resend dropped the message's images, files and mentions without a word,
// and a message with only an image could not be resent. The edit box shows them now, each
// with a way to remove it, and the new message keeps the rest.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { MentionRef, MessageDto } from "../lib/types";
import { LOCAL, MODELS } from "./studio/fixtures/models";
import type { MediaModel, RefItem } from "./studio/types";

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  getAttachment: vi.fn(async (id: number) => `data:image/png;base64,${id}`),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("./context/PinMessageButton", () => ({ PinMessageButton: () => null }));
vi.mock("./studio/api");

const { MessageItem } = await import("./MessageItem");
const studioApi = await import("./studio/api");
const { useStudio } = await import("../store/studio");

const mention: MentionRef = {
  attachmentId: 7,
  label: "calc.py:add",
  kind: "function",
  path: "/work/calc.py",
  symbol: "add",
  startLine: 4,
  endLine: 5,
  truncated: false,
};

function user(over: Partial<MessageDto> = {}): MessageDto {
  return {
    id: 12,
    parentId: 11,
    role: "user",
    text: "What's wrong here?",
    reasoning: null,
    status: "complete",
    modelRef: null,
    promptTokens: null,
    completionTokens: null,
    tokensPerSecond: null,
    error: null,
    createdAt: 0,
    siblingIds: [12],
    imageIds: [5],
    documents: [{ id: 6, name: "notes.txt" }],
    toolCalls: [],
    toolResult: null,
    mentions: [mention],
    ...over,
  };
}

async function edit(m: MessageDto, onEditResend = vi.fn()) {
  await act(async () => {
    render(
      <MessageItem
        message={m}
        isLastAssistant={false}
        busy={false}
        onRegenerate={() => undefined}
        onEditResend={onEditResend}
        onSwitchBranch={() => undefined}
      />,
    );
  });
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Edit and resend" }));
  });
  return onEditResend;
}

describe("edit and resend (0.0.7 P6)", () => {
  afterEach(cleanup);

  it("shows the message's image, file and mention in the edit box, and resends them with the new text", async () => {
    const onEditResend = await edit(user());
    const form = screen.getByRole("textbox", { name: "Edit message" }).closest("form") as HTMLElement;
    expect(form.querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,5");
    expect(form.textContent).toContain("notes.txt");
    expect(form.textContent).toContain("calc.py:add");
    fireEvent.change(screen.getByRole("textbox", { name: "Edit message" }), {
      target: { value: "What is wrong in this picture?" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onEditResend).toHaveBeenCalledWith(expect.objectContaining({ id: 12 }), "What is wrong in this picture?", [
      5, 6, 7,
    ]);
  });

  it("lets each attachment be removed before sending", async () => {
    const onEditResend = await edit(user());
    fireEvent.click(screen.getByRole("button", { name: "Remove notes.txt" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove mention calc.py:add" }));
    const form = screen.getByRole("textbox", { name: "Edit message" }).closest("form") as HTMLElement;
    expect(form.textContent).not.toContain("notes.txt");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onEditResend).toHaveBeenCalledWith(expect.objectContaining({ id: 12 }), "What's wrong here?", [5]);
  });

  it("resends a message that has only an image, and needs text once nothing is left", async () => {
    const onEditResend = await edit(user({ text: "", documents: [], mentions: [] }));
    const send = screen.getByRole("button", { name: "Send" }) as HTMLButtonElement;
    expect(send.disabled).toBe(false);
    fireEvent.click(send);
    expect(onEditResend).toHaveBeenCalledWith(expect.objectContaining({ id: 12 }), "", [5]);
    cleanup();
    const nothing = await edit(user({ text: "", documents: [], mentions: [] }));
    fireEvent.click(screen.getByRole("button", { name: "Remove image 1" }));
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.submit(screen.getByRole("textbox", { name: "Edit message" }).closest("form")!);
    expect(nothing).not.toHaveBeenCalled();
  });

  it("starts again from all of the message's attachments each time it is edited", async () => {
    await edit(user());
    fireEvent.click(screen.getByRole("button", { name: "Remove notes.txt" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Edit and resend" }));
    });
    const form = screen.getByRole("textbox", { name: "Edit message" }).closest("form") as HTMLElement;
    expect(form.textContent).toContain("notes.txt");
  });
});

// [0.0.7 P6a review] An edit box opened before a reply started stayed open with Send on. The app
// refused that send as busy, and on the way the running reply's text and tool cards vanished.
describe("the edit box while a reply is being written", () => {
  afterEach(cleanup);

  it("can't send until the reply is done, and keeps what was written", async () => {
    const onEditResend = vi.fn();
    const props = {
      message: user(),
      isLastAssistant: false,
      onRegenerate: () => undefined,
      onEditResend,
      onSwitchBranch: () => undefined,
    };
    let view!: ReturnType<typeof render>;
    await act(async () => {
      view = render(<MessageItem {...props} busy={false} />);
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Edit and resend" }));
    });
    fireEvent.change(screen.getByRole("textbox", { name: "Edit message" }), { target: { value: "Look again" } });
    // A reply starts (sent from the message box) while the edit box is open.
    await act(async () => {
      view.rerender(<MessageItem {...props} busy />);
    });
    const send = screen.getByRole("button", { name: "Send" }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    fireEvent.click(send);
    fireEvent.submit(send.closest("form")!);
    expect(onEditResend).not.toHaveBeenCalled();
    // Still open with the new text, to send when the reply is done.
    expect((screen.getByRole("textbox", { name: "Edit message" }) as HTMLTextAreaElement).value).toBe("Look again");
    await act(async () => {
      view.rerender(<MessageItem {...props} busy={false} />);
    });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onEditResend).toHaveBeenCalledTimes(1);
    expect(onEditResend).toHaveBeenCalledWith(expect.objectContaining({ id: 12 }), "Look again", [5, 6, 7]);
  });
});

// [0.1.0 B11] *Open in Studio* on a picture attached in a chat: the picture is kept as an added
// picture (`studio_add_ref`, the same checks as a picture dropped on Studio's tray) and goes
// into the Pictures tab's tray as a reference, and Studio is shown there.
describe("Open in Studio", () => {
  const INITIAL = useStudio.getState();
  const KEPT: RefItem = { sha256: "d".repeat(64), mime: "image/png", sizeBytes: 1200, role: null };
  /** The fixture's first picture model, which takes up to 16 references. */
  const flare = MODELS[0]!;
  /** A picture model that takes no pictures. */
  const noRefs: MediaModel = {
    ...flare,
    id: "test/no-refs",
    params: flare.params.filter((p) => p.kind.type !== "images"),
  };

  /** Studio read once, its Pictures tab on `model` at OpenRouter. */
  function studioOn(model: MediaModel | null) {
    const draft = useStudio.getState().drafts.image;
    useStudio.setState({
      loaded: true,
      endpoints: [
        {
          providerId: 1,
          name: "OpenRouter",
          company: "openrouter",
          host: "openrouter.ai",
          cloud: true,
          hasKey: true,
          fetchedAt: 1,
          models: model ? [model] : [],
        },
      ],
      drafts: {
        ...useStudio.getState().drafts,
        image: { ...draft, target: model ? { endpoint: 1 } : null, model: model?.id ?? null },
      },
    });
  }

  async function shown(m: MessageDto) {
    await act(async () => {
      render(
        <MessageItem
          message={m}
          isLastAssistant={false}
          busy={false}
          onRegenerate={() => undefined}
          onEditResend={() => undefined}
          onSwitchBranch={() => undefined}
        />,
      );
    });
  }

  async function click(el: HTMLElement) {
    await act(async () => {
      fireEvent.click(el);
    });
    await act(async () => {
      for (let i = 0; i < 10; i++) await Promise.resolve();
    });
  }

  beforeEach(() => {
    vi.clearAllMocks();
    useStudio.setState(INITIAL, true);
    vi.mocked(studioApi.studioAddRef).mockResolvedValue(KEPT);
  });
  afterEach(cleanup);

  it("Open in Studio puts an attached picture in the tray", async () => {
    studioOn(flare);
    // Get local models was left open in Studio: the picture's tray is shown, not that view.
    useStudio.setState({ localView: true });
    ipcMock.getAttachment.mockResolvedValueOnce("data:image/webp;base64,UklGRg==");
    await shown(user({ documents: [], mentions: [] }));
    await click(screen.getByRole("button", { name: "Open in Studio" }));
    expect(studioApi.studioAddRef).toHaveBeenCalledWith({
      mime: "image/webp",
      fileName: "This picture",
      data: "UklGRg==",
    });
    expect(useStudio.getState().drafts.image.refs).toEqual([{ ...KEPT, role: "reference" }]);
    expect(useStudio.getState()).toMatchObject({ view: "studio", tab: "image", localView: false });
  });

  it("Open in Studio puts the cursor in Studio's prompt (the button is hidden now)", async () => {
    studioOn(flare);
    await shown(user({ documents: [], mentions: [] }));
    const frame = () => act(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
    await frame();
    const focused = vi.fn();
    window.addEventListener("app:focus-studio-prompt", focused);
    try {
      await click(screen.getByRole("button", { name: "Open in Studio" }));
      await frame();
      expect(focused).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("app:focus-studio-prompt", focused);
    }
  });

  it("with several pictures, each says which it is", async () => {
    studioOn(flare);
    await shown(user({ imageIds: [5, 8], documents: [], mentions: [] }));
    await click(screen.getByRole("button", { name: "Open picture 2 in Studio" }));
    expect(studioApi.studioAddRef).toHaveBeenCalledWith({ mime: "image/png", fileName: "This picture", data: "8" });
    screen.getByRole("button", { name: "Open picture 1 in Studio" });
  });

  it("is not offered in the edit box", async () => {
    await edit(user());
    expect(screen.queryByRole("button", { name: /in Studio/ })).toBeNull();
  });

  it("a model that takes no pictures keeps it out, says why, and stays in the chat", async () => {
    studioOn(noRefs);
    await shown(user({ documents: [], mentions: [] }));
    await click(screen.getByRole("button", { name: "Open in Studio" }));
    screen.getByText("Not added to Studio. This model takes no added pictures.");
    expect(studioApi.studioAddRef).not.toHaveBeenCalled();
    expect(useStudio.getState().view).toBe("chat");
    expect(useStudio.getState().drafts.image.refs).toEqual([]);
  });

  it("with no picture model in Studio yet, says so", async () => {
    studioOn(null);
    await shown(user({ documents: [], mentions: [] }));
    await click(screen.getByRole("button", { name: "Open in Studio" }));
    screen.getByText("Not added to Studio. There is no picture model yet: open Studio to get one.");
    expect(useStudio.getState().view).toBe("chat");
  });

  it("a picture the backend refuses says why and stays in the chat", async () => {
    studioOn(flare);
    vi.mocked(studioApi.studioAddRef).mockRejectedValue({
      code: "bad_request",
      message: "This picture is larger than 20 MB.",
      nextStep: "",
      details: "",
    });
    await shown(user({ documents: [], mentions: [] }));
    await click(screen.getByRole("button", { name: "Open in Studio" }));
    screen.getByText("Not added to Studio. This picture is larger than 20 MB.");
    expect(useStudio.getState().view).toBe("chat");
  });

  it("a refusal with a next step says what to do", async () => {
    studioOn(flare);
    vi.mocked(studioApi.studioAddRef).mockRejectedValue({
      code: "studio_disk_full",
      message: "The disk is full.",
      nextStep: "Free up some space, then try again.",
      details: "",
    });
    await shown(user({ documents: [], mentions: [] }));
    await click(screen.getByRole("button", { name: "Open in Studio" }));
    screen.getByText("Not added to Studio. The disk is full.");
    screen.getByText("Free up some space, then try again.");
  });

  it("reads what Studio has saved first when Studio was never opened, and sends nothing", async () => {
    vi.mocked(studioApi.studioModels).mockResolvedValue({
      endpoints: [
        {
          providerId: 1,
          name: "OpenRouter",
          company: "openrouter",
          host: "openrouter.ai",
          cloud: true,
          hasKey: true,
          fetchedAt: 1,
          models: [flare],
        },
      ],
    });
    vi.mocked(studioApi.studioLocalModels).mockResolvedValue({ ...LOCAL, installed: [] });
    vi.mocked(studioApi.studioRuns).mockResolvedValue({ runs: [], next: null });
    vi.mocked(studioApi.studioLive).mockResolvedValue([]);
    vi.mocked(studioApi.studioGetPrefs).mockResolvedValue({
      lastModel: { image: { target: { endpoint: 1 }, model: flare.id } },
      advancedOpen: false,
      settingsOpen: false,
      prefsVersion: 1,
      confirmEveryVideo: true,
      confirmUnknownPrice: false,
      confirmAtUsd: 1,
      dailyLimitUsd: 10,
      setups: [],
      galleryDir: null,
      gpuPolicy: "unload",
      reloadChatAfter: false,
      suggestionDismissed: true,
      localNoticeAcceptedAt: null,
      lastImportMode: null,
    });
    vi.mocked(studioApi.studioUsage).mockResolvedValue({
      spentTodayUsd: 0,
      pendingUsd: 0,
      limitUsd: 10,
      folder: "/home/me/harnessUI/studio",
      bytes: 0,
      pictures: 0,
      videos: 0,
    });
    await shown(user({ documents: [], mentions: [] }));
    await click(screen.getByRole("button", { name: "Open in Studio" }));
    expect(useStudio.getState().loaded).toBe(true);
    expect(useStudio.getState().drafts.image).toMatchObject({ model: flare.id, refs: [{ ...KEPT, role: "reference" }] });
    expect(useStudio.getState().view).toBe("studio");
    expect(studioApi.studioRefreshModels).not.toHaveBeenCalled();
    expect(studioApi.studioModelPrice).not.toHaveBeenCalled();
  });
});
