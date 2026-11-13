import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ApprovedFolder, CodingAppsStatus, ToolDto, ToolSelection } from "../lib/types";

// Radix positions the popover with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const CHAT = 7;
let folders: ApprovedFolder[] = [];
let loaded: string[] = [];

const NOT_CHECKED = vi.hoisted<CodingAppsStatus>(() => ({
  checked: false,
  apps: [
    {
      app: "claudeCode",
      label: "Claude Code",
      tool: "ask_claude_code",
      path: null,
      version: null,
      usable: false,
      problem: null,
    },
    { app: "codex", label: "Codex", tool: "ask_codex", path: null, version: null, usable: false, problem: null },
  ],
  message: null,
}));

const ipcMock = vi.hoisted(() => ({
  listChatTools: vi.fn<(chatId: number) => Promise<ToolDto[]>>(async () => []),
  gitInfo: vi.fn(async () => ({ backend: "system", notice: null, tokenHosts: [], identity: { name: "", email: "" } })),
  onMcpChanged: vi.fn(async () => () => undefined),
  listApprovedFolders: vi.fn<(chatId: number | null) => Promise<ApprovedFolder[]>>(async () => []),
  loadedSkills: vi.fn<(chatId: number) => Promise<string[]>>(async () => []),
  approveFolder: vi.fn<(chatId: number | null) => Promise<ApprovedFolder | null>>(async () => null),
  removeApprovedFolder: vi.fn<(id: number) => Promise<void>>(async () => undefined),
  setChatWorkingFolder: vi.fn<(chatId: number, path: string | null) => Promise<void>>(async () => undefined),
  unloadSkill: vi.fn<(chatId: number, name: string) => Promise<void>>(async () => undefined),
  clearChatGrants: vi.fn(async () => undefined),
  setChatTools: vi.fn<(chatId: number, enabled: string[]) => Promise<void>>(async () => undefined),
  getToolSelection: vi.fn<() => Promise<ToolSelection>>(async () => ({ auto: true, max: 14 })),
  setToolSelection: vi.fn<(auto: boolean) => Promise<ToolSelection>>(async (auto) => ({ auto, max: 14 })),
  getChat: vi.fn(async () => {
    throw new Error("not used");
  }),
  // Coding apps (0.0.6): never looked for unless a test says otherwise.
  codingAppsStatus: vi.fn<() => Promise<CodingAppsStatus>>(async () => NOT_CHECKED),
  findCodingApps: vi.fn<() => Promise<CodingAppsStatus>>(async () => NOT_CHECKED),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { ToolsPanel } = await import("./ToolsPanel");

async function openPanel(workingFolder: string | null = null) {
  await act(async () => {
    render(<ToolsPanel chatId={CHAT} nCtx={8192} workingFolder={workingFolder} />);
  });
  await act(async () => {
    fireEvent.click(screen.getByTitle("Tools for this chat"));
  });
}

function radio(path: string): HTMLInputElement {
  return screen.getByTitle(path).querySelector("input[type=radio]") as HTMLInputElement;
}

describe("ToolsPanel: folders and loaded skills", () => {
  beforeEach(() => {
    folders = [
      { id: 1, path: "/home/u/all", chatId: null },
      { id: 2, path: "/home/u/proj", chatId: CHAT },
    ];
    loaded = [];
    vi.clearAllMocks();
    ipcMock.listApprovedFolders.mockImplementation(async () => folders);
    ipcMock.loadedSkills.mockImplementation(async () => loaded);
  });
  afterEach(cleanup);

  it("lists global and this chat's folders, and lets only this chat's own be removed", async () => {
    await openPanel();
    expect(ipcMock.listApprovedFolders).toHaveBeenCalledWith(CHAT);
    expect(screen.getByText("/home/u/all")).toBeTruthy();
    expect(screen.getByText("/home/u/proj")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Remove /home/u/all from this chat" })).toBeNull();
    folders = [folders[0]!];
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove /home/u/proj from this chat" }));
    });
    expect(ipcMock.removeApprovedFolder).toHaveBeenCalledWith(2);
    expect(screen.queryByText("/home/u/proj")).toBeNull();
  });

  it("approves a folder for this chat only", async () => {
    await openPanel();
    const added = { id: 3, path: "/home/u/new", chatId: CHAT };
    ipcMock.approveFolder.mockImplementationOnce(async () => {
      folders = [...folders, added];
      return added;
    });
    await act(async () => {
      fireEvent.click(screen.getByText(/Approve a folder for this chat/));
    });
    expect(ipcMock.approveFolder).toHaveBeenCalledWith(CHAT);
    expect(screen.getByText("/home/u/new")).toBeTruthy();
  });

  it("marks the folder the backend uses as the working folder", async () => {
    await openPanel(null);
    expect(radio("/home/u/all").checked).toBe(true);
    cleanup();
    await openPanel("/home/u/proj");
    expect(radio("/home/u/proj").checked).toBe(true);
    expect(radio("/home/u/all").checked).toBe(false);
    cleanup();
    // A stored folder that is no longer approved is not used: the first one is.
    await openPanel("/home/u/removed");
    expect(radio("/home/u/all").checked).toBe(true);
  });

  it("saves the chosen working folder", async () => {
    await openPanel();
    await act(async () => {
      fireEvent.click(radio("/home/u/proj"));
    });
    expect(ipcMock.setChatWorkingFolder).toHaveBeenCalledWith(CHAT, "/home/u/proj");
  });

  it("shows a folder's hidden and direction-changing characters escaped, like the folder chip", async () => {
    const odd = "/home/u/odd\u202Egnp.exe\u200B";
    const shown = "/home/u/odd\\u{202E}gnp.exe\\u{200B}";
    folders = [...folders, { id: 3, path: odd, chatId: CHAT }];
    await openPanel();
    const list = screen.getByRole("radiogroup", { name: "Working folder" });
    // Not in the text, the tooltips or the Remove buttons' names.
    const said = [
      list.textContent ?? "",
      ...[...list.querySelectorAll("[title], [aria-label]")].map(
        (e) => `${e.getAttribute("title") ?? ""} ${e.getAttribute("aria-label") ?? ""}`,
      ),
    ].join("\n");
    expect(said.includes("\u202E")).toBe(false);
    expect(said.includes("\u200B")).toBe(false);
    expect(list.textContent).toContain(shown);
    // The path reads left to right whatever its letters are.
    const path = screen.getByTitle(shown).querySelector("bdi");
    expect(path?.getAttribute("dir")).toBe("ltr");
    expect(path?.textContent).toBe(shown);
    // The commands still get the folder's real path and id.
    await act(async () => {
      fireEvent.click(radio(shown));
    });
    expect(ipcMock.setChatWorkingFolder).toHaveBeenCalledWith(CHAT, odd);
    folders = folders.slice(0, 2);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: `Remove ${shown} from this chat` }));
    });
    expect(ipcMock.removeApprovedFolder).toHaveBeenCalledWith(3);
  });

  it("unloads a loaded skill from this chat", async () => {
    loaded = ["pdf-forms", "release-notes"];
    await openPanel();
    expect(ipcMock.loadedSkills).toHaveBeenCalledWith(CHAT);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Unload pdf-forms" }));
    });
    expect(ipcMock.unloadSkill).toHaveBeenCalledWith(CHAT, "pdf-forms");
    expect(screen.queryByText("pdf-forms")).toBeNull();
    expect(screen.getByText("release-notes")).toBeTruthy();
  });
});

