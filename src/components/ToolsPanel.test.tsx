import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ApprovedFolder } from "../lib/types";

// Radix positions the popover with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const CHAT = 7;
let folders: ApprovedFolder[] = [];
let loaded: string[] = [];

const ipcMock = vi.hoisted(() => ({
  listChatTools: vi.fn(async () => []),
  gitInfo: vi.fn(async () => ({ backend: "system", notice: null, tokenHosts: [], identity: { name: "", email: "" } })),
  onMcpChanged: vi.fn(async () => () => undefined),
  listApprovedFolders: vi.fn<(chatId: number | null) => Promise<ApprovedFolder[]>>(async () => []),
  loadedSkills: vi.fn<(chatId: number) => Promise<string[]>>(async () => []),
  approveFolder: vi.fn<(chatId: number | null) => Promise<ApprovedFolder | null>>(async () => null),
  removeApprovedFolder: vi.fn<(id: number) => Promise<void>>(async () => undefined),
  setChatWorkingFolder: vi.fn<(chatId: number, path: string | null) => Promise<void>>(async () => undefined),
  unloadSkill: vi.fn<(chatId: number, name: string) => Promise<void>>(async () => undefined),
  clearChatGrants: vi.fn(async () => undefined),
  getChat: vi.fn(async () => {
    throw new Error("not used");
  }),
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
