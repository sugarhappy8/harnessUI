import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ApprovalModeName } from "../lib/ipc";
import type { ApprovedFolder, ChatDetail, CodingAppsStatus, ToolDto, ToolSelection } from "../lib/types";

// Radix positions menus and popovers with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const CHAT = 7;
let folders: ApprovedFolder[] = [];
let stored: string | null = null;

const NOT_CHECKED = vi.hoisted<CodingAppsStatus>(() => ({ checked: false, apps: [], message: null }));

const ipcMock = vi.hoisted(() => ({
  // The folder commands the Tools panel uses, and nothing else changes folders.
  listApprovedFolders: vi.fn<(chatId: number | null) => Promise<ApprovedFolder[]>>(async () => []),
  approveFolder: vi.fn<(chatId: number | null) => Promise<ApprovedFolder | null>>(async () => null),
  removeApprovedFolder: vi.fn<(id: number) => Promise<void>>(async () => undefined),
  setChatWorkingFolder: vi.fn<(chatId: number, path: string | null) => Promise<void>>(async () => undefined),
  getChat: vi.fn<(chatId: number) => Promise<ChatDetail>>(),
  // The approval mode's mirror and the hint under the message box.
  getChatApprovalMode: vi.fn<(chatId: number) => Promise<ApprovalModeName>>(async () => "ask"),
  getDefaultApprovalMode: vi.fn<() => Promise<ApprovalModeName>>(async () => "ask"),
  // The Tools panel, rendered beside the chip in the header tests.
  listChatTools: vi.fn<(chatId: number) => Promise<ToolDto[]>>(async () => []),
  gitInfo: vi.fn(async () => ({ backend: "system", notice: null, tokenHosts: [], identity: { name: "", email: "" } })),
  onMcpChanged: vi.fn(async () => () => undefined),
  loadedSkills: vi.fn<(chatId: number) => Promise<string[]>>(async () => []),
  getToolSelection: vi.fn<() => Promise<ToolSelection>>(async () => ({ auto: true, max: 14 })),
  setToolSelection: vi.fn<(auto: boolean) => Promise<ToolSelection>>(async (auto) => ({ auto, max: 14 })),
  setChatTools: vi.fn<(chatId: number, enabled: string[]) => Promise<void>>(async () => undefined),
  clearChatGrants: vi.fn(async () => undefined),
  unloadSkill: vi.fn(async () => undefined),
  codingAppsStatus: vi.fn<() => Promise<CodingAppsStatus>>(async () => NOT_CHECKED),
  findCodingApps: vi.fn<() => Promise<CodingAppsStatus>>(async () => NOT_CHECKED),
  // The message box.
  searchMentions: vi.fn(),
  resolveMentions: vi.fn(),
}));
vi.mock("../lib/ipc", () => ipcMock);

const {
  ALL_CHATS_NOTE,
  ALLOW_ALL_TERMINAL,
  FOLDERS_POLL_MS,
  FULL_ACCESS_LEAD,
  FULL_ACCESS_LIMIT,
  FolderChip,
  NO_FOLDER,
  NO_FOLDER_LINE,
  WHOLE_COMPUTER,
  chipText,
  folderName,
  fullAccessLine,
  messagePlaceholder,
  useChatFolders,
  useWorkingFolderName,
  visiblePath,
  workingFolderOf,
} = await import("./FolderChip");
const { ToolsPanel } = await import("./ToolsPanel");
const { Composer } = await import("./Composer");
const { MODE_LOOK } = await import("./AllowAllButton");
const { useApprovalMode } = await import("../lib/approvalMode");
const { useApp } = await import("../store/app");

const HARNESS = "/home/u/harnessUI";
const global = (id: number, path: string): ApprovedFolder => ({ id, path, chatId: null });
const own = (id: number, path: string): ApprovedFolder => ({ id, path, chatId: CHAT });

function detail(): ChatDetail {
  return {
    chat: {
      id: CHAT,
      title: "t",
      modelRef: null,
      systemPrompt: null,
      leafMessageId: null,
      createdAt: 0,
      updatedAt: 0,
      thinking: false,
      workingFolder: stored,
    },
    messages: [],
    summary: null,
  };
}

/** The chat header as ChatView builds it: the Tools panel and the chip, both reading the
 *  open chat's stored working folder. */