function tool(server: string, name: string, over: Partial<ToolDto> = {}): ToolDto {
  return {
    exposed: name,
    server,
    original: name,
    description: "",
    builtin: true,
    enabledInChat: false,
    available: true,
    changed: false,
    allowAnyArgs: false,
    alwaysAsk: false,
    destructive: false,
    ready: true,
    schemaError: null,
    tokenEstimate: 140,
    companion: false,
    // As the backend reports it: a tool that is on is shown unless a test says otherwise.
    shown: over.enabledInChat === true,
    ...over,
  };
}

describe("ToolsPanel: the Browser group", () => {
  const NO_BROWSER = "The browser tool needs Chrome, Edge, Brave, Vivaldi or Chromium, and none was found.";

  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listApprovedFolders.mockImplementation(async () => []);
    ipcMock.loadedSkills.mockImplementation(async () => []);
  });
  afterEach(cleanup);

  it("says why the browser cannot work and opens Settings on the Browser page", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "files_read", { enabledInChat: true }),
      tool("browser", "browser_navigate", { available: false, schemaError: NO_BROWSER }),
      tool("browser", "browser_click", { available: false, schemaError: NO_BROWSER }),
    ]);
    const opened: unknown[] = [];
    const listener = (e: Event) => opened.push((e as CustomEvent<unknown>).detail);
    window.addEventListener("app:open-settings", listener);
    try {
      await openPanel();
      // Once on the group, not once per tool.
      expect(screen.getAllByText(NO_BROWSER)).toHaveLength(1);
      const navigate = screen.getByText("browser_navigate").closest("label")!.querySelector("input")!;
      expect(navigate.disabled).toBe(true);
      expect(navigate.checked).toBe(false);
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Set up the browser" }));
      });
      expect(opened).toEqual(["browser"]);
      // The panel closes so the Settings dialog is not behind it.
      expect(screen.queryByText("Tools for this chat", { selector: "h2" })).toBeNull();
    } finally {
      window.removeEventListener("app:open-settings", listener);
    }
  });

  it("shows no set-up button when a browser is available", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("browser", "browser_navigate", { enabledInChat: true }),
      tool("browser", "browser_click", { enabledInChat: true }),
    ]);
    await openPanel();
    expect(screen.queryByRole("button", { name: "Set up the browser" })).toBeNull();
    expect(screen.getByTitle("Tools for this chat").textContent).toContain("Tools (2)");
  });

  it("keeps another group's per-tool reason on the tool", async () => {
    const why = "This model cannot see images, which computer use needs. Pick a vision model.";
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("computer", "computer_screenshot", { available: false, schemaError: why }),
      tool("computer", "computer_click", { available: false, schemaError: why }),
    ]);
    await openPanel();
    expect(screen.getAllByText(why)).toHaveLength(2);
    expect(screen.queryByRole("button", { name: "Set up the browser" })).toBeNull();
  });

  it("reads the tools again when the model's context becomes known", async () => {
    let view!: ReturnType<typeof render>;
    await act(async () => {
      view = render(<ToolsPanel chatId={CHAT} nCtx={null} />);
    });
    const before = ipcMock.listChatTools.mock.calls.length;
    await act(async () => {
      view.rerender(<ToolsPanel chatId={CHAT} nCtx={8192} />);
    });
    expect(ipcMock.listChatTools.mock.calls.length).toBe(before + 1);
  });
});

