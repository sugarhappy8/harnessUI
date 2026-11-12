import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { MentionCheck, MentionHit, MentionInput, MentionSearch } from "../lib/types";

const ipcMock = vi.hoisted(() => ({
  searchMentions: vi.fn<(chatId: number | null, query: string, limit?: number) => Promise<MentionSearch>>(),
  resolveMentions: vi.fn<(chatId: number, mentions: MentionInput[]) => Promise<MentionCheck[]>>(),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("./AllowAllButton", () => ({ ApprovalHint: () => null }));

const { Composer } = await import("./Composer");
const { mentionQueryAt, tokenIndex, removeToken, insertMention, chipsInText } = await import("./MentionPicker");

const ADD: MentionHit = { kind: "function", label: "calc.py#add", path: "/w/calc.py", symbol: "add", line: 4, endLine: 5 };
const NOTES: MentionHit = { kind: "file", label: "notes.txt", path: "/w/notes.txt" };

const ok = (label: string): MentionCheck => ({
  label,
  kind: "file",
  startLine: 1,
  endLine: 2,
  totalLines: 2,
  truncated: false,
});

function setup(chatId: number | null = 3) {
  const onSend = vi.fn();
  render(<Composer busy={false} disabledReason={null} chatId={chatId} onSend={onSend} onStop={() => undefined} />);
  const box = screen.getByRole("combobox", { name: "Message" }) as HTMLTextAreaElement;
  return { onSend, box };
}

/** Types `value` with the caret at its end. */
function type(box: HTMLTextAreaElement, value: string) {
  fireEvent.change(box, { target: { value } });
  box.setSelectionRange(value.length, value.length);
}

describe("Composer mentions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.searchMentions.mockImplementation(async (_chat, query) => ({
      hits: [ADD, NOTES].filter((h) => h.label.includes(query)),
      indexing: false,
      files: 2,
      folders: 1,
    }));
    ipcMock.resolveMentions.mockImplementation(async (_chat, mentions) => mentions.map((m) => ok(m.path)));
  });
  afterEach(cleanup);

  it("typing @ opens a listbox above the box with the matches, as a combobox", async () => {
    const { box } = setup();
    type(box, "explain @ad");
    const list = await screen.findByRole("listbox", { name: "Files and definitions" });
    const options = within(list).getAllByRole("option");
    expect(options).toHaveLength(1);
    expect(options[0]?.textContent).toContain("add");
    expect(options[0]?.textContent).toContain("calc.py:4");
    expect(options[0]?.getAttribute("aria-selected")).toBe("true");
    expect(box.getAttribute("aria-expanded")).toBe("true");
    expect(box.getAttribute("aria-controls")).toBe(list.id);
    expect(box.getAttribute("aria-activedescendant")).toBe(options[0]?.id);
    expect(ipcMock.searchMentions).toHaveBeenLastCalledWith(3, "ad", 20);
  });

  it("arrow keys and Enter choose; the mention goes into the text and a chip appears", async () => {
    const { box, onSend } = setup();
    type(box, "see @");
    const list = await screen.findByRole("listbox");
    expect(within(list).getAllByRole("option")).toHaveLength(2);
    fireEvent.keyDown(box, { key: "ArrowDown" });
    expect(within(list).getAllByRole("option")[1]?.getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(box, { key: "ArrowUp" });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box.value).toBe("see @calc.py#add ");
    expect(screen.queryByRole("listbox")).toBeNull();
    const chips = screen.getByRole("list", { name: "Mentions" });
    expect(chips.textContent).toContain("calc.py#add");
    // Enter chose; it did not send.
    expect(onSend).not.toHaveBeenCalled();
  });

  it("Escape closes the picker and a mouse click chooses", async () => {
    const { box } = setup();
    type(box, "@not");
    await screen.findByRole("listbox");
    fireEvent.keyDown(box, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(box.value).toBe("@not");
    type(box, "@note");
    const option = await screen.findByRole("option");
    fireEvent.click(option);
    expect(box.value).toBe("@notes.txt ");
  });

  it("a chip can be removed before sending, and its text goes with it", async () => {
    const { box, onSend } = setup();
    type(box, "look at @ad");
    fireEvent.click(await screen.findByRole("option"));
    expect(box.value).toBe("look at @calc.py#add ");
    fireEvent.click(screen.getByRole("button", { name: "Remove mention calc.py#add" }));
    expect(screen.queryByRole("list", { name: "Mentions" })).toBeNull();
    expect(box.value).toBe("look at ");
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(onSend).toHaveBeenCalled());
    expect(onSend.mock.calls[0]?.[3]).toEqual([]);
    expect(ipcMock.resolveMentions).not.toHaveBeenCalled();
  });

  it("sends only chips whose mention is still in the text; typed @names are plain text", async () => {
    const { box, onSend } = setup();
    type(box, "@ad");
    fireEvent.click(await screen.findByRole("option"));
    type(box, `${box.value}and @notes.txt typed by hand`);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Send" }));
    });
    await waitFor(() => expect(onSend).toHaveBeenCalled());
    const [text, , , mentions] = onSend.mock.calls[0] ?? [];
    expect(text).toBe("@calc.py#add and @notes.txt typed by hand");
    expect(mentions).toEqual([{ path: "/w/calc.py", symbol: "add", kind: "function", line: 4 }]);
    expect(ipcMock.resolveMentions).toHaveBeenCalledWith(3, mentions);
    expect(box.value).toBe("");
    expect(screen.queryByRole("list", { name: "Mentions" })).toBeNull();
  });

  it("deleting a mention's text drops its chip", async () => {
    const { box, onSend } = setup();
    type(box, "@ad");
    fireEvent.click(await screen.findByRole("option"));
    type(box, "no mention now");
    expect(screen.queryByRole("list", { name: "Mentions" })).toBeNull();
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(onSend).toHaveBeenCalled());
    expect(onSend.mock.calls[0]?.[3]).toEqual([]);
  });

  it("a mention that cannot be read keeps the text and says why", async () => {
    ipcMock.resolveMentions.mockResolvedValue([
      { ...ok(""), error: "calc.py cannot be mentioned: the path is outside the approved folders." },
    ]);
    const { box, onSend } = setup();
    type(box, "@ad");
    fireEvent.click(await screen.findByRole("option"));
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
    });
    expect(await screen.findByText(/outside the approved folders/)).toBeTruthy();
    expect(onSend).not.toHaveBeenCalled();
    expect(box.value).toBe("@calc.py#add ");
  });

  it("an @ inside a word (an e-mail address) opens nothing", async () => {
    const { box } = setup();
    type(box, "mail me at a@b");
    await new Promise((r) => setTimeout(r, 150));
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(ipcMock.searchMentions).not.toHaveBeenCalled();
  });
});

