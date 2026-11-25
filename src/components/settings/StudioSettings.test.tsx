// Settings → Studio (0.1.0 Task B10; design §4 and §5, local addendum L1/L2): the local block
// first (models on this computer, the Studio engine, what happens to the chat model when a
// local job needs the graphics card), then what the gallery holds and where, Delete stopped
// and failed runs, the confirmations and the daily limit for cloud runs, when each model list
// was updated, and the gallery folder. Opening the page reads what is saved and sends nothing.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { LOCAL } from "../studio/fixtures/models";
import type { EngineProgress, EngineStatus, LocalModels, StudioEndpoint, StudioPrefs, Usage } from "../studio/types";
import { amountGb } from "../../lib/words";

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("../studio/api");
const dialogOpen = vi.hoisted(() => vi.fn<() => Promise<string | string[] | null>>());
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: dialogOpen }));

const api = await import("../studio/api");
const { StudioSettings } = await import("./StudioSettings");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

const PREFS: StudioPrefs = {
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
  suggestionDismissed: false,
  localNoticeAcceptedAt: null,
  lastImportMode: null,
};

const USAGE: Usage = {
  spentTodayUsd: 0.84,
  pendingUsd: 0,
  limitUsd: 10,
  folder: "~/harnessUI/studio",
  bytes: 1_290_000_000,
  pictures: 312,
  videos: 14,
};

const OPENROUTER: StudioEndpoint = {
  providerId: 1,
  name: "OpenRouter",
  company: "openrouter",
  host: "openrouter.ai",
  cloud: true,
  hasKey: true,
  // A local time, so the date reads the same in every time zone.
  fetchedAt: new Date(2026, 10, 17, 9).getTime(),
  models: [],
};

/** The Windows engine pack: 29 MB to download. */
const PACK = 30_303_000;

function engine(patch: Partial<EngineStatus>): EngineStatus {
  return { ...LOCAL.engine, downloadBytes: PACK, ...patch };
}

function withEngine(patch: Partial<EngineStatus>, local: Partial<LocalModels> = {}): LocalModels {
  return { ...LOCAL, engine: engine(patch), ...local };
}

/** The preferences as the backend holds them: each change applied to them. */
let held: StudioPrefs = PREFS;
/** The `studio-engine-progress` handler the page listens with. */
let progress: ((p: EngineProgress) => void) | null = null;

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

async function show(local: LocalModels = LOCAL) {
  vi.mocked(api.studioLocalModels).mockResolvedValue(local);
  await act(async () => {
    render(<StudioSettings />);
  });
  await flush();
}

/** Types `value` into the amount box named `label` and leaves it, as a person does. */
async function setAmount(label: string, value: string) {
  const box = screen.getByLabelText(label);
  await act(async () => {
    fireEvent.change(box, { target: { value } });
    fireEvent.blur(box);
  });
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
}

const button = (name: string | RegExp) => screen.getByRole("button", { name });

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  held = PREFS;
  progress = null;
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [OPENROUTER] });
  vi.mocked(api.studioGetPrefs).mockImplementation(async () => held);
  vi.mocked(api.studioUsage).mockResolvedValue(USAGE);
  vi.mocked(api.studioSetPrefs).mockImplementation(async (patch) => {
    held = { ...held, ...patch };
    return held;
  });
  vi.mocked(api.onEngineProgress).mockImplementation(async (handler) => {
    progress = handler;
    return () => {
      progress = null;
    };
  });
});

afterEach(() => cleanup());