describe("ToolsPanel: the Subagents group", () => {
  const NOTE =
    "Lets the model hand a task to a helper that works in its own context with the same tools and the same approvals.";

  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listApprovedFolders.mockImplementation(async () => []);
    ipcMock.loadedSkills.mockImplementation(async () => []);
  });
  afterEach(cleanup);

  it("is named, explained in one line, listed last and off until it is switched on", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true }),
      tool("subagents", "run_subagent"),
      tool("browser", "browser_navigate"),
    ]);
    await openPanel();
    expect(screen.getAllByText(NOTE)).toHaveLength(1);
    const titles = [...document.querySelectorAll("section > label")].map((l) => l.textContent?.trim());
    expect(titles).toEqual(["Files", "Browser", "Subagents"]);
    const group = screen.getByText("Subagents").closest("label")!.querySelector("input")!;
    expect(group.checked).toBe(false);
    expect(screen.getByTitle("Tools for this chat").textContent).toContain("Tools (1)");
    await act(async () => {
      fireEvent.click(group);
    });
    expect(ipcMock.setChatTools).toHaveBeenCalledWith(CHAT, ["read_file", "run_subagent"]);
  });

  it("does not add its note to other groups", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [tool("files", "read_file", { enabledInChat: true })]);
    await openPanel();
    expect(screen.queryByText(NOTE)).toBeNull();
  });
});