function Header() {
  const d = useApp((s) => s.detail);
  if (!d) return null;
  return (
    <header>
      <ToolsPanel chatId={d.chat.id} nCtx={8192} workingFolder={d.chat.workingFolder ?? null} />
      <FolderChip chatId={d.chat.id} workingFolder={d.chat.workingFolder ?? null} />
    </header>
  );
}

function setMode(mode: ApprovalModeName) {
  useApprovalMode.setState({ modes: { [CHAT]: mode }, defaultMode: "ask" });
}

async function show() {
  useApp.setState({ activeChatId: CHAT, detail: detail(), error: null });
  await act(async () => {
    render(<Header />);
  });
}

function chip(): HTMLElement {
  return document.querySelector("[data-folder-chip]") as HTMLElement;
}

/** Opens the chip's menu the way a mouse does (Radix opens it on pointer down). */
async function openMenu() {
  await act(async () => {
    fireEvent.pointerDown(chip(), { button: 0, ctrlKey: false });
  });
  return screen.getByRole("menu");
}

async function choose(menu: HTMLElement, name: string | RegExp) {
  await act(async () => {
    fireEvent.click(within(menu).getByRole("menuitem", { name }));
  });
}

describe("folder names and visible paths", () => {
  it("takes the last name of Unix and Windows paths, keeping a root or a drive", () => {
    expect(folderName("/home/u/harnessUI")).toBe("harnessUI");
    expect(folderName("/home/u/harnessUI/")).toBe("harnessUI");
    expect(folderName("C:\\Users\\me\\harnessUI")).toBe("harnessUI");
    expect(folderName("C:\\Users\\me\\harnessUI\\")).toBe("harnessUI");
    expect(folderName("\\\\server\\share")).toBe("share");
    expect(folderName("C:\\")).toBe("C:\\");
    expect(folderName("D:")).toBe("D:\\");
    expect(folderName("/")).toBe("/");
  });

  it("shows right-to-left, zero-width, control and tag characters as codes", () => {
    expect(visiblePath("/home/u/plain folder")).toBe("/home/u/plain folder");
    expect(visiblePath("/home/u/evil\u202Egnp.exe")).toBe("/home/u/evil\\u{202E}gnp.exe");
    expect(visiblePath("/home/u/a\u200Bb\u2066c\u2069")).toBe("/home/u/a\\u{200B}b\\u{2066}c\\u{2069}");
    expect(visiblePath("/x/line\nbreak\ttab")).toBe("/x/line\\u{000A}break\\u{0009}tab");
    expect(visiblePath("/x/\u{E0041}tag\uFEFF")).toBe("/x/\\u{E0041}tag\\u{FEFF}");
    // Letters of right-to-left scripts are ordinary text and stay as they are.
    expect(visiblePath("/home/u/\u05E9\u05DC\u05D5\u05DD")).toBe("/home/u/\u05E9\u05DC\u05D5\u05DD");
  });

  it("also shows the combining grapheme joiner, shorthand format controls and the whole tag plane as codes", () => {
    // "/home/u/pro\u034Fj" would look exactly like "/home/u/proj" in the menu.
    expect(visiblePath("/home/u/pro\u034Fj")).toBe("/home/u/pro\\u{034F}j");
    expect(visiblePath("/x/a\u{1BCA0}b\u{1BCA1}c\u{1BCA2}d\u{1BCA3}")).toBe(
      "/x/a\\u{1BCA0}b\\u{1BCA1}c\\u{1BCA2}d\\u{1BCA3}",
    );
    // Not assigned yet, but invisible wherever a font has nothing for them.
    expect(visiblePath("/x/\u{E0080}\u{E01F0}\u{E0FFF}")).toBe("/x/\\u{E0080}\\u{E01F0}\\u{E0FFF}");
    // Ordinary marks that do show stay as they are.
    expect(visiblePath("/x/caf\u00E9 e\u0301")).toBe("/x/caf\u00E9 e\u0301");
  });

  it("finds the working folder the way the backend does", () => {
    const list = [global(1, "/a"), own(2, "/b")];
    expect(workingFolderOf(list, "/b")?.path).toBe("/b");
    expect(workingFolderOf(list, null)?.path).toBe("/a");
    // A stored folder that is no longer approved is not used.
    expect(workingFolderOf(list, "/gone")?.path).toBe("/a");
    expect(workingFolderOf([], "/b")).toBeNull();
  });
});

