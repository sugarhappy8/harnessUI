// Studio's viewer (0.1.0 Task B9; design Wireframe 4, local addendum): a result full size,
// moved through with the arrow keys; every setting, the cost and the time it took, the file
// and where it ran (for a local run: free, the device, the engine build and the AI label);
// and the actions on it: Star, Again, Reuse all and Reuse without seed (which never start a
// run), Edit, Animate, Use as reference, Save as (the settings file left off unless asked),
// Copy (Save as when the clipboard refuses), Show in folder and Delete (with what the audit
// log keeps), and Send to chat for a picture (Task B11). Esc closes the viewer and never
// cancels a run.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { alertDialogsAnimate, endAnimation } from "./fixtures/dialogs";
import { COMPOSE_IMAGES_EVENT } from "../../lib/composeImages";
import { LOCAL, LOCAL_RUN, MODELS, RUN } from "./fixtures/models";
import type { MediaModel, Output, RefItem, Run, StudioEndpoint, StudioPrefs, Usage } from "./types";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
// Nor ClipboardItem: Copy hands the clipboard one.
globalThis.ClipboardItem ??= class {
  constructor(public items: Record<string, Blob | Promise<Blob>>) {}
} as unknown as typeof ClipboardItem;

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));

const api = await import("./api");
const dialog = await import("@tauri-apps/plugin-dialog");
const { Viewer, capturePoster } = await import("./Viewer");
const { StudioView } = await import("./StudioView");
const { useStudio } = await import("../../store/studio");
const { useApp } = await import("../../store/app");

const INITIAL = useStudio.getState();