describe("ToolsPanel: tools chosen for each task", () => {
  const companions = [
    tool("toolfinder", "find_tools", { enabledInChat: true, companion: true, tokenEstimate: 100 }),
    tool("toolfinder", "call_tool", { enabledInChat: true, companion: true, tokenEstimate: 100 }),
    tool("task", "update_task", { enabledInChat: true, companion: true, tokenEstimate: 100 }),
  ];

  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listApprovedFolders.mockImplementation(async () => []);
    ipcMock.loadedSkills.mockImplementation(async () => []);
    ipcMock.getToolSelection.mockImplementation(async () => ({ auto: true, max: 14 }));
    ipcMock.setToolSelection.mockImplementation(async (auto) => ({ auto, max: 14 }));
  });
  afterEach(cleanup);

  function row(name: string): HTMLElement {
    return screen.getByText(name).closest("li") as HTMLElement;
  }

  it("marks each tool that is on as shown or on request, and a tool that is off not at all", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true, shown: true }),
      tool("git", "git_commit", { enabledInChat: true, shown: false }),
      tool("git", "git_push", { enabledInChat: false }),
      ...companions,
    ]);
    await openPanel();
    expect(row("read_file").textContent).toContain("shown");
    expect(row("read_file").textContent).not.toContain("on request");
    expect(row("git_commit").textContent).toContain("on request");
    expect(row("git_push").textContent).not.toContain("shown");
    expect(row("git_push").textContent).not.toContain("on request");
  });

  it("counts the tools that are on, not the ones shown, and never the companions", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true, shown: true }),
      tool("git", "git_commit", { enabledInChat: true, shown: false }),
      tool("git", "git_push", { enabledInChat: false }),
      ...companions,
    ]);
    await openPanel();
    expect(screen.getByTitle("Tools for this chat").textContent).toContain("Tools (2)");
    // The context estimate is what a request carries: the shown tools, companions included.
    expect(screen.getByTitle("Context the tool descriptions take on every request").textContent).toContain("~440 tokens");
  });

  it("lists the companions in one line, not as switches", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true, shown: true }),
      ...companions,
    ]);
    await openPanel();
    const titles = [...document.querySelectorAll("section > label")].map((l) => l.textContent?.trim());
    expect(titles).toEqual(["Files"]);
    const line = screen.getByText(/Always included/);
    expect(line.textContent).toContain("find_tools, call_tool, update_task");
    expect(line.closest("label")).toBeNull();
    // The automatic-choice switch, Files, read_file and (0.0.6) the Coding apps switch:
    // no companion has one.
    expect(document.querySelectorAll("input[type=checkbox]").length).toBe(4);
    expect(screen.getByRole("switch", { name: "Coding apps (experimental)" })).toBeTruthy();
    // Switching a group on never sends a companion as a switch.
    await act(async () => {
      fireEvent.click(screen.getByText("Files").closest("label")!.querySelector("input")!);
    });
    expect(ipcMock.setChatTools).toHaveBeenCalledWith(CHAT, []);
  });

  it("says nothing about companions when no tool is on", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file"),
      ...companions.map((c) => ({ ...c, enabledInChat: false, shown: false })),
    ]);
    await openPanel();
    expect(screen.queryByText(/Always included/)).toBeNull();
    expect(screen.getByTitle("Tools for this chat").textContent).toContain("Tools (0)");
  });

  it("has a switch for choosing tools automatically, with one line saying what it does", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true, shown: true }),
      tool("git", "git_commit", { enabledInChat: true, shown: false }),
    ]);
    await openPanel();
    const auto = screen.getByRole("switch", { name: "Choose tools for each task automatically" }) as HTMLInputElement;
    expect(auto.checked).toBe(true);
    expect(screen.getByText(/gets the tools that fit your message/)).toBeTruthy();
    // It is not a tool group.
    expect(auto.closest("section")).toBeNull();
    // Off: the setting is saved and the list is read again (every tool is shown then).
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true, shown: true }),
      tool("git", "git_commit", { enabledInChat: true, shown: true }),
    ]);
    const before = ipcMock.listChatTools.mock.calls.length;
    await act(async () => {
      fireEvent.click(auto);
    });
    expect(ipcMock.setToolSelection).toHaveBeenCalledWith(false);
    expect(ipcMock.listChatTools.mock.calls.length).toBeGreaterThan(before);
    expect(auto.checked).toBe(false);
    expect(row("git_commit").textContent).not.toContain("on request");
  });

  it("shows the switch as off when the setting is off", async () => {
    ipcMock.getToolSelection.mockImplementation(async () => ({ auto: false, max: 14 }));
    ipcMock.listChatTools.mockImplementation(async () => [tool("files", "read_file", { enabledInChat: true })]);
    await openPanel();
    const auto = screen.getByRole("switch", { name: "Choose tools for each task automatically" }) as HTMLInputElement;
    expect(auto.checked).toBe(false);
  });

  it("names the new groups", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("artifacts", "create_artifact", { enabledInChat: true }),
      tool("search", "web_search", { enabledInChat: true }),
    ]);
    await openPanel();
    const titles = [...document.querySelectorAll("section > label")].map((l) => l.textContent?.trim());
    expect(titles).toEqual(["Artifacts", "Web search"]);
  });
});

