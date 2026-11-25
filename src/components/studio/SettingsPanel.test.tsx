// Studio's side panel (0.1.0 Task B5; shown only when asked for since Task U3): the Model field
// and where the model runs, the simple fields in the model's order (a video's length and
// resolution at their cheap defaults, a local video's chips each with its own time here), and
// the Advanced toggle with its change count. Shape and How many's menus and the picture zones
// are the composer's (Composer.test.tsx).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { StudioTab } from "../../store/studio";
import type { Values } from "./canonical";
import { LOCAL, MODELS } from "./fixtures/models";
import type { Eta, LocalFit, LocalModels, MediaModel, RunTarget, StudioEndpoint, StudioPrefs } from "./types";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { SettingsPanel } = await import("./SettingsPanel");
const { localClipSize } = await import("./SettingField");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

function model(id: string): MediaModel {
  const found = MODELS.find((m) => m.id === id);
  if (!found) throw new Error(`no fixture model ${id}`);
  return found;
}
const flare = model("openai/gpt-image-2.6-flare");
const nano = model("google/gemini-nano-banana-2.1");
const veo = model("google/veo-3.1");
const klein = LOCAL.installed[0]!.model;
const wan = LOCAL.installed[1]!.model;

/** OpenRouter with `models`, as `studio_models` gives it. */
function endpoint(models: MediaModel[] = MODELS): StudioEndpoint {
  return {
    providerId: 1,
    name: "OpenRouter",
    company: "openrouter",
    host: "openrouter.ai",
    cloud: true,
    hasKey: true,
    fetchedAt: 1794909600000,
    models,
  };
}

function prefs(patch: Partial<StudioPrefs> = {}): StudioPrefs {
  return {
    lastModel: {},
    advancedOpen: false,
    settingsOpen: false,
    confirmEveryVideo: true,
    confirmAtUsd: 1,
    dailyLimitUsd: 10,
    setups: [],
    galleryDir: null,
    gpuPolicy: "unload",
    reloadChatAfter: false,
    suggestionDismissed: true,
    localNoticeAcceptedAt: null,
    lastImportMode: null,
    ...patch,
  };
}

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Reads what the backend has saved, as Studio does when it opens; each tab gets its model. */
async function load(saved: { endpoints?: StudioEndpoint[]; local?: LocalModels; prefs?: StudioPrefs } = {}) {
  const p = saved.prefs ?? prefs();
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: saved.endpoints ?? [endpoint()] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(saved.local ?? LOCAL);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(p);
  vi.mocked(api.studioSetPrefs).mockImplementation(async (patch) => ({ ...p, ...patch }));
  vi.mocked(api.studioUsage).mockResolvedValue({
    spentTodayUsd: 0,
    pendingUsd: 0,
    limitUsd: 10,
    folder: "/home/me/harnessUI/studio",
    bytes: 0,
    pictures: 0,
    videos: 0,
  });
  await act(async () => {
    await useStudio.getState().load();
  });
}

/** Studio open on `m` (used last, so its draft starts at its own starting values), at
 *  OpenRouter unless `target` says this computer; `values` replaces the draft's settings. */
async function renderPanel(m: MediaModel, opts: { target?: RunTarget; values?: Values } = {}) {
  const target = opts.target ?? { endpoint: 1 };
  const tab: StudioTab = m.kind;
  const models = target === "thisComputer" ? MODELS : [m, ...MODELS.filter((x) => x.id !== m.id)];
  await load({ endpoints: [endpoint(models)], prefs: prefs({ lastModel: { [tab]: { target, model: m.id } } }) });
  if (opts.values) {
    const values = opts.values;
    useStudio.setState((s) => ({ drafts: { ...s.drafts, [tab]: { ...s.drafts[tab], values } } }));
  }
  useStudio.getState().showStudio(tab);
  await act(async () => {
    render(<SettingsPanel tab={tab} />);
  });
  await flush();
}

/** A field of the panel, by its label. */
function field(label: string): HTMLElement {
  return screen.getByRole("group", { name: label });
}

function checked(radio: HTMLElement): boolean {
  return radio.getAttribute("aria-checked") === "true";
}

/** How long a clip takes here, by its length and the number of lines (a measured time unless
 *  `firstGuess`): 2 s at 480p about 3 min, 5 s about 9 min, 2 s at 720p about 10 min. */
