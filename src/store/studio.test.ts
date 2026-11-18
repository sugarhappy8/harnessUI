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
});

describe("progress", () => {
  it("keeps each run's newest state by its id", () => {
    useStudio.getState().applyProgress(run({ id: "a", status: "sending" }));
    useStudio.getState().applyProgress(run({ id: "b", status: "queued" }));
    useStudio.getState().applyProgress(run({ id: "a", status: "done" }));
    const live = useStudio.getState().live;
    expect(Object.keys(live).sort()).toEqual(["a", "b"]);
    expect(live.a?.status).toBe("done");
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
    vi.mocked(api.studioLive).mockResolvedValue([run({ id: "a", status: "waiting", watching: true })]);
    const stop = followStudio();
    await flush();
    expect(api.studioLive).toHaveBeenCalledTimes(1);
    expect(useStudio.getState().live.a?.status).toBe("waiting");
    heard[0]!(run({ id: "a", status: "downloading" }));
    expect(useStudio.getState().live.a?.status).toBe("downloading");
    stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("keeps what an event said over an older answer of the read at start", async () => {
    listen();
    let answer: (runs: Run[]) => void = () => undefined;
    vi.mocked(api.studioLive).mockImplementation(() => new Promise((resolve) => (answer = resolve)));
    const stop = followStudio();
    await flush();
    heard[0]!(run({ id: "a", status: "downloading" }));
    answer([run({ id: "a", status: "sending" }), run({ id: "b", status: "queued" })]);
    await flush();
    expect(useStudio.getState().live.a?.status).toBe("downloading");
    expect(useStudio.getState().live.b?.status).toBe("queued");
    stop();
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

  it("changing the preferences sends only what changed", async () => {
    saved();
    vi.mocked(api.studioSetPrefs).mockResolvedValue({ ...prefs, suggestionDismissed: true });
    await useStudio.getState().setPrefs({ suggestionDismissed: true });
    expect(api.studioSetPrefs).toHaveBeenCalledWith({ suggestionDismissed: true });
    expect(useStudio.getState().prefs?.suggestionDismissed).toBe(true);
  });
});