describe("mention text", () => {
  it("finds the @query before the caret", () => {
    expect(mentionQueryAt("see @src/li", 11)).toEqual({ start: 4, query: "src/li" });
    expect(mentionQueryAt("@", 1)).toEqual({ start: 0, query: "" });
    expect(mentionQueryAt("a@b", 3)).toBeNull();
    expect(mentionQueryAt("@done here", 10)).toBeNull();
  });

  it("inserts, finds and removes whole tokens only", () => {
    expect(insertMention("see @ca and more", 4, 7, "calc.py")).toEqual({ text: "see @calc.py and more", caret: 13 });
    expect(tokenIndex("x @calc.py#add y", "calc.py")).toBe(-1);
    expect(tokenIndex("x @calc.py#add y", "calc.py#add")).toBe(2);
    expect(tokenIndex("read @notes.txt.", "notes.txt")).toBe(5);
    expect(tokenIndex("read @notes.txt.bak", "notes.txt")).toBe(-1);
    expect(tokenIndex("mail@notes.txt", "notes.txt")).toBe(-1);
    expect(removeToken("a @calc.py b", "calc.py")).toBe("a b");
    const chips = [
      { label: "calc.py", kind: "file", input: { path: "/w/calc.py" } },
      { label: "calc.py#add", kind: "function", input: { path: "/w/calc.py", symbol: "add" } },
    ];
    expect(chipsInText(chips, "only @calc.py#add").map((c) => c.label)).toEqual(["calc.py#add"]);
  });
});