function localFit(seconds: number, height: number, firstGuess = false): LocalFit {
  const minutes: Record<number, number> = { 1: 1.5, 2: 3, 3: 5, 4: 7, 5: 9 };
  const middle = (height >= 704 ? 10 : (minutes[seconds] ?? 1)) * 60_000;
  const eta: Eta = { lowMs: middle - 30_000, highMs: middle + 30_000, measured: !firstGuess, leftMs: null };
  return { word: "fits", verdict: "green", reason: null, suggestion: null, eta, verySlow: false, videoOnProcessor: false };
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

afterEach(() => {
  cleanup();
});

describe("SettingsPanel", () => {
  it("shows only the simple fields the model declares", async () => {
    await renderPanel(nano);
    expect(screen.queryByText("How many")).toBeNull();
    expect(screen.queryByText("Quality")).toBeNull();
    expect(within(field("Resolution")).getAllByRole("radio").map((r) => r.textContent)).toEqual(["1K", "2K", "4K"]);
    // Its reference pictures are added in the tray, not here.
    expect(screen.queryByText("Reference pictures")).toBeNull();
  });

  it("draws the simple fields in the model's order, each Automatic until set", async () => {
    await renderPanel(flare);
    expect(screen.getAllByRole("group").map((g) => g.getAttribute("data-param"))).toEqual(["aspect_ratio", "n", "quality"]);
    within(field("Shape")).getByText("Automatic (company default)");
    within(field("How many")).getByText("Automatic (company default)");
    expect((within(field("Quality")).getByRole("combobox") as HTMLSelectElement).value).toBe("");
  });

  it("starts a video at the shortest length of at least 4 s and 720p", async () => {
    await renderPanel(veo);
    expect(checked(within(field("Length")).getByRole("radio", { name: "4 s" }))).toBe(true);
    expect(checked(within(field("Resolution")).getByRole("radio", { name: "720p" }))).toBe(true);
    expect((screen.getByRole("switch", { name: "Sound" }) as HTMLInputElement).checked).toBe(true);
    expect(useStudio.getState().drafts.video.values).toEqual({ duration: 4, resolution: "720p" });
  });

  it("a local video starts at 2 s and 480p, and each chip shows its own time", async () => {
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h));
    await renderPanel(wan, { target: "thisComputer" });
    await flush();
    const length = within(field("Length"));
    expect(checked(length.getByRole("radio", { name: "2 s · about 3 min" }))).toBe(true);
    length.getByRole("radio", { name: "5 s · about 9 min" });
    const resolution = within(field("Resolution"));
    expect(checked(resolution.getByRole("radio", { name: "480p · about 3 min" }))).toBe(true);
    resolution.getByRole("radio", { name: "720p · about 10 min" });

    // Each chip at its own size, once: a Length chip at the draft's 480p and 16:9 (832 × 480),
    // a Resolution chip at the draft's 2 s; nothing goes anywhere for this.
    const source = { installed: { genModelId: wan.id } };
    expect(api.studioLocalFit).toHaveBeenCalledWith(source, 832, 480, 2);
    expect(api.studioLocalFit).toHaveBeenCalledWith(source, 832, 480, 5);
    expect(api.studioLocalFit).toHaveBeenCalledWith(source, 1280, 704, 2);
    expect(api.studioLocalFit).toHaveBeenCalledTimes(6);
    expect(api.studioModelPrice).not.toHaveBeenCalled();
  });

  it("a chip's time not yet measured here says it is a first guess", async () => {
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h, true));
    await renderPanel(wan, { target: "thisComputer" });
    await flush();
    within(field("Length")).getByRole("radio", { name: "2 s · about 3 min (first guess)" });
  });

  it("asks again only for the chips whose size changed", async () => {
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h));
    await renderPanel(wan, { target: "thisComputer" });
    await flush();
    vi.mocked(api.studioLocalFit).mockClear();
    fireEvent.click(within(field("Length")).getByRole("radio", { name: /^5 s/ }));
    await flush();
    // Only the Resolution chips follow the length; 5 s at 480p was asked already.
    expect(api.studioLocalFit).toHaveBeenCalledTimes(1);
    expect(api.studioLocalFit).toHaveBeenCalledWith({ installed: { genModelId: wan.id } }, 1280, 704, 5);
  });

  it("asks again once the local models are read again, as a run may have been measured since", async () => {
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h, true));
    await renderPanel(wan, { target: "thisComputer" });
    await flush();
    expect(api.studioLocalFit).toHaveBeenCalledTimes(6);
    vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h));
    await act(async () => {
      await useStudio.getState().reread();
    });
    await flush();
    expect(api.studioLocalFit).toHaveBeenCalledTimes(12);
    within(field("Length")).getByRole("radio", { name: "2 s · about 3 min" });
  });

  it("a time that could not be worked out is not asked again until the local models are read again", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      // 720p at 2 s always fails; every other chip has its time.
      vi.mocked(api.studioLocalFit).mockImplementation(async (_source, w, h, seconds) => {
        if (w === 1280 && seconds === 2) throw new Error("no fit");
        return localFit(seconds, h);
      });
      await renderPanel(wan, { target: "thisComputer" });
      await flush();
      expect(api.studioLocalFit).toHaveBeenCalledTimes(6);
      // The chip shows no time; the others do.
      within(field("Resolution")).getByRole("radio", { name: "720p" });
      within(field("Resolution")).getByRole("radio", { name: "480p · about 3 min" });

      // Another field changed, and the store asked directly: neither asks again.
      act(() => useStudio.getState().setValue("video", "seed", 42));
      await flush();
      act(() => useStudio.getState().askLocalFit(wan.id, 1280, 704, 2));
      await flush();
      expect(api.studioLocalFit).toHaveBeenCalledTimes(6);
      expect(warn).toHaveBeenCalledTimes(1);

      // Read again, as a run may have been measured since: asked again.
      vi.mocked(api.studioLocalFit).mockImplementation(async (_source, _w, h, seconds) => localFit(seconds, h));
      await act(async () => {
        await useStudio.getState().reread();
      });
      await flush();
      expect(api.studioLocalFit).toHaveBeenCalledTimes(12);
      within(field("Resolution")).getByRole("radio", { name: "720p · about 10 min" });
    } finally {
      warn.mockRestore();
    }
  });

  it("a local picture's fields ask for no time", async () => {
    await renderPanel(klein, { target: "thisComputer" });
    expect(api.studioLocalFit).not.toHaveBeenCalled();
    within(field("Shape")).getByText("Automatic");
  });

  it("the panel says where a local model runs", async () => {
    await renderPanel(LOCAL.installed[0]!.model, { target: "thisComputer" });
    screen.getByText(`On this computer · ${LOCAL.hardware.device}`);
    expect(screen.queryByText(/via OpenRouter/)).toBeNull();
  });

  it("and where a cloud model's prompt goes", async () => {
    await renderPanel(flare);
    screen.getByText("via OpenRouter · Cloud");
    expect(screen.queryByText(/On this computer/)).toBeNull();
  });

  it("holds no picture zones: the composer has them", async () => {
    await renderPanel(veo);
    expect(screen.queryByText("Start frame")).toBeNull();
    expect(screen.queryByRole("button", { name: /Add the start frame/ })).toBeNull();
  });

  it("the Advanced toggle counts the changes and opens the Advanced fields", async () => {
    await renderPanel(flare, { values: { output_format: "png", background: "transparent" } });
    const toggle = screen.getByRole("button", { name: "Advanced · 2 changed" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("File type")).toBeNull();
    await act(async () => {
      fireEvent.click(toggle);
    });
    expect(api.studioSetPrefs).toHaveBeenCalledWith({ advancedOpen: true });
    expect(screen.getByRole("button", { name: "Advanced · 2 changed" }).getAttribute("aria-expanded")).toBe("true");
    screen.getByText("File type");
    expect(screen.queryByText("Compression")).toBeNull();
  });

  it("the Advanced toggle opens and closes even when saving that fails", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      await renderPanel(flare);
      vi.mocked(api.studioSetPrefs).mockRejectedValue({ code: "io", message: "The disk is full.", nextStep: "", details: "" });
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Advanced" }));
      });
      await flush();
      expect(api.studioSetPrefs).toHaveBeenCalledWith({ advancedOpen: true });
      expect(screen.getByRole("button", { name: "Advanced" }).getAttribute("aria-expanded")).toBe("true");
      screen.getByText("File type");
      await act(async () => {
        fireEvent.click(screen.getByRole("button", { name: "Advanced" }));
      });
      await flush();
      expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ advancedOpen: false });
      expect(screen.getByRole("button", { name: "Advanced" }).getAttribute("aria-expanded")).toBe("false");
      expect(screen.queryByText("File type")).toBeNull();
      expect(warn).toHaveBeenCalledTimes(2);
    } finally {
      warn.mockRestore();
    }
  });

  it("the Advanced toggle follows a saved choice changed elsewhere", async () => {
    await renderPanel(flare);
    expect(screen.getByRole("button", { name: "Advanced" }).getAttribute("aria-expanded")).toBe("false");
    await act(async () => {
      await useStudio.getState().setPrefs({ advancedOpen: true });
    });
    expect(screen.getByRole("button", { name: "Advanced" }).getAttribute("aria-expanded")).toBe("true");
  });

  it("with nothing changed, the toggle reads Advanced", async () => {
    await renderPanel(flare);
    screen.getByRole("button", { name: "Advanced" });
  });

  it("says what changed when a setting moved, until the note is put away", async () => {
    await renderPanel(flare, { values: { output_format: "jpeg" } });
    act(() => useStudio.getState().setValue("image", "background", "transparent"));
    screen.getByText("Transparent needs PNG or WebP, so the file type is now PNG.");
    fireEvent.click(screen.getByRole("button", { name: "Put the note away" }));
    expect(screen.queryByText(/Transparent needs/)).toBeNull();
    expect(useStudio.getState().drafts.image.note).toBeNull();
  });
});

