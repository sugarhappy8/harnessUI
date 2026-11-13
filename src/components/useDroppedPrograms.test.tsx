import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ApprovedFolder, DropAnswer, MentionCheck, MentionInput, MentionSearch } from "../lib/types";

const CHAT = 3;

const ipcMock = vi.hoisted(() => ({
  reverseDropped: vi.fn<(chatId: number, paths: string[]) => Promise<DropAnswer>>(),
  onDroppedPaths: vi.fn<(handler: (paths: string[]) => void) => Promise<() => void>>(),
  onMcpChanged: vi.fn<(handler: () => void) => Promise<() => void>>(async () => () => undefined),
  approveFolder: vi.fn<(chatId: number | null) => Promise<ApprovedFolder | null>>(async () => null),
  listApprovedFolders: vi.fn<(chatId: number | null) => Promise<ApprovedFolder[]>>(async () => []),
  searchMentions: vi.fn<(chatId: number | null, query: string, limit?: number) => Promise<MentionSearch>>(),
  resolveMentions: vi.fn<(chatId: number, mentions: MentionInput[]) => Promise<MentionCheck[]>>(),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("./AllowAllButton", () => ({ ApprovalHint: () => null }));

const { Composer } = await import("./Composer");
const { dropNoteText, insertAtCaret, isProgramHead, quotePath, MAX_DROPPED } = await import("./useDroppedPrograms");

/** The window's drop event, as the app delivers it. */
let drop: ((paths: string[]) => void) | null = null;
/** What `reverse_dropped` answers, by path (no paths: only whether rea is installed). */
let installed = true;
let answers: Record<string, DropAnswer["items"][number]> = {};

const ELF = new Uint8Array([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1, 0]);

async function setup(chatId: number | null = CHAT) {
  await act(async () => {
    render(<Composer busy={false} disabledReason={null} chatId={chatId} onSend={() => undefined} onStop={() => undefined} />);
  });
  return screen.getByRole("combobox", { name: "Message" }) as HTMLTextAreaElement;
}

function type(box: HTMLTextAreaElement, value: string, caret = value.length) {
  fireEvent.change(box, { target: { value } });
  box.setSelectionRange(caret, caret);
}

async function dropPaths(paths: string[]) {
  await act(async () => {
    drop?.(paths);
  });
  // The caret moves on the next frame.
  await act(async () => {
    await new Promise((r) => requestAnimationFrame(() => r(undefined)));
  });
}

describe("dropping a program file on the window (0.0.6 item 5)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    drop = null;
    installed = true;
    answers = {};
    ipcMock.onDroppedPaths.mockImplementation(async (handler) => {
      drop = handler;
      return () => {
        drop = null;
      };
    });
    ipcMock.reverseDropped.mockImplementation(async (_chat, paths) => ({
      installed,
      items: paths.map((p) => (installed ? (answers[p] ?? { kind: "ignore" }) : { kind: "ignore" })),
    }));
    ipcMock.searchMentions.mockImplementation(async () => ({ hits: [], indexing: false, files: 0, folders: 0 }));
  });
  afterEach(cleanup);

  it("inserts the path of a program inside the chat's folders at the caret, in quotes", async () => {
    answers["/w/hello-linux-x64"] = { kind: "insert", path: "/w/hello-linux-x64" };
    const box = await setup();
    // Asked once when the box appears, only whether rea is installed.
    expect(ipcMock.reverseDropped).toHaveBeenCalledWith(CHAT, []);
    type(box, "What does do?", "What does ".length);
    await dropPaths(["/w/hello-linux-x64"]);
    expect(ipcMock.reverseDropped).toHaveBeenLastCalledWith(CHAT, ["/w/hello-linux-x64"]);
    expect(box.value).toBe('What does "/w/hello-linux-x64" do?');
    expect(box.selectionStart).toBe('What does "/w/hello-linux-x64" '.length);
    expect(screen.queryByText(/add its folder/)).toBeNull();
  });

  it("says how to add the folder of a program outside the chat's folders, and that another can't be reached", async () => {
    answers["/elsewhere/tool.exe"] = { kind: "addFolder", fileName: "tool.exe" };
    answers["/app-data/reverse/node"] = { kind: "notReachable", fileName: "node" };
    answers["/w/lib.so"] = { kind: "insert", path: "/w/lib.so" };
    const box = await setup();
    await dropPaths(["/elsewhere/tool.exe", "/app-data/reverse/node", "/w/lib.so", "/w/notes.txt"]);
    expect(box.value).toBe('"/w/lib.so"');
    expect(screen.getByText("To let the model look inside tool.exe, add its folder to this chat.")).toBeTruthy();
    expect(screen.getByText("rea can't reach node from this chat.")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Add a folder…" })).toHaveLength(1);
  });

  it("Add a folder… approves a folder the Tools panel's way, then inserts the file once it is inside", async () => {
    answers["/elsewhere/tool.exe"] = { kind: "addFolder", fileName: "tool.exe" };
    const box = await setup();
    await dropPaths(["/elsewhere/tool.exe"]);
    const added: ApprovedFolder = { id: 9, path: "/elsewhere", chatId: CHAT };
    ipcMock.approveFolder.mockImplementationOnce(async () => {
      answers["/elsewhere/tool.exe"] = { kind: "insert", path: "/elsewhere/tool.exe" };
      return added;
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Add a folder…" }));
    });
    await act(async () => {
      await new Promise((r) => requestAnimationFrame(() => r(undefined)));
    });
    expect(ipcMock.approveFolder).toHaveBeenCalledWith(CHAT);
    // The folder chip reads the chat's folders again.
    expect(ipcMock.listApprovedFolders).toHaveBeenCalledWith(CHAT);
    expect(ipcMock.reverseDropped).toHaveBeenLastCalledWith(CHAT, ["/elsewhere/tool.exe"]);
    expect(box.value).toBe('"/elsewhere/tool.exe"');
    expect(screen.queryByText(/add its folder/)).toBeNull();
  });

  it("keeps the note when the owner cancels the folder picker", async () => {
    answers["/elsewhere/tool.exe"] = { kind: "addFolder", fileName: "tool.exe" };
    await setup();
    await dropPaths(["/elsewhere/tool.exe"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Add a folder…" }));
    });
    expect(screen.getByText(/add its folder to this chat/)).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    });
    expect(screen.queryByText(/add its folder to this chat/)).toBeNull();
  });

  it("changes nothing for files that are not programs, or while rea is not installed", async () => {
    answers["/elsewhere/tool.exe"] = { kind: "addFolder", fileName: "tool.exe" };
    const box = await setup();
    type(box, "hello");
    await dropPaths(["/w/notes.txt"]);
    expect(box.value).toBe("hello");
    await dropPaths(["/elsewhere/tool.exe"]);
    expect(screen.getByText(/add its folder/)).toBeTruthy();
    // A later drop of ordinary files leaves the note as it was.
    await dropPaths(["/w/photo.png"]);
    expect(screen.getByText(/add its folder/)).toBeTruthy();
    cleanup();
    installed = false;
    const again = await setup();
    await dropPaths(["/elsewhere/tool.exe"]);
    expect(again.value).toBe("");
    expect(screen.queryByText(/add its folder/)).toBeNull();
  });

  it(`sends at most ${MAX_DROPPED} paths`, async () => {
    await setup();
    const many = Array.from({ length: 12 }, (_, i) => `/w/f${i}`);
    await dropPaths(many);
    expect(ipcMock.reverseDropped).toHaveBeenLastCalledWith(CHAT, many.slice(0, MAX_DROPPED));
  });

  it("does nothing without an open chat", async () => {
    await setup(null);
    expect(ipcMock.reverseDropped).not.toHaveBeenCalled();
    expect(ipcMock.onDroppedPaths).not.toHaveBeenCalled();
  });

  it("asks again whether rea is installed when the tools change", async () => {
    let changed: (() => void) | null = null;
    ipcMock.onMcpChanged.mockImplementationOnce(async (h) => {
      changed = h;
      return () => undefined;
    });
    await setup();
    const before = ipcMock.reverseDropped.mock.calls.length;
    await act(async () => {
      changed?.();
    });
    expect(ipcMock.reverseDropped.mock.calls.length).toBe(before + 1);
    expect(ipcMock.reverseDropped).toHaveBeenLastCalledWith(CHAT, []);
  });

  describe("the message box's own drop handler", () => {
    function files() {
      return [new File([ELF], "hello-linux-x64"), new File(["just notes"], "notes.txt", { type: "text/plain" })];
    }

    async function htmlDrop(box: HTMLTextAreaElement) {
      await act(async () => {
        fireEvent.drop(box, { dataTransfer: { files: files(), types: ["Files"] } });
      });
      // Reading the files takes a few turns of the event loop.
      await act(async () => {
        await new Promise((r) => setTimeout(r, 20));
      });
    }

    it("leaves a program file to the window's drop event while rea is installed", async () => {
      const box = await setup();
      await htmlDrop(box);
      expect(screen.getByText("notes.txt")).toBeTruthy();
      expect(screen.queryByText("hello-linux-x64")).toBeNull();
    });

    it("attaches it as before while rea is not installed", async () => {
      installed = false;
      const box = await setup();
      await htmlDrop(box);
      expect(screen.getByText("notes.txt")).toBeTruthy();
      expect(screen.getByText("hello-linux-x64")).toBeTruthy();
    });
  });
});

