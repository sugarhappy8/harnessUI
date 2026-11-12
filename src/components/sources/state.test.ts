import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Source } from "./types";

const apiMock = vi.hoisted(() => ({
  listSources: vi.fn<(chatId: number) => Promise<Source[]>>(),
  onSourcesChanged: vi.fn<(handler: (chatId: number) => void) => Promise<() => void>>(),
}));
vi.mock("./api", () => apiMock);
// A test elsewhere may put anything in place of the app's IPC module. This one has
// nothing in it: the sources must not need it.
vi.mock("../../lib/ipc", () => ({}));

const { resetSources, startSources, useSources } = await import("./state");

function source(n: number): Source {
  return {
    n,
    url: `https://example.org/${n}`,
    title: `Page ${n}`,
    site: "example.org",
    snippet: "",
    excerpt: "Text.",
    contentSha256: null,
    official: false,
    versionNote: null,
    queries: ["q"],
    fetchedAt: null,
    createdAt: 0,
  };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

function insideTheApp() {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
}

describe("the sources a window knows", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetSources();
    apiMock.listSources.mockResolvedValue([]);
    apiMock.onSourcesChanged.mockResolvedValue(() => undefined);
  });
  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("asks nothing of the app outside its window", async () => {
    startSources();
    useSources.getState().ensure(1);
    await settle();
    expect(apiMock.onSourcesChanged).not.toHaveBeenCalled();
    expect(apiMock.listSources).not.toHaveBeenCalled();
    expect(useSources.getState().byChat[1]).toBeUndefined();
  });

  it("does nothing when its modules are only imported", async () => {
    insideTheApp();
    await import("./useCitations");
    await import("./SearchHits");
    await import("../workbench/SourcesTab");
    await settle();
    expect(apiMock.onSourcesChanged).not.toHaveBeenCalled();
    expect(apiMock.listSources).not.toHaveBeenCalled();
  });

  it("reads a chat once, listens once, and reads again when a search stored something", async () => {
    insideTheApp();
    apiMock.listSources.mockResolvedValueOnce([source(1)]);
    useSources.getState().ensure(7);
    useSources.getState().ensure(7);
    await settle();
    expect(apiMock.listSources).toHaveBeenCalledTimes(1);
    expect(apiMock.onSourcesChanged).toHaveBeenCalledTimes(1);
    expect(useSources.getState().byChat[7]?.map((s) => s.n)).toEqual([1]);

    // Another chat is read too, and the one listener serves both.
    useSources.getState().ensure(8);
    await settle();
    expect(apiMock.onSourcesChanged).toHaveBeenCalledTimes(1);

    apiMock.listSources.mockResolvedValueOnce([source(1), source(2)]);
    const changed = apiMock.onSourcesChanged.mock.calls[0]![0];
    changed(7);
    await settle();
    expect(useSources.getState().byChat[7]?.map((s) => s.n)).toEqual([1, 2]);
  });

  it("starts listening when the tab reads a chat, too", async () => {
    insideTheApp();
    await useSources.getState().load(3);
    expect(apiMock.onSourcesChanged).toHaveBeenCalledTimes(1);
    expect(useSources.getState().byChat[3]).toEqual([]);
  });

  it("tries again when listening could not start, without an error left unhandled", async () => {
    insideTheApp();
    const unhandled = vi.fn();
    process.on("unhandledRejection", unhandled);
    try {
      apiMock.onSourcesChanged.mockRejectedValueOnce(new Error("no event system"));
      useSources.getState().ensure(1);
      await settle();
      await settle();
      expect(unhandled).not.toHaveBeenCalled();
      useSources.getState().ensure(2);
      await settle();
      expect(apiMock.onSourcesChanged).toHaveBeenCalledTimes(2);
    } finally {
      process.off("unhandledRejection", unhandled);
    }
  });

  it("leaves a chat unknown when it could not be read, and reads it again when asked", async () => {
    insideTheApp();
    apiMock.listSources.mockRejectedValueOnce(new Error("closed"));
    useSources.getState().ensure(5);
    await settle();
    expect(useSources.getState().byChat[5]).toBeUndefined();
    apiMock.listSources.mockResolvedValueOnce([source(4)]);
    useSources.getState().ensure(5);
    await settle();
    expect(useSources.getState().byChat[5]?.map((s) => s.n)).toEqual([4]);
  });

  it("remembers which source the tab is asked to show, each request anew", () => {
    useSources.getState().requestFocus(1, 2);
    const first = useSources.getState().focus;
    useSources.getState().requestFocus(1, 2);
    const second = useSources.getState().focus;
    expect(first).toMatchObject({ chatId: 1, n: 2 });
    expect(second).toMatchObject({ chatId: 1, n: 2 });
    expect(second?.seq).not.toBe(first?.seq);
  });
});