// The same five cases are pinned on the backend's side by `size_of_matches_the_webviews_local_clip_size`
// in src-tauri/src/studio/local.rs: a change to either rule shows in both tests.
describe("localClipSize", () => {
  it("follows the shape and resolution on the model's multiple and longest side", () => {
    expect(localClipSize(wan, { resolution: "480p" })).toEqual({ w: 832, h: 480 });
    expect(localClipSize(wan, {}, "720p")).toEqual({ w: 1280, h: 704 });
    expect(localClipSize(wan, { aspect_ratio: "9:16" }, "720p")).toEqual({ w: 704, h: 1280 });
    expect(localClipSize(wan, { aspect_ratio: "1:1" }, "720p")).toEqual({ w: 704, h: 704 });
  });

  it("a Length chip uses the draft's resolution, else the Resolution field's starting value", () => {
    expect(localClipSize(wan, { resolution: "720p" })).toEqual({ w: 1280, h: 704 });
    expect(localClipSize(wan, {})).toEqual({ w: 832, h: 480 });
  });

  it("an exact size is used as it is, for every chip", () => {
    expect(localClipSize(wan, { size: "1024x576" })).toEqual({ w: 1024, h: 576 });
    expect(localClipSize(wan, { size: "1024x576", resolution: "720p" }, "480p")).toEqual({ w: 1024, h: 576 });
  });
});