describe("ToolsPanel: the Coding apps group (0.0.6)", () => {
  const NOTE =
    "Lets the model hand a task to Claude Code or Codex on this computer. They work online with your own account. Every request asks you first, except in Full access.";
  const NOT_INSTALLED = "Claude Code and Codex are not installed on this computer.";
  const FOUND: CodingAppsStatus = {
    checked: true,
    apps: [
      {
        app: "claudeCode",
        label: "Claude Code",
        tool: "ask_claude_code",
        path: "/home/u/.local/share/claude/versions/2.1.322",
        version: "2.1.322",
        usable: true,
        problem: null,
      },
      {
        app: "codex",
        label: "Codex",
        tool: "ask_codex",
        path: null,
        version: null,
        usable: false,
        problem: "Codex is not installed on this computer.",
      },
    ],
    message: null,
  };
  const groupSwitch = () => screen.getByRole("switch", { name: "Coding apps (experimental)" }) as HTMLInputElement;

  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listApprovedFolders.mockImplementation(async () => []);
    ipcMock.loadedSkills.mockImplementation(async () => []);
    ipcMock.codingAppsStatus.mockImplementation(async () => NOT_CHECKED);
    ipcMock.findCodingApps.mockImplementation(async () => NOT_CHECKED);
  });
  afterEach(cleanup);

  it("is named, marked experimental, explained in one line and off in a new chat", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [tool("files", "read_file", { enabledInChat: true })]);
    await openPanel();
    expect(ipcMock.codingAppsStatus).toHaveBeenCalled();
    expect(groupSwitch().checked).toBe(false);
    expect(screen.getAllByText(NOTE)).toHaveLength(1);
    // Nothing was looked for yet, so no program is named.
    expect(ipcMock.findCodingApps).not.toHaveBeenCalled();
    expect(screen.queryByText("Claude Code")).toBeNull();
    // It is not one of the ordinary groups, and it does not count as a tool that is on.
    const titles = [...document.querySelectorAll("section > label")].map((l) => l.textContent?.trim());
    expect(titles).toEqual(["Files"]);
    expect(screen.getByTitle("Tools for this chat").textContent).toContain("Tools (1)");
  });

  it("looks for the programs when switched on, names each by its full path and turns its tools on", async () => {
    let found = false;
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true }),
      ...(found ? [tool("coding_apps", "ask_claude_code")] : []),
    ]);
    ipcMock.findCodingApps.mockImplementation(async () => {
      found = true;
      return FOUND;
    });
    await openPanel();
    await act(async () => {
      fireEvent.click(groupSwitch());
    });
    expect(ipcMock.findCodingApps).toHaveBeenCalledTimes(1);
    expect(ipcMock.setChatTools).toHaveBeenCalledWith(CHAT, ["read_file", "ask_claude_code"]);
    expect(screen.getByText("/home/u/.local/share/claude/versions/2.1.322")).toBeTruthy();
    expect(screen.getByText("Codex is not installed on this computer.")).toBeTruthy();
    // The coding apps never show up as an ordinary group as well.
    const titles = [...document.querySelectorAll("section > label")].map((l) => l.textContent?.trim());
    expect(titles).toEqual(["Files"]);
  });

  it("says when neither program is installed and turns nothing on", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [tool("files", "read_file", { enabledInChat: true })]);
    ipcMock.findCodingApps.mockImplementation(async () => ({
      checked: true,
      apps: FOUND.apps.map((a) => ({ ...a, path: null, version: null, usable: false, problem: `${a.label} is not installed on this computer.` })),
      message: NOT_INSTALLED,
    }));
    await openPanel();
    await act(async () => {
      fireEvent.click(groupSwitch());
    });
    expect(screen.getByText(NOT_INSTALLED)).toBeTruthy();
    expect(ipcMock.setChatTools).not.toHaveBeenCalled();
    expect(groupSwitch().checked).toBe(false);
  });

  it("is on when its tools are on, and switching it off turns them off without looking again", async () => {
    ipcMock.codingAppsStatus.mockImplementation(async () => FOUND);
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true }),
      tool("coding_apps", "ask_claude_code", { enabledInChat: true }),
    ]);
    await openPanel();
    expect(groupSwitch().checked).toBe(true);
    expect(screen.getByText("/home/u/.local/share/claude/versions/2.1.322")).toBeTruthy();
    await act(async () => {
      fireEvent.click(groupSwitch());
    });
    expect(ipcMock.findCodingApps).not.toHaveBeenCalled();
    expect(ipcMock.setChatTools).toHaveBeenCalledWith(CHAT, ["read_file"]);
  });

  it("shows why a program found earlier can no longer be used", async () => {
    const changed = "Claude Code changed on this computer after Coding apps was turned on, perhaps by an update. Turn Coding apps off and on again in the Tools panel to use it.";
    ipcMock.codingAppsStatus.mockImplementation(async () => ({
      ...FOUND,
      apps: [{ ...FOUND.apps[0]!, usable: false, problem: changed }, FOUND.apps[1]!],
    }));
    ipcMock.listChatTools.mockImplementation(async () => [tool("coding_apps", "ask_claude_code")]);
    await openPanel();
    expect(screen.getByText(changed)).toBeTruthy();
  });
});

