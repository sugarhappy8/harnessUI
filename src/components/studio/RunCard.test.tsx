// Studio's run cards (0.1.0 Task B8; design Wireframe 3, local addendum Wireframe L4): the
// settings line, the status line with its timer and progress, the honest charge line of a
// cloud run, Stop watching and Get it anyway for a video, a local run's stages, steps and time
// left, the graphics card's wait and question, the unloaded chat model, the engine's and a
// damaged part's download, and the queue strip's counts. Esc never cancels a card.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { COMPOSE_IMAGES_EVENT } from "../../lib/composeImages";
import { LOCAL, LOCAL_RUN, MODELS, RUN } from "./fixtures/models";
import type { LocalRun, Run, RunError, StudioPrefs } from "./types";

vi.mock("../../lib/ipc", () => ({ isTauri: () => true, loadLocalModel: vi.fn(), modelBusy: vi.fn() }));
vi.mock("./api");

const api = await import("./api");
const ipc = await import("../../lib/ipc");
const { RunCard, localLine, queueSummary, usualSeconds, usualTimesOf } = await import("./RunCard");
const { useStudio } = await import("../../store/studio");
const { useApp } = await import("../../store/app");

const INITIAL = useStudio.getState();
const INITIAL_APP = useApp.getState();

/** The fixture's cloud run (a finished OpenRouter picture) with `patch`. */
function run(patch: Partial<Run> = {}): Run {
  return { ...RUN, ...patch };
}

/** A cloud run still working: no results, no cost yet. */
function working(patch: Partial<Run> = {}): Run {
  return run({ status: "running", outputs: [], costUsd: null, finishedAt: null, ...patch });
}

/** A video at OpenRouter that the company is making, followed by Studio. */
function video(patch: Partial<Run> = {}): Run {
  return working({
    id: "video-run",
    kind: "video",
    model: "google/veo-3.1",
    modelLabel: "Veo 3.1",
    params: { aspect_ratio: "16:9", duration: 8, resolution: "720p", generate_audio: true },
    status: "waiting",
    watching: true,
    remoteId: "job-7Hq2",
    ...patch,
  });
}

/** The fixture's local run with `patch`, its local state with `local`. */
function localRun(patch: Partial<Run> = {}, local: Partial<LocalRun> = {}): Run {
  return { ...LOCAL_RUN, ...patch, local: { ...LOCAL_RUN.local!, ...local } };
}

/** A local run working now: started `ago` ms ago, no results yet. */
function localWorking(local: Partial<LocalRun> = {}, patch: Partial<Run> = {}, ago = 21_000): Run {
  return localRun(
    { status: "running", outputs: [], finishedAt: null, costUsd: null, startedAt: Date.now() - ago, ...patch },
    { elapsedMs: null, ...local },
  );
}

function err(patch: Partial<RunError> = {}): RunError {
  return {
    code: "studio_failed",
    message: "",
    nextStep: "",
    details: "",
    charged: null,
    requestId: null,
    company: "openrouter",
    stage: null,
    ...patch,
  };
}

function prefs(patch: Partial<StudioPrefs> = {}): StudioPrefs {
  return {
    lastModel: {},
    advancedOpen: false,
    confirmEveryVideo: true,
    confirmAtUsd: 1,
    dailyLimitUsd: 10,
    setups: [],
    galleryDir: null,
    gpuPolicy: "unload",
    reloadChatAfter: false,
    suggestionDismissed: true,
    localNoticeAcceptedAt: null,
    ...patch,
  };
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

async function show(r: Run) {
  await act(async () => {
    render(<RunCard run={r} />);
  });
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

const button = (name: string | RegExp) => screen.getByRole("button", { name });
const noButton = (name: string | RegExp) => expect(screen.queryByRole("button", { name })).toBeNull();

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  useApp.setState(INITIAL_APP, true);
  // A result's tile waits for its bytes; the tests of its results give them.
  vi.mocked(api.studioOutputBytes).mockReturnValue(new Promise(() => {}));
});

afterEach(cleanup);

