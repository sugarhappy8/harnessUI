// Studio's prompt dock (0.1.0 Task B7): Create with the price of the whole run (cloud) or its
// time (local), where the prompt goes, Enter to create, one start per press, the confirmation
// a cloud run may need, the daily limit (cloud only), cloud as the backup for a local run that
// does not fit or is very slow, and what a refused start says.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { STUDIO_DOCK, STUDIO_REUSE } from "../../lib/words";
import type { StudioTab } from "../../store/studio";
import { LOCAL, MODELS, RUN } from "./fixtures/models";
import type {
  Estimate,
  LocalEstimate,
  LocalModels,
  Run,
  StartRequest,
  StudioEndpoint,
  StudioPrefs,
  Usage,
} from "./types";

// Radix positions popovers with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { StudioView } = await import("./StudioView");
const { cloudBackup } = await import("./cost");
const { PROMPT_MAX_CHARS, promptProblems, settingsProblems, useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

/** OpenRouter with the fixture's models, or OpenAI (pictures only). */
function endpoint(patch: Partial<StudioEndpoint> = {}): StudioEndpoint {
  const openai = patch.company === "openai";
  return {
    providerId: 1,
    name: openai ? "OpenAI" : "OpenRouter",
    company: "openrouter",
    host: openai ? "api.openai.com" : "openrouter.ai",
    cloud: true,
    hasKey: true,
    fetchedAt: new Date(2026, 10, 17, 9).getTime(),
    models: openai ? [{ ...MODELS[0]!, id: "gpt-image-2.6-flare" }] : MODELS,
    ...patch,
  };
}
const openrouter = endpoint();

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

function usage(patch: Partial<Usage> = {}): Usage {
  return {
    spentTodayUsd: 0,
    pendingUsd: 0,
    limitUsd: 10,
    folder: "/home/me/harnessUI/studio",
    bytes: 0,
    pictures: 0,
    videos: 0,
    ...patch,
  };
}

/** A cloud estimate: $0.16, nothing to confirm, within the limit. */
function est(patch: Partial<Estimate> = {}): Estimate {
  return {
    usd: 0.16,
    reasons: [],
    confirmAtUsd: 1,
    limit: { limitUsd: 10, spentTodayUsd: 0, pendingUsd: 0, wouldPass: true, refusal: null, reserveUsd: 5 },
    invalid: [],
    eta: null,
    local: null,
    ...patch,
  };
}

function localEst(patch: Partial<LocalEstimate> = {}): LocalEstimate {
  return { fit: "fits", unloadChat: false, verySlow: false, over15Min: false, reason: null, ...patch };
}

/** A local estimate: free, about 4 minutes here (measured), fitting. */
function here(local: Partial<LocalEstimate> = {}, patch: Partial<Estimate> = {}): Estimate {
  return est({
    usd: 0,
    eta: { lowMs: 180_000, highMs: 300_000, measured: true, leftMs: null },
    local: localEst(local),
    ...patch,
  });
}

/** The model used last on each tab: GPT Image 2.6 Flare and Veo 3.1 on OpenRouter. */
const CLOUD_LAST = {
  image: { target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" },
  video: { target: { endpoint: 1 }, model: "google/veo-3.1" },
} as const;

/** What `studio_start` gives back: the run, queued. */
function accepted(req: StartRequest): Run {
  return {
    ...RUN,
    id: `run-${req.clientToken}`,
    kind: req.kind,
    runsOn: req.target === "thisComputer" ? "local" : "cloud",
    model: req.model,
    prompt: req.prompt,
    status: "queued",
    outputs: [],
  };
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Waits past the estimate's 150 ms pause, and for its answer. */
async function estimated() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 200));
  });
  await flush();
}

/** Opens Studio on `tab` with what the backend has saved; the dock asks for the draft's
 *  estimate (`estimate`), and is waited for unless `settle` is false. */
async function show(
  saved: {
    tab?: StudioTab;
    endpoints?: StudioEndpoint[];
    local?: LocalModels;
    prefs?: StudioPrefs;
    usage?: Usage;
    estimate?: Estimate;
    settle?: boolean;
  } = {},
) {
  const endpoints = saved.endpoints ?? [openrouter];
  const p = saved.prefs ?? prefs();
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints });
  vi.mocked(api.studioLocalModels).mockResolvedValue(saved.local ?? LOCAL);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(p);
  vi.mocked(api.studioSetPrefs).mockImplementation(async (patch) => ({ ...p, ...patch }));
  vi.mocked(api.studioUsage).mockResolvedValue(saved.usage ?? usage());
  vi.mocked(api.studioLocalFit).mockResolvedValue(LOCAL.installed[1]!.fit);
  vi.mocked(api.studioEstimate).mockResolvedValue(saved.estimate ?? est());
  vi.mocked(api.studioStart).mockImplementation(async (req) => accepted(req));
  useStudio.getState().showStudio(saved.tab ?? "image");
  await act(async () => {
    render(<StudioView />);
  });
  await flush();
  if (saved.settle ?? true) await estimated();
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

function promptBox(): HTMLTextAreaElement {
  return screen.getByRole("textbox", { name: "Prompt" }) as HTMLTextAreaElement;
}

function typePrompt(text: string) {
  fireEvent.change(promptBox(), { target: { value: text } });
}