describe("the draft", () => {
  /** The fixture's first picture model under another name, with no WebP. */
  const lite: MediaModel = {
    ...flare,
    id: "example/no-webp",
    displayName: "No WebP",
    params: flare.params.map((p) =>
      p.key === "output_format" && p.kind.type === "enum"
        ? { ...p, kind: { ...p.kind, values: p.kind.values.filter((c) => c.value !== "webp") } }
        : p,
    ),
  };

  it("a model picked drops the carried settings that no longer apply there", async () => {
    await load({
      endpoints: [endpoint([flare, lite])],
      prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: flare.id } } }),
    });
    useStudio.setState((s) => ({
      drafts: { ...s.drafts, image: { ...s.drafts.image, values: { output_format: "webp", output_compression: 80 } } },
    }));
    useStudio.getState().chooseModel("image", { endpoint: 1 }, lite.id);
    expect(useStudio.getState().drafts.image.values).toEqual({});
  });

  it("so does the model's answer when it comes back with its price", async () => {
    // Nano Banana as its lists name it, and as OpenRouter answers for its price: there, its
    // resolution applies only to a square picture.
    const answered: MediaModel = {
      ...nano,
      priceFetchedAt: 1794909600000,
      params: [
        ...nano.params.map((p) =>
          p.key === "resolution" ? { ...p, requires: { key: "aspect_ratio", values: ["1:1"] } } : p,
        ),
        { ...flare.params.find((p) => p.kind.type === "extra")!, kind: { type: "extra", allowedKeys: ["moderation"] } },
      ],
    };
    await load({ prefs: prefs({ lastModel: { image: { target: { endpoint: 1 }, model: flare.id } } }) });
    useStudio.setState((s) => ({
      drafts: { ...s.drafts, image: { ...s.drafts.image, values: { quality: "low" }, extra: { moderation: "low" } } },
    }));
    let answer: (m: MediaModel) => void = () => undefined;
    vi.mocked(api.studioModelPrice).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    useStudio.getState().chooseModel("image", { endpoint: 1 }, nano.id);
    act(() => useStudio.getState().setValue("image", "resolution", "2K"));
    expect(useStudio.getState().drafts.image.values).toEqual({ resolution: "2K" });
    await act(async () => {
      answer(answered);
    });
    await flush();
    expect(useStudio.getState().drafts.image.values).toEqual({});
    expect(useStudio.getState().drafts.image.extra).toEqual({ moderation: "low" });
  });
});
