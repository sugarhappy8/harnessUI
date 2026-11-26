// 0.2.1: Studio that fails to draw from data its store keeps (here, a repository read with no
// file list, which throws in its Get models card) never strands the person. The notice has
// *Back to Studio*, which leaves Get models and drops what was read; leaving Studio from the
// sidebar and coming back does the same; and Studio then draws again.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { DownloadGroup } from "../../lib/types";
import type { LocalRepo, LocalSearchPage } from "./types";
import searchJson from "./fixtures/real/search.json";
import bugRepoJson from "./fixtures/real/chatrlabs__Qwen-Image-2.1-Uncensored-Omni-Quantized/repo.json";

const ipc = vi.hoisted(() => ({
  isTauri: () => true,
  listDownloads: vi.fn<() => Promise<DownloadGroup[]>>(),
  onDownloadProgress: vi.fn<(handler: (g: DownloadGroup) => void) => Promise<() => void>>(),
  hubTokenStatus: vi.fn<() => Promise<{ saved: boolean; account: string | null }>>(),
}));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("./api");

const api = await import("./api");
const { LOCAL } = await import("./fixtures/models");
const { flush, showLocal } = await import("./testing/local");
const { useStudio } = await import("../../store/studio");
const { useDownloads } = await import("../../store/downloads");
const { StudioBoundary } = await import("./StudioBoundary");
const { StudioView } = await import("./StudioView");
const { UiProviders } = await import("../ui/UiProviders");

Element.prototype.scrollIntoView ??= () => {};

const INITIAL = useStudio.getState();
const SEARCH = searchJson as unknown as LocalSearchPage;
const REPO = bugRepoJson as unknown as LocalRepo;
/** What a malformed answer would leave in the store: the repository with no file list. */
const MALFORMED = { ...REPO, files: undefined } as unknown as LocalRepo;

let quiet: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  useDownloads.setState({ groups: [] });
  ipc.listDownloads.mockResolvedValue([]);
  ipc.hubTokenStatus.mockResolvedValue({ saved: false, account: null });
  ipc.onDownloadProgress.mockResolvedValue(() => undefined);
  quiet = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  quiet.mockRestore();
});

/** Studio, inside its boundary as App draws it, on its results; then Get models opened on a
 *  search whose chatrlabs card reads the malformed repository and throws. */
async function brokenGetModels() {
  await showLocal(LOCAL, { open: false });
  cleanup();
  vi.mocked(api.studioLocalSearch).mockResolvedValue(SEARCH);
  await act(async () => {
    render(
      <UiProviders>
        <nav>sidebar</nav>
        <StudioBoundary>
          <StudioView />
        </StudioBoundary>
      </UiProviders>,
    );
  });
  await flush();
  await act(async () => {
    useStudio.setState({ repos: { [REPO.repo]: MALFORMED } });
    useStudio.getState().showLocalModels(true);
  });
  await flush();
  expect(screen.getByRole("alert", { name: "Studio couldn't be shown." })).toBeTruthy();
  expect(screen.getByText("sidebar")).toBeTruthy();
}

it("Back to Studio leaves Get models, drops what was read, and Studio draws again", async () => {
  await brokenGetModels();
  // Try again draws the same store, which throws again: the notice is back.
  await act(async () => fireEvent.click(screen.getByRole("button", { name: "Try again" })));
  await flush();
  expect(screen.getByRole("alert", { name: "Studio couldn't be shown." })).toBeTruthy();

  await act(async () => fireEvent.click(screen.getByRole("button", { name: "Back to Studio" })));
  await flush();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(useStudio.getState()).toMatchObject({ localView: false, detail: null, localSearch: null, repos: {} });

  // Get models opens again, with the same search, and draws its cards.
  await act(async () => useStudio.getState().showLocalModels(true));
  await flush();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.getByRole("searchbox", { name: "Search Hugging Face" })).toBeTruthy();
});

it("leaving Studio from the sidebar and coming back draws Studio again", async () => {
  await brokenGetModels();
  await act(async () => useStudio.getState().showChat());
  await act(async () => useStudio.getState().showStudio());
  await flush();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(useStudio.getState().localView).toBe(false);
});