describe("Settings → Studio", () => {
  it("says how much Studio holds and where", async () => {
    await show();
    screen.getByText("Studio uses 1.2 GB in ~/harnessUI/studio (312 pictures, 14 videos)");
    vi.mocked(api.studioOpenFolder).mockResolvedValue(undefined);
    await click(button("Open folder"));
    expect(api.studioOpenFolder).toHaveBeenCalledTimes(1);
  });

  it("opening the page sends nothing", async () => {
    await show();
    // What is saved is read...
    expect(api.studioGetPrefs).toHaveBeenCalled();
    expect(api.studioUsage).toHaveBeenCalled();
    expect(api.studioModels).toHaveBeenCalled();
    expect(api.studioLocalModels).toHaveBeenCalled();
    // ...and nothing is fetched.
    expect(api.studioRefreshModels).not.toHaveBeenCalled();
    expect(api.studioModelPrice).not.toHaveBeenCalled();
    expect(api.studioLocalSearch).not.toHaveBeenCalled();
    expect(api.studioLocalRepo).not.toHaveBeenCalled();
    expect(api.studioEngineDownload).not.toHaveBeenCalled();
  });

  it("the local block comes first: engine, models on this computer, and the chat model choice", async () => {
    await show();
    const headings = screen.getAllByRole("heading").map((h) => h.textContent);
    expect(headings.indexOf("On this computer")).toBeGreaterThanOrEqual(0);
    expect(headings.indexOf("On this computer")).toBeLessThan(headings.indexOf("Cloud spending"));
    screen.getByText(`Picture and video models: ${LOCAL.installed.length} (${amountGb(LOCAL.installedBytes)})`);
    button("Get local models");
    screen.getByText("Studio engine: built in"); // LOCAL.engine.state "builtIn" (a Mac)
    expect(screen.queryByRole("button", { name: "Remove" })).toBeNull();
    const unload = screen.getByRole("radio", { name: "Unload the chat model" }) as HTMLInputElement;
    expect(unload.checked).toBe(true);
    screen.getByRole("radio", { name: "Use the processor for local pictures" });
    await click(screen.getByRole("radio", { name: "Ask" }));
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ gpuPolicy: "ask" });
    expect((screen.getByRole("radio", { name: "Ask" }) as HTMLInputElement).checked).toBe(true);
    const reload = screen.getByRole("checkbox", { name: "Reload it after Studio" }) as HTMLInputElement;
    expect(reload.checked).toBe(false);
    await click(reload);
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ reloadChatAfter: true });
  });

  it("Get local models opens Studio's own view", async () => {
    await show();
    const opened: unknown[] = [];
    const onOpen = (e: Event) => opened.push((e as CustomEvent<unknown>).detail);
    window.addEventListener("app:open-studio", onOpen);
    try {
      await click(button("Get local models"));
    } finally {
      window.removeEventListener("app:open-studio", onOpen);
    }
    expect(opened).toEqual(["local"]);
  });

  it("What local models mean opens the content notice, read only", async () => {
    await show();
    await click(button("What local models mean"));
    const notice = screen.getByRole("alertdialog");
    within(notice).getByText("Local models run only on your computer.");
    within(notice).getByText(/Don't create sexual images of children/);
    expect(screen.queryByRole("button", { name: "I understand" })).toBeNull();
    await click(within(notice).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
  });

  it("with no local model yet the line says so", async () => {
    await show({ ...LOCAL, installed: [], installedBytes: 0 });
    screen.getByText("Picture and video models: none yet");
    button("Get local models");
  });

  it("removing the Windows engine asks first, then reads the engine again", async () => {
    await show(withEngine({ state: "installed" }));
    screen.getByText("Studio engine: installed · build master-1106-532d2c1");
    vi.mocked(api.studioEngineRemove).mockResolvedValue(undefined);
    vi.mocked(api.studioEngineStatus).mockResolvedValue(engine({ state: "notInstalled" }));
    await click(button("Remove"));
    expect(api.studioEngineRemove).not.toHaveBeenCalled();
    const ask = screen.getByRole("alertdialog");
    within(ask).getByText("Remove the Studio engine?");
    await click(within(ask).getByRole("button", { name: "Remove" }));
    await flush();
    expect(api.studioEngineRemove).toHaveBeenCalledTimes(1);
    // No `finished` event follows a removal: the engine is read again.
    expect(api.studioEngineStatus).toHaveBeenCalledTimes(1);
    expect(useStudio.getState().local?.engine.state).toBe("notInstalled");
    // A model is still installed, so the engine is offered again.
    button("Download the Studio engine (29 MB, MIT)");
  });

  it("a refused removal says why, and the engine is read again", async () => {
    await show(withEngine({ state: "installed" }));
    vi.mocked(api.studioEngineRemove).mockRejectedValue({
      code: "studio_engine_in_use",
      message: "The Studio engine is working on a local job.",
      nextStep: "Stop the job in Studio, or remove the engine when it finishes.",
      details: "",
    });
    vi.mocked(api.studioEngineStatus).mockResolvedValue(engine({ state: "installed" }));
    await click(button("Remove"));
    await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Remove" }));
    await flush();
    screen.getByText("The Studio engine is working on a local job.");
    expect(api.studioEngineStatus).toHaveBeenCalledTimes(1);
    screen.getByText("Studio engine: installed · build master-1106-532d2c1");
  });

  it("Cancel keeps the engine", async () => {
    await show(withEngine({ state: "installed" }));
    await click(button("Remove"));
    await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    expect(api.studioEngineRemove).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("a missing engine on Windows offers its download, and shows how it goes", async () => {
    await show(withEngine({ state: "notInstalled" }));
    screen.getByText("Studio engine: missing");
    const download = button("Download the Studio engine (29 MB, MIT)");
    expect(download.getAttribute("title")).toBe(LOCAL.engine.licenseName);
    vi.mocked(api.studioEngineDownload).mockResolvedValue(undefined);
    vi.mocked(api.studioEngineStatus).mockResolvedValue(engine({ state: "downloading", bytesDone: 0 }));
    await click(download);
    await flush();
    expect(api.studioEngineDownload).toHaveBeenCalledWith(true);
    screen.getByText("Studio engine: downloading…");
    expect(screen.queryByRole("button", { name: /Download the Studio engine/ })).toBeNull();
    await act(async () => progress?.({ done: 12 * 1024 * 1024, total: PACK, unpacking: false, finished: false, error: null }));
    screen.getByText("Studio engine: downloading · 12 MB of 29 MB");
    await act(async () => progress?.({ done: PACK, total: PACK, unpacking: true, finished: false, error: null }));
    screen.getByText("Studio engine: unpacking…");
    vi.mocked(api.studioEngineStatus).mockResolvedValue(engine({ state: "installed" }));
    await act(async () => progress?.({ done: PACK, total: PACK, unpacking: false, finished: true, error: null }));
    await flush();
    screen.getByText("Studio engine: installed · build master-1106-532d2c1");
  });

  it("a partly downloaded engine is offered again; with no model it comes with the first one", async () => {
    await show(withEngine({ state: "partial" }));
    screen.getByText("Studio engine: partly downloaded");
    button("Download the Studio engine (29 MB, MIT)");
    cleanup();
    await show(withEngine({ state: "notInstalled" }, { installed: [], installedBytes: 0 }));
    const line = screen.getByText("Studio engine: comes with your first model (29 MB, MIT)");
    expect(line.getAttribute("title")).toBe(LOCAL.engine.licenseName);
    expect(screen.queryByRole("button", { name: /Download the Studio engine/ })).toBeNull();
  });

  it("an engine download that fails says why, and the engine is read again", async () => {
    await show(withEngine({ state: "notInstalled" }));
    vi.mocked(api.studioEngineDownload).mockRejectedValue({
      code: "studio_engine_not_installed",
      message: "The Studio engine could not be downloaded.",
      nextStep: "Check your connection, then try again.",
      details: "",
    });
    vi.mocked(api.studioEngineStatus).mockResolvedValue(engine({ state: "notInstalled" }));
    await click(button("Download the Studio engine (29 MB, MIT)"));
    await flush();
    screen.getByText("The Studio engine could not be downloaded.");
    // It never started, so no `finished` event comes: the engine is read again.
    expect(api.studioEngineStatus).toHaveBeenCalledTimes(1);
    cleanup();
    // A download that started and then failed ends with a `finished` event carrying the error.
    vi.mocked(api.studioEngineStatus).mockClear();
    await show(withEngine({ state: "downloading", bytesDone: 0 }));
    await act(async () =>
      progress?.({
        done: 0,
        total: PACK,
        unpacking: false,
        finished: true,
        error: { code: "studio_engine_not_installed", message: "The download stopped.", nextStep: "Try again.", details: "" },
      }),
    );
    await flush();
    screen.getByText("The download stopped.");
    expect(api.studioEngineStatus).toHaveBeenCalledTimes(1);
  });

  it("the daily limit is on at $10 and can be changed or turned off", async () => {
    await show();
    const on = screen.getByRole("checkbox", { name: "Daily limit" }) as HTMLInputElement;
    expect(on.checked).toBe(true);
    expect((screen.getByLabelText("Daily limit in US dollars") as HTMLInputElement).value).toBe("10");
    await setAmount("Daily limit in US dollars", "20");
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ dailyLimitUsd: 20 });
    await click(screen.getByRole("checkbox", { name: "Daily limit" }));
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ dailyLimitUsd: null });
    expect((screen.getByLabelText("Daily limit in US dollars") as HTMLInputElement).disabled).toBe(true);
    // Turned on again at the amount it had.
    await click(screen.getByRole("checkbox", { name: "Daily limit" }));
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ dailyLimitUsd: 20 });
    vi.mocked(api.studioSetPrefs).mockClear();
    await setAmount("Daily limit in US dollars", "0.5");
    screen.getByText("Choose an amount from $1 to $1,000.");
    await setAmount("Daily limit in US dollars", "1001");
    await setAmount("Daily limit in US dollars", "lots");
    expect(api.studioSetPrefs).not.toHaveBeenCalled();
    await setAmount("Daily limit in US dollars", "$15");
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ dailyLimitUsd: 15 });
    expect(screen.queryByText("Choose an amount from $1 to $1,000.")).toBeNull();
  });

  it("an amount may be written with thousands separators", async () => {
    await show();
    await setAmount("Daily limit in US dollars", "1,000");
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ dailyLimitUsd: 1000 });
    expect(screen.queryByText("Choose an amount from $1 to $1,000.")).toBeNull();
    await setAmount("Daily limit in US dollars", "500");
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ dailyLimitUsd: 500 });
    await setAmount("Daily limit in US dollars", "$1,000.00");
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ dailyLimitUsd: 1000 });
    expect(screen.queryByText("Choose an amount from $1 to $1,000.")).toBeNull();
    // A comma that does not group thousands is not read as one (a decimal comma would be
    // read ten times too big).
    vi.mocked(api.studioSetPrefs).mockClear();
    await setAmount("Daily limit in US dollars", "1,5");
    screen.getByText("Choose an amount from $1 to $1,000.");
    expect(api.studioSetPrefs).not.toHaveBeenCalled();
    // The confirmation amount reads them too.
    await setAmount("Ask before runs of", "$1,00");
    screen.getByText("Choose an amount from $0.25 to $100.");
    await setAmount("Ask before runs of", "$2.50");
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ confirmAtUsd: 2.5 });
  });

  it("a limit the backend refused is not the amount the limit comes back at", async () => {
    await show();
    vi.mocked(api.studioSetPrefs).mockRejectedValueOnce({
      code: "io",
      message: "Studio's preferences could not be saved.",
      nextStep: "Try again.",
      details: "",
    });
    await setAmount("Daily limit in US dollars", "20");
    await flush();
    screen.getByText("Studio's preferences could not be saved.");
    // The box shows the saved amount, and turned off and on again the limit is that amount.
    expect((screen.getByLabelText("Daily limit in US dollars") as HTMLInputElement).value).toBe("10");
    await click(screen.getByRole("checkbox", { name: "Daily limit" }));
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ dailyLimitUsd: null });
    await click(screen.getByRole("checkbox", { name: "Daily limit" }));
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ dailyLimitUsd: 10 });
  });

  it("a limit typed and turned off at once comes back at the amount typed", async () => {
    await show();
    // The backend answers only when let: unticking the box leaves the amount box first, so its
    // change is sent and still unanswered when the box is unticked.
    const answers: (() => void)[] = [];
    vi.mocked(api.studioSetPrefs).mockImplementation(
      (patch) =>
        new Promise((resolve) =>
          answers.push(() => {
            held = { ...held, ...patch };
            resolve(held);
          }),
        ),
    );
    const box = screen.getByLabelText("Daily limit in US dollars");
    await act(async () => {
      fireEvent.change(box, { target: { value: "20" } });
      fireEvent.blur(box);
    });
    await click(screen.getByRole("checkbox", { name: "Daily limit" }));
    expect(vi.mocked(api.studioSetPrefs).mock.calls).toEqual([[{ dailyLimitUsd: 20 }], [{ dailyLimitUsd: null }]]);
    await act(async () => {
      for (const answer of answers.splice(0)) answer();
    });
    await flush();
    expect(useStudio.getState().prefs?.dailyLimitUsd).toBeNull();
    await click(screen.getByRole("checkbox", { name: "Daily limit" }));
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ dailyLimitUsd: 20 });
  });

  it("the confirmation choices can be changed", async () => {
    await show();
    const video = screen.getByRole("checkbox", { name: "Ask before every video" }) as HTMLInputElement;
    expect(video.checked).toBe(true);
    await click(video);
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ confirmEveryVideo: false });
    expect((screen.getByLabelText("Ask before runs of") as HTMLInputElement).value).toBe("1");
    await setAmount("Ask before runs of", "2.5");
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ confirmAtUsd: 2.5 });
    vi.mocked(api.studioSetPrefs).mockClear();
    await setAmount("Ask before runs of", "0.1");
    screen.getByText("Choose an amount from $0.25 to $100.");
    expect(api.studioSetPrefs).not.toHaveBeenCalled();
    // Enter keeps it too.
    const box = screen.getByLabelText("Ask before runs of");
    await act(async () => {
      fireEvent.change(box, { target: { value: "5" } });
      fireEvent.keyDown(box, { key: "Enter" });
    });
    expect(api.studioSetPrefs).toHaveBeenLastCalledWith({ confirmAtUsd: 5 });
  });

  it("a change the backend refuses says why", async () => {
    await show();
    vi.mocked(api.studioSetPrefs).mockRejectedValue({
      code: "bad_request",
      message: "Choose an amount from $0.25 to $100.",
      nextStep: "",
      details: "",
    });
    await click(screen.getByRole("checkbox", { name: "Ask before every video" }));
    await flush();
    screen.getByRole("alert");
    // The box still shows what is saved.
    expect((screen.getByRole("checkbox", { name: "Ask before every video" }) as HTMLInputElement).checked).toBe(true);
  });

  it("Delete stopped and failed runs asks first", async () => {
    await show();
    vi.mocked(api.studioDeleteUnfinished).mockResolvedValue(3);
    await click(button("Delete stopped and failed runs"));
    expect(api.studioDeleteUnfinished).not.toHaveBeenCalled();
    within(screen.getByRole("alertdialog")).getByText("Delete stopped and failed runs?");
    await click(button("Delete"));
    await waitFor(() => expect(api.studioDeleteUnfinished).toHaveBeenCalledTimes(1));
    await flush();
    screen.getByText("3 runs were deleted.");
  });

  it("each model list says when it was updated, and Update now refreshes it", async () => {
    await show();
    screen.getByText("OpenRouter · updated 17 Nov 2026 09:00");
    vi.mocked(api.studioRefreshModels).mockResolvedValue({
      endpoints: [{ ...OPENROUTER, fetchedAt: new Date(2026, 10, 19, 14, 5).getTime() }],
    });
    await click(button("Update now"));
    expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
    await flush();
    screen.getByText("OpenRouter · updated 19 Nov 2026 14:05");
  });

  it("a list never saved says so, and Get the list fetches it", async () => {
    vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [{ ...OPENROUTER, fetchedAt: null }] });
    await show();
    screen.getByText("OpenRouter · no lists saved yet");
    vi.mocked(api.studioRefreshModels).mockResolvedValue({ endpoints: [OPENROUTER] });
    await click(button("Get the list"));
    expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
  });

  it("changing the folder says existing results stay where they are", async () => {
    await show();
    dialogOpen.mockResolvedValue("/data/studio");
    await click(button("Change…"));
    await waitFor(() => expect(api.studioSetPrefs).toHaveBeenCalledWith({ galleryDir: "/data/studio" }));
    expect(dialogOpen).toHaveBeenCalledWith({ directory: true, multiple: false });
    await flush();
    screen.getByText("New results go to the new folder; existing ones stay where they are.");
  });

  it("a folder picker closed without a choice changes nothing", async () => {
    await show();
    dialogOpen.mockResolvedValue(null);
    await click(button("Change…"));
    await flush();
    expect(api.studioSetPrefs).not.toHaveBeenCalled();
    expect(screen.queryByText("New results go to the new folder; existing ones stay where they are.")).toBeNull();
  });
});