describe("the chip's text", () => {
  const one = [global(1, HARNESS)];
  const several = [global(1, "/srv/shared"), own(2, HARNESS), own(3, "/tmp/scratch")];

  it("names no folder, one folder, or the working folder and how many more", () => {
    expect(chipText("ask", [], null)).toBe(NO_FOLDER);
    expect(NO_FOLDER).toBe("No folder · Choose");
    expect(chipText("ask", one, null)).toBe("harnessUI");
    expect(chipText("ask", several, HARNESS)).toBe("harnessUI + 2 more");
    expect(chipText("ask", several, null)).toBe("shared + 2 more");
    // Not read yet: nothing rather than a wrong "No folder".
    expect(chipText("ask", undefined, null)).toBe("");
  });

  it("is the same in Allow all, where the file tools still stay in the approved folders", () => {
    expect(chipText("allowAll", [], null)).toBe(NO_FOLDER);
    expect(chipText("allowAll", one, null)).toBe("harnessUI");
    expect(chipText("allowAll", several, HARNESS)).toBe("harnessUI + 2 more");
  });

  it("says Whole computer in Full access, with or without folders", () => {
    expect(WHOLE_COMPUTER).toBe("Whole computer");
    expect(chipText("fullAccess", [], null)).toBe("Whole computer");
    expect(chipText("fullAccess", undefined, null)).toBe("Whole computer");
    expect(chipText("fullAccess", several, HARNESS)).toBe("Whole computer");
  });

  it("escapes a folder name with invisible characters", () => {
    expect(chipText("ask", [own(4, "/home/u/evil\u202Egnp")], null)).toBe("evil\\u{202E}gnp");
  });

  it("words the Full access line and the message box exactly", () => {
    expect(fullAccessLine(HARNESS)).toBe(
      "Full access is on, so the file and terminal tools can reach every folder on this computer except harnessUI's own. Working folder: /home/u/harnessUI.",
    );
    expect(fullAccessLine(null)).toBe(`${FULL_ACCESS_LEAD} Working folder: your home folder.`);
    expect(messagePlaceholder("harnessUI")).toBe("Ask anything. Files: harnessUI");
    expect(messagePlaceholder(null)).toBe("Ask anything, or type @ to mention a file");
  });
});