/** The fixture's models at OpenRouter (endpoint 1), with a key. */
function endpoint(models: MediaModel[] = MODELS): StudioEndpoint {
  return {
    providerId: 1,
    name: "OpenRouter",
    company: "openrouter",
    host: "openrouter.ai",
    cloud: true,
    hasKey: true,
    fetchedAt: new Date(2026, 10, 17, 9).getTime(),
    models,
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

function usage(): Usage {
  return { spentTodayUsd: 0, pendingUsd: 0, limitUsd: 10, folder: "/home/me/harnessUI/studio", bytes: 0, pictures: 0, videos: 0 };
}

const veo = MODELS.find((m) => m.id === "google/veo-3.1")!;
const grok = MODELS.find((m) => m.id === "x-ai/grok-imagine-video-1.5-lite")!;
/** A video model that takes no start or end frame. */
const noFrames: MediaModel = {
  ...veo,
  id: "test/no-frames",
  displayName: "No Frames",
  family: "test/no-frames",
  params: veo.params.filter((p) => p.key !== "first_frame" && p.key !== "last_frame"),
};
/** A picture model that takes no pictures. */
const noRefs: MediaModel = {
  ...MODELS[0]!,
  id: "test/no-refs",
  displayName: "No Refs",
  family: "test/no-refs",
  params: MODELS[0]!.params.filter((p) => p.kind.type !== "images"),
};

/** A finished OpenRouter video with one result. */
function videoRun(patch: Partial<Run> = {}, output: Partial<Output> = {}): Run {
  return {
    ...RUN,
    id: "video-run",
    kind: "video",
    model: veo.id,
    modelLabel: veo.displayName,
    prompt: "Slow push-in on a lighthouse, waves crashing",
    params: { aspect_ratio: "16:9", duration: 8, resolution: "720p", generate_audio: true },
    upstream: null,
    costUsd: 0.96,
    outputs: [
      {
        id: "v1",
        runId: "video-run",
        idx: 0,
        mime: "video/mp4",
        sizeBytes: 9_000_000,
        width: 1280,
        height: 720,
        durationMs: 8000,
        seed: null,
        starred: false,
        hasThumb: false,
        path: "/home/me/harnessUI/studio/2026-11/video-run-0.mp4",
        ...output,
      },
    ],
    ...patch,
  };
}

/** What `studio_ref_from_output` keeps for a result. */
const KEPT: RefItem = { sha256: "c".repeat(64), mime: "image/png", sizeBytes: 1_812_334, role: null };

/** Studio as it stands with `patch`: the OpenRouter endpoint, the local models, the cloud run
 *  and the local run in the gallery. */
function setStore(patch: Partial<ReturnType<typeof useStudio.getState>> = {}) {
  useStudio.setState({
    endpoints: [endpoint()],
    local: LOCAL,
    runs: [RUN, LOCAL_RUN],
    loaded: true,
    prefs: prefs(),
    usage: usage(),
    view: "studio",
    ...patch,
  });
}

/** `run` as if made now, today on this computer's clock (its cost still counts toward today's
 *  spending once deleted). */
function madeToday(run: Run): Run {
  return { ...run, createdAt: Date.now() };
}

/** The local day `days` days before today, at noon, in Unix ms. */
function daysAgo(days: number): number {
  const day = new Date();
  day.setHours(12, 0, 0, 0);
  day.setDate(day.getDate() - days);
  return day.getTime();
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Opens result `outputId` in the viewer; its `onClose`. */
async function open(outputId = "o1") {
  const onClose = vi.fn();
  await act(async () => {
    render(<Viewer outputId={outputId} onClose={onClose} />);
  });
  await flush();
  return onClose;
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

async function key(init: KeyboardEventInit) {
  await act(async () => {
    fireEvent.keyDown(window, init);
  });
  await flush();
}

const button = (name: string | RegExp) => screen.getByRole("button", { name });
const noButton = (name: string | RegExp) => expect(screen.queryByRole("button", { name })).toBeNull();
const draft = (tab: "image" | "video") => useStudio.getState().drafts[tab];

const original = { create: URL.createObjectURL, revoke: URL.revokeObjectURL };

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  URL.createObjectURL = vi.fn(() => "blob:result");
  URL.revokeObjectURL = vi.fn();
  vi.mocked(api.studioOutputBytes).mockResolvedValue(new ArrayBuffer(8));
  vi.mocked(api.studioRefBytes).mockResolvedValue(new ArrayBuffer(8));
  vi.mocked(api.studioUsage).mockResolvedValue(usage());
  vi.mocked(api.studioStar).mockImplementation(async (id, starred) => {
    const output = [...RUN.outputs, ...LOCAL_RUN.outputs].find((o) => o.id === id)!;
    return { ...output, starred };
  });
});

afterEach(() => {
  cleanup();
  URL.createObjectURL = original.create;
  URL.revokeObjectURL = original.revoke;
});

describe("Viewer", () => {
  it("arrow keys move, F stars, R reuses all, Shift+R reuses without seed, Esc closes the viewer only", async () => {
    setStore();
    const onClose = await open("o1");
    screen.getByText("1 / 4");
    await key({ key: "ArrowRight" });
    screen.getByText("2 / 4");
    await key({ key: "f" });
    expect(api.studioStar).toHaveBeenCalledWith("o2", true);
    await key({ key: "r" });
    expect(draft("image")).toMatchObject({ prompt: RUN.prompt, values: { seed: 81723 } });
    await key({ key: "R", shiftKey: true });
    expect(draft("image").prompt).toBe(RUN.prompt);
    expect(draft("image").values.seed).not.toBe(81723);
    // Reuse fills the draft and leaves the viewer open; it never starts a run.
    expect(onClose).not.toHaveBeenCalled();
    expect(api.studioStart).not.toHaveBeenCalled();
    await key({ key: "ArrowLeft" });
    screen.getByText("1 / 4");
    await key({ key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(api.studioCancel).not.toHaveBeenCalled();
  });

  it("the arrows stop at the first and the last result", async () => {
    setStore();
    await open("o4");
    screen.getByText("4 / 4");
    await key({ key: "ArrowRight" });
    screen.getByText("4 / 4");
    await click(button("Previous"));
    screen.getByText("3 / 4");
    await click(button("Next"));
    screen.getByText("4 / 4");
  });

  it("a key pressed with Ctrl or in a text box is left alone", async () => {
    setStore();
    await open("o1");
    await key({ key: "f", ctrlKey: true });
    expect(api.studioStar).not.toHaveBeenCalled();
    // A key typed into a text box goes to the box, wherever the box is.
    const box = document.createElement("input");
    box.type = "text";
    document.body.append(box);
    try {
      box.focus();
      await act(async () => {
        fireEvent.keyDown(box, { key: "f" });
        fireEvent.keyDown(box, { key: "ArrowRight" });
      });
      await flush();
      expect(api.studioStar).not.toHaveBeenCalled();
      screen.getByText("1 / 4");
    } finally {
      box.remove();
    }
  });

  it("while the delete question is open, its keys are its own", async () => {
    setStore();
    await open("o1");
    await click(button("Delete"));
    screen.getByRole("alertdialog");
    await key({ key: "f" });
    await key({ key: "ArrowRight" });
    expect(api.studioStar).not.toHaveBeenCalled();
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    await click(button("Cancel"));
    screen.getByText("1 / 4");
  });

  it("Esc that the viewer's own dialog takes closes it once", async () => {
    setStore();
    const onClose = await open("o1");
    const content = document.querySelector<HTMLElement>("[data-studio-viewer]")!;
    await act(async () => {
      fireEvent.keyDown(content, { key: "Escape" });
    });
    await flush();
    // The dialog closes on it and marks it taken; the viewer's own keys leave it alone.
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("shows every setting, the cost, the time taken, the file and where it ran", async () => {
    setStore();
    await open("o1");
    screen.getByText(RUN.prompt);
    screen.getByText("GPT Image 2.6 Flare");
    screen.getByText("$0.04 · took 13.8 s");
    screen.getByText("OpenRouter (runs at OpenAI)"); // RUN.upstream "OpenAI"
    screen.getByText("1024×1024 · seed 81723");
    screen.getByText("Shape 1:1 · How many 4 · Quality low");
    screen.getByText(RUN.outputs[0]!.path);
    expect(document.querySelector("[data-cloud-mark]")).not.toBeNull();
    expect(document.querySelector("[data-local-mark]")).toBeNull();
    screen.getByRole("button", { name: "See the exact request" });
  });

  it("a run OpenRouter chose the company for says only OpenRouter", async () => {
    setStore({ runs: [{ ...RUN, upstream: null }] });
    await open("o1");
    screen.getByText("OpenRouter");
    expect(screen.queryByText(/runs at/)).toBeNull();
  });

  it("the model's notes show, such as a watermark", async () => {
    setStore({ runs: [videoRun()] });
    await open("v1");
    screen.getByText("Google adds an invisible SynthID watermark.");
  });

  it("a local result says it was free, how long it took, where it ran and that the file is labelled", async () => {
    // LOCAL_RUN: elapsedMs 228000, device "Radeon RX 7600 (Vulkan)", engineBuild 1106, labelled
    setStore();
    await open("l1");
    screen.getByText("Cost: free · took 3 min 48 s · on Radeon RX 7600 (Vulkan) · engine build 1106");
    screen.getByText("Labelled as AI-made in the file");
    screen.getByRole("button", { name: "See the exact command" });
    expect(document.querySelector("[data-cloud-mark]")).toBeNull();
    expect(document.querySelector("[data-local-mark]")).not.toBeNull();
  });

  it("See the exact command shows the command the run would be", async () => {
    setStore();
    vi.mocked(api.studioPreview).mockResolvedValue({
      method: "run",
      url: "",
      host: "",
      body: { argv: ["sd-cli", "--steps", "4"], prompt: LOCAL_RUN.prompt },
    });
    await open("l1");
    await click(button("See the exact command"));
    expect(api.studioPreview).toHaveBeenCalledWith(
      expect.objectContaining({ target: "thisComputer", model: LOCAL_RUN.model, params: LOCAL_RUN.params }),
    );
    screen.getByText("--steps");
    screen.getByText(`Prompt (passed in prompt.txt): ${LOCAL_RUN.prompt}`);
  });

  it("Delete says what the audit log keeps", async () => {
    setStore({ runs: [madeToday(RUN), LOCAL_RUN] });
    vi.mocked(api.studioDelete).mockResolvedValue(undefined);
    await open("o1");
    await click(button("Delete"));
    screen.getByText(
      "The audit log keeps a line saying a run happened (model and cost), but not your prompt. It still counts toward today's spending.",
    );
    await click(button("Delete"));
    expect(api.studioDelete).toHaveBeenCalledWith(["o1"]);
    // The next result of the run takes its place.
    screen.getByText("1 / 3");
    expect(useStudio.getState().runs[0]!.outputs.map((o) => o.id)).toEqual(["o2", "o3", "o4"]);
  });

  it("while the question closes it keeps its words, and a second click deletes nothing more", async () => {
    // In the app the question fades out for 130 ms, its buttons still there, with the next
    // result already behind it: a double-click's second click must not delete that one.
    alertDialogsAnimate();
    setStore({ runs: [madeToday(RUN), LOCAL_RUN] });
    vi.mocked(api.studioDelete).mockResolvedValue(undefined);
    await open("o1");
    await click(button("Delete"));
    const question = screen.getByRole("alertdialog");
    const confirm = within(question).getByRole("button", { name: "Delete" });
    await click(confirm);
    expect(question.getAttribute("data-state")).toBe("closed");
    within(question).getByText("Delete this picture?");
    within(question).getByText("The file and its record are removed from the gallery.");
    within(question).getByText(
      "The audit log keeps a line saying a run happened (model and cost), but not your prompt. It still counts toward today's spending.",
    );
    screen.getByText("1 / 3");
    await click(confirm);
    expect(api.studioDelete).toHaveBeenCalledTimes(1);
    expect(api.studioDelete).toHaveBeenCalledWith(["o1"]);
    expect(useStudio.getState().runs[0]!.outputs.map((o) => o.id)).toEqual(["o2", "o3", "o4"]);
    await endAnimation(question);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("deleting a cloud result made on an earlier day says only what the audit log keeps", async () => {
    setStore({ runs: [{ ...RUN, createdAt: daysAgo(1) }] });
    await open("o1");
    await click(button("Delete"));
    screen.getByText("The audit log keeps a line saying a run happened (model and cost), but not your prompt.");
    expect(screen.queryByText(/today's spending/)).toBeNull();
  });

  it("deleting a local result says only what the audit log keeps", async () => {
    setStore();
    await open("l1");
    await click(button("Delete"));
    screen.getByText("The audit log keeps a line saying a run happened (model and cost), but not your prompt.");
    expect(screen.queryByText(/today's spending/)).toBeNull();
  });

  it("Delete can be cancelled, and the last result deleted closes the viewer", async () => {
    setStore({ runs: [{ ...RUN, outputs: [RUN.outputs[0]!] }] });
    vi.mocked(api.studioDelete).mockResolvedValue(undefined);
    const onClose = await open("o1");
    await click(button("Delete"));
    await click(button("Cancel"));
    expect(api.studioDelete).not.toHaveBeenCalled();
    await key({ key: "Delete" });
    await click(button("Delete"));
    expect(api.studioDelete).toHaveBeenCalledWith(["o1"]);
    expect(onClose).toHaveBeenCalledTimes(1);
    // A finished run left without results goes too.
    expect(useStudio.getState().runs).toEqual([]);
  });

  it("a result that leaves its run while shown gives way to the nearest one left", async () => {
    setStore();
    let deleted: () => void = () => undefined;
    vi.mocked(api.studioDelete).mockReturnValue(new Promise((resolve) => (deleted = () => resolve(undefined))));
    await open("o2");
    await click(button("Delete"));
    await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    // The next result shows while the delete is on its way; ← goes back to the one going.
    screen.getByText("3 / 4");
    await key({ key: "ArrowLeft" });
    screen.getByText("2 / 4");
    await act(async () => deleted());
    await flush();
    // It has gone: the result nearest its place shows.
    screen.getByText("2 / 3");
    screen.getByText(RUN.outputs[2]!.path);
  });

  it("the viewer closes once its run has no result left", async () => {
    setStore({ runs: [{ ...RUN, outputs: [RUN.outputs[0]!] }] });
    const onClose = await open("o1");
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => {
      useStudio.setState({ runs: [] });
    });
    await flush();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("Save as leaves the settings file off unless asked", async () => {
    setStore();
    vi.mocked(dialog.save).mockResolvedValue("/home/me/Pictures/lighthouse.png");
    vi.mocked(api.studioExport).mockResolvedValue("/home/me/Pictures/lighthouse.png");
    await open("o1");
    await click(button("Save as…"));
    expect(dialog.save).toHaveBeenCalledWith(
      expect.objectContaining({ defaultPath: "cloud-run-0.png", filters: [expect.objectContaining({ extensions: ["png"] })] }),
    );
    expect(api.studioExport).toHaveBeenCalledWith("o1", "/home/me/Pictures/lighthouse.png", false);
    await click(screen.getByRole("checkbox", { name: "Also save the settings as a .json file" }));
    await click(button("Save as…"));
    expect(api.studioExport).toHaveBeenLastCalledWith("o1", "/home/me/Pictures/lighthouse.png", true);
  });

  it("Save as that is cancelled saves nothing", async () => {
    setStore();
    vi.mocked(dialog.save).mockResolvedValue(null);
    await open("o1");
    await key({ key: "s" });
    expect(dialog.save).toHaveBeenCalledTimes(1);
    expect(api.studioExport).not.toHaveBeenCalled();
  });

  it("Reuse all fills the prompt and every setting and starts nothing", async () => {
    setStore();
    await open("o1");
    await click(button("Reuse all"));
    expect(api.studioStart).not.toHaveBeenCalled();
    expect(draft("image")).toMatchObject({
      target: { endpoint: 1 },
      model: RUN.model,
      prompt: RUN.prompt,
      values: RUN.params,
      extra: RUN.extra,
    });
    screen.getByText("Filled in the prompt and settings. Nothing was started.");
  });

  it("Reuse without seed fills everything but the seed", async () => {
    setStore();
    await open("o1");
    await click(button("Reuse without seed"));
    const { seed, ...rest } = RUN.params;
    expect(seed).toBe(81723);
    expect(draft("image").values).toEqual(rest);
    expect(api.studioStart).not.toHaveBeenCalled();
  });

  it("Again pressed twice while starting starts once", async () => {
    setStore();
    vi.mocked(api.studioStart).mockReturnValue(new Promise(() => {})); // still on its way
    await open("o1");
    fireEvent.click(button("Again"));
    fireEvent.click(button("Again"));
    await flush();
    expect(api.studioStart).toHaveBeenCalledTimes(1);
    expect(api.studioStart).toHaveBeenCalledWith(
      expect.objectContaining({ model: RUN.model, prompt: RUN.prompt, target: { endpoint: 1 } }),
      false,
    );
  });

  it("Reuse on a retired model picks the closest one and says what changed", async () => {
    const retired: Run = { ...RUN, model: "openai/gpt-image-1.5", modelLabel: "GPT Image 1.5" };
    setStore({ runs: [retired] });
    await open("o1");
    await click(button("Reuse all"));
    expect(draft("image")).toMatchObject({ model: "openai/gpt-image-2.6-flare", prompt: RUN.prompt });
    expect(draft("image").note).toMatch(/^OpenRouter no longer offers this model, so GPT Image 2.6 Flare is used\./);
    expect(api.studioStart).not.toHaveBeenCalled();
  });

  it("Reuse on a retired model with none in its family says so and changes nothing", async () => {
    const retired: Run = { ...RUN, model: "acme/painter-2", modelLabel: "Painter 2" };
    setStore({ runs: [retired] });
    await open("o1");
    await click(button("Reuse all"));
    screen.getByText("OpenRouter no longer offers this model.");
    expect(draft("image")).toMatchObject({ model: null, prompt: "" });
  });

  it("Animate switches to Video with the picture as the start frame", async () => {
    // No local video model is installed; the video draft is on a model without frames, and
    // Grok Imagine Video 1.5 Lite was used last.
    const used: Run = videoRun({ id: "grok-run", model: grok.id, modelLabel: grok.displayName, createdAt: RUN.createdAt + 10 });
    setStore({
      endpoints: [endpoint([...MODELS, noFrames])],
      local: { ...LOCAL, installed: LOCAL.installed.filter((m) => m.kind === "image") },
      recentRuns: [used, RUN],
    });
    useStudio.getState().chooseModel("video", { endpoint: 1 }, noFrames.id);
    vi.mocked(api.studioRefFromOutput).mockResolvedValue(KEPT);
    const onClose = await open("o1");
    await click(button("Animate"));
    screen.getByText("No Frames takes no start frame. Use Grok Imagine Video 1.5 Lite instead?");
    expect(api.studioRefFromOutput).not.toHaveBeenCalled();
    await click(button("Use Grok Imagine Video 1.5 Lite"));
    expect(api.studioRefFromOutput).toHaveBeenCalledWith("o1");
    expect(useStudio.getState()).toMatchObject({ tab: "video", drafts: { video: { model: grok.id } } });
    expect(draft("video").refs[0]).toMatchObject({ sha256: KEPT.sha256, role: "startFrame" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("Animate offers the installed local video model before a cloud one", async () => {
    setStore({ endpoints: [endpoint([...MODELS, noFrames])], recentRuns: [videoRun({ model: grok.id })] });
    useStudio.getState().chooseModel("video", { endpoint: 1 }, noFrames.id);
    await open("o1");
    await click(button("Animate"));
    screen.getByText("No Frames takes no start frame. Use Wan 2.2 5B, fast instead?");
    button("Use Wan 2.2 5B, fast");
  });

  it("Animate on a video model that takes a start frame goes straight to it", async () => {
    setStore();
    useStudio.getState().chooseModel("video", { endpoint: 1 }, veo.id);
    vi.mocked(api.studioRefFromOutput).mockResolvedValue(KEPT);
    const onClose = await open("o1");
    await key({ key: "a" });
    expect(useStudio.getState().tab).toBe("video");
    expect(draft("video")).toMatchObject({ model: veo.id, refs: [{ sha256: KEPT.sha256, role: "startFrame" }] });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("Edit on a model that takes no pictures offers one that does, local first", async () => {
    setStore({ endpoints: [endpoint([...MODELS, noRefs])] });
    useStudio.getState().chooseModel("image", { endpoint: 1 }, noRefs.id);
    await open("o1");
    await click(button("Edit"));
    screen.getByText("No Refs cannot change a picture. Use FLUX.2 klein 4B instead?");
    await click(button("Not now"));
    expect(screen.queryByText(/cannot change a picture/)).toBeNull();
  });

  it("Use as reference adds the picture to the tab's added pictures", async () => {
    setStore();
    useStudio.getState().chooseModel("image", { endpoint: 1 }, RUN.model);
    vi.mocked(api.studioRefFromOutput).mockResolvedValue(KEPT);
    const onClose = await open("o2");
    await click(button("Use as reference"));
    expect(api.studioRefFromOutput).toHaveBeenCalledWith("o2");
    expect(draft("image").refs).toEqual([{ ...KEPT, role: "reference" }]);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("Star and Show in folder act on the result shown", async () => {
    setStore();
    vi.mocked(api.studioShowInFolder).mockResolvedValue(undefined);
    await open("o3");
    const star = button("Star");
    expect(star.getAttribute("aria-pressed")).toBe("false");
    await click(star);
    expect(api.studioStar).toHaveBeenCalledWith("o3", true);
    expect(button("Star").getAttribute("aria-pressed")).toBe("true");
    expect(useStudio.getState().runs[0]!.outputs[2]!.starred).toBe(true);
    await click(button("Show in folder"));
    expect(api.studioShowInFolder).toHaveBeenCalledWith("o3");
  });

  it("Copy falls back to Save as when the clipboard refuses", async () => {
    setStore();
    const write = vi.fn().mockRejectedValue(new DOMException("Not allowed", "NotAllowedError"));
    Object.defineProperty(navigator, "clipboard", { value: { write }, configurable: true });
    await open("o1");
    await click(button("Copy"));
    expect(write).toHaveBeenCalledTimes(1);
    screen.getByText("Copy is not available here. Use Save as… instead.");
  });

  it("Copy writes a PNG result to the clipboard", async () => {
    setStore();
    const write = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { write }, configurable: true });
    await open("o1");
    await key({ key: "c" });
    await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
    screen.getByText("Copied");
  });

  it("an SVG result is shown with an img element only", async () => {
    const svg: Run = {
      ...RUN,
      outputs: [{ ...RUN.outputs[0]!, mime: "image/svg+xml", hasThumb: false, path: "/home/me/s/cloud-run-0.svg" }],
    };
    setStore({ runs: [svg] });
    await open("o1");
    expect(api.studioOutputBytes).toHaveBeenCalledWith("o1", false);
    expect(document.body.querySelector("object, iframe, embed")).toBeNull();
    expect(document.body.querySelector("img[src='blob:result']")).not.toBeNull();
    const made = vi.mocked(URL.createObjectURL).mock.calls[0]![0] as Blob;
    expect(made.type).toBe("image/svg+xml");
  });

  it("a video plays in the viewer, without the picture-only actions", async () => {
    setStore({ runs: [videoRun()] });
    await open("v1");
    expect(screen.getByTestId("studio-video").getAttribute("src")).toBe("blob:result");
    screen.getByText("1280×720 · 8 s");
    for (const name of ["Copy", "Edit", "Animate", "Use as reference"]) noButton(name);
    button("Again");
    button("Reuse all");
  });

  it("the blob address is let go when the viewer closes", async () => {
    setStore();
    await open("o1");
    cleanup();
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:result");
  });

  it("showing the chat closes the viewer, so it never stays over the chat", async () => {
    await showStudio();
    await act(async () => {
      useStudio.getState().openViewer("o1");
    });
    await flush();
    expect(document.querySelector("[data-studio-viewer]")).not.toBeNull();
    await act(async () => {
      useStudio.getState().showChat();
    });
    await flush();
    expect(useStudio.getState().viewer).toBeNull();
    expect(document.querySelector("[data-studio-viewer]")).toBeNull();
  });

  it("Edit puts the picture in the tray and focuses the prompt", async () => {
    await showStudio();
    await act(async () => {
      useStudio.getState().openViewer("o1");
    });
    await flush();
    await click(button("Edit"));
    expect(draft("image").refs.map((r) => r.role)).toEqual(["reference"]);
    expect(useStudio.getState().viewer).toBeNull();
    const promptBox = screen.getByRole("textbox", { name: "Prompt" });
    await waitFor(() => expect(document.activeElement).toBe(promptBox));
  });
});

// [0.1.0 B11] Send to chat: the picture as the composer takes one (`studio_output_for_chat`, which
// refuses a video and a picture over 20 MB) goes into the chat's message box through the window
// event `app:compose-images`, and the chat is shown (a new chat when none is open). From there it
// goes through the chat's own send path, so deleting it from the gallery never breaks the chat.
describe("Send to chat", () => {
  /** The picture `studio_output_for_chat` gives. */
  const PICTURE = { mime: "image/png", fileName: "lighthouse-1.png", data: "iVBORw0KGgo=" };
  let composed: unknown[] = [];
  const onCompose = (e: Event) => composed.push((e as CustomEvent<unknown>).detail);
  const INITIAL_APP = useApp.getState();

  beforeEach(() => {
    composed = [];
    window.addEventListener(COMPOSE_IMAGES_EVENT, onCompose);
    useApp.setState(INITIAL_APP, true);
    useApp.setState({ activeChatId: 3 });
    vi.mocked(api.studioOutputForChat).mockResolvedValue(PICTURE);
  });
  afterEach(() => {
    window.removeEventListener(COMPOSE_IMAGES_EVENT, onCompose);
  });

  it("Send to chat puts the picture in the message box", async () => {
    setStore();
    await open("o2");
    await click(button("Send to chat"));
    expect(api.studioOutputForChat).toHaveBeenCalledWith("o2");
    expect(useStudio.getState()).toMatchObject({ view: "chat", viewer: null });
    expect(composed).toEqual([[PICTURE]]);
  });

  it("with no chat open, a new chat is made first and gets it", async () => {
    const newChat = vi.fn(async () => {
      useApp.setState({ activeChatId: 9 });
    });
    useApp.setState({ activeChatId: null, newChat });
    setStore();
    await open("o1");
    await click(button("Send to chat"));
    expect(newChat).toHaveBeenCalledTimes(1);
    expect(useStudio.getState().view).toBe("chat");
    expect(composed).toEqual([[PICTURE]]);
  });

  it("Send to chat is hidden for videos", async () => {
    setStore({ runs: [videoRun()] });
    await open("v1");
    expect(screen.queryByRole("button", { name: "Send to chat" })).toBeNull();
  });

  it("a picture over 20 MB cannot be sent and says why", async () => {
    vi.mocked(api.studioOutputForChat).mockRejectedValue({
      code: "bad_request",
      message: "This picture is over 20 MB, so it cannot go into a chat.",
      nextStep: "",
      details: "",
    });
    setStore();
    await open("o1");
    await click(button("Send to chat"));
    await screen.findByText("This picture is over 20 MB, so it cannot go into a chat.");
    expect(useStudio.getState().view).toBe("studio");
    expect(composed).toEqual([]);
  });

  it("a new chat that cannot be made leaves Studio as it was and says why", async () => {
    const newChat = vi.fn(async () => {
      throw { code: "internal", message: "The chat could not be made.", nextStep: "", details: "" };
    });
    useApp.setState({ activeChatId: null, newChat });
    setStore();
    await open("o1");
    await click(button("Send to chat"));
    await screen.findByText("The chat could not be made.");
    expect(useStudio.getState().view).toBe("studio");
    expect(composed).toEqual([]);
  });
});

// [0.1.0 B12] A video plays from a `blob:` made of its bytes (`studio_output_bytes`; the CSP's
// `media-src blob:`). The first time a video without a thumbnail plays, the frame shown is drawn
// on a canvas at most 512 px on its longest side and saved as its thumbnail
// (`studio_save_thumb`), which its tile shows from then on. A video over 256 MiB is not played
// here: the viewer says so, as a limit and not a fault, and Show in folder opens it elsewhere.
describe("Video playback and poster frames", () => {
  /** The video `o9`, its poster frame not saved yet. */
  const nine = (output: Partial<Output> = {}) => videoRun({}, { id: "o9", ...output });
  let drawImage = vi.fn();
  let toDataURL = vi.fn((): string => "data:image/jpeg;base64,AAAA");

  /** A video element whose frame is `width` × `height` (0 × 0: no frame yet). */
  function sized(video: HTMLVideoElement, width: number, height: number): HTMLVideoElement {
    Object.defineProperty(video, "videoWidth", { value: width, configurable: true });
    Object.defineProperty(video, "videoHeight", { value: height, configurable: true });
    return video;
  }

  async function playing(video: HTMLElement) {
    await act(async () => {
      fireEvent.playing(video);
    });
    await flush();
  }

  beforeEach(() => {
    drawImage = vi.fn();
    toDataURL = vi.fn((): string => "data:image/jpeg;base64,AAAA");
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
      () => ({ drawImage }) as unknown as CanvasRenderingContext2D,
    );
    vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockImplementation(toDataURL);
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("a video plays from a blob: address", async () => {
    setStore({ runs: [nine()] });
    await open("o9");
    expect(screen.getByTestId("studio-video").getAttribute("src")).toMatch(/^blob:/);
    expect(api.studioOutputBytes).toHaveBeenCalledWith("o9", false);
    const made = vi.mocked(URL.createObjectURL).mock.calls[0]![0] as Blob;
    expect(made.type).toBe("video/mp4");
  });

  it("the first play saves a poster frame", async () => {
    setStore({ runs: [nine()] });
    let saved: (output: Output) => void = () => undefined;
    vi.mocked(api.studioSaveThumb).mockImplementation(() => new Promise((resolve) => (saved = resolve)));
    await open("o9");
    const video = sized(screen.getByTestId("studio-video") as HTMLVideoElement, 1280, 720);
    await playing(video);
    // Played again before the save has answered: the poster is not taken twice.
    await playing(video);
    expect(api.studioSaveThumb).toHaveBeenCalledTimes(1);
    expect(api.studioSaveThumb).toHaveBeenCalledWith("o9", "AAAA");
    expect(drawImage).toHaveBeenCalledWith(video, 0, 0, 512, 288);
    expect(toDataURL).toHaveBeenCalledWith("image/jpeg", 0.8);
    await act(async () => saved({ ...nine().outputs[0]!, hasThumb: true }));
    await flush();
    // Its tile shows the poster from now on.
    expect(useStudio.getState().runs[0]!.outputs[0]!.hasThumb).toBe(true);
    await playing(video);
    expect(api.studioSaveThumb).toHaveBeenCalledTimes(1);
  });

  it("a video that has its thumbnail saves no poster", async () => {
    setStore({ runs: [nine({ hasThumb: true })] });
    await open("o9");
    await playing(sized(screen.getByTestId("studio-video") as HTMLVideoElement, 1280, 720));
    expect(api.studioSaveThumb).not.toHaveBeenCalled();
  });

  it("a play before the first frame has its size takes the poster at the next play", async () => {
    setStore({ runs: [nine()] });
    vi.mocked(api.studioSaveThumb).mockResolvedValue({ ...nine().outputs[0]!, hasThumb: true });
    await open("o9");
    const video = sized(screen.getByTestId("studio-video") as HTMLVideoElement, 0, 0);
    await playing(video);
    expect(api.studioSaveThumb).not.toHaveBeenCalled();
    await playing(sized(video, 720, 1280));
    expect(api.studioSaveThumb).toHaveBeenCalledExactlyOnceWith("o9", "AAAA");
    expect(drawImage).toHaveBeenCalledWith(video, 0, 0, 288, 512);
  });

  it("a poster that cannot be saved leaves the viewer as it was", async () => {
    setStore({ runs: [nine()] });
    vi.mocked(api.studioSaveThumb).mockRejectedValue(appError("bad_request", "That poster frame is not a small JPEG."));
    await open("o9");
    await playing(sized(screen.getByTestId("studio-video") as HTMLVideoElement, 1280, 720));
    expect(api.studioSaveThumb).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(useStudio.getState().runs[0]!.outputs[0]!.hasThumb).toBe(false);
  });

  it("capturePoster gives the frame as base64 JPEG, at most 512 px on its longest side", () => {
    const video = sized(document.createElement("video"), 1920, 1080);
    expect(capturePoster(video)).toBe("AAAA");
    expect(drawImage).toHaveBeenLastCalledWith(video, 0, 0, 512, 288);
    // A small video is not enlarged.
    capturePoster(sized(video, 320, 240));
    expect(drawImage).toHaveBeenLastCalledWith(video, 0, 0, 320, 240);
    // A very thin one keeps at least one pixel on its short side.
    capturePoster(sized(video, 4096, 2));
    expect(drawImage).toHaveBeenLastCalledWith(video, 0, 0, 512, 1);
  });

  it("capturePoster gives nothing without a frame, a canvas or a JPEG", () => {
    const video = sized(document.createElement("video"), 0, 480);
    expect(capturePoster(video)).toBeNull();
    expect(capturePoster(sized(video, 640, 0))).toBeNull();
    sized(video, 640, 480);
    toDataURL.mockReturnValueOnce("data:image/png;base64,BBBB");
    expect(capturePoster(video)).toBeNull();
    toDataURL.mockReturnValueOnce("data:,");
    expect(capturePoster(video)).toBeNull();
    toDataURL.mockImplementationOnce(() => {
      throw new DOMException("The canvas has been tainted", "SecurityError");
    });
    expect(capturePoster(video)).toBeNull();
    vi.mocked(HTMLCanvasElement.prototype.getContext).mockReturnValueOnce(null);
    expect(capturePoster(video)).toBeNull();
    expect(capturePoster(video)).toBe("AAAA");
  });

  it("a video too big to play here offers Show in folder", async () => {
    setStore({ runs: [nine({ sizeBytes: 300 * 1024 * 1024 })] });
    vi.mocked(api.studioOutputBytes).mockRejectedValue(
      appError(
        "studio_too_big_to_show",
        "This video is too big to play here.",
        "Use Show in folder to open it with your video player.",
      ),
    );
    vi.mocked(api.studioShowInFolder).mockResolvedValue(undefined);
    await open("o9");
    await screen.findByText("This video is too big to play here.");
    screen.getByText("Use Show in folder to open it with your video player.");
    // A limit, not a fault: said as a notice.
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByTestId("studio-video")).toBeNull();
    await click(screen.getByRole("button", { name: "Show in folder" }));
    expect(api.studioShowInFolder).toHaveBeenCalledWith("o9");
  });
});

/** An error as the backend sends one. */
function appError(code: string, message: string, nextStep = "") {
  return { code, message, nextStep, details: "" };
}

/** Studio shown, its saved things read: the OpenRouter endpoint with the cloud run, on the
 *  Pictures tab at the run's model. */
async function showStudio() {
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [endpoint()] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(LOCAL);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [RUN], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(
    prefs({ lastModel: { image: { target: { endpoint: 1 }, model: RUN.model } } }),
  );
  vi.mocked(api.studioEstimate).mockResolvedValue({
    usd: 0.04,
    reasons: [],
    confirmAtUsd: 1,
    limit: { limitUsd: 10, spentTodayUsd: 0, pendingUsd: 0, wouldPass: true },
    invalid: [],
    eta: null,
    local: null,
  });
  vi.mocked(api.studioRefFromOutput).mockResolvedValue(KEPT);
  useStudio.getState().showStudio();
  await act(async () => {
    render(<StudioView />);
  });
  await flush();
}
