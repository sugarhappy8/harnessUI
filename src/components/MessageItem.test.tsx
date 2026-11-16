// [0.0.7 P6] Edit and resend dropped the message's images, files and mentions without a word,
// and a message with only an image could not be resent. The edit box shows them now, each
// with a way to remove it, and the new message keeps the rest.
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { MentionRef, MessageDto } from "../lib/types";

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  getAttachment: vi.fn(async (id: number) => `data:image/png;base64,${id}`),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("./context/PinMessageButton", () => ({ PinMessageButton: () => null }));

const { MessageItem } = await import("./MessageItem");

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