describe("FolderChip", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    folders = [global(1, HARNESS)];
    stored = null;
    useChatFolders.setState({ byChat: {} });
    setMode("ask");
    ipcMock.listApprovedFolders.mockImplementation(async () => folders);
    ipcMock.getChat.mockImplementation(async () => detail());
    ipcMock.setChatWorkingFolder.mockImplementation(async (_chat, path) => {
      stored = path;
    });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("with no folder reads No folder · Choose, and the menu says why and offers Add a folder", async () => {
    folders = [];
    await show();
    expect(ipcMock.listApprovedFolders).toHaveBeenCalledWith(CHAT);
    expect(chip().textContent).toBe("No folder · Choose");
    expect(chip().getAttribute("aria-label")).toBe("Folders for this chat: No folder · Choose");
    const menu = await openMenu();
    expect(menu.textContent).toContain(NO_FOLDER_LINE);
    expect(within(menu).getByRole("menuitem", { name: "Add a folder…" })).toBeTruthy();
    expect(within(menu).queryByRole("menuitem", { name: /working folder/ })).toBeNull();
  });

  it("with one folder shows its last name and the full path in the menu, marked as working", async () => {
    folders = [global(1, "/home/u/projects/a/very/long/path/that/would/not/fit/harnessUI")];
    await show();
    expect(chip().textContent).toBe("harnessUI");
    const menu = await openMenu();
    const row = menu.querySelector("[data-folder-path='1']") as HTMLElement;
    // The whole path, never shortened.
    expect(row.textContent).toContain("/home/u/projects/a/very/long/path/that/would/not/fit/harnessUI");
    expect(row.textContent).toContain("Working folder");
    expect(row.textContent).toContain("all chats");
    expect(menu.textContent).toContain(ALL_CHATS_NOTE);
    // Nothing to make working, and a folder for every chat is not removed from here.
    expect(within(menu).queryByRole("menuitem", { name: /the working folder/ })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: /^Remove/ })).toBeNull();
  });

  it("with several folders names the working one and how many more", async () => {
    folders = [global(1, "/srv/shared"), own(2, HARNESS), own(3, "/tmp/scratch")];
    stored = HARNESS;
    await show();
    expect(chip().textContent).toBe("harnessUI + 2 more");
    const menu = await openMenu();
    expect((menu.querySelector("[data-folder-path='2']") as HTMLElement).textContent).toContain("Working folder");
    expect(within(menu).getByRole("menuitem", { name: "Make /srv/shared the working folder" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "Make /tmp/scratch the working folder" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "Remove /home/u/harnessUI from this chat" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "Remove /tmp/scratch from this chat" })).toBeTruthy();
    expect(within(menu).queryByRole("menuitem", { name: "Remove /srv/shared from this chat" })).toBeNull();
  });

  it("in Allow all looks as in Ask first", async () => {
    setMode("allowAll");
    folders = [global(1, HARNESS), own(2, "/tmp/scratch")];
    await show();
    expect(chip().textContent).toBe("harnessUI + 1 more");
    expect(chip().className).not.toContain("text-danger");
    const menu = await openMenu();
    expect(menu.textContent).not.toContain("Full access");
  });

  it("in Full access reads Whole computer in the mode menu's Full access colour and says what is reachable", async () => {
    setMode("fullAccess");
    folders = [global(1, "/srv/shared"), own(2, HARNESS)];
    stored = HARNESS;
    await show();
    expect(chip().textContent).toBe("Whole computer");
    for (const c of MODE_LOOK.fullAccess.button.split(" ")) expect(chip().className).toContain(c);
    const menu = await openMenu();
    const line = menu.querySelector("[data-full-access-line]") as HTMLElement;
    expect(line.textContent).toBe(fullAccessLine(HARNESS));
    // The folders stay listed: the working folder is still where relative paths start.
    expect(menu.textContent).toContain("/srv/shared");
    expect(within(menu).getByRole("menuitem", { name: "Make /srv/shared the working folder" })).toBeTruthy();
  });

  it("in Full access with no folder says the tools work in the home folder", async () => {
    setMode("fullAccess");
    folders = [];
    await show();
    expect(chip().textContent).toBe("Whole computer");
    const menu = await openMenu();
    expect((menu.querySelector("[data-full-access-line]") as HTMLElement).textContent).toBe(fullAccessLine(null));
    expect(menu.textContent).not.toContain(NO_FOLDER_LINE);
  });

  it("never says the folders hold the whole chat: they are the file tools', and in Allow all the terminal is not held to them", async () => {
    folders = [global(1, HARNESS), own(2, "/tmp/scratch")];
    const tools = `Working folder: ${HARNESS}. Click to see or change the folders the file tools can use.`;
    expect(ALLOW_ALL_TERMINAL).toBe("In Allow all, terminal commands are not held to these folders.");

    // Ask first: a command shows its card before it runs.
    await show();
    expect(chip().getAttribute("title")).toBe(tools);
    let menu = await openMenu();
    expect(menu.querySelector("[data-allow-all-line]")).toBeNull();
    expect(menu.textContent).not.toContain(ALLOW_ALL_TERMINAL);
    cleanup();

    // Allow all: a command runs without a card and its arguments are not held to the folders.
    setMode("allowAll");
    await show();
    expect(chip().getAttribute("title")).toBe(`${tools} ${ALLOW_ALL_TERMINAL}`);
    menu = await openMenu();
    expect((menu.querySelector("[data-allow-all-line]") as HTMLElement).textContent).toBe(ALLOW_ALL_TERMINAL);
    cleanup();

    // Full access says what it reaches instead.
    setMode("fullAccess");
    await show();
    expect(chip().getAttribute("title")).not.toContain("can use");
    menu = await openMenu();
    expect(menu.querySelector("[data-allow-all-line]")).toBeNull();
    cleanup();

    // Allow all with no folder: there is nothing to hold commands to (they cannot start anywhere).
    setMode("allowAll");
    folders = [];
    await show();
    menu = await openMenu();
    expect(menu.querySelector("[data-allow-all-line]")).toBeNull();
    expect(menu.textContent).toContain(NO_FOLDER_LINE);
  });

  it("in Full access also says that a program the terminal starts is not held back", async () => {
    setMode("fullAccess");
    folders = [global(1, "/srv/shared"), own(2, HARNESS)];
    stored = HARNESS;
    await show();
    expect(FULL_ACCESS_LIMIT).toBe(
      "A program the terminal starts has your full rights, so it could still reach harnessUI's own folders.",
    );
    expect(chip().getAttribute("title")).toBe(`${fullAccessLine(HARNESS)} ${FULL_ACCESS_LIMIT}`);
    const menu = await openMenu();
    // The spec's sentence stays exactly as it is, and the limit follows it.
    expect((menu.querySelector("[data-full-access-line]") as HTMLElement).textContent).toBe(fullAccessLine(HARNESS));
    expect((menu.querySelector("[data-full-access-limit]") as HTMLElement).textContent).toBe(FULL_ACCESS_LIMIT);
  });

  it("says Whole computer and No folder · Choose in words at every width; only ' more' waits for @2xl", async () => {
    // What a narrow header shows: the parts hidden below the breakpoint (a header of 42rem, a
    // window about 950 pixels wide) left out. jsdom does not apply container queries, so the
    // real widths are checked in the e2e scenarios folder-chip and header-layout. In Full access
    // the words, not only the red icon, say that the whole computer is reachable (the owner's
    // README items 6 and 7).
    const narrow = () => {
      const copy = chip().cloneNode(true) as HTMLElement;
      for (const el of copy.querySelectorAll(".hidden")) el.remove();
      return copy.textContent;
    };
    const cases: [ApprovalModeName, ApprovedFolder[], string, string][] = [
      ["ask", [], "No folder · Choose", "No folder · Choose"],
      ["ask", [global(1, HARNESS)], "harnessUI", "harnessUI"],
      ["ask", [global(1, HARNESS), own(2, "/a"), own(3, "/b")], "harnessUI + 2 more", "harnessUI + 2"],
      ["allowAll", [global(1, HARNESS), own(2, "/a")], "harnessUI + 1 more", "harnessUI + 1"],
      ["fullAccess", [global(1, HARNESS)], "Whole computer", "Whole computer"],
    ];
    for (const [mode, list, wide, short] of cases) {
      cleanup();
      setMode(mode);
      folders = list;
      await show();
      expect(chip().textContent).toBe(wide);
      expect(narrow()).toBe(short);
      // " more" is the one part a narrow header leaves out (the accessible name and the
      // tooltip keep it).
      for (const el of chip().querySelectorAll(".hidden")) {
        expect(el.className).toBe("hidden @2xl:inline");
        expect(el.textContent).toBe(" more");
      }
    }
  });

  it("follows the mode when it changes", async () => {
    folders = [global(1, HARNESS)];
    await show();
    expect(chip().textContent).toBe("harnessUI");
    await act(async () => setMode("fullAccess"));
    expect(chip().textContent).toBe("Whole computer");
    await act(async () => setMode("ask"));
    expect(chip().textContent).toBe("harnessUI");
  });

  it("shows a path with right-to-left or zero-width characters escaped, in the chip and the menu", async () => {
    const evil = "/home/u/inv\u200Boice\u202Efdp.exe";
    folders = [own(5, evil)];
    await show();
    expect(chip().textContent).toBe("inv\\u{200B}oice\\u{202E}fdp.exe");
    expect(chip().getAttribute("title")).toContain("/home/u/inv\\u{200B}oice\\u{202E}fdp.exe");
    const menu = await openMenu();
    expect(menu.textContent).toContain("/home/u/inv\\u{200B}oice\\u{202E}fdp.exe");
    expect(within(menu).getByRole("menuitem", { name: "Remove /home/u/inv\\u{200B}oice\\u{202E}fdp.exe from this chat" }))
      .toBeTruthy();
    // The raw characters are nowhere on the page.
    expect(document.body.textContent).not.toMatch(/[\u200B\u202E]/);
    for (const el of document.querySelectorAll("[title],[aria-label]")) {
      expect(el.getAttribute("title") ?? "").not.toMatch(/[\u200B\u202E]/);
      expect(el.getAttribute("aria-label") ?? "").not.toMatch(/[\u200B\u202E]/);
    }
  });

  it("Make this the working folder uses the Tools panel's command and reads the chat again", async () => {
    folders = [global(1, "/srv/shared"), own(2, HARNESS)];
    await show();
    expect(chip().textContent).toBe("shared + 1 more");
    const menu = await openMenu();
    await choose(menu, `Make ${HARNESS} the working folder`);
    expect(ipcMock.setChatWorkingFolder).toHaveBeenCalledWith(CHAT, HARNESS);
    expect(ipcMock.getChat).toHaveBeenCalledWith(CHAT);
    expect(useApp.getState().detail?.chat.workingFolder).toBe(HARNESS);
    expect(chip().textContent).toBe("harnessUI + 1 more");
  });

  it("Remove uses the Tools panel's command for a folder of this chat only", async () => {
    folders = [global(1, "/srv/shared"), own(2, HARNESS)];
    await show();
    const menu = await openMenu();
    ipcMock.removeApprovedFolder.mockImplementationOnce(async () => {
      folders = [folders[0]!];
    });
    await choose(menu, `Remove ${HARNESS} from this chat`);
    expect(ipcMock.removeApprovedFolder).toHaveBeenCalledWith(2);
    expect(chip().textContent).toBe("shared");
  });

  it("Add a folder opens the existing folder picker and approval for this chat", async () => {
    folders = [];
    await show();
    const added = own(9, "/home/u/new-project");
    ipcMock.approveFolder.mockImplementationOnce(async () => {
      folders = [added];
      return added;
    });
    const menu = await openMenu();
    await choose(menu, "Add a folder…");
    expect(ipcMock.approveFolder).toHaveBeenCalledWith(CHAT);
    expect(chip().textContent).toBe("new-project");
  });

  it("a cancelled picker changes nothing", async () => {
    folders = [];
    await show();
    const calls = ipcMock.listApprovedFolders.mock.calls.length;
    const menu = await openMenu();
    await choose(menu, "Add a folder…");
    expect(ipcMock.approveFolder).toHaveBeenCalledWith(CHAT);
    // Read on opening the menu, not again after a cancelled picker.
    expect(ipcMock.listApprovedFolders.mock.calls.length).toBe(calls + 1);
    expect(chip().textContent).toBe(NO_FOLDER);
  });

  it("a refused change is shown as an error", async () => {
    folders = [global(1, "/srv/shared"), own(2, HARNESS)];
    await show();
    ipcMock.setChatWorkingFolder.mockRejectedValueOnce({
      code: "bad_request",
      message: "Only a folder approved for this chat can be its working folder.",
    });
    const menu = await openMenu();
    await choose(menu, `Make ${HARNESS} the working folder`);
    expect(useApp.getState().error?.message).toBe("Only a folder approved for this chat can be its working folder.");
    expect(chip().textContent).toBe("shared + 1 more");
  });

  it("Open in Tools panel opens the Tools panel of the same header", async () => {
    await show();
    const menu = await openMenu();
    await choose(menu, "Open in Tools panel");
    await waitFor(() => expect(screen.getByText("Tools for this chat", { selector: "h2" })).toBeTruthy());
    expect(screen.getByRole("radiogroup", { name: "Working folder" })).toBeTruthy();
  });

  it("shows a folder approved in the Tools panel", async () => {
    folders = [global(1, HARNESS)];
    await show();
    expect(chip().textContent).toBe("harnessUI");
    await act(async () => {
      fireEvent.click(screen.getByTitle("Tools for this chat"));
    });
    const added = own(2, "/tmp/scratch");
    ipcMock.approveFolder.mockImplementationOnce(async () => {
      folders = [...folders, added];
      return added;
    });
    await act(async () => {
      fireEvent.click(screen.getByText(/Approve a folder for this chat/));
    });
    // The native picker gives the focus back to the window when it closes.
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(chip().textContent).toBe("harnessUI + 1 more");
  });

  it("shows a working folder chosen in the Tools panel", async () => {
    folders = [global(1, HARNESS), own(2, "/tmp/scratch")];
    await show();
    expect(chip().textContent).toBe("harnessUI + 1 more");
    await act(async () => {
      fireEvent.click(screen.getByTitle("Tools for this chat"));
    });
    await act(async () => {
      fireEvent.click(screen.getByTitle("/tmp/scratch").querySelector("input[type=radio]") as HTMLInputElement);
    });
    expect(ipcMock.setChatWorkingFolder).toHaveBeenCalledWith(CHAT, "/tmp/scratch");
    expect(chip().textContent).toBe("scratch + 1 more");
  });

  it("reads the folders again every few seconds while the window is visible", async () => {
    vi.useFakeTimers();
    folders = [global(1, HARNESS)];
    await show();
    expect(chip().textContent).toBe("harnessUI");
    // Changed elsewhere (the Tools panel's trash button, Settings → Permissions).
    folders = [global(1, HARNESS), own(2, "/tmp/scratch"), own(3, "/tmp/other")];
    await act(async () => {
      vi.advanceTimersByTime(FOLDERS_POLL_MS + 10);
    });
    expect(chip().textContent).toBe("harnessUI + 2 more");
    cleanup();
    const calls = ipcMock.listApprovedFolders.mock.calls.length;
    await act(async () => {
      vi.advanceTimersByTime(FOLDERS_POLL_MS * 3);
    });
    // Unmounted: no more reads.
    expect(ipcMock.listApprovedFolders.mock.calls.length).toBe(calls);
  });

  it("drops a read that ends after a newer one", async () => {
    let finishOld: (l: ApprovedFolder[]) => void = () => undefined;
    ipcMock.listApprovedFolders
      .mockImplementationOnce(() => new Promise((resolve) => (finishOld = resolve)))
      .mockImplementationOnce(async () => [global(1, HARNESS), own(2, "/tmp/new")]);
    const first = useChatFolders.getState().load(CHAT);
    await useChatFolders.getState().load(CHAT);
    finishOld([global(1, HARNESS)]);
    await first;
    expect(useChatFolders.getState().byChat[CHAT]?.map((f) => f.path)).toEqual([HARNESS, "/tmp/new"]);
  });

  it("changes made from the chip show in the Tools panel", async () => {
    folders = [global(1, HARNESS), own(2, "/tmp/scratch")];
    await show();
    const menu = await openMenu();
    await choose(menu, "Make /tmp/scratch the working folder");
    await act(async () => {
      fireEvent.click(screen.getByTitle("Tools for this chat"));
    });
    const radio = (path: string) =>
      screen.getByTitle(path).querySelector("input[type=radio]") as HTMLInputElement;
    expect(radio("/tmp/scratch").checked).toBe(true);
    expect(radio(HARNESS).checked).toBe(false);
  });
});

