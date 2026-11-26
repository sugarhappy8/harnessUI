// Studio's confirmation before a cloud run (0.1.0 Task B7, design Wireframe 3): every cloud
// video, a run estimated at $1 or more, a run whose price is unknown. It shows the run as it
// would be sent, says a started video is charged, and can stop asking for videos under the
// amount; Cancel sends nothing.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { StudioTab } from "../../store/studio";
import { LOCAL, MODELS, RUN } from "./fixtures/models";
import type { ConfirmReason, Estimate, Run, StartRequest, StudioEndpoint, StudioPrefs } from "./types";

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
const { UiProviders } = await import("../ui/UiProviders");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

/** OpenRouter with the fixture's models, Veo 3.1's prices saved on 17 Nov (in local time). */
const openrouter: StudioEndpoint = {
  providerId: 1,
  name: "OpenRouter",
  company: "openrouter",
  host: "openrouter.ai",
  cloud: true,
  hasKey: true,
  fetchedAt: new Date(2026, 10, 17, 9).getTime(),
  models: MODELS.map((m) =>
    m.id === "google/veo-3.1" ? { ...m, priceFetchedAt: new Date(2026, 10, 17, 9).getTime() } : m,
  ),
};

function prefs(patch: Partial<StudioPrefs> = {}): StudioPrefs {
  return {
    // GPT Image 2.6 Flare and Veo 3.1 on OpenRouter.
    lastModel: {
      image: { target: { endpoint: 1 }, model: "openai/gpt-image-2.6-flare" },
      video: { target: { endpoint: 1 }, model: "google/veo-3.1" },
    },
    advancedOpen: false,
    settingsOpen: false,
    prefsVersion: 1,
    confirmEveryVideo: true,
    confirmUnknownPrice: false,
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

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Opens Studio on `tab`'s cloud model, with `estimate` for its draft, and waits for it. */
async function show(tab: StudioTab, estimate: Estimate, p: StudioPrefs = prefs()) {
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [openrouter] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(LOCAL);
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
  vi.mocked(api.studioEstimate).mockResolvedValue(estimate);
  vi.mocked(api.studioStart).mockImplementation(
    async (req): Promise<Run> => ({ ...RUN, id: "new", kind: req.kind, status: "queued", outputs: [] }),
  );
  useStudio.getState().showStudio(tab);
  await act(async () => {
    render(
      <UiProviders>
        <StudioView />
      </UiProviders>,
    );
  });
  await flush();
  fireEvent.change(screen.getByRole("textbox", { name: "Prompt" }), { target: { value: "slow drone push" } });
  // Past the estimate's 150 ms pause, and its answer.
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 200));
  });
  await flush();
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

async function create() {
  await click(screen.getByRole("button", { name: /Create/ }));
}

function dialog(): HTMLElement {
  return screen.getByRole("alertdialog");
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("ConfirmRun", () => {
  it.each<[string, StudioTab, number | null, ConfirmReason[], string]>([
    ["a video", "video", 0.09, ["video"], "$0.09"],
    ["a $1.00 picture run", "image", 1, ["atLeast"], "$1.00"],
    ["an unknown price", "image", null, ["unknownPrice"], "price unknown"],
  ])("asks before %s", async (_, kind, usd, reasons, price) => {
    await show(kind, est({ usd, reasons }));
    await create();
    expect(api.studioStart).not.toHaveBeenCalled();
    expect(dialog().textContent).toContain(price);
    // A price list is named only for a price it gave.
    expect(dialog().textContent?.includes("price list")).toBe(usd !== null);
    screen.getByRole("button", { name: "Cancel" });
    if (kind === "image") screen.getByText("Make this picture?");
  });

  it("does not ask for a $0.99 picture run", async () => {
    await show("image", est({ usd: 0.99, reasons: [] }));
    await create();
    expect(api.studioStart).toHaveBeenCalledTimes(1);
    expect(api.studioStart).toHaveBeenCalledWith(expect.objectContaining({ prompt: "slow drone push" }), false);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("says a started video is charged even if you stop watching", async () => {
    // The day names no year in the year it is.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 10, 18, 12));
    await show("video", est({ usd: 0.96, reasons: ["video"] }));
    await create();
    screen.getByText("Make this video?");
    screen.getByText("OpenRouter charges for a video once it has started, even if you stop watching it.");
    // What is sent, to whom and for how much (OpenRouter's list was saved on 17 Nov).
    screen.getByText("Veo 3.1 through OpenRouter");
    screen.getByText("4 s · 720p · sound on · 1 video");
    screen.getByText("(OpenRouter's price list, 17 Nov)");
    screen.getByText("Your prompt goes to openrouter.ai and the company it sends the job to.");
  });

  it("Don't ask again for videos under $1.00 saves the choice and starts", async () => {
    await show("video", est({ usd: 0.09, reasons: ["video"], confirmAtUsd: 1 }));
    await create();
    await click(screen.getByRole("checkbox", { name: "Don't ask again for videos under $1.00" }));
    await click(screen.getByRole("button", { name: "Make video" }));
    expect(api.studioSetPrefs).toHaveBeenCalledTimes(1);
    expect(api.studioSetPrefs).toHaveBeenCalledWith({ confirmEveryVideo: false }); // only that field
    expect(api.studioStart).toHaveBeenCalledTimes(1);
    const [req, confirmed] = vi.mocked(api.studioStart).mock.calls[0]!;
    expect(confirmed).toBe(true);
    expect(req).toMatchObject({ kind: "video", model: "google/veo-3.1", prompt: "slow drone push" });
    const saved = vi.mocked(api.studioSetPrefs).mock.invocationCallOrder[0]!;
    expect(saved).toBeLessThan(vi.mocked(api.studioStart).mock.invocationCallOrder[0]!);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("with asking from an amount off, Don't ask again names every video, not an amount", async () => {
    await show("video", est({ usd: 0.09, reasons: ["video"], confirmAtUsd: null }));
    await create();
    screen.getByRole("checkbox", { name: "Don't ask again before every video" });
    expect(screen.queryByRole("checkbox", { name: /under/ })).toBeNull();
  });

  it("confirming sends the press that asked, whatever the draft became", async () => {
    await show("video", est({ usd: 0.09, reasons: ["video"] }));
    await create();
    const asked: StartRequest | null = useStudio.getState().pending;
    expect(asked).toMatchObject({ prompt: "slow drone push" });
    act(() => useStudio.getState().setPrompt("video", "something else"));
    await click(screen.getByRole("button", { name: "Make video" }));
    expect(api.studioSetPrefs).not.toHaveBeenCalledWith({ confirmEveryVideo: false });
    expect(api.studioStart).toHaveBeenCalledWith(asked, true);
  });

  it("offers Don't ask again only when a video is the only reason", async () => {
    await show("video", est({ usd: 1.2, reasons: ["video", "atLeast"] }));
    await create();
    dialog();
    expect(screen.queryByRole("checkbox")).toBeNull();
  });

  it("Cancel sends nothing", async () => {
    await show("video", est({ usd: 0.09, reasons: ["video"] }));
    await create();
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.studioStart).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(useStudio.getState()).toMatchObject({ starting: false, pending: null, pendingEstimate: null });
    expect((screen.getByRole("button", { name: /Create/ }) as HTMLButtonElement).disabled).toBe(false);
  });
});
