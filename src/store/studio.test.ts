// Studio's store (0.1.0 Task B1): which view is shown, the runs still working as the backend
// reports them, and the notice for a video that finished while the chat was shown.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LOCAL_RUN, RUN } from "../components/studio/fixtures/models";
import type { Run } from "../components/studio/types";

vi.mock("../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("../components/studio/api");

const api = await import("../components/studio/api");
const { followStudio, runningCount, useStudio } = await import("./studio");

function run(patch: Partial<Run>): Run {
  return { ...RUN, ...patch };
}

async function flush() {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState({ view: "chat", tab: "image", live: {}, readyNotice: null });
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