describe("dropped programs: the parts", () => {
  it("knows a program by its first bytes, as the backend does", () => {
    const yes: number[][] = [
      [0x4d, 0x5a, 0x90, 0],
      [0x7f, 0x45, 0x4c, 0x46, 2],
      [0xfe, 0xed, 0xfa, 0xce],
      [0xce, 0xfa, 0xed, 0xfe],
      [0xfe, 0xed, 0xfa, 0xcf],
      [0xcf, 0xfa, 0xed, 0xfe],
      [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2],
      [0xca, 0xfe, 0xba, 0xbf, 0, 0, 0, 2],
      [0x6f, 0x0d, 0x0d, 0x0a],
      [0x03, 0xf3, 0x0d, 0x0a],
    ];
    const no: number[][] = [
      [],
      [0x4d],
      [...new TextEncoder().encode("hello world")],
      [...new TextEncoder().encode("OK\r\nmore")],
      [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0x34],
      [0xca, 0xfe, 0xba, 0xbe],
      [...new TextEncoder().encode("GGUF")],
      [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
    ];
    for (const b of yes) expect(isProgramHead(new Uint8Array(b)), JSON.stringify(b)).toBe(true);
    for (const b of no) expect(isProgramHead(new Uint8Array(b)), JSON.stringify(b)).toBe(false);
  });

  it("quotes paths, and words the notes", () => {
    expect(quotePath("C:\\work\\notepad.exe")).toBe('"C:\\work\\notepad.exe"');
    expect(quotePath('/w/odd"name')).toBe("'/w/odd\"name'");
    expect(dropNoteText({ kind: "addFolder", fileName: "a.exe", path: "/x/a.exe" })).toBe(
      "To let the model look inside a.exe, add its folder to this chat.",
    );
    expect(dropNoteText({ kind: "notReachable", fileName: "b", path: "/x/b" })).toBe("rea can't reach b from this chat.");
  });

  it("puts spaces around an insertion only where needed and replaces a selection", () => {
    const box = document.createElement("textarea");
    let text = "";
    const setText = (v: string | ((old: string) => string)) => {
      text = typeof v === "function" ? v(text) : v;
    };
    box.value = "look atnow";
    box.setSelectionRange(7, 7);
    insertAtCaret(box, setText, '"/a"');
    expect(text).toBe('look at "/a" now');
    box.value = "look at  now";
    box.setSelectionRange(8, 8);
    insertAtCaret(box, setText, '"/a"');
    expect(text).toBe('look at "/a" now');
    box.value = "replace THIS please";
    box.setSelectionRange(8, 12);
    insertAtCaret(box, setText, '"/a"');
    expect(text).toBe('replace "/a" please');
    box.value = "";
    insertAtCaret(box, setText, '"/a"');
    expect(text).toBe('"/a"');
  });
});
