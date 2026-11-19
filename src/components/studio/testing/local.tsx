// What the tests of Get local models, the download confirmation and the content notice share
// (0.1.0 Tasks B13 and B14): the preferences and usage Studio reads, a few steps to let
// promises settle, and Studio shown on Get local models. vi.mock works per test file, so each
// file mocks "../../lib/ipc" and "./api" itself before it imports this module.
import { act, fireEvent, render } from "@testing-library/react";
import { vi } from "vitest";
import { useStudio } from "../../../store/studio";
import * as api from "../api";
import { LOCAL } from "../fixtures/models";
import { StudioView } from "../StudioView";
import type { LocalModels, StudioPrefs, Usage } from "../types";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

/** Studio's preferences, with the note about local models never accepted. */
export const PREFS: StudioPrefs = {
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

/** Nothing spent and nothing made yet. */
export const USAGE: Usage = {
  spentTodayUsd: 0,
  pendingUsd: 0,
  limitUsd: 10,
  folder: "~/harnessUI/studio",
  bytes: 0,
  pictures: 0,
  videos: 0,
};

/** Lets the promises waiting now settle. */
export async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Clicks `el`, then lets what it started settle. */
export async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
  await flush();
}

/** Studio shown on Get local models, with `local` as the backend reads it (the preferences say
 *  the note was accepted when `local` does). */
export async function showLocal(local: LocalModels = LOCAL) {
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(local);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue({ ...PREFS, localNoticeAcceptedAt: local.noticeAccepted ? 1 : null });
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
  vi.mocked(api.studioEngineDownload).mockResolvedValue(undefined);
  vi.mocked(api.studioLocalDownload).mockResolvedValue("group-1");
  vi.mocked(api.studioLocalDelete).mockResolvedValue(undefined);
  useStudio.setState({ local, loaded: true });
  useStudio.getState().showStudio();
  useStudio.getState().showLocalModels(true);
  await act(async () => {
    render(<StudioView />);
  });
  await flush();
}