describe("RunCard", () => {
  describe("a cloud run", () => {
    it("a picture card shows making it, a timer and Cancel", async () => {
      const r = working({ startedAt: Date.now() - 5_000 });
      vi.mocked(api.studioCancel).mockResolvedValue({
        ...r,
        status: "cancelled",
        error: err({ code: "studio_stopped", charged: false, message: "Stopped. Nothing was charged." }),
      });
      await show(r);
      screen.getByText(/Making it · 0:0\d/);
      await click(button("Cancel"));
      expect(api.studioCancel).toHaveBeenCalledWith(RUN.id);
      // The card follows the run's new state.
      screen.getByText("Stopped. Nothing was charged.");
      noButton("Cancel");
    });

    it("the settings line names the model, where it went and its settings, then its cost", async () => {
      await show(run());
      screen.getByText("GPT Image 2.6 Flare · My OpenRouter · 1:1 · ×4 · seed 81723");
      screen.getByText("$0.04");
      button("Again");
      cleanup();
      await show(working({ estimateUsd: 0.16 }));
      screen.getByText("~ $0.16");
    });

    it("a stopped picture shows the backend's charge line", async () => {
      await show(
        run({
          status: "cancelled",
          outputs: [],
          costUsd: null,
          error: err({ code: "studio_stopped", company: "openai", charged: null, message: "Stopped waiting. OpenAI may still charge for it." }),
        }),
      );
      screen.getByText("Stopped waiting. OpenAI may still charge for it.");
      screen.getByText("May have been charged");
      // A picture has no job to get later, though a cloud picture carries a remote id.
      noButton("Get it anyway");
      cleanup();
      await show(
        run({
          status: "cancelled",
          outputs: [],
          costUsd: null,
          error: err({
            code: "studio_stopped",
            charged: false,
            message: "Stopped. OpenRouter does not charge for a picture you did not receive.",
          }),
        }),
      );
      screen.getByText("Stopped. OpenRouter does not charge for a picture you did not receive.");
      screen.getByText("Not charged");
    });

    it("a video card shows time so far and the usual time", async () => {
      // Three finished runs of the same model took 100, 120 and 150 s: usually 120 s.
      const took = (id: string, seconds: number, finishedAt: number) =>
        video({ id, status: "done", watching: false, startedAt: finishedAt - seconds * 1000, finishedAt });
      useStudio.setState({ runs: [took("a", 100, 1_000_000), took("b", 120, 2_000_000), took("c", 150, 3_000_000)] });
      await show(video({ startedAt: Date.now() - 72_400 }));
      screen.getByText("Making it · 1:12 so far · usually about 2 min");
      screen.getByText("You can keep working or close the app; it will appear here.");
      button("Stop watching");
    });

    it("Stop watching keeps a way to get the video", async () => {
      const v = video({ startedAt: Date.now() - 30_000 });
      const stopped = {
        ...v,
        status: "cancelled" as const,
        watching: false,
        error: err({
          code: "studio_stopped",
          charged: null,
          message: "OpenRouter may still finish and charge for this.",
          nextStep: "Press Get it anyway to download it if it finished.",
        }),
      };
      vi.mocked(api.studioCancel).mockResolvedValue(stopped);
      vi.mocked(api.studioFetchAnyway).mockResolvedValue({ ...stopped, status: "waiting", watching: true, error: null });
      await show(v);
      await click(button("Stop watching"));
      expect(api.studioCancel).toHaveBeenCalledWith("video-run");
      screen.getByText("OpenRouter may still finish and charge for this.");
      screen.getByText("May have been charged");
      await click(button("Get it anyway"));
      expect(api.studioFetchAnyway).toHaveBeenCalledWith("video-run");
      expect(api.studioStart).not.toHaveBeenCalled();
    });

    it("a video stopped while sending has no job to get", async () => {
      await show(
        video({
          status: "cancelled",
          watching: false,
          remoteId: null,
          error: err({ code: "studio_stopped", message: "Stopped while sending; it may or may not have started." }),
        }),
      );
      screen.getByText("Stopped while sending; it may or may not have started.");
      noButton("Get it anyway");
    });

    it("a video whose download failed offers Get it anyway, which never makes a new one", async () => {
      vi.mocked(api.studioFetchAnyway).mockResolvedValue(video({ status: "downloading" }));
      await show(
        video({
          status: "failed",
          watching: false,
          error: err({
            code: "studio_download_failed",
            charged: true,
            message: "The result could not be downloaded from OpenRouter.",
            nextStep: "Press Get it anyway to try again.",
          }),
        }),
      );
      await click(button("Get it anyway"));
      expect(api.studioFetchAnyway).toHaveBeenCalledWith("video-run");
      expect(api.studioStart).not.toHaveBeenCalled();
    });

    it("a still-not-ready video offers Check again", async () => {
      vi.mocked(api.studioFetchAnyway).mockResolvedValue(video({ status: "waiting", watching: true }));
      await show(video({ id: RUN.id, status: "waiting", watching: false, stale: true }));
      screen.getByText("Still not ready.");
      await click(button("Check again"));
      expect(api.studioFetchAnyway).toHaveBeenCalledWith(RUN.id);
      expect(api.studioStart).not.toHaveBeenCalled();
    });

    it("a waiting video Studio does not follow yet, its watch not run out, just waits", async () => {
      // As read from the saved runs before its watch resumes: not followed, but not stale.
      await show(video({ status: "waiting", watching: false, stale: false }));
      screen.getByText("Waiting");
      expect(screen.queryByText("Still not ready.")).toBeNull();
      noButton("Check again");
      noButton("Again");
    });

    it("a refused run explains it and offers no retry", async () => {
      await show(
        run({
          status: "blocked",
          outputs: [],
          costUsd: null,
          error: err({
            code: "studio_blocked",
            company: "openai",
            stage: "prompt",
            message: "OpenAI's safety filter refused the prompt.",
            nextStep: "Not charged. Change the prompt or the added pictures if you want to try again.",
            charged: false,
            requestId: "req_8f2",
          }),
        }),
      );
      screen.getByText("OpenAI's safety filter refused the prompt.");
      screen.getByText("Not charged · request req_8f2");
      button("Edit prompt");
      expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
      noButton("Again");
    });

    it("Edit prompt puts the run's prompt in the box and the cursor there", async () => {
      const focus = vi.fn();
      window.addEventListener("app:focus-studio-prompt", focus);
      try {
        await show(
          run({
            status: "blocked",
            outputs: [],
            error: err({ code: "studio_blocked", message: "OpenAI's safety filter refused the prompt.", charged: false }),
          }),
        );
        await click(button("Edit prompt"));
        expect(useStudio.getState().drafts.image.prompt).toBe(RUN.prompt);
        expect(focus).toHaveBeenCalledTimes(1);
      } finally {
        window.removeEventListener("app:focus-studio-prompt", focus);
      }
    });

    it("an interrupted run says it may or may not have started", async () => {
      await show(
        run({
          status: "interrupted",
          outputs: [],
          costUsd: null,
          error: err({
            code: "studio_interrupted",
            charged: null,
            message: "The app closed while sending; it may or may not have started.",
            nextStep: "Check your activity page on openrouter.ai.",
          }),
        }),
      );
      screen.getByText("The app closed while sending; it may or may not have started.");
      screen.getByText("May have been charged");
      screen.getByText("Check your activity page on openrouter.ai.");
    });

    it("progress events update the card", async () => {
      const r = working({ startedAt: Date.now() - 2_000, progress: 0.1 });
      await show(r);
      expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("10");
      act(() => useStudio.getState().applyProgress({ ...r, progress: 0.4 }));
      expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("40");
    });

    it("Esc does not cancel a running card", async () => {
      await show(working({ startedAt: Date.now() }));
      fireEvent.keyDown(window, { key: "Escape" });
      fireEvent.keyDown(button("Cancel"), { key: "Escape" });
      expect(api.studioCancel).not.toHaveBeenCalled();
    });

    it("a failed stop is said on the card", async () => {
      vi.mocked(api.studioCancel).mockRejectedValue({
        code: "studio_failed",
        message: "The run could not be stopped.",
        nextStep: "Try again.",
        details: "",
      });
      await show(working({ startedAt: Date.now() }));
      await click(button("Cancel"));
      screen.getByText("The run could not be stopped.");
    });
  });

  describe("its results", () => {
    const original = { create: URL.createObjectURL, revoke: URL.revokeObjectURL };
    beforeEach(() => {
      URL.createObjectURL = vi.fn(() => "blob:tile");
      URL.revokeObjectURL = vi.fn();
      vi.mocked(api.studioOutputBytes).mockResolvedValue(new ArrayBuffer(8));
    });
    afterEach(() => {
      URL.createObjectURL = original.create;
      URL.revokeObjectURL = original.revoke;
    });

    const tile = (id: string) => document.querySelector<HTMLElement>(`[data-output-tile="${id}"]`)!;

    it("a finished run shows each result as a tile from its thumbnail, and a tile opens the viewer", async () => {
      await show(run());
      await flush();
      expect([...document.querySelectorAll("[data-output-tile]")].map((t) => t.getAttribute("data-output-tile"))).toEqual(
        ["o1", "o2", "o3", "o4"],
      );
      expect(api.studioOutputBytes).toHaveBeenCalledWith("o2", true);
      await click(within(tile("o2")).getByRole("button", { name: /^Open picture 2 of 4/ }));
      expect(useStudio.getState().viewer).toBe("o2");
    });

    it("a result's star stars it", async () => {
      vi.mocked(api.studioStar).mockResolvedValue({ ...RUN.outputs[1]!, starred: true });
      useStudio.setState({ runs: [RUN] });
      await show(run());
      const star = within(tile("o2")).getByRole("button", { name: "Star picture 2 of 4" });
      expect(star.getAttribute("aria-pressed")).toBe("false");
      await click(star);
      expect(api.studioStar).toHaveBeenCalledWith("o2", true);
      expect(within(tile("o2")).getByRole("button", { name: "Star picture 2 of 4" }).getAttribute("aria-pressed")).toBe(
        "true",
      );
    });

    it("Reuse fills the prompt and every setting and starts nothing", async () => {
      useStudio.setState({
        endpoints: [
          {
            providerId: 1,
            name: "OpenRouter",
            company: "openrouter",
            host: "openrouter.ai",
            cloud: true,
            hasKey: true,
            fetchedAt: 1,
            models: MODELS,
          },
        ],
      });
      await show(run());
      await click(button("Reuse"));
      expect(api.studioStart).not.toHaveBeenCalled();
      expect(useStudio.getState().drafts.image).toMatchObject({ prompt: RUN.prompt, values: RUN.params, model: RUN.model });
    });

    it("Reuse says why when the run's model is gone", async () => {
      await show(run());
      await click(button("Reuse"));
      screen.getByText("This run's endpoint was removed in Settings → Endpoints.");
    });

    it("a run still working shows no tiles yet", async () => {
      await show(working());
      expect(document.querySelector("[data-output-tile]")).toBeNull();
      noButton("Reuse");
    });

    // [0.1.0 B11] A card with one picture sends it to the chat; with several, the viewer sends
    // the one wanted; a video is never sent (chat models do not take video).
    it("a finished card with one picture sends it to the chat", async () => {
      const picture = { mime: "image/png", fileName: "Studio picture.png", data: "iVBORw0KGgo=" };
      vi.mocked(api.studioOutputForChat).mockResolvedValue(picture);
      useApp.setState({ activeChatId: 3 });
      const composed: unknown[] = [];
      const onCompose = (e: Event) => composed.push((e as CustomEvent<unknown>).detail);
      window.addEventListener(COMPOSE_IMAGES_EVENT, onCompose);
      try {
        useStudio.getState().showStudio();
        await show(run({ outputs: [RUN.outputs[0]!] }));
        await click(button("Send to chat"));
        expect(api.studioOutputForChat).toHaveBeenCalledWith("o1");
        expect(useStudio.getState().view).toBe("chat");
        expect(composed).toEqual([[picture]]);
      } finally {
        window.removeEventListener(COMPOSE_IMAGES_EVENT, onCompose);
      }
    });

    it("Send to chat is not on a card with several pictures, a video or a run still working", async () => {
      await show(run());
      noButton("Send to chat");
      cleanup();
      await show(video({ status: "done", watching: false, outputs: [{ ...RUN.outputs[0]!, mime: "video/mp4" }] }));
      noButton("Send to chat");
      cleanup();
      await show(working());
      noButton("Send to chat");
    });

    it("a picture that cannot go into a chat says why on the card", async () => {
      vi.mocked(api.studioOutputForChat).mockRejectedValue({
        code: "bad_request",
        message: "This picture is over 20 MB, so it cannot go into a chat.",
        nextStep: "",
        details: "",
      });
      useStudio.getState().showStudio();
      await show(run({ outputs: [RUN.outputs[0]!] }));
      await click(button("Send to chat"));
      within(screen.getByRole("alert")).getByText("This picture is over 20 MB, so it cannot go into a chat.");
      expect(useStudio.getState().view).toBe("studio");
    });
  });

  describe("the queue strip and the usual time", () => {
    it("the queue strip counts", () => {
      const runs = [
        run({ id: "1", status: "running" }),
        run({ id: "2", status: "queued" }),
        run({ id: "3", status: "done" }),
        run({ id: "4", status: "done" }),
        run({ id: "5", status: "done" }),
        run({ id: "6", status: "cancelled" }),
      ];
      expect(queueSummary(runs)).toBe("1 making · 1 waiting · 3 done · 1 stopped");
    });

    it("leaves out what has none, and counts refused runs as failed", () => {
      expect(queueSummary([run({ status: "failed" }), run({ status: "blocked" })])).toBe("2 failed");
      expect(queueSummary([video({ status: "waiting", watching: true }), run({ status: "interrupted" })])).toBe(
        "1 making · 1 stopped",
      );
      expect(queueSummary([])).toBe("");
    });

    it("the usual time is the median of the last 10 finished runs of that model", () => {
      const done = (seconds: number, finishedAt: number, model = RUN.model): Run =>
        run({ id: `r${finishedAt}`, model, status: "done", startedAt: finishedAt - seconds * 1000, finishedAt });
      // Eleven runs: the oldest (1000 s) is not among the last 10.
      const runs = [done(1000, 1), ...Array.from({ length: 10 }, (_, i) => done(10 + i * 10, 100 + i))];
      expect(usualSeconds(runs, RUN.model)).toBe(55);
      expect(usualSeconds([...runs, done(5, 500, "other"), run({ status: "failed" })], RUN.model)).toBe(55);
      expect(usualSeconds(runs, "other")).toBeNull();
    });

    it("the usual times are worked out once per change of the store's runs, whichever card asks", () => {
      const done = (seconds: number, finishedAt: number): Run =>
        run({ id: `u${finishedAt}`, status: "done", startedAt: finishedAt - seconds * 1000, finishedAt });
      const s = { live: {}, recentRuns: [], runs: [done(100, 1), done(120, 2), done(150, 3)] };
      const times = usualTimesOf(s);
      expect(times.get(RUN.model)).toBe(120);
      // The same runs (another card, or an update of something else): the same answer, kept.
      expect(usualTimesOf({ ...s })).toBe(times);
      // New runs: worked out again.
      const more = { ...s, runs: [...s.runs, done(200, 4)] };
      expect(usualTimesOf(more).get(RUN.model)).toBe(135);
      expect(usualTimesOf(more).get(RUN.model)).toBe(usualSeconds(more.runs, RUN.model));
    });
  });

  describe("a local run", () => {
    it("a local card shows the stage, the step and the time left", async () => {
      await show(
        localWorking(
          { stage: "drawing", step: 3, steps: 4, picture: 1, pictures: 2 },
          { eta: { lowMs: 90_000, highMs: 120_000, measured: true, leftMs: 30_000 } },
          21_300,
        ),
      );
      screen.getByText("Picture 1 of 2 · step 3 of 4 · 0:21 so far · about 0:30 left");
      button("Stop");
      screen.getByText(/on this computer/);
      screen.getByText("free");
      expect(document.querySelector("[data-cloud-mark]")).toBeNull();
    });

    it("the time left is timed live; before a step is timed it is the first guess", () => {
      const now = Date.now();
      const guess = { lowMs: 180_000, highMs: 300_000, measured: false, leftMs: null };
      const loading = localWorking({ stage: "loading" }, { startedAt: now - 5_000, eta: guess });
      expect(localLine(loading, now)).toBe("Loading the model · 0:05 so far · about 4 min (first guess)");
      const early = localWorking({ stage: "drawing", step: null, steps: 4 }, { startedAt: now - 5_000, eta: guess });
      expect(localLine(early, now)).toBe("Drawing · 0:05 so far · about 4 min (first guess)");
      const timed = localWorking(
        { stage: "drawing", step: 1, steps: 4 },
        { startedAt: now - 9_000, eta: { ...guess, leftMs: 30_000 } },
      );
      expect(localLine(timed, now)).toBe("step 1 of 4 · 0:09 so far · about 0:30 left");
      const saving = localWorking({ stage: "saving" }, { startedAt: now - 60_000, eta: guess });
      expect(localLine(saving, now)).toBe("Saving · 1:00 so far");
    });

    it("the settings line comes only from the run: its exact size, else its results', else its shape", async () => {
      await show(LOCAL_RUN);
      screen.getByText("FLUX.2 klein 4B · on this computer · 1024×1024 · ×2 · seed 81723");
      screen.getByText("free");
      screen.getByText("took 3 min 48 s");
      cleanup();
      await show(localWorking({ stage: "loading" }));
      screen.getByText("FLUX.2 klein 4B · on this computer · 1:1 · ×2 · seed 81723");
      cleanup();
      await show(localWorking({ stage: "loading" }, { params: { size: "832x480", n: 1, seed: 7 } }));
      screen.getByText("FLUX.2 klein 4B · on this computer · 832×480 · seed 7");
    });

    it("waiting for the graphics card says why, and asks when the setting says Ask", async () => {
      // Each a run of its own: the card follows the newest state the store has of its run.
      const queued = (waitingFor: LocalRun["waitingFor"], id = `wait-${waitingFor}`) =>
        localRun({ id, status: "queued", outputs: [], startedAt: null, finishedAt: null }, { waitingFor, elapsedMs: null });
      vi.mocked(api.studioAnswerUnload).mockImplementation(async (id) => queued(null, id));
      vi.mocked(api.studioCancel).mockImplementation(async (id) => localRun({ id, status: "cancelled" }));
      await show(queued("reply"));
      screen.getByText("Waiting for the graphics card: a chat reply is being written. Starts when the reply ends.");
      await click(button("Cancel"));
      expect(api.studioCancel).toHaveBeenCalledWith("wait-reply");
      cleanup();
      await show(queued("answer"));
      screen.getByText("This needs the graphics card's memory. Unload the chat model?");
      await click(button("Unload"));
      expect(api.studioAnswerUnload).toHaveBeenCalledWith("wait-answer", true);
      // Answered, the run waits as it did before the question.
      screen.getByText("Waiting");
      cleanup();
      await show(queued("answer", "wait-answer-2"));
      await click(button("Keep the chat model"));
      expect(api.studioAnswerUnload).toHaveBeenLastCalledWith("wait-answer-2", false);
      cleanup();
      await show(queued("chatModelLoading"));
      screen.getByText("Waiting for the chat model to finish loading.");
      button("Cancel");
    });

    it("an answer to the question waits like the card's other actions, and a failed one says what to do", async () => {
      let refuse: (e: unknown) => void = () => undefined;
      vi.mocked(api.studioAnswerUnload).mockImplementation(
        () => new Promise((_, reject) => (refuse = reject)),
      );
      await show(
        localRun(
          { id: "ask", status: "queued", outputs: [], startedAt: null, finishedAt: null },
          { waitingFor: "answer", elapsedMs: null },
        ),
      );
      await click(button("Unload"));
      expect(api.studioAnswerUnload).toHaveBeenCalledWith("ask", true);
      // On its way: the answers and Cancel wait.
      for (const name of ["Unload", "Keep the chat model", "Cancel"]) {
        expect((button(name) as HTMLButtonElement).disabled).toBe(true);
      }
      await act(async () =>
        refuse({
          code: "studio_failed",
          message: "The chat model could not be unloaded.",
          nextStep: "Try again when the chat reply ends.",
          details: "",
        }),
      );
      await flush();
      screen.getByText("The chat model could not be unloaded.");
      screen.getByText("Try again when the chat reply ends.");
      expect((button("Unload") as HTMLButtonElement).disabled).toBe(false);
    });

    it("the unloaded chat model can be loaded again from the card", async () => {
      const pickModel = vi.fn(async () => undefined);
      useApp.setState({ pickModel, runtime: { modelPath: null, state: "stopped" } as never });
      let loaded: (done: boolean) => void = () => undefined;
      vi.mocked(ipc.loadLocalModel).mockReturnValue(new Promise((resolve) => (loaded = resolve)));
      await show(localWorking({ stage: "drawing", step: 1, steps: 4, unloadedChat: "/m/chat.gguf" }));
      screen.getByText("The chat model was unloaded to make room.");
      await click(button("Load it again"));
      // The runtime loads it as it was: the open chat keeps its own model.
      expect(ipc.loadLocalModel).toHaveBeenCalledWith("/m/chat.gguf", null, true);
      expect(pickModel).not.toHaveBeenCalled();
      // It may wait while this job holds the graphics card: the card says it is on its way.
      screen.getByText("Loading the chat model…");
      noButton("Load it again");
      await act(async () => loaded(true));
      await flush();
      button("Load it again");
      // A load that fails says why on the card, with what to do.
      vi.mocked(ipc.loadLocalModel).mockRejectedValue({
        code: "load_failed",
        message: "The model could not be loaded.",
        nextStep: "Check the file in Models → Installed.",
        details: "",
      });
      await click(button("Load it again"));
      await flush();
      screen.getByText("The model could not be loaded.");
      screen.getByText("Check the file in Models → Installed.");
      cleanup();
      // Said after the run too, until the chat model is loaded again.
      await show(localRun({}, { unloadedChat: "/m/chat.gguf" }));
      button("Load it again");
      cleanup();
      useApp.setState({ runtime: { modelPath: "/m/chat.gguf", state: "ready" } as never });
      await show(localRun({}, { unloadedChat: "/m/chat.gguf" }));
      noButton("Load it again");
    });

    it("Stop on a local card says nothing was charged and offers Run again", async () => {
      const r = localWorking({ stage: "drawing", step: 2, steps: 4 });
      const stopped = localRun(
        { status: "cancelled", outputs: [], finishedAt: Date.now() },
        { elapsedMs: null },
      );
      stopped.error = err({
        code: "studio_stopped",
        company: "local",
        charged: false,
        message: "Stopped. Your computer is no longer working on it. Nothing was charged.",
        nextStep: "Make it again if you still want it.",
      });
      vi.mocked(api.studioCancel).mockResolvedValue(stopped);
      vi.mocked(api.studioStart).mockImplementation(async (req) => ({ ...LOCAL_RUN, id: "again", status: "queued", prompt: req.prompt }));
      vi.mocked(api.studioSetPrefs).mockImplementation(async (patch) => ({ ...prefs(), ...patch }));
      useStudio.setState({ prefs: prefs() });
      await show(r);
      await click(button("Stop"));
      expect(api.studioCancel).toHaveBeenCalledWith(LOCAL_RUN.id);
      screen.getByText("Stopped. Your computer is no longer working on it. Nothing was charged.");
      expect(screen.queryByText(/charged$/)).toBeNull();
      await click(button("Run again"));
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(vi.mocked(api.studioStart).mock.calls[0]![0]).toMatchObject({
        target: "thisComputer",
        model: LOCAL_RUN.model,
        prompt: LOCAL_RUN.prompt,
      });
    });

    it("a local run beside the chat model, or on the processor, says what that means", async () => {
      await show(localWorking({ stage: "drawing", step: 1, steps: 4, alongside: true }));
      screen.getByText("Chat replies may be slower while this runs");
      cleanup();
      await show(localWorking({ stage: "drawing", step: 1, steps: 4, onProcessor: true }));
      screen.getByText("Running on the processor: slower, and chat will be slow while this runs.");
    });

    it("a missing engine or a damaged part offers the download again", async () => {
      const engine = { ...LOCAL.engine, state: "notInstalled" as const, downloadBytes: 30_303_000 };
      useStudio.setState({ local: { ...LOCAL, engine } });
      vi.mocked(api.studioEngineDownload).mockResolvedValue(undefined);
      vi.mocked(api.studioEngineStatus).mockResolvedValue({ ...engine, state: "downloading", bytesDone: 0 });
      vi.mocked(api.studioLocalDownload).mockResolvedValue("group-1");
      const failed = (code: string, message: string) =>
        localRun({ status: "failed", outputs: [], error: err({ code, message, company: "local", charged: false }) });
      await show(failed("studio_engine_missing", "The Studio engine is missing or was blocked."));
      const download = button("Download the Studio engine (29 MB, MIT)");
      expect(download.getAttribute("title")).toBe(LOCAL.engine.licenseName);
      await click(download);
      expect(api.studioEngineDownload).toHaveBeenCalledWith(true);
      expect(useStudio.getState().local?.engine.state).toBe("downloading");
      noButton(/Download the Studio engine/);
      cleanup();
      // Built in (a Mac): nothing to download.
      useStudio.setState({ local: LOCAL });
      await show(failed("studio_engine_missing", "The Studio engine is missing or was blocked."));
      noButton(/Download the Studio engine/);
      cleanup();
      await show(failed("studio_model_file_bad", "A model file is damaged."));
      await click(button("Download again"));
      expect(api.studioLocalDownload).toHaveBeenCalledWith({ installed: { genModelId: LOCAL_RUN.model } });
    });

    it("a refused repair says why on the card", async () => {
      vi.mocked(api.studioLocalDownload).mockRejectedValue({
        code: "studio_disk_too_small",
        message: "There is not enough free space for this model.",
        nextStep: "Free up space or choose another models folder.",
        details: "",
      });
      await show(
        localRun({
          status: "failed",
          outputs: [],
          error: err({ code: "studio_model_file_bad", message: "A model file is damaged.", company: "local" }),
        }),
      );
      await click(button("Download again"));
      screen.getByText("There is not enough free space for this model.");
      screen.getByText("Free up space or choose another models folder.");
    });

    it("an out-of-memory retry and failure are said plainly", async () => {
      await show(localWorking({ stage: "drawing", step: 1, steps: 4, retrying: true }));
      screen.getByText("Ran out of graphics memory, trying again with memory saving (slower)");
      cleanup();
      await show(
        localRun({
          kind: "video",
          status: "failed",
          outputs: [],
          error: err({
            code: "studio_out_of_memory",
            company: "local",
            charged: false,
            message: "Ran out of graphics memory.",
            nextStep: "Try 480p or a shorter clip.",
          }),
        }),
      );
      screen.getByText("Ran out of graphics memory.");
      screen.getByText(/Try 480p or a shorter clip/);
      button("Run again");
    });

    it("a stalled local run offers Stop and Keep waiting", async () => {
      await show(localWorking({ stage: "drawing", step: 1, steps: 4, stalled: true }));
      screen.getByText("No progress for 10 minutes");
      button("Stop");
      await click(button("Keep waiting"));
      expect(screen.queryByText("No progress for 10 minutes")).toBeNull();
      button("Stop");
    });

    it("an interrupted local run offers Run again for free", async () => {
      await show(
        localRun({
          status: "interrupted",
          outputs: [],
          error: err({
            code: "studio_interrupted",
            company: "local",
            charged: false,
            message: "The app closed while this was being made.",
            nextStep: "Nothing was charged. Make it again if you still want it.",
          }),
        }),
      );
      screen.getByText("The app closed while this was being made.");
      button("Run again");
      expect(screen.queryByText(/^Not charged|^May have been charged|^Charged/)).toBeNull();
    });
  });
});