function createButton(): HTMLButtonElement {
  return screen.getByRole("button", { name: /Create/ }) as HTMLButtonElement;
}

/** The request each `studio_start` got. */
function started(): StartRequest[] {
  return vi.mocked(api.studioStart).mock.calls.map(([req]) => req);
}

/** Lets the estimate's 150 ms pause run out under fake timers, and its answer come in. */
async function pauseEnds() {
  await act(async () => {
    vi.advanceTimersByTime(150);
  });
  await flush();
}

/** The backend's refusal of a cloud run that needs confirming first. */
const NEEDS_CONFIRMATION = {
  code: "studio_needs_confirmation",
  message: "Confirm this run before it is sent.",
  nextStep: "Check the price, then confirm to send it.",
  details: "",
};

/** An estimate the backend could not work out. */
const ESTIMATE_FAILED = {
  code: "internal",
  message: "Something went wrong.",
  nextStep: "Try again.",
  details: "",
};

/** What each `app:open-settings` asked for (its detail). */
let opened: unknown[] = [];
const onOpenSettings = (e: Event) => opened.push((e as CustomEvent<unknown>).detail);

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  opened = [];
  window.addEventListener("app:open-settings", onOpenSettings);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  window.removeEventListener("app:open-settings", onOpenSettings);
});