describe("ToolsPanel: the Reverse engineering group (0.0.6)", () => {
  // The reasons as the backend gives them (`ReverseServer::unavailable_reason`, pinned in
  // `mcp_cmd.rs`'s tests).
  const NOT_SET_UP = "Not set up yet. Set it up in Settings → Reverse engineering.";
  const APPLE = "Needs Apple's command line tools. Install them in Settings → Reverse engineering.";
  const UPDATE = "Needs an update in Settings → Reverse engineering.";
  const DECOMPILER = "Needs a decompiler. Install one in Settings → Reverse engineering.";
  const CHANGED = "Changed in the installed rea. Update rea in Settings → Reverse engineering.";
  const NOTE = "Lets the model look inside program files in this chat's folders with rea. It never runs them.";
  const DECOMPILER_LINE = "Readable code needs a decompiler. Install one in Settings → Reverse engineering.";
  // A few of the 37, one of each kind: opening, a file check, two that work on the open file.
  const OPEN = "open_binary";
  const CHECK = "inspect_pe_imports";
  const SESSION = ["list_procedures", "procedure_pseudo_code"];

  function reverse(
    reasons: { open?: string | null; check?: string | null; session?: string | null },
    over: Partial<ToolDto> = {},
  ): ToolDto[] {
    const one = (name: string, why: string | null | undefined) =>
      tool("reverse", name, { available: !why, schemaError: why ?? null, ...over });
    return [one(OPEN, reasons.open), one(CHECK, reasons.check), ...SESSION.map((n) => one(n, reasons.session))];
  }

  function groupBox(): HTMLInputElement {
    return screen.getByText("Reverse engineering").closest("label")!.querySelector("input")!;
  }

  function toolBox(name: string): HTMLInputElement {
    return screen.getByText(name).closest("label")!.querySelector("input")!;
  }

  async function opensSettingsOnTheTab(button: string) {
    const opened: unknown[] = [];
    const listener = (e: Event) => opened.push((e as CustomEvent<unknown>).detail);
    window.addEventListener("app:open-settings", listener);
    try {
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: button }));
      });
      expect(opened).toEqual(["reverse"]);
      // The panel closes so the Settings dialog is not behind it.
      expect(screen.queryByText("Tools for this chat", { selector: "h2" })).toBeNull();
    } finally {
      window.removeEventListener("app:open-settings", listener);
    }
  }

  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listApprovedFolders.mockImplementation(async () => []);
    ipcMock.loadedSkills.mockImplementation(async () => []);
  });
  afterEach(cleanup);

  it("is named and explained in one line", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true }),
      ...reverse({}),
    ]);
    await openPanel();
    const titles = [...document.querySelectorAll("section > label")].map((l) => l.textContent?.trim());
    expect(titles).toEqual(["Files", "Reverse engineering"]);
    expect(screen.getAllByText(NOTE)).toHaveLength(1);
    expect(groupBox().checked).toBe(false);
    expect(groupBox().disabled).toBe(false);
    // Everything can run: no line about setting it up.
    expect(screen.queryByRole("button", { name: "Set up reverse engineering" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Open Reverse engineering settings" })).toBeNull();
    expect(screen.queryByText(DECOMPILER_LINE)).toBeNull();
  });

  it("before rea is set up says so once on the group, and Set up reverse engineering opens its Settings tab", async () => {
    ipcMock.listChatTools.mockImplementation(async () => reverse({ open: NOT_SET_UP, check: NOT_SET_UP, session: NOT_SET_UP }));
    await openPanel();
    expect(screen.getAllByText(NOT_SET_UP)).toHaveLength(1);
    // Nothing to switch on yet.
    expect(groupBox().disabled).toBe(true);
    expect(groupBox().checked).toBe(false);
    expect(toolBox(OPEN).disabled).toBe(true);
    expect(screen.queryByRole("button", { name: "Open Reverse engineering settings" })).toBeNull();
    await opensSettingsOnTheTab("Set up reverse engineering");
  });

  for (const [what, why] of [
    ["Apple's command line tools are missing", APPLE],
    ["rea needs an update", UPDATE],
  ] as const) {
    it(`when ${what} says so once, keeps open_binary a switch and opens Settings`, async () => {
      ipcMock.listChatTools.mockImplementation(async () => reverse({ check: why, session: why }));
      await openPanel();
      expect(screen.getAllByText(why)).toHaveLength(1);
      expect(toolBox(OPEN).disabled).toBe(false);
      expect(toolBox(CHECK).disabled).toBe(true);
      expect(screen.queryByRole("button", { name: "Set up reverse engineering" })).toBeNull();
      expect(screen.queryByText(DECOMPILER_LINE)).toBeNull();
      await opensSettingsOnTheTab("Open Reverse engineering settings");
    });
  }

  it("without a decompiler says readable code needs one, greys those tools and opens Settings", async () => {
    ipcMock.listChatTools.mockImplementation(async () => reverse({ session: DECOMPILER }));
    await openPanel();
    expect(screen.getAllByText(DECOMPILER_LINE)).toHaveLength(1);
    // The tools' own reason is said by that line, not once per tool.
    expect(screen.queryByText(DECOMPILER)).toBeNull();
    for (const name of SESSION) {
      expect(toolBox(name).disabled).toBe(true);
      expect(screen.getByText(name).className).toContain("text-fg-muted");
    }
    expect(toolBox(OPEN).disabled).toBe(false);
    expect(toolBox(CHECK).disabled).toBe(false);
    expect(screen.getByText(OPEN).className).not.toContain("text-fg-muted");
    await opensSettingsOnTheTab("Open Reverse engineering settings");
  });

  it("switched on saves every reverse tool, also those that can't run yet, so a decompiler installed later finds them on", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true }),
      ...reverse({ session: DECOMPILER }),
    ]);
    await openPanel();
    await act(async () => {
      fireEvent.click(groupBox());
    });
    expect(ipcMock.setChatTools).toHaveBeenCalledWith(CHAT, ["read_file", OPEN, CHECK, ...SESSION]);
  });

  it("is on when every tool that can run is on, and switching it off turns every reverse tool off", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("files", "read_file", { enabledInChat: true }),
      ...reverse({ session: DECOMPILER }, { enabledInChat: true }),
    ]);
    await openPanel();
    expect(groupBox().checked).toBe(true);
    // Saved but not able to run: not counted as tools that are on.
    expect(screen.getByTitle("Tools for this chat").textContent).toContain("Tools (3)");
    await act(async () => {
      fireEvent.click(groupBox());
    });
    expect(ipcMock.setChatTools).toHaveBeenCalledWith(CHAT, ["read_file"]);
  });

  it("keeps a reason that only one tool has on that tool", async () => {
    ipcMock.listChatTools.mockImplementation(async () => [
      ...reverse({}),
      tool("reverse", "list_strings", { available: false, schemaError: CHANGED }),
    ]);
    await openPanel();
    expect(screen.getAllByText(CHANGED)).toHaveLength(1);
    expect(screen.getByText(CHANGED).closest("li")?.textContent).toContain("list_strings");
    expect(screen.queryByRole("button", { name: "Open Reverse engineering settings" })).toBeNull();
    expect(screen.queryByText(DECOMPILER_LINE)).toBeNull();
  });

  it("does not change how other groups switch: a browser tool that can't run is not saved", async () => {
    const NO_BROWSER = "The browser tool needs Chrome, Edge, Brave, Vivaldi or Chromium, and none was found.";
    ipcMock.listChatTools.mockImplementation(async () => [
      tool("computer", "computer_screenshot"),
      tool("computer", "computer_click", { available: false, schemaError: NO_BROWSER }),
    ]);
    await openPanel();
    await act(async () => {
      fireEvent.click(screen.getByText("Computer").closest("label")!.querySelector("input")!);
    });
    expect(ipcMock.setChatTools).toHaveBeenCalledWith(CHAT, ["computer_screenshot"]);
  });
});

