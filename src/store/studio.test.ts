// Studio's store (0.1.0 Task B1): which view is shown, the runs still working as the backend
// reports them, and the notice for a video that finished while the chat was shown.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LOCAL, LOCAL_RUN, MODELS, RUN } from "../components/studio/fixtures/models";
import type { Run, StudioEndpoint, StudioPrefs } from "../components/studio/types";

vi.mock("../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("../components/studio/api");

const api = await import("../components/studio/api");
const { followStudio, runningCount, useStudio } = await import("./studio");

const INITIAL = useStudio.getState();

function run(patch: Partial<Run>): Run {
  return { ...RUN, ...patch };
}

async function flush() {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

describe("the view", () => {
  it("opens Studio on the tab asked for, else on the last one, and goes back to the chat", () => {
    useStudio.getState().showStudio("video");
    expect(useStudio.getState()).toMatchObject({ view: "studio", tab: "video" });
    useStudio.getState().showChat();
    useStudio.getState().showStudio();
    expect(useStudio.getState()).toMatchObject({ view: "studio", tab: "video" });
    useStudio.getState().showChat();
    expect(useStudio.getState().view).toBe("chat");
  });

  it("a tab asked for closes Get local models; opened without one, Studio keeps it", () => {
    useStudio.getState().showStudio();
    useStudio.getState().showLocalModels(true);
    useStudio.getState().showChat();
    // Ctrl/⌘ G, the sidebar: Studio as it was left.
    useStudio.getState().showStudio();
    expect(useStudio.getState()).toMatchObject({ view: "studio", localView: true });
    useStudio.getState().showChat();
    // Open in Studio, a finished video's Open: the tab's cards, not Get local models.
    useStudio.getState().showStudio("video");
    expect(useStudio.getState()).toMatchObject({ view: "studio", tab: "video", localView: false });
  });

  it("showing the chat closes the viewer", () => {
    useStudio.getState().showStudio();
    useStudio.getState().openViewer("o1");
    useStudio.getState().showChat();
    expect(useStudio.getState()).toMatchObject({ view: "chat", viewer: null });
  });
});

describe("progress", () => {
  it("keeps each run's newest state by its id", () => {
    useStudio.getState().applyProgress(run({ id: "a", status: "sending", seq: 1 }));
    useStudio.getState().applyProgress(run({ id: "b", status: "queued", seq: 2 }));
    useStudio.getState().applyProgress(run({ id: "a", status: "done", seq: 3 }));
    const live = useStudio.getState().live;
    expect(Object.keys(live).sort()).toEqual(["a", "b"]);
    expect(live.a?.status).toBe("done");
  });

  it("an older copy of a run that comes later is dropped, wherever it comes from", () => {
    useStudio.getState().applyProgress(run({ id: "a", status: "running", seq: 7 }));
    // A command's answer read before the event was sent, arriving after it.
    useStudio.getState().applyProgress(run({ id: "a", status: "queued", seq: 5 }));
    expect(useStudio.getState().live.a?.status).toBe("running");
    expect(useStudio.getState().recentRuns.find((r) => r.id === "a")?.status).toBe("running");
    // The same copy twice changes nothing either.
    useStudio.getState().applyProgress(run({ id: "a", status: "queued", seq: 7 }));
    expect(useStudio.getState().live.a?.status).toBe("running");
    useStudio.getState().applyProgress(run({ id: "a", status: "done", seq: 8 }));
    expect(useStudio.getState().live.a?.status).toBe("done");
  });

  it("a video that finishes while the chat is shown raises the ready notice", () => {
    useStudio.getState().applyProgress(run({ id: "v", kind: "video", status: "done" }));
    expect(useStudio.getState().readyNotice).toBe("v");
  });

  it("a finished picture, or a video finished in Studio, raises no notice", () => {
    useStudio.getState().applyProgress(run({ id: "p", kind: "image", status: "done" }));
    useStudio.getState().applyProgress(run({ id: "v", kind: "video", status: "failed" }));
    expect(useStudio.getState().readyNotice).toBeNull();
    useStudio.getState().showStudio();
    useStudio.getState().applyProgress(run({ id: "w", kind: "video", status: "done" }));
    expect(useStudio.getState().readyNotice).toBeNull();
  });

  it("raises the notice once, not again for the same finished video", () => {
    useStudio.getState().applyProgress(run({ id: "v", kind: "video", status: "done" }));
    useStudio.setState({ readyNotice: null });
    useStudio.getState().applyProgress(run({ id: "v", kind: "video", status: "done" }));
    expect(useStudio.getState().readyNotice).toBeNull();
  });

  it("runningCount ignores stopped and stale runs and counts a local run", () => {
    const live = {
      a: run({ status: "sending" }),
      b: run({ status: "waiting", watching: true }),
      c: run({ status: "waiting", watching: false, stale: true }),
      d: run({ status: "cancelled" }),
      e: run({ status: "done" }),
      f: { ...LOCAL_RUN, id: "f", status: "running" as const },
    };
    expect(runningCount({ ...useStudio.getState(), live })).toBe(3);
  });
});

describe("following the backend", () => {
  type Handler = (run: Run) => void;
  let heard: Handler[] = [];
  const unlisten = vi.fn();
  afterEach(() => {
    heard = [];
  });

  function listen() {
    vi.mocked(api.onStudioProgress).mockImplementation(async (handler: Handler) => {
      heard.push(handler);
      return unlisten;
    });
  }

  it("applies each studio-progress and reads the runs still working once", async () => {
    listen();
    vi.mocked(api.studioLive).mockResolvedValue([run({ id: "a", status: "waiting", watching: true, seq: 1 })]);
    const stop = followStudio();
    await flush();
    expect(api.studioLive).toHaveBeenCalledTimes(1);
    expect(useStudio.getState().live.a?.status).toBe("waiting");
    heard[0]!(run({ id: "a", status: "downloading", seq: 2 }));
    expect(useStudio.getState().live.a?.status).toBe("downloading");
    stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("a failed read of the runs still working leaves a warning", async () => {
    listen();
    const failure = new Error("database is locked");
    vi.mocked(api.studioLive).mockRejectedValue(failure);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      const stop = followStudio();
      await flush();
      expect(warn).toHaveBeenCalledTimes(1);
      expect(warn).toHaveBeenCalledWith(expect.stringContaining("Studio"), failure);
      stop();
    } finally {
      warn.mockRestore();
    }
  });

  it("keeps what an event said over an older answer of the read at start", async () => {
    listen();
    let answer: (runs: Run[]) => void = () => undefined;
    vi.mocked(api.studioLive).mockImplementation(() => new Promise((resolve) => (answer = resolve)));
    const stop = followStudio();
    await flush();
    heard[0]!(run({ id: "a", status: "downloading", seq: 4 }));
    answer([run({ id: "a", status: "sending", seq: 3 }), run({ id: "b", status: "queued", seq: 3 })]);
    await flush();
    expect(useStudio.getState().live.a?.status).toBe("downloading");
    expect(useStudio.getState().live.b?.status).toBe("queued");
    stop();
  });
});

describe("an installed local model deleted", () => {
  it("is off the list at once, and a failed read afterwards only leaves a trace (the files are gone)", async () => {
    const id = LOCAL.installed[0]!.id;
    useStudio.setState({ local: LOCAL });
    vi.mocked(api.studioLocalDelete).mockResolvedValue(undefined);
    vi.mocked(api.studioLocalModels).mockRejectedValue(new Error("database is locked"));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      await expect(useStudio.getState().deleteLocal(id)).resolves.toBeUndefined();
      expect(useStudio.getState().local?.installed.map((m) => m.id)).not.toContain(id);
      expect(warn).toHaveBeenCalledWith(expect.stringContaining("Studio"), expect.any(Error));
    } finally {
      warn.mockRestore();
    }
  });
});

describe("why a press was refused, and the model it was for", () => {
  const endpoint: StudioEndpoint = {
    providerId: 1,
    name: "OpenRouter",
    company: "openrouter",
    host: "openrouter.ai",
    cloud: true,
    hasKey: true,
    fetchedAt: 1794900000000,
    models: MODELS,
  };
  const limit = { code: "studio_daily_limit", message: "Today's limit is reached.", nextStep: "Raise it.", details: "" };
  const refused = () => useStudio.getState().refusal.image;

  beforeEach(() => {
    vi.mocked(api.studioModelPrice).mockImplementation(async (_, id) => MODELS.find((m) => m.id === id)!);
    useStudio.setState({ local: LOCAL, endpoints: [endpoint] });
    useStudio.getState().chooseModel("image", { endpoint: 1 }, "openai/gpt-image-2.6-flare");
    useStudio.setState({ refusal: { image: limit, video: null } });
  });

  it("goes when another model is picked (a local one included)", () => {
    useStudio.getState().chooseModel("image", "thisComputer", LOCAL.installed[0]!.model.id);
    expect(refused()).toBeNull();
  });

  it("goes when a run is reused, or the tab starts again on its first model", () => {
    expect(useStudio.getState().reuse(RUN, true)).toBeNull();
    expect(refused()).toBeNull();
    useStudio.setState({ refusal: { image: limit, video: null } });
    useStudio.getState().selectInitialModel("image");
    expect(refused()).toBeNull();
  });

  it("stays while the same model is picked again", () => {
    useStudio.getState().chooseModel("image", { endpoint: 1 }, "openai/gpt-image-2.6-flare");
    expect(refused()).toEqual(limit);
  });
});

// A command's answer is read before the run's task goes on, so the task's own event can come
// first: the copy the backend numbered higher stays, whichever comes last.
describe("a command's answer and a newer event", () => {
  /** A command whose answer (`answer`, numbered 5) waits until the event (numbered 6) is in. */
  async function eventFirst(command: () => Promise<unknown>, mock: { mockImplementation(f: () => Promise<Run>): unknown }) {
    let answer: (run: Run) => void = () => undefined;
    mock.mockImplementation(() => new Promise<Run>((resolve) => (answer = resolve)));
    const pressed = command();
    await flush();
    useStudio.getState().applyProgress(run({ id: "a", status: "running", seq: 6 }));
    answer(run({ id: "a", status: "queued", seq: 5 }));
    await pressed;
    expect(useStudio.getState().live.a?.status).toBe("running");
  }

  it("Cancel", async () => {
    await eventFirst(() => useStudio.getState().cancel("a"), vi.mocked(api.studioCancel));
  });

  it("Get it anyway", async () => {
    await eventFirst(() => useStudio.getState().fetchAnyway("a"), vi.mocked(api.studioFetchAnyway));
  });

  it("the answer to Unload the chat model?", async () => {
    await eventFirst(() => useStudio.getState().answerUnload("a", true), vi.mocked(api.studioAnswerUnload));
  });

  it("a press of Create, Again or Run again", async () => {
    vi.mocked(api.studioUsage).mockResolvedValue({ spentTodayUsd: 0, pendingUsd: 0, limitUsd: 10, folder: "", bytes: 0, pictures: 0, videos: 0 });
    vi.mocked(api.studioGetPrefs).mockResolvedValue({ lastModel: {} } as unknown as StudioPrefs);
    vi.mocked(api.studioSetPrefs).mockResolvedValue({ lastModel: {} } as unknown as StudioPrefs);
    await eventFirst(() => useStudio.getState().again(run({ id: "old", status: "done" })), vi.mocked(api.studioStart));
  });
});

// Task B2: what Studio has saved, read once when it opens, and the drafts it starts with.
describe("what is saved", () => {
  const openrouter: StudioEndpoint = {
    providerId: 1,
    name: "OpenRouter",
    company: "openrouter",
    host: "openrouter.ai",
    cloud: true,
    hasKey: true,
    fetchedAt: 1794900000000,
    models: MODELS,
  };
  const prefs = { lastModel: {}, suggestionDismissed: true } as unknown as StudioPrefs;

  function saved(local = LOCAL, endpoints: StudioEndpoint[] = []) {
    vi.mocked(api.studioModels).mockResolvedValue({ endpoints });
    vi.mocked(api.studioLocalModels).mockResolvedValue(local);
    vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
    vi.mocked(api.studioLive).mockResolvedValue([]);
    vi.mocked(api.studioGetPrefs).mockResolvedValue(prefs);
    vi.mocked(api.studioUsage).mockResolvedValue({ spentTodayUsd: 0, pendingUsd: 0, limitUsd: 10, folder: "", bytes: 0, pictures: 0, videos: 0 });
  }

  it("reads the Studio engine's state again into the local models, and a failed read leaves them", async () => {
    useStudio.setState({ local: LOCAL });
    const removed = { ...LOCAL.engine, state: "notInstalled" as const, downloadBytes: 30_303_000 };
    vi.mocked(api.studioEngineStatus).mockResolvedValue(removed);
    await useStudio.getState().refreshEngine();
    expect(useStudio.getState().local).toEqual({ ...LOCAL, engine: removed });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    vi.mocked(api.studioEngineStatus).mockRejectedValue(new Error("gone"));
    await useStudio.getState().refreshEngine();
    expect(useStudio.getState().local?.engine).toEqual(removed);
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
    // Before the local models were read there is nothing to put it in.
    useStudio.setState({ local: null });
    vi.mocked(api.studioEngineStatus).mockResolvedValue(removed);
    await useStudio.getState().refreshEngine();
    expect(useStudio.getState().local).toBeNull();
  });

  it("reads each saved thing once, even when asked twice at once", async () => {
    saved();
    await Promise.all([useStudio.getState().load(), useStudio.getState().load()]);
    for (const read of [api.studioModels, api.studioLocalModels, api.studioRuns, api.studioLive, api.studioGetPrefs, api.studioUsage]) {
      expect(read).toHaveBeenCalledTimes(1);
    }
    expect(useStudio.getState().loaded).toBe(true);
  });

  it("the runs still working are kept as live", async () => {
    saved();
    vi.mocked(api.studioLive).mockResolvedValue([run({ id: "a", status: "running" })]);
    await useStudio.getState().load();
    expect(useStudio.getState().live.a?.status).toBe("running");
  });

  it("an event heard while the saved things are read is not overwritten by the older read of the runs working", async () => {
    saved();
    vi.mocked(api.studioLive).mockResolvedValue([run({ id: "a", status: "queued", seq: 4 })]);
    // The local models take longer to read than the runs still working.
    let localRead: (local: typeof LOCAL) => void = () => undefined;
    vi.mocked(api.studioLocalModels).mockImplementation(() => new Promise((resolve) => (localRead = resolve)));
    const loading = useStudio.getState().load();
    await flush();
    useStudio.getState().applyProgress(run({ id: "a", status: "running", seq: 6 }));
    localRead(LOCAL);
    await loading;
    expect(useStudio.getState().live.a?.status).toBe("running");
  });

  it("a model got later fills a tab that had none, and a chosen model stays", async () => {
    saved({ ...LOCAL, installed: [] });
    await useStudio.getState().load();
    expect(useStudio.getState().drafts.image.model).toBeNull();
    useStudio.getState().setPrompt("image", "lighthouse");
    vi.mocked(api.studioLocalModels).mockResolvedValue(LOCAL);
    await useStudio.getState().refreshLocal();
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: "flux2-klein-4b-q4", prompt: "lighthouse" });
    expect(useStudio.getState().drafts.video).toMatchObject({ target: "thisComputer", model: "wan22-ti2v-5b-fast-q6" });
    // A local video model starts at 2 s and 480p (its Simple fields' starting values only).
    expect(useStudio.getState().drafts.video.values).toEqual({ resolution: "480p", duration: 2 });
    vi.mocked(api.studioRefreshModels).mockResolvedValue({ endpoints: [openrouter] });
    await useStudio.getState().refreshModels(1);
    expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
    expect(useStudio.getState().endpoints).toEqual([openrouter]);
    expect(useStudio.getState().drafts.image).toMatchObject({ target: "thisComputer", model: "flux2-klein-4b-q4" });
  });

  it("reading again takes the lists, local models, preferences and spending, not the gallery's pages", async () => {
    saved({ ...LOCAL, installed: [] });
    await useStudio.getState().load();
    expect(useStudio.getState().drafts.image.model).toBeNull();
    useStudio.setState({ runs: [RUN, LOCAL_RUN] }); // more pages read since
    useStudio.getState().setPrompt("image", "lighthouse");
    // OpenRouter was added in Settings, and something was spent in the meantime.
    vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [openrouter] });
    vi.mocked(api.studioUsage).mockResolvedValue({ spentTodayUsd: 0.5, pendingUsd: 0, limitUsd: 10, folder: "", bytes: 0, pictures: 0, videos: 0 });
    await useStudio.getState().reread();
    for (const read of [api.studioModels, api.studioLocalModels, api.studioGetPrefs, api.studioUsage]) {
      expect(read).toHaveBeenCalledTimes(2);
    }
    expect(api.studioRuns).toHaveBeenCalledTimes(1);
    expect(api.studioLive).toHaveBeenCalledTimes(1);
    expect(api.studioRefreshModels).not.toHaveBeenCalled();
    expect(useStudio.getState()).toMatchObject({ endpoints: [openrouter], runs: [RUN, LOCAL_RUN], usage: { spentTodayUsd: 0.5 } });
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare", prompt: "lighthouse" });
    // A model chosen stays when a local one turns up later.
    vi.mocked(api.studioLocalModels).mockResolvedValue(LOCAL);
    await useStudio.getState().reread();
    expect(useStudio.getState().local).toEqual(LOCAL);
    expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" });
  });

  it("a search or Starred reads the gallery's first page again with the filter", async () => {
    saved();
    await useStudio.getState().load();
    vi.mocked(api.studioRuns).mockResolvedValue({ runs: [RUN], next: null });
    await useStudio.getState().setFilter({ text: "fox" });
    await useStudio.getState().setFilter({ starred: true });
    expect(api.studioRuns).toHaveBeenLastCalledWith({ text: "fox", starred: true }, null, expect.any(Number));
    expect(useStudio.getState().runs).toEqual([RUN]);
  });

  it("an older answer does not replace a newer filter's runs", async () => {
    saved();
    await useStudio.getState().load();
    let answerFirst: (page: { runs: Run[]; next: null }) => void = () => undefined;
    vi.mocked(api.studioRuns).mockImplementationOnce(() => new Promise((resolve) => (answerFirst = resolve)));
    const first = useStudio.getState().setFilter({ text: "f" });
    vi.mocked(api.studioRuns).mockResolvedValueOnce({ runs: [LOCAL_RUN], next: null });
    await useStudio.getState().setFilter({ text: "fox" });
    answerFirst({ runs: [RUN], next: null });
    await first;
    expect(useStudio.getState().runs).toEqual([LOCAL_RUN]);
  });

  it("a filter whose first page could not be read is put back, so the runs and the filter shown agree", async () => {
    saved();
    vi.mocked(api.studioRuns).mockResolvedValue({ runs: [RUN], next: { createdAt: 1, id: "x" } });
    await useStudio.getState().load();
    const before = useStudio.getState().filter;
    vi.mocked(api.studioRuns).mockRejectedValueOnce(new Error("database is locked"));
    await expect(useStudio.getState().setFilter({ starred: true })).rejects.toThrow("database is locked");
    expect(useStudio.getState().filter).toBe(before);
    expect(useStudio.getState().runs).toEqual([RUN]);
    // Load more then reads the next page of the filter the runs belong to.
    vi.mocked(api.studioRuns).mockResolvedValueOnce({ runs: [], next: null });
    await useStudio.getState().loadMore();
    expect(api.studioRuns).toHaveBeenLastCalledWith(before, { createdAt: 1, id: "x" }, 60);
  });

  it("the first page read when Studio opens does not replace a newer filter's runs", async () => {
    saved();
    let answerLoad: (page: { runs: Run[]; next: null }) => void = () => undefined;
    vi.mocked(api.studioRuns).mockImplementationOnce(() => new Promise((resolve) => (answerLoad = resolve)));
    const loading = useStudio.getState().load();
    vi.mocked(api.studioRuns).mockResolvedValueOnce({ runs: [LOCAL_RUN], next: null });
    await useStudio.getState().setFilter({ text: "fox" });
    answerLoad({ runs: [RUN], next: null }); // the unfiltered page, answered last
    await loading;
    expect(useStudio.getState().runs).toEqual([LOCAL_RUN]);
    expect(useStudio.getState()).toMatchObject({ loaded: true, filter: { text: "fox" } });
  });

  it("the newest runs are kept whatever the gallery's filter, with the runs heard of since", async () => {
    saved();
    vi.mocked(api.studioRuns).mockImplementation(async (filter) => ({ runs: filter.kind ? [] : [RUN], next: null }));
    // Opened on an unfiltered gallery, its first page is the newest runs: one read.
    await useStudio.getState().load();
    expect(api.studioRuns).toHaveBeenCalledTimes(1);
    expect(useStudio.getState().recentRuns).toEqual([RUN]);
    // A filter changes the gallery, not the newest runs; a run heard of comes first.
    await useStudio.getState().setFilter({ kind: "video" });
    useStudio.getState().applyProgress(LOCAL_RUN);
    expect(useStudio.getState().runs).toEqual([]);
    expect(useStudio.getState().recentRuns).toEqual([LOCAL_RUN, RUN]);
    // Read again on a filtered gallery: the newest runs are read without the filter.
    useStudio.setState({ recentRuns: [] });
    vi.mocked(api.studioRuns).mockClear();
    await useStudio.getState().load();
    expect(api.studioRuns).toHaveBeenCalledTimes(2);
    expect(api.studioRuns).toHaveBeenCalledWith({ kind: "video" }, null, 60);
    expect(api.studioRuns).toHaveBeenCalledWith({}, null, 50);
    expect(useStudio.getState()).toMatchObject({ runs: [], recentRuns: [RUN] });
  });

  it("changing the preferences sends only what changed", async () => {
    saved();
    vi.mocked(api.studioSetPrefs).mockResolvedValue({ ...prefs, suggestionDismissed: true });
    await useStudio.getState().setPrefs({ suggestionDismissed: true });
    expect(api.studioSetPrefs).toHaveBeenCalledWith({ suggestionDismissed: true });
    expect(useStudio.getState().prefs?.suggestionDismissed).toBe(true);
  });

  describe("lists read and fetched at the same time", () => {
    const unlisted: StudioEndpoint = { ...openrouter, fetchedAt: null, models: [] };
    const openai: StudioEndpoint = {
      providerId: 2,
      name: "OpenAI",
      company: "openai",
      host: "api.openai.com",
      cloud: true,
      hasKey: true,
      fetchedAt: 1794800000000,
      models: [],
    };

    it("a read of what is saved does not put back the lists a fetch got while it was on its way", async () => {
      saved({ ...LOCAL, installed: [] }, [unlisted]);
      await useStudio.getState().load();
      let answerRead: (models: { endpoints: StudioEndpoint[] }) => void = () => undefined;
      vi.mocked(api.studioModels).mockImplementationOnce(() => new Promise((resolve) => (answerRead = resolve)));
      const reading = useStudio.getState().reread(); // Settings closed
      vi.mocked(api.studioRefreshModels).mockResolvedValueOnce({ endpoints: [openrouter] });
      await useStudio.getState().refreshModels(1); // *Get the list*, answered first
      answerRead({ endpoints: [{ ...unlisted, name: "My OpenRouter" }] }); // read before the lists were saved
      await reading;
      // The endpoint as read (its new name), with the lists fetched.
      expect(useStudio.getState().endpoints).toEqual([{ ...openrouter, name: "My OpenRouter" }]);
      expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" });
      // A read started after the fetch takes what is saved, as it is.
      vi.mocked(api.studioModels).mockResolvedValueOnce({ endpoints: [unlisted] }); // the endpoint's address changed
      await useStudio.getState().reread();
      expect(useStudio.getState().endpoints).toEqual([unlisted]);
    });

    it("so does the first read, when Studio opens", async () => {
      saved({ ...LOCAL, installed: [] }, [unlisted]);
      let answerRead: (models: { endpoints: StudioEndpoint[] }) => void = () => undefined;
      vi.mocked(api.studioModels).mockImplementationOnce(() => new Promise((resolve) => (answerRead = resolve)));
      const loading = useStudio.getState().load();
      vi.mocked(api.studioRefreshModels).mockResolvedValueOnce({ endpoints: [openrouter] });
      await useStudio.getState().refreshModels(1);
      answerRead({ endpoints: [unlisted] });
      await loading;
      expect(useStudio.getState().endpoints).toEqual([openrouter]);
    });

    it("an older fetch's answer does not put back another endpoint's lists fetched since", async () => {
      saved(LOCAL, [unlisted, openai]);
      await useStudio.getState().load();
      let answerFirst: (models: { endpoints: StudioEndpoint[] }) => void = () => undefined;
      vi.mocked(api.studioRefreshModels).mockImplementationOnce(() => new Promise((resolve) => (answerFirst = resolve)));
      const first = useStudio.getState().refreshModels(1);
      const openaiNow: StudioEndpoint = { ...openai, fetchedAt: 1794950000000, models: [{ ...MODELS[0]!, id: "gpt-image-2.6-flare" }] };
      vi.mocked(api.studioRefreshModels).mockResolvedValueOnce({ endpoints: [unlisted, openaiNow] });
      await useStudio.getState().refreshModels(2);
      answerFirst({ endpoints: [openrouter, openai] }); // OpenAI's lists as they were when it started
      await first;
      expect(useStudio.getState().endpoints).toEqual([openrouter, openaiNow]);
    });
  });

  describe("an endpoint removed in Settings", () => {
    const openai: StudioEndpoint = {
      providerId: 2,
      name: "OpenAI",
      company: "openai",
      host: "api.openai.com",
      cloud: true,
      hasKey: true,
      fetchedAt: 1794900000000,
      models: [{ ...MODELS[0]!, id: "gpt-image-2.6-flare" }],
    };

    it("a tab set to it gets its first model again, and a tab set elsewhere stays", async () => {
      saved({ ...LOCAL, installed: [] }, [openrouter, openai]);
      await useStudio.getState().load();
      useStudio.getState().chooseModel("image", { endpoint: 2 }, "gpt-image-2.6-flare");
      useStudio.getState().setPrompt("video", "waves");
      expect(useStudio.getState().drafts.video).toMatchObject({ target: { endpoint: 1 }, model: "google/veo-3.1" });
      const image = useStudio.getState().drafts.image;
      vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [openai] }); // OpenRouter removed
      await useStudio.getState().reread();
      expect(useStudio.getState().drafts.image).toBe(image);
      // OpenAI makes no video, and no local video model is installed: no model, the prompt kept.
      expect(useStudio.getState().drafts.video).toMatchObject({ target: null, model: null, prompt: "waves" });
    });

    it("also when an answer to Update model lists no longer has it", async () => {
      saved({ ...LOCAL, installed: [] }, [openrouter, openai]);
      await useStudio.getState().load();
      expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" });
      vi.mocked(api.studioRefreshModels).mockResolvedValueOnce({ endpoints: [openai] });
      await useStudio.getState().refreshModels(2);
      expect(useStudio.getState().drafts.image).toMatchObject({ target: { endpoint: 2 }, model: "gpt-image-2.6-flare" });
    });

    it("never moves a tab between this computer and the cloud otherwise", async () => {
      saved(LOCAL, [openrouter]);
      await useStudio.getState().load();
      const image = useStudio.getState().drafts.image;
      expect(image).toMatchObject({ target: "thisComputer", model: "flux2-klein-4b-q4" });
      vi.mocked(api.studioLocalModels).mockResolvedValue({ ...LOCAL, installed: [] }); // deleted
      await useStudio.getState().reread();
      expect(useStudio.getState().drafts.image).toBe(image);
    });
  });
});

