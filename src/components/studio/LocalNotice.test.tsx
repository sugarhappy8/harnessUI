// The one-time content notice (0.1.0 Task B14, local addendum L1): shown before the first
// local download, word for word, and accepted with *I understand* (recorded by the backend);
// read again, read only, from Get local models' footer and from Settings → Studio.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { LOCAL } from "./fixtures/models";
import type { LocalModels, StudioPrefs, Usage } from "./types";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { StudioView } = await import("./StudioView");
const { LocalNotice } = await import("./LocalNotice");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

const PREFS: StudioPrefs = {
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
};

const USAGE: Usage = {
  spentTodayUsd: 0,
  pendingUsd: 0,
  limitUsd: 10,
  folder: "~/harnessUI/studio",
  bytes: 0,
  pictures: 0,
  videos: 0,
};

/** The notice, addendum L1, word for word after its bold first sentence. */
const BODY =
  "No company checks your prompts or pictures. Don't create sexual images of children, or sexual or intimate images of real people without their consent, and don't impersonate people. These are crimes in many countries. Each model's licence and use policy also applies.";

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

const onAccepted = vi.fn<() => void>();
const onClose = vi.fn<() => void>();

function renderNotice(readOnly?: boolean) {
  useStudio.setState({ local: { ...LOCAL, noticeAccepted: false } });
  render(<LocalNotice open readOnly={readOnly} onAccepted={onAccepted} onClose={onClose} />);
  return screen.getByRole("alertdialog");
}

/** Studio shown on Get local models, with `local` as the backend reads it. */
async function showLocal(local: LocalModels = LOCAL) {
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(local);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(PREFS);
  vi.mocked(api.studioUsage).mockResolvedValue(USAGE);
  vi.mocked(api.studioEstimate).mockResolvedValue({
    usd: 0,
    reasons: [],
    confirmAtUsd: 1,
    limit: { limitUsd: 10, spentTodayUsd: 0, pendingUsd: 0, wouldPass: true },
    invalid: [],
    eta: null,
    local: null,
  });
  vi.mocked(api.onEngineProgress).mockResolvedValue(() => undefined);
  vi.mocked(api.studioEngineStatus).mockResolvedValue(local.engine);
  useStudio.setState({ local, loaded: true });
  useStudio.getState().showStudio();
  useStudio.getState().showLocalModels(true);
  await act(async () => {
    render(<StudioView />);
  });
  await flush();
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  vi.mocked(api.studioAcceptLocalNotice).mockResolvedValue({ ...PREFS, localNoticeAcceptedAt: 1_700_000_000_000 });
});

afterEach(cleanup);

describe("the content notice", () => {
  it("says what local models mean, word for word", () => {
    renderNotice();
    screen.getByText("Local models run only on your computer.");
    screen.getByText(BODY);
  });

  it("I understand records it with the backend, then says so", async () => {
    renderNotice();
    fireEvent.click(screen.getByRole("button", { name: "I understand" }));
    await waitFor(() => expect(onAccepted).toHaveBeenCalledTimes(1));
    expect(api.studioAcceptLocalNotice).toHaveBeenCalledTimes(1);
    expect(useStudio.getState().local?.noticeAccepted).toBe(true);
    expect(useStudio.getState().prefs?.localNoticeAcceptedAt).toBe(1_700_000_000_000);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("a refusal to record it says why, and the notice stays", async () => {
    vi.mocked(api.studioAcceptLocalNotice).mockRejectedValue({
      code: "studio_prefs",
      message: "Studio's settings could not be saved.",
      nextStep: "Try again.",
      details: "",
    });
    renderNotice();
    await click(screen.getByRole("button", { name: "I understand" }));
    screen.getByText("Studio's settings could not be saved.");
    screen.getByText("Local models run only on your computer.");
    expect(onAccepted).not.toHaveBeenCalled();
    expect(useStudio.getState().local?.noticeAccepted).toBe(false);
  });

  it("Cancel before a first download accepts nothing", async () => {
    renderNotice();
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
  });

  it("read only, it has Close and nothing to accept", async () => {
    renderNotice(true);
    screen.getByText("Local models run only on your computer.");
    screen.getByText(BODY);
    expect(screen.queryByRole("button", { name: "I understand" })).toBeNull();
    await click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
  });

  it("the footer shows the notice again, read only", async () => {
    await showLocal();
    await click(screen.getByRole("button", { name: "What that means" }));
    screen.getByText("Local models run only on your computer.");
    expect(screen.queryByRole("button", { name: "I understand" })).toBeNull();
    await click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByText("Local models run only on your computer.")).toBeNull();
    expect(useStudio.getState().noticeOpen).toBeNull();
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
  });

  it("Cancel on the notice before a first download puts the offer away", async () => {
    await showLocal({ ...LOCAL, noticeAccepted: false });
    await click(within(screen.getByRole("article", { name: "Z-Image Turbo" })).getByRole("button", { name: "Download" }));
    screen.getByText("Local models run only on your computer.");
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByText("Local models run only on your computer.")).toBeNull();
    expect(screen.queryByText("Download Z-Image Turbo?")).toBeNull();
    expect(useStudio.getState().offer).toBeNull();
    expect(api.studioAcceptLocalNotice).not.toHaveBeenCalled();
  });
});