describe("ToolsPanel: the header button (header-layout r2)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    ipcMock.listChatTools.mockImplementation(async () => []);
    ipcMock.listApprovedFolders.mockImplementation(async () => []);
    ipcMock.loadedSkills.mockImplementation(async () => []);
  });
  afterEach(cleanup);

  it("says No tools in the narrowest header for a model that can't use tools, and why in the panel", async () => {
    await act(async () => {
      render(<ToolsPanel chatId={CHAT} nCtx={8192} unsupported />);
    });
    const button = screen.getByTitle("Tools for this chat");
    const full = button.querySelector("[data-tools-label='full']") as HTMLElement;
    const short = button.querySelector("[data-tools-label='short']") as HTMLElement;
    // jsdom has no layout: the classes are checked here, the real 720-pixel window in the e2e
    // scenario header-layout (the chat's buttons stay on one row there in Full access).
    expect(full.textContent).toBe("Tools (unavailable)");
    expect(full.className).toBe("@max-md:hidden");
    expect(short.textContent).toBe("No tools");
    expect(short.className).toBe("hidden @max-md:inline");
    await act(async () => {
      fireEvent.click(button);
    });
    expect(screen.getByText(/This model's chat template has no tool-calling format/)).toBeTruthy();
  });

  it("counts the tools that are on otherwise, at every width", async () => {
    await act(async () => {
      render(<ToolsPanel chatId={CHAT} nCtx={8192} />);
    });
    const button = screen.getByTitle("Tools for this chat");
    expect(button.textContent).toBe(" Tools (0)");
    expect(button.querySelector("[data-tools-label]")).toBeNull();
  });
});