describe("the gallery's results (Task B9)", () => {
  it("a star joins the result wherever the store holds it", async () => {
    useStudio.setState({ runs: [RUN], recentRuns: [RUN], live: { [RUN.id]: RUN } });
    vi.mocked(api.studioStar).mockResolvedValue({ ...RUN.outputs[1]!, starred: true });
    await useStudio.getState().star("o2", true);
    expect(api.studioStar).toHaveBeenCalledWith("o2", true);
    const s = useStudio.getState();
    for (const held of [s.runs[0]!, s.recentRuns[0]!, s.live[RUN.id]!]) {
      expect(held.outputs.map((o) => o.starred)).toEqual([false, true, false, false]);
    }
  });

  it("deleted results leave the store, and a finished run left without results goes with them", async () => {
    const failed = run({ id: "failed", status: "failed", outputs: [{ ...RUN.outputs[0]!, id: "f1", runId: "failed" }] });
    useStudio.setState({ runs: [RUN, LOCAL_RUN, failed], recentRuns: [RUN, LOCAL_RUN], live: { [RUN.id]: RUN } });
    vi.mocked(api.studioDelete).mockResolvedValue(undefined);
    vi.mocked(api.studioUsage).mockResolvedValue({
      spentTodayUsd: 0.04,
      pendingUsd: 0,
      limitUsd: 10,
      folder: "/g",
      bytes: 0,
      pictures: 0,
      videos: 0,
    });
    await useStudio.getState().remove(["o1", "o2", "o3", "o4", "l1", "f1"]);
    expect(api.studioDelete).toHaveBeenCalledWith(["o1", "o2", "o3", "o4", "l1", "f1"]);
    const s = useStudio.getState();
    expect(s.runs.map((r) => r.id)).toEqual(["local-run", "failed"]);
    expect(s.runs[0]!.outputs.map((o) => o.id)).toEqual(["l2"]);
    // A run that ended without finishing keeps its card (as the backend keeps it).
    expect(s.runs[1]!.outputs).toEqual([]);
    expect(s.recentRuns.map((r) => r.id)).toEqual(["local-run"]);
    expect(s.live).toEqual({});
    await flush();
    // What the gallery holds is read again; today's spending stays as the backend says.
    expect(api.studioUsage).toHaveBeenCalled();
  });

  it("a run is judged on its newest copy: an older page copy without results does not bring its card back", async () => {
    // The gallery's page read the video while it was waiting; it finished since (`live`).
    const waiting = run({ id: "v", kind: "video", status: "waiting", outputs: [], seq: 3 });
    const done = run({ id: "v", kind: "video", status: "done", outputs: [{ ...RUN.outputs[0]!, id: "v1", runId: "v" }], seq: 9 });
    const other = run({ id: "kept", seq: 2 });
    useStudio.setState({ runs: [waiting, other], recentRuns: [waiting, other], live: { v: done } });
    vi.mocked(api.studioDelete).mockResolvedValue(undefined);
    vi.mocked(api.studioUsage).mockResolvedValue({ spentTodayUsd: 0, pendingUsd: 0, limitUsd: 10, folder: "/g", bytes: 0, pictures: 0, videos: 0 });
    await useStudio.getState().remove(["v1"]);
    const s = useStudio.getState();
    expect(s.runs.map((r) => r.id)).toEqual(["kept"]);
    expect(s.recentRuns.map((r) => r.id)).toEqual(["kept"]);
    expect(s.live).toEqual({});
  });

  it("a run kept after a delete shows its newest copy, stripped, in every place", async () => {
    const outputs = RUN.outputs.slice(0, 2).map((o, i) => ({ ...o, id: `r${i}`, runId: "r" }));
    const older = run({ id: "r", status: "running", outputs: [], seq: 1 });
    const newest = run({ id: "r", status: "done", outputs, seq: 5 });
    useStudio.setState({ runs: [older], recentRuns: [older], live: { r: newest } });
    vi.mocked(api.studioDelete).mockResolvedValue(undefined);
    vi.mocked(api.studioUsage).mockResolvedValue({ spentTodayUsd: 0, pendingUsd: 0, limitUsd: 10, folder: "/g", bytes: 0, pictures: 0, videos: 0 });
    await useStudio.getState().remove(["r0"]);
    const s = useStudio.getState();
    for (const copy of [s.runs[0], s.recentRuns[0], s.live.r]) {
      expect(copy).toMatchObject({ status: "done", seq: 5 });
      expect(copy?.outputs.map((o) => o.id)).toEqual(["r1"]);
    }
  });

  it("Delete stopped and failed runs takes every run that ended without finishing off the cards", async () => {
    const ended = (id: string, status: Run["status"]) => run({ id, status, outputs: [] });
    const failed = ended("failed", "failed");
    const blocked = ended("blocked", "blocked");
    const cancelled = ended("cancelled", "cancelled");
    const interrupted = ended("interrupted", "interrupted");
    const waiting = run({ id: "waiting", status: "waiting", outputs: [] });
    // Each run is judged as it stands now: a video read waiting that has failed since goes, and
    // one read failed that *Get it anyway* took back to waiting stays.
    const failedSince = ended("failed-since", "waiting");
    const takenBack = ended("taken-back", "failed");
    useStudio.setState({
      runs: [RUN, failed, blocked, failedSince, takenBack, LOCAL_RUN],
      recentRuns: [RUN, cancelled],
      live: {
        [interrupted.id]: interrupted,
        [waiting.id]: waiting,
        [RUN.id]: RUN,
        [failedSince.id]: { ...failedSince, status: "failed" },
        [takenBack.id]: { ...takenBack, status: "waiting" },
      },
    });
    vi.mocked(api.studioDeleteUnfinished).mockResolvedValue(4);
    vi.mocked(api.studioUsage).mockResolvedValue({
      spentTodayUsd: 0.04,
      pendingUsd: 0,
      limitUsd: 10,
      folder: "/g",
      bytes: 0,
      pictures: 0,
      videos: 0,
    });
    await expect(useStudio.getState().removeUnfinished()).resolves.toBe(4);
    expect(api.studioDeleteUnfinished).toHaveBeenCalledTimes(1);
    const s = useStudio.getState();
    expect(s.runs.map((r) => r.id)).toEqual([RUN.id, "taken-back", LOCAL_RUN.id]);
    expect(s.recentRuns.map((r) => r.id)).toEqual([RUN.id]);
    expect(Object.keys(s.live).sort()).toEqual([RUN.id, "taken-back", "waiting"].sort());
    await flush();
    // What the gallery holds is read again.
    expect(api.studioUsage).toHaveBeenCalled();
  });

  it("a Delete stopped and failed runs that fails keeps the cards", async () => {
    const failed = run({ id: "failed", status: "failed", outputs: [] });
    useStudio.setState({ runs: [failed] });
    vi.mocked(api.studioDeleteUnfinished).mockRejectedValue({ code: "internal", message: "no", nextStep: "", details: "" });
    await expect(useStudio.getState().removeUnfinished()).rejects.toMatchObject({ code: "internal" });
    expect(useStudio.getState().runs).toEqual([failed]);
  });

  it("a delete that fails changes nothing", async () => {
    useStudio.setState({ runs: [RUN] });
    vi.mocked(api.studioDelete).mockRejectedValue({ code: "internal", message: "no", nextStep: "", details: "" });
    await expect(useStudio.getState().remove(["o1"])).rejects.toMatchObject({ code: "internal" });
    expect(useStudio.getState().runs).toEqual([RUN]);
  });

  it("Load more reads the page after the last one for the same filter, once at a time", async () => {
    const next = { createdAt: RUN.createdAt, id: RUN.id };
    useStudio.setState({ runs: [RUN], next, filter: { starred: true } });
    let answer: (page: { runs: Run[]; next: null }) => void = () => undefined;
    vi.mocked(api.studioRuns).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    const first = useStudio.getState().loadMore();
    const second = useStudio.getState().loadMore();
    answer({ runs: [LOCAL_RUN, RUN], next: null });
    await Promise.all([first, second]);
    expect(api.studioRuns).toHaveBeenCalledTimes(1);
    expect(api.studioRuns).toHaveBeenCalledWith({ starred: true }, next, 60);
    // A run already read is not shown twice.
    expect(useStudio.getState()).toMatchObject({ runs: [RUN, LOCAL_RUN], next: null });
  });

  it("Load more after the last page reads nothing, and reads again once there is a next page", async () => {
    useStudio.setState({ runs: [RUN], next: null, filter: {} });
    await useStudio.getState().loadMore();
    expect(api.studioRuns).not.toHaveBeenCalled();
    const next = { createdAt: RUN.createdAt, id: RUN.id };
    useStudio.setState({ next });
    vi.mocked(api.studioRuns).mockResolvedValue({ runs: [LOCAL_RUN], next: null });
    await useStudio.getState().loadMore();
    expect(api.studioRuns).toHaveBeenCalledWith({}, next, 60);
    expect(useStudio.getState()).toMatchObject({ runs: [RUN, LOCAL_RUN], next: null });
  });

  it("a page for a filter changed since is dropped", async () => {
    const next = { createdAt: RUN.createdAt, id: RUN.id };
    useStudio.setState({ runs: [RUN], next, filter: {} });
    let answer: (page: { runs: Run[]; next: null }) => void = () => undefined;
    vi.mocked(api.studioRuns).mockReturnValueOnce(new Promise((resolve) => (answer = resolve)));
    const more = useStudio.getState().loadMore();
    vi.mocked(api.studioRuns).mockResolvedValueOnce({ runs: [], next: null });
    await useStudio.getState().setFilter({ kind: "video" });
    answer({ runs: [LOCAL_RUN], next: null });
    await more;
    expect(useStudio.getState().runs).toEqual([]);
  });
});