describe("PromptDock", () => {
  describe("a cloud model", () => {
    it("Create shows the price for the whole run", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      createButton();
      screen.getByText("~ $0.16");
      // Asked for the draft as Create would send it: nothing is sent anywhere.
      expect(api.studioEstimate).toHaveBeenLastCalledWith(
        expect.objectContaining({ target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare", kind: "image" }),
      );
    });

    it("Create waits while extra settings are held for the model's price, and says so", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a fox");
      await flush();
      act(() => useStudio.setState({ holding: { image: true, video: false } }));
      expect(createButton().disabled).toBe(true);
      screen.getByText(STUDIO_DOCK.holdingExtra(false));
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      await act(async () => {
        await useStudio.getState().start("image", false);
      });
      expect(api.studioStart).not.toHaveBeenCalled();
      act(() => useStudio.setState({ holding: { image: false, video: false } }));
      expect(createButton().disabled).toBe(false);
    });

    it("Create waits while a field shows a size that cannot be sent, and says why", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a fox");
      await flush();
      expect(createButton().disabled).toBe(false);
      act(() => useStudio.getState().setInvalid("image", "size", "Both sides must be multiples of 16."));
      expect(createButton().disabled).toBe(true);
      expect(screen.getByText("Both sides must be multiples of 16.").className).toContain("text-danger");
      act(() => useStudio.getState().setInvalid("image", "size", null));
      expect(createButton().disabled).toBe(false);
      expect(screen.queryByText("Both sides must be multiples of 16.")).toBeNull();
    });

    it("colours a price of $1 or more as a second cue", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }), estimate: est({ usd: 2.6 }) });
      expect(screen.getByText("~ $2.60").className).toContain("text-danger");
    });

    it("names where the prompt goes", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      screen.getByText("Cloud: your prompt and added pictures go to openrouter.ai");
    });

    it("asks again 150 ms after the draft stops changing", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      vi.mocked(api.studioEstimate).mockClear();
      typePrompt("f");
      typePrompt("fo");
      typePrompt("fox");
      await flush();
      expect(api.studioEstimate).not.toHaveBeenCalled();
      await estimated();
      expect(api.studioEstimate).toHaveBeenCalledTimes(1);
      expect(api.studioEstimate).toHaveBeenCalledWith(expect.objectContaining({ prompt: "fox" }));
    });

    it("Enter creates and Shift+Enter adds a line", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a");
      fireEvent.keyDown(promptBox(), { key: "Enter", shiftKey: true });
      expect(api.studioStart).not.toHaveBeenCalled();
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(api.studioStart).toHaveBeenCalledWith(
        expect.objectContaining({ prompt: "a", target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" }),
        false,
      );
      await flush();
    });

    it("holding Enter down starts one run, not one per key repeat", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a lighthouse");
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      await flush();
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      // The start was answered, and the prompt is still there; the key is still down.
      for (let i = 0; i < 3; i++) {
        const repeat = fireEvent.keyDown(promptBox(), { key: "Enter", repeat: true });
        // No line is added either.
        expect(repeat).toBe(false);
        await flush();
      }
      expect(api.studioStart).toHaveBeenCalledTimes(1);
    });

    it("Create is disabled while a start is on its way", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      vi.mocked(api.studioStart).mockReturnValue(new Promise<Run>(() => undefined));
      typePrompt("a lighthouse");
      await click(createButton());
      expect(createButton().disabled).toBe(true);
      await click(createButton());
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      await flush();
      expect(api.studioStart).toHaveBeenCalledTimes(1);
    });

    it("each press is its own run: a new client token every time", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a lighthouse");
      await click(createButton());
      await click(createButton());
      const [first, second] = started();
      expect(first?.clientToken).toBeTruthy();
      expect(second?.clientToken).toBeTruthy();
      expect(first?.clientToken).not.toBe(second?.clientToken);
    });

    it("the daily limit replaces Create", async () => {
      await show({
        prefs: prefs({ lastModel: CLOUD_LAST }),
        estimate: est({ limit: { limitUsd: 10, spentTodayUsd: 9.5, pendingUsd: 0, wouldPass: false, refusal: "over", reserveUsd: 5 } }),
      });
      screen.getByText("Daily limit reached: $9.50 of $10 spent today");
      await click(screen.getByRole("button", { name: "Raise the limit" }));
      expect(opened).toEqual(["studio"]); // app:open-settings detail
      expect(screen.queryByRole("button", { name: /Create/ })).toBeNull();
    });

    it("a run whose price is unknown, refused for the reserve, says so in Create's place", async () => {
      await show({
        prefs: prefs({ lastModel: CLOUD_LAST }),
        estimate: est({
          usd: null,
          limit: {
            limitUsd: 3,
            spentTodayUsd: 0,
            pendingUsd: 0,
            wouldPass: false,
            refusal: "unknownNeedsReserve",
            reserveUsd: 5,
          },
        }),
      });
      screen.getByText("This model's price is unknown, so Studio needs $5 of today's limit left ($3.00 left).");
      expect(screen.queryByText(/Daily limit reached/)).toBeNull();
      screen.getByRole("button", { name: "Raise the limit" });
    });

    it("Enter starts nothing while the daily limit has taken Create's place", async () => {
      await show({
        prefs: prefs({ lastModel: CLOUD_LAST }),
        estimate: est({ limit: { limitUsd: 10, spentTodayUsd: 9.5, pendingUsd: 0, wouldPass: false, refusal: "over", reserveUsd: 5 } }),
      });
      typePrompt("a lighthouse");
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      await flush();
      expect(api.studioStart).not.toHaveBeenCalled();
      screen.getByText("Daily limit reached: $9.50 of $10 spent today");
    });

    it("a refused start says why under the prompt, and Create works again", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      vi.mocked(api.studioStart).mockRejectedValueOnce({
        code: "studio_wont_make",
        message: "Studio won't make this.",
        nextStep: "Change the prompt.",
        details: "",
      });
      typePrompt("something it refuses");
      await click(createButton());
      screen.getByText("Studio won't make this.");
      screen.getByText("Change the prompt.");
      expect(createButton().disabled).toBe(false);
    });

    it("the model used last is saved when a run starts, and only when it changed", async () => {
      await show({ estimate: here() }); // the default: the local model
      typePrompt("a fox");
      await click(createButton());
      expect(api.studioSetPrefs).toHaveBeenCalledTimes(1);
      expect(api.studioSetPrefs).toHaveBeenCalledWith({
        lastModel: { image: { target: "thisComputer", model: "flux2-klein-4b-q4" } },
      });
      await click(createButton());
      expect(api.studioStart).toHaveBeenCalledTimes(2);
      expect(api.studioSetPrefs).toHaveBeenCalledTimes(1);
    });

    it("today's spending follows a cloud run: read again when it is accepted and when it ends", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      expect(api.studioUsage).toHaveBeenCalledTimes(1);
      vi.mocked(api.studioUsage).mockResolvedValue(usage({ spentTodayUsd: 0, pendingUsd: 0.16 }));
      typePrompt("a lighthouse");
      await click(createButton());
      expect(api.studioUsage).toHaveBeenCalledTimes(2);
      const run = accepted(started()[0]!);
      vi.mocked(api.studioUsage).mockResolvedValue(usage({ spentTodayUsd: 0.16 }));
      act(() => useStudio.getState().applyProgress({ ...run, status: "running", seq: run.seq + 1 }));
      await flush();
      expect(api.studioUsage).toHaveBeenCalledTimes(2);
      act(() => useStudio.getState().applyProgress({ ...run, status: "done", costUsd: 0.16, seq: run.seq + 2 }));
      await flush();
      expect(api.studioUsage).toHaveBeenCalledTimes(3);
      screen.getByText("$0.16 today");
    });

    it("an empty prompt says so quietly until a press is refused for it, then aloud", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      const sentence = () => screen.getByText("Write a prompt first.").parentElement!;
      // Nothing was done yet: not a fault.
      expect(sentence().className).toContain("text-fg-muted");
      expect(sentence().className).not.toContain("text-danger");
      expect(sentence().getAttribute("role")).toBeNull();
      // The prompt box is described by it.
      expect(promptBox().getAttribute("aria-describedby")).toBe(sentence().id);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).not.toHaveBeenCalled();
      expect(sentence().className).toContain("text-danger");
      expect(sentence().getAttribute("role")).toBe("alert");
      expect(promptBox().getAttribute("aria-describedby")).toBe(sentence().id);
    });

    it("a prompt typed and emptied again says so as a fault", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      typePrompt("a");
      expect(screen.queryByText("Write a prompt first.")).toBeNull();
      expect(promptBox().getAttribute("aria-describedby")).toBeNull();
      typePrompt("");
      expect(screen.getByText("Write a prompt first.").parentElement!.className).toContain("text-danger");
    });

    it("a prompt over 32,000 characters, or of white space only, says so before Create", async () => {
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }) });
      // Judged on the prompt as it is, before any answer for it.
      vi.mocked(api.studioEstimate).mockClear();
      typePrompt(" \n \u0085 ");
      screen.getByText("Write a prompt first.");
      expect(createButton().disabled).toBe(true);
      typePrompt("x".repeat(32_001));
      screen.getByText("Keep the prompt under 32,000 characters.");
      expect(createButton().disabled).toBe(true);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).not.toHaveBeenCalled();
      typePrompt("x".repeat(32_000));
      expect(screen.queryByText(/^Keep the prompt under/)).toBeNull();
      expect(createButton().disabled).toBe(false);
      expect(api.studioEstimate).not.toHaveBeenCalled();
    });

    it.each([
      ["fails", () => vi.mocked(api.studioEstimate).mockRejectedValue(ESTIMATE_FAILED)],
      ["never answers", () => vi.mocked(api.studioEstimate).mockReturnValue(new Promise<Estimate>(() => undefined))],
    ])("an estimate that %s does not keep Create greyed out", async (_, answer) => {
      // The answer for the empty prompt says to write one.
      await show({ prefs: prefs({ lastModel: CLOUD_LAST }), estimate: est({ invalid: ["Write a prompt first."] }) });
      expect(createButton().disabled).toBe(true);
      answer();
      vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
      typePrompt("a lighthouse");
      expect(createButton().disabled).toBe(false);
      await pauseEnds();
      expect(api.studioEstimate).toHaveBeenLastCalledWith(expect.objectContaining({ prompt: "a lighthouse" }));
      expect(screen.queryByText("Write a prompt first.")).toBeNull();
      expect(createButton().disabled).toBe(false);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(api.studioStart).toHaveBeenCalledWith(expect.objectContaining({ prompt: "a lighthouse" }), false);
      await flush();
    });

    it("a setting the model refuses keeps Create greyed out while the answer shown says so", async () => {
      await show({
        prefs: prefs({ lastModel: CLOUD_LAST }),
        estimate: est({ invalid: ["This model takes at most 8 reference pictures."] }),
      });
      vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
      typePrompt("a lighthouse");
      // Typing changes the prompt, not the settings: the answer for this model and place holds.
      screen.getByText("This model takes at most 8 reference pictures.");
      expect(createButton().disabled).toBe(true);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      await pauseEnds();
      expect(createButton().disabled).toBe(true);
      expect(api.studioStart).not.toHaveBeenCalled();
    });
  });

  describe("the confirmation", () => {
    it("asks before a cloud video even when the backend is the first to say so", async () => {
      await show({ tab: "video", prefs: prefs({ lastModel: CLOUD_LAST }), settle: false });
      vi.mocked(api.studioStart).mockRejectedValueOnce({
        code: "studio_needs_confirmation",
        message: "Confirm this run before it is sent.",
        nextStep: "Check the price, then confirm to send it.",
        details: "It asks because it is a cloud video.",
      });
      vi.mocked(api.studioEstimate).mockResolvedValue(est({ usd: 0.09, reasons: ["video"], confirmAtUsd: 1 }));
      typePrompt("waves");
      // Before the 150 ms pause: the store has no estimate for this draft.
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(useStudio.getState().estimates.video).toBeNull();
      await flush();
      screen.getByText("Make this video?");
      screen.getByText("Don't ask again for videos under $1.00");
      const [req] = started();
      expect(api.studioEstimate).toHaveBeenCalledWith(req);
    });

    it("a local run never opens the confirm dialog, even for a video", async () => {
      await show({ tab: "video", estimate: here() });
      expect(useStudio.getState().drafts.video.target).toBe("thisComputer");
      typePrompt("waves");
      await click(createButton());
      expect(api.studioStart).toHaveBeenCalledWith(expect.objectContaining({ target: "thisComputer" }), false);
      expect(screen.queryByText("Make this video?")).toBeNull();
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });

    it("a local run never opens it, even when the estimate for the draft as it is names a reason", async () => {
      await show({ tab: "video", estimate: here({}, { reasons: ["video"] }) });
      typePrompt("waves");
      // The estimate for this very draft: the one a cloud draft's press would go by.
      await estimated();
      expect(useStudio.getState().estimates.video?.estimate?.reasons).toEqual(["video"]);
      await click(createButton());
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(api.studioStart).toHaveBeenCalledWith(
        expect.objectContaining({ target: "thisComputer", prompt: "waves" }),
        false,
      );
      expect(useStudio.getState().pendingEstimate).toBeNull();
      expect(screen.queryByText("Make this video?")).toBeNull();
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });
  });

  describe("a local model", () => {
    it("a local model's Create shows the time, not a price", async () => {
      await show({ endpoints: [], estimate: here() });
      createButton();
      screen.getByText("about 4 min");
      expect(screen.queryByText(/\$/)).toBeNull();
    });

    it("the line above Create says the prompt stays here, and when the chat model will be unloaded", async () => {
      await show({ estimate: here({ fit: "afterUnload", unloadChat: true }) });
      screen.getByText(
        "On this computer: your prompt and pictures stay here · the chat model will be unloaded to make room",
      );
      expect(document.querySelector("[data-cloud-mark]")).toBeNull();
    });

    it("a run over 15 minutes says so in amber, without a dialog", async () => {
      await show({
        estimate: here(
          { over15Min: true },
          { eta: { lowMs: 1_380_000, highMs: 1_620_000, measured: true, leftMs: null } },
        ),
      });
      const line = screen.getByText(
        "This will take about 25 min and keep the graphics card busy. On a MacBook it will get warm.",
      );
      expect(line.className).toContain("text-warning");
      typePrompt("a fox in deep snow");
      await click(createButton());
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(screen.queryByRole("alertdialog")).toBeNull();
    });

    it("when a local run would be very slow, Studio offers cloud as the backup", async () => {
      const slow = here({ fit: "slow", verySlow: true });
      await show({ estimate: slow });
      expect(cloudBackup("image", slow, [openrouter])).toBe("useCloud");
      screen.getByText("Too slow here? A cloud model makes this in minutes, for a price you see first.");
      await click(screen.getByRole("button", { name: "Choose a cloud model" }));
      // The picker opens on the first cloud group; the draft stays where it was.
      screen.getByRole("listbox", { name: "via OpenRouter" });
      expect(document.activeElement?.textContent).toContain("GPT Image 2.6 Flare");
      expect(useStudio.getState().drafts.image.target).toBe("thisComputer");
      expect(useStudio.getState().pickerOpen).toBe("cloud");
    });

    it("an endpoint whose lists are not saved yet is the backup, and the picker gets its list", async () => {
      const unsaved = endpoint({ fetchedAt: null, models: [] });
      const slow = here({ fit: "slow", verySlow: true });
      await show({ endpoints: [unsaved], estimate: slow });
      expect(cloudBackup("image", slow, [unsaved])).toBe("useCloud");
      vi.mocked(api.studioRefreshModels).mockResolvedValue({ endpoints: [openrouter] });
      await click(screen.getByRole("button", { name: "Choose a cloud model" }));
      // The picker has a row for it, and starts there: it has no model to show yet.
      const getList = screen.getByRole("button", { name: "OpenRouter: Get the list" });
      expect(document.activeElement).toBe(getList);
      await click(getList);
      expect(api.studioRefreshModels).toHaveBeenCalledTimes(1);
      expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
      screen.getByRole("listbox", { name: "via OpenRouter" });
      expect(screen.queryByRole("button", { name: "OpenRouter: Get the list" })).toBeNull();
      expect(useStudio.getState().drafts.image.target).toBe("thisComputer");
    });

    it("without an endpoint that makes this kind, the backup is Settings → Endpoints", async () => {
      const slow = here({ fit: "slow", verySlow: true });
      await show({ endpoints: [], estimate: slow });
      expect(cloudBackup("image", slow, [])).toBe("addEndpoint");
      screen.getByText("Add OpenRouter or OpenAI in Settings → Endpoints to use a cloud model instead.");
      await click(screen.getByRole("button", { name: "Open Endpoints" }));
      expect(opened).toEqual(["endpoints"]);
      expect(screen.queryByRole("button", { name: "Choose a cloud model" })).toBeNull();
    });

    it("on the Video tab, an endpoint that is only OpenAI never counts", async () => {
      const slow = here({ fit: "slow", verySlow: true });
      const openai = endpoint({ company: "openai" });
      await show({ tab: "video", endpoints: [openai], estimate: slow });
      expect(cloudBackup("video", slow, [openai])).toBe("addEndpoint");
      screen.getByText("Add OpenRouter in Settings → Endpoints to use a cloud model instead.");
      screen.getByRole("button", { name: "Open Endpoints" });
      expect(screen.queryByText(/OpenAI/)).toBeNull();
    });

    it("a local model that does not fit here offers cloud instead of Create", async () => {
      await show({ estimate: here({ fit: "notHere", reason: "needs 6 GB of graphics memory" }, { eta: null }) });
      expect(createButton().disabled).toBe(true);
      screen.getByText("Not for this computer: needs 6 GB of graphics memory");
      // It cannot run here at all: not "too slow".
      screen.getByText("A cloud model can make this instead.");
      expect(screen.queryByText(/Too slow here\?/)).toBeNull();
      await click(screen.getByRole("button", { name: "Choose a cloud model" }));
      screen.getByRole("listbox", { name: "via OpenRouter" });
      expect(useStudio.getState().drafts.image.target).toBe("thisComputer");
    });

    it("a local model that does not fit here, without an endpoint for this kind, points to Settings → Endpoints", async () => {
      await show({
        endpoints: [],
        estimate: here({ fit: "notHere", reason: "needs 6 GB of graphics memory" }, { eta: null }),
      });
      screen.getByText("Not for this computer: needs 6 GB of graphics memory");
      screen.getByText("Add OpenRouter or OpenAI in Settings → Endpoints to use a cloud model instead.");
      expect(screen.queryByText(/Too slow here\?/)).toBeNull();
      await click(screen.getByRole("button", { name: "Open Endpoints" }));
      expect(opened).toEqual(["endpoints"]);
    });

    it("typing on a model that does not fit here never enables Create", async () => {
      await show({ estimate: here({ fit: "notHere", reason: "needs 6 GB of graphics memory" }, { eta: null }) });
      vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
      for (const text of ["a", "a f", "a fo", "a fox"]) {
        typePrompt(text);
        expect(createButton().disabled).toBe(true);
        fireEvent.keyDown(promptBox(), { key: "Enter" });
        act(() => vi.advanceTimersByTime(100));
        expect(createButton().disabled).toBe(true);
      }
      // The answer for the draft as it is now comes back the same.
      await act(async () => {
        vi.advanceTimersByTime(150);
      });
      await flush();
      expect(api.studioEstimate).toHaveBeenLastCalledWith(expect.objectContaining({ prompt: "a fox" }));
      expect(createButton().disabled).toBe(true);
      expect(api.studioStart).not.toHaveBeenCalled();
      // An answer that never comes leaves the one shown, for this model and place, in charge.
      vi.mocked(api.studioEstimate).mockReturnValue(new Promise<Estimate>(() => undefined));
      typePrompt("a fox in deep snow");
      expect(createButton().disabled).toBe(true);
      await pauseEnds();
      expect(createButton().disabled).toBe(true);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).not.toHaveBeenCalled();
      screen.getByText("Not for this computer: needs 6 GB of graphics memory");
    });

    it("a local model that fits once the chat model is unloaded can be created", async () => {
      await show({ estimate: here({ fit: "afterUnload", unloadChat: true }) });
      typePrompt("a fox");
      expect(createButton().disabled).toBe(false);
      expect(screen.getByText(/^On this computer: your prompt/).textContent).toMatch(
        / · the chat model will be unloaded to make room$/,
      );
      await click(createButton());
      expect(api.studioStart).toHaveBeenCalledTimes(1);
    });

    it("a local prompt over 4,000 characters says so before Create", async () => {
      const tooLong = "Keep the prompt under 4,000 characters for local models.";
      await show({ estimate: here() });
      vi.mocked(api.studioEstimate).mockResolvedValue(here({}, { invalid: [tooLong] }));
      typePrompt("x".repeat(4_001));
      // Said at once, on the prompt as it is.
      screen.getByText(tooLong);
      expect(createButton().disabled).toBe(true);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).not.toHaveBeenCalled();
      // The answer says the same; it is said once.
      await estimated();
      expect(screen.getAllByText(tooLong)).toHaveLength(1);
      // Counted as the backend counts: 4,000 characters, however many UTF-16 units they take.
      // The answer shown, for the longer prompt, no longer holds Create back.
      typePrompt("🦊".repeat(4_000));
      expect(screen.queryByText(tooLong)).toBeNull();
      expect(createButton().disabled).toBe(false);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      await flush();
    });

    it("the daily limit never replaces Create for a local model", async () => {
      await show({
        estimate: here({}, { limit: { limitUsd: 10, spentTodayUsd: 10, pendingUsd: 0, wouldPass: false, refusal: "over", reserveUsd: 5 } }),
      });
      typePrompt("a fox");
      expect(createButton().disabled).toBe(false);
      expect(screen.queryByText(/Daily limit reached/)).toBeNull();
    });

    it("typing a prompt and pressing Enter at once starts the run", async () => {
      // The answer for the empty prompt says to write one.
      await show({ estimate: here({}, { invalid: ["Write a prompt first."] }) });
      screen.getByText("Write a prompt first.");
      expect(createButton().disabled).toBe(true);
      vi.mocked(api.studioEstimate).mockClear();
      vi.mocked(api.studioEstimate).mockResolvedValue(here());
      vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
      typePrompt("a fox");
      // The prompt is judged as it is: no waiting for the answer for it.
      expect(screen.queryByText("Write a prompt first.")).toBeNull();
      expect(createButton().disabled).toBe(false);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(api.studioStart).toHaveBeenCalledWith(expect.objectContaining({ prompt: "a fox" }), false);
      expect(api.studioEstimate).not.toHaveBeenCalled();
      await pauseEnds();
    });

    it("Create does not wait for the estimate", async () => {
      await show({ estimate: here() });
      // The answer for the new prompt never comes: an answer that greys nothing out is no reason
      // to wait.
      vi.mocked(api.studioEstimate).mockReturnValue(new Promise<Estimate>(() => undefined));
      typePrompt("a fox");
      expect(createButton().disabled).toBe(false);
      fireEvent.keyDown(promptBox(), { key: "Enter" });
      expect(api.studioStart).toHaveBeenCalledTimes(1);
      expect(api.studioStart).toHaveBeenCalledWith(expect.objectContaining({ prompt: "a fox" }), false);
      await flush();
    });
  });

  describe("the seed after a press", () => {
    /** The Pictures draft's seed (the local model's seed field). */
    const seedOf = () => useStudio.getState().drafts.image.values.seed;

    it("a seed from the dice is used once: after the press it is Random again", async () => {
      await show({ estimate: here() });
      typePrompt("a fox");
      act(() => useStudio.getState().newSeed("image"));
      const dice = seedOf();
      expect(typeof dice).toBe("number");
      await click(createButton());
      expect(started()[0]?.params).toMatchObject({ seed: dice });
      expect(seedOf()).toBeUndefined();
      await click(createButton());
      expect(started()[1]?.params).not.toHaveProperty("seed");
    });

    it("a refused press keeps the seed", async () => {
      await show({ estimate: here() });
      vi.mocked(api.studioStart).mockRejectedValueOnce({
        code: "studio_wont_make",
        message: "Studio won't make this.",
        nextStep: "Change the prompt.",
        details: "",
      });
      typePrompt("a fox");
      act(() => useStudio.getState().newSeed("image"));
      const dice = seedOf();
      await click(createButton());
      expect(seedOf()).toBe(dice);
    });

    it("a kept seed stays for the next press", async () => {
      await show({ estimate: here() });
      typePrompt("a fox");
      act(() => {
        useStudio.getState().newSeed("image");
        useStudio.getState().lockSeed("image", true);
      });
      const dice = seedOf();
      await click(createButton());
      expect(seedOf()).toBe(dice);
      await click(createButton());
      expect(started().map((req) => req.params.seed)).toEqual([dice, dice]);
    });

    /** The Video draft's seed (Veo 3.1's seed field). */
    const videoSeed = () => useStudio.getState().drafts.video.values.seed;
    /** The seed and the confirmation each `studio_start` was sent with. */
    const sent = () => vi.mocked(api.studioStart).mock.calls.map(([req, confirmed]) => [req.params.seed, confirmed]);

    it("a press confirmed from its estimate uses up a dice seed, and a kept one stays", async () => {
      await show({
        tab: "video",
        prefs: prefs({ lastModel: CLOUD_LAST }),
        estimate: est({ usd: 0.09, reasons: ["video"] }),
      });
      typePrompt("waves");
      act(() => useStudio.getState().newSeed("video"));
      const dice = videoSeed();
      expect(typeof dice).toBe("number");
      // The estimate for the draft as it is asks first: the confirmation opens from it.
      await estimated();
      await click(createButton());
      expect(api.studioStart).not.toHaveBeenCalled();
      await click(screen.getByRole("button", { name: "Make video" }));
      expect(sent()).toEqual([[dice, true]]);
      expect(videoSeed()).toBeUndefined();

      act(() => {
        useStudio.getState().newSeed("video");
        useStudio.getState().lockSeed("video", true);
      });
      const kept = videoSeed();
      await estimated();
      await click(createButton());
      await click(screen.getByRole("button", { name: "Make video" }));
      expect(sent()).toEqual([
        [dice, true],
        [kept, true],
      ]);
      expect(videoSeed()).toBe(kept);
    });

    it("a press the backend asks to confirm uses up a dice seed once confirmed, and a kept one stays", async () => {
      await show({ tab: "video", prefs: prefs({ lastModel: CLOUD_LAST }) });
      // The draft's estimate asks for nothing: only the backend asks, and the estimate for that
      // very press fills the confirmation.
      vi.mocked(api.studioEstimate).mockImplementation(async (req) =>
        req.clientToken === "estimate" ? est({ usd: 0.09 }) : est({ usd: 0.09, reasons: ["video"] }),
      );
      typePrompt("waves");
      act(() => useStudio.getState().newSeed("video"));
      const dice = videoSeed();
      vi.mocked(api.studioStart).mockRejectedValueOnce(NEEDS_CONFIRMATION);
      await click(createButton());
      screen.getByText("Make this video?");
      expect(videoSeed()).toBe(dice);
      await click(screen.getByRole("button", { name: "Make video" }));
      expect(sent()).toEqual([
        [dice, false],
        [dice, true],
      ]);
      expect(videoSeed()).toBeUndefined();

      act(() => {
        useStudio.getState().newSeed("video");
        useStudio.getState().lockSeed("video", true);
      });
      const kept = videoSeed();
      vi.mocked(api.studioStart).mockRejectedValueOnce(NEEDS_CONFIRMATION);
      await click(createButton());
      screen.getByText("Make this video?");
      await click(screen.getByRole("button", { name: "Make video" }));
      expect(sent().slice(2)).toEqual([
        [kept, false],
        [kept, true],
      ]);
      expect(videoSeed()).toBe(kept);
    });
  });

  it("Edit prompt elsewhere in Studio puts the cursor in the prompt", async () => {
    await show({ estimate: here() });
    await act(async () => {
      window.dispatchEvent(new Event("app:focus-studio-prompt"));
    });
    expect(document.activeElement).toBe(promptBox());
  });

  describe("pressing again from a run", () => {
    const veoRun: Run = {
      ...RUN,
      id: "veo-run",
      kind: "video",
      model: "google/veo-3.1",
      modelLabel: "Veo 3.1",
      prompt: "slow drone push over the lighthouse",
      params: { duration: 8, resolution: "720p", seed: 81723 },
      outputs: [],
    };

    it("sends the run's own settings with a new seed, and leaves the draft alone", async () => {
      await show({ estimate: here() });
      typePrompt("my own draft");
      await act(async () => {
        await useStudio.getState().start("video", false, veoRun);
      });
      const [req] = started();
      expect(req).toMatchObject({
        target: { endpoint: 1 },
        kind: "video",
        model: "google/veo-3.1",
        prompt: "slow drone push over the lighthouse",
        params: { duration: 8, resolution: "720p" },
      });
      expect(req?.params).not.toHaveProperty("seed");
      expect(useStudio.getState().drafts.image.prompt).toBe("my own draft");
    });

    it("drops the seed of a run whose model is no longer listed", async () => {
      await show({ estimate: here() });
      const gone: Run = { ...veoRun, id: "gone-run", model: "google/veo-0-gone", modelLabel: "Veo 0" };
      const goneHere: Run = {
        ...veoRun,
        id: "gone-here",
        runsOn: "local",
        providerId: null,
        model: "removed-local-model",
        params: { width: 832, height: 480, seed: 4242 },
      };
      await act(async () => {
        await useStudio.getState().start("video", false, gone);
      });
      await act(async () => {
        await useStudio.getState().start("video", false, goneHere);
      });
      const [cloud, local] = started();
      expect(cloud).toMatchObject({ target: { endpoint: 1 }, model: "google/veo-0-gone", params: { duration: 8 } });
      expect(cloud?.params).not.toHaveProperty("seed");
      expect(local).toMatchObject({ target: "thisComputer", model: "removed-local-model", params: { width: 832 } });
      expect(local?.params).not.toHaveProperty("seed");
    });

    it("sends a new seed even while the draft keeps its seed (Keep it is for Create only)", async () => {
      await show({ estimate: here() });
      act(() => useStudio.getState().lockSeed("video", true));
      await act(async () => {
        await useStudio.getState().start("video", false, veoRun);
      });
      expect(started()[0]?.params).toMatchObject({ duration: 8 });
      expect(started()[0]?.params).not.toHaveProperty("seed");
    });

    it("a run whose endpoint was removed is refused with why, under the prompt", async () => {
      await show({ estimate: here() });
      act(() => useStudio.getState().setTab("video"));
      const orphan: Run = { ...veoRun, id: "orphan", providerId: null };
      let answer: Run | null = veoRun;
      await act(async () => {
        answer = await useStudio.getState().start("video", false, orphan);
      });
      expect(answer).toBeNull();
      expect(api.studioStart).not.toHaveBeenCalled();
      expect(useStudio.getState().refusal.video?.message).toBe(STUDIO_REUSE.endpointGone);
      expect(useStudio.getState().starting).toBe(false);
    });

    it("the confirmation shows the run's settings, not the draft's", async () => {
      await show({ estimate: here() }); // the Pictures draft is the local model
      vi.mocked(api.studioStart).mockRejectedValueOnce({
        code: "studio_needs_confirmation",
        message: "Confirm this run before it is sent.",
        nextStep: "Check the price, then confirm to send it.",
        details: "",
      });
      vi.mocked(api.studioEstimate).mockResolvedValue(est({ usd: 0.96, reasons: ["video"] }));
      await act(async () => {
        await useStudio.getState().start("video", false, veoRun);
      });
      await flush();
      screen.getByText("Make this video?");
      screen.getByText("Veo 3.1 through OpenRouter");
      screen.getByText("8 s · 720p · sound on · 1 video");
    });
  });
});

