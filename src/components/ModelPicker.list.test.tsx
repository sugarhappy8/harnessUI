import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { DownloadGroup, ModelOption } from "../lib/types";

// [0.0.6 item 8] The chat's model list: every installed model, folder hints for two files of
// the same name, and a fresh read when a download finishes.

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const ipcMock = vi.hoisted(() => ({
  isTauri: vi.fn(() => true),
  onDownloadProgress: vi.fn<(h: (g: DownloadGroup) => void) => Promise<() => void>>(),
  listModelOptions: vi.fn<(chatModel?: string | null) => Promise<ModelOption[]>>(),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const { ModelPicker } = await import("./ModelPicker");
const { useApp } = await import("../store/app");

const NAME = "Ornith-1.5-9B-uncensored.Q8_0.gguf";
const COPIES: ModelOption[] = [
  {
    ref: `local:C:\\Users\\Me\\Downloads\\models\\${NAME}`,
    label: NAME,
    source: "local",
    contextLength: null,
    hint: "Downloads\\models",
  },
  {
    ref: `local:C:\\Users\\Me\\AppData\\Roaming\\harnessUI\\models\\${NAME}`,
    label: NAME,
    source: "local",
    contextLength: null,
    hint: "harnessUI\\models",
  },
];

function group(status: DownloadGroup["status"]): DownloadGroup {
  return {
    id: "g1",
    repo: "someone/Model-GGUF",
    label: "Q8_0",
    fileCount: 1,
    bytesDone: 10,
    bytesTotal: 10,
    status,
    error: null,
    bytesPerSecond: null,
    unverified: false,
  };
}

async function openMenu() {
  await act(async () => {
    fireEvent.keyDown(screen.getByRole("button", { name: "Model for this chat" }), { key: "Enter" });
  });
}

describe("ModelPicker: On this computer", () => {
  let handler: ((g: DownloadGroup) => void) | null = null;
  const unlisten = vi.fn();
  beforeEach(() => {
    vi.clearAllMocks();
    handler = null;
    ipcMock.isTauri.mockReturnValue(true);
    ipcMock.onDownloadProgress.mockImplementation(async (h) => {
      handler = h;
      return unlisten;
    });
    ipcMock.listModelOptions.mockResolvedValue([]);
    useApp.setState({ detail: null, modelOptions: [] });
  });
  afterEach(cleanup);

  it("reads the list again when a download finishes, without the menu being opened", async () => {
    const view = await act(async () => render(<ModelPicker />));
    expect(ipcMock.onDownloadProgress).toHaveBeenCalledTimes(1);
    expect(handler).not.toBeNull();
    const fresh: ModelOption = {
      ref: "local:/models/fresh.gguf",
      label: "fresh.gguf",
      source: "local",
      contextLength: null,
    };
    ipcMock.listModelOptions.mockResolvedValue([fresh]);

    // Progress, a pause or a failure adds no model.
    for (const status of ["downloading", "verifying", "paused", "error", "cancelled"] as const) {
      await act(async () => handler?.(group(status)));
    }
    expect(ipcMock.listModelOptions).not.toHaveBeenCalled();

    await act(async () => handler?.(group("done")));
    expect(ipcMock.listModelOptions).toHaveBeenCalledTimes(1);
    expect(useApp.getState().modelOptions).toEqual([fresh]);

    view.unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("a failed read after a download leaves the list as it was", async () => {
    useApp.setState({ modelOptions: COPIES });
    await act(async () => render(<ModelPicker />));
    ipcMock.listModelOptions.mockRejectedValue(new Error("store busy"));
    await act(async () => handler?.(group("done")));
    expect(useApp.getState().modelOptions).toEqual(COPIES);
  });

  it("outside the app nothing listens", async () => {
    ipcMock.isTauri.mockReturnValue(false);
    await act(async () => render(<ModelPicker />));
    expect(ipcMock.onDownloadProgress).not.toHaveBeenCalled();
  });

  it("shows two copies of one name with the end of each folder", async () => {
    // The menu reads the list again as it opens.
    ipcMock.listModelOptions.mockResolvedValue(COPIES);
    await act(async () => render(<ModelPicker />));
    await openMenu();
    const items = screen.getAllByRole("menuitem", { name: new RegExp(`^${NAME.replace(/\./g, "\\.")}`) });
    expect(items.map((i) => i.textContent?.trim())).toEqual([
      `${NAME} · Downloads\\models`,
      `${NAME} · harnessUI\\models`,
    ]);
    // The hint is small text beside the name, and the full path is in the item's tooltip.
    expect(screen.getByText("· Downloads\\models").className).toContain("text-xs");
    expect(items[1]?.getAttribute("title")).toBe(`C:\\Users\\Me\\AppData\\Roaming\\harnessUI\\models\\${NAME}`);
    // Each gear says which copy it is for.
    expect(
      screen.getAllByRole("menuitem", { name: /^Settings for this model/ }).map((g) => g.getAttribute("aria-label")),
    ).toEqual([
      `Settings for this model: ${NAME} · Downloads\\models`,
      `Settings for this model: ${NAME} · harnessUI\\models`,
    ]);
  });

  it("picks the copy that was chosen", async () => {
    const pickModel = vi.fn(async () => undefined);
    ipcMock.listModelOptions.mockResolvedValue(COPIES);
    useApp.setState({ pickModel });
    await act(async () => render(<ModelPicker />));
    await openMenu();
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: `${NAME} · harnessUI\\models` }));
    });
    expect(pickModel).toHaveBeenCalledWith(COPIES[1]?.ref);
  });

  it("shows a name with direction or invisible characters as codes, as text", async () => {
    const tricky: ModelOption = {
      ref: "local:/models/x\u202Efug.gguf",
      label: "x\u202Efug.gguf",
      source: "local",
      contextLength: null,
      hint: "<b>a</b>\u200B/models",
    };
    ipcMock.listModelOptions.mockResolvedValue([tricky]);
    await act(async () => render(<ModelPicker />));
    await openMenu();
    const item = screen.getByRole("menuitem", { name: /^x/ });
    expect(item.textContent).toBe(" x\\u{202E}fug.gguf · <b>a</b>\\u{200B}/models");
    expect(item.querySelector("b")).toBeNull();
    expect(item.getAttribute("title")).toBe("/models/x\\u{202E}fug.gguf");
  });

  it("shows an endpoint's model name with direction or invisible characters as codes", async () => {
    const remote: ModelOption = {
      ref: "provider:1:bad‮ledom",
      label: "bad‮ledom⁦​",
      source: "provider",
      contextLength: null,
    };
    ipcMock.listModelOptions.mockResolvedValue([COPIES[0] as ModelOption, remote]);
    await act(async () => render(<ModelPicker />));
    await openMenu();
    const item = screen.getByRole("menuitem", { name: /^bad/ });
    expect(item.textContent).toBe(" bad\\u{202E}ledom\\u{2066}\\u{200B}");
  });

  it("stops at the window's edge and scrolls when the list is long", async () => {
    const many: ModelOption[] = Array.from({ length: 40 }, (_, i) => ({
      ref: `local:/models/m${i}.gguf`,
      label: `m${i}.gguf`,
      source: "local" as const,
      contextLength: null,
    }));
    ipcMock.listModelOptions.mockResolvedValue(many);
    await act(async () => render(<ModelPicker />));
    await openMenu();
    const menu = screen.getByRole("menu");
    // Radix sets the room left below the button as this variable; the menu never grows past it.
    expect(menu.className).toContain("max-h-[var(--radix-dropdown-menu-content-available-height)]");
    expect(menu.className).toContain("overflow-y-auto");
    expect(screen.getAllByRole("menuitem", { name: /^m\d+\.gguf$/ })).toHaveLength(40);
    expect(screen.getByRole("menuitem", { name: /Find and manage models/ })).toBeTruthy();
  });

  it("names the chat's model without invisible characters in the button", async () => {
    useApp.setState({
      modelOptions: [],
      detail: { chat: { modelRef: "local:/models/y\u202Egguf.exe" } } as never,
    });
    await act(async () => render(<ModelPicker />));
    expect(screen.getByRole("button", { name: "Model for this chat" }).textContent).toBe("y\\u{202E}gguf.exe");
  });

  it("asks for the list with the chat's own model, so a file it opened through a link is listed that way", async () => {
    // The backend lists this chat's own file the way its reference names it (a link, a
    // mapped drive); every other chat, and no chat, gets the path Models → Installed uses.
    const own: ModelOption = { ref: "local:/links/M.gguf", label: "M.gguf", source: "local", contextLength: null };
    ipcMock.listModelOptions.mockResolvedValue([own]);
    useApp.setState({ detail: { chat: { modelRef: own.ref } } as never });
    await act(async () => render(<ModelPicker />));
    ipcMock.listModelOptions.mockClear();
    await openMenu();
    expect(ipcMock.listModelOptions).toHaveBeenCalledTimes(1);
    expect(ipcMock.listModelOptions).toHaveBeenLastCalledWith(own.ref);
    expect(screen.getByRole("menuitem", { name: "M.gguf" }).getAttribute("title")).toBe("/links/M.gguf");
    cleanup();

    useApp.setState({ detail: null });
    await act(async () => render(<ModelPicker />));
    await openMenu();
    expect(ipcMock.listModelOptions).toHaveBeenLastCalledWith(null);
  });

  it("reads the list again when the chat's model changes, not when it stays the same", async () => {
    await act(async () => render(<ModelPicker />));
    expect(ipcMock.listModelOptions).not.toHaveBeenCalled();
    await act(async () => useApp.setState({ detail: { chat: { id: 1, modelRef: "local:/links/M.gguf" } } as never }));
    expect(ipcMock.listModelOptions).toHaveBeenCalledTimes(1);
    expect(ipcMock.listModelOptions).toHaveBeenLastCalledWith("local:/links/M.gguf");
    // Another chat on the same model: the same list.
    await act(async () => useApp.setState({ detail: { chat: { id: 2, modelRef: "local:/links/M.gguf" } } as never }));
    expect(ipcMock.listModelOptions).toHaveBeenCalledTimes(1);
    await act(async () => useApp.setState({ detail: { chat: { id: 3, modelRef: "local:/models/M.gguf" } } as never }));
    expect(ipcMock.listModelOptions).toHaveBeenCalledTimes(2);
    expect(ipcMock.listModelOptions).toHaveBeenLastCalledWith("local:/models/M.gguf");
  });

  it("keeps the newest answer when two reads of the list cross", async () => {
    const first: ModelOption[] = [{ ref: "local:/links/M.gguf", label: "M.gguf", source: "local", contextLength: null }];
    const second: ModelOption[] = [{ ref: "local:/models/M.gguf", label: "M.gguf", source: "local", contextLength: null }];
    let answerFirst: (v: ModelOption[]) => void = () => undefined;
    ipcMock.listModelOptions
      .mockImplementationOnce(() => new Promise((resolve) => (answerFirst = resolve)))
      .mockResolvedValueOnce(second);
    const refresh = useApp.getState().refreshModelOptions;
    const a = refresh();
    await act(async () => refresh());
    expect(useApp.getState().modelOptions).toEqual(second);
    await act(async () => {
      answerFirst(first);
      await a;
    });
    expect(useApp.getState().modelOptions).toEqual(second);
  });
});