describe("the message box names the working folder", () => {
  function Probe() {
    return <span data-testid="name">{String(useWorkingFolderName(CHAT))}</span>;
  }

  beforeEach(() => {
    vi.clearAllMocks();
    useChatFolders.setState({ byChat: {} });
    setMode("ask");
    stored = null;
  });
  afterEach(cleanup);

  it("gives the working folder's escaped name once the folders are read", async () => {
    useApp.setState({ activeChatId: CHAT, detail: detail() });
    render(<Probe />);
    expect(screen.getByTestId("name").textContent).toBe("null");
    await act(async () => {
      useChatFolders.setState({ byChat: { [CHAT]: [global(1, "/srv/shared"), own(2, "/home/u/a\u202Eb")] } });
    });
    expect(screen.getByTestId("name").textContent).toBe("shared");
    stored = "/home/u/a\u202Eb";
    await act(async () => {
      useApp.setState({ detail: detail() });
    });
    expect(screen.getByTestId("name").textContent).toBe("a\\u{202E}b");
  });

  it("puts it in the placeholder, and keeps the old one with no folder", async () => {
    useApp.setState({ activeChatId: CHAT, detail: detail() });
    useChatFolders.setState({ byChat: { [CHAT]: [global(1, HARNESS)] } });
    render(<Composer busy={false} disabledReason={null} chatId={CHAT} onSend={() => undefined} onStop={() => undefined} />);
    const box = screen.getByRole("combobox", { name: "Message" }) as HTMLTextAreaElement;
    expect(box.placeholder).toBe("Ask anything. Files: harnessUI");
    await act(async () => {
      useChatFolders.setState({ byChat: { [CHAT]: [] } });
    });
    expect(box.placeholder).toBe("Ask anything, or type @ to mention a file");
    cleanup();
    // A reason the box is off still wins.
    render(<Composer busy={false} disabledReason="Choose a model above to start chatting" chatId={CHAT} onSend={() => undefined} onStop={() => undefined} />);
    expect((screen.getByRole("combobox", { name: "Message" }) as HTMLTextAreaElement).placeholder).toBe(
      "Choose a model above to start chatting",
    );
  });
});