describe("the prompt's own checks", () => {
  /** A number constant of a Rust source file, as written there (`4_000`). */
  function rustConst(path: string, name: string): number {
    const source = readFileSync(resolve(process.cwd(), path), "utf8");
    const found = new RegExp(`const ${name}: usize = ([\\d_]+);`).exec(source)?.[1];
    return Number(found?.replace(/_/g, ""));
  }

  it("are the backend's: the same caps, in the same words", () => {
    expect(rustConst("crates/runtime/src/sd/args.rs", "PROMPT_MAX_CHARS")).toBe(PROMPT_MAX_CHARS.thisComputer);
    expect(rustConst("crates/agent/src/media/spec.rs", "PROMPT_MAX_CHARS")).toBe(PROMPT_MAX_CHARS.cloud);
    const errors = readFileSync(resolve(process.cwd(), "src-tauri/src/studio/errors.rs"), "utf8");
    for (const sentence of [STUDIO_DOCK.writePrompt, STUDIO_DOCK.promptTooLong, STUDIO_DOCK.promptTooLongHere]) {
      expect(errors).toContain(`"${sentence}"`);
    }
  });

  it("count as the backend counts: white space as Unicode has it, characters not UTF-16 units", () => {
    const cloud = { endpoint: 1 };
    expect(promptProblems("", "thisComputer")).toEqual(["Write a prompt first."]);
    // Rust's trim takes NEL (U+0085) and the line separator, and leaves U+FEFF.
    expect(promptProblems(" \t\n \u0085 　", cloud)).toEqual(["Write a prompt first."]);
    expect(promptProblems("﻿", cloud)).toEqual([]);
    expect(promptProblems("x".repeat(4_000), "thisComputer")).toEqual([]);
    expect(promptProblems("x".repeat(4_001), "thisComputer")).toEqual([
      "Keep the prompt under 4,000 characters for local models.",
    ]);
    expect(promptProblems("🦊".repeat(4_000), "thisComputer")).toEqual([]);
    expect(promptProblems("x".repeat(32_000), cloud)).toEqual([]);
    expect(promptProblems("🦊".repeat(32_000), cloud)).toEqual([]);
    expect(promptProblems("x".repeat(32_001), cloud)).toEqual(["Keep the prompt under 32,000 characters."]);
  });

  it("leave the prompt's sentences out of an estimate's, and keep the rest", () => {
    const answer = est({
      invalid: [
        "Write a prompt first.",
        "This model takes at most 8 reference pictures.",
        "Keep the prompt under 32,000 characters.",
        "Keep the prompt under 4,000 characters for local models.",
      ],
    });
    expect(settingsProblems(answer)).toEqual(["This model takes at most 8 reference pictures."]);
    expect(settingsProblems(null)).toEqual([]);
  });
});
